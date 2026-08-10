# Windows Build Guide

How to set up a Windows machine to build and run winmux, and how to run the Windows-side
verification. Two apps share this guide:

- **`apps/winmux`** — the MVP app, active development from 계획 v2 section 17 stage 10
  onward (section 3 below).
- **`apps/spike`** — the Tauri v2 + xterm.js spike. Its sign-off was completed on
  2026-08-08 (candidate A adopted — see
  [`docs/adr/0001`](adr/0001-adopt-tauri-webview2-xterm-stack.md)); it's now **frozen as
  a measurement harness** — no new features land there, only compiling is kept green —
  and its build steps (section 2) and checklist (section 5) remain the reference for
  re-running those checks as regression tests during MVP work.

## 1. Prerequisites

### rustup + MSVC target(s)

Install [rustup](https://rustup.rs/) (defaults to the `stable-x86_64-pc-windows-msvc` toolchain
on a 64-bit Windows install). Tauri on Windows requires the **MSVC** ABI, not GNU.

```powershell
winget install Rustlang.Rustup
```

After install, verify:

```powershell
rustup show
rustc --version
```

### Visual Studio Build Tools (C++)

Rust's MSVC toolchain needs the MSVC linker and Windows SDK, which come from Visual Studio
Build Tools — not the full Visual Studio IDE.

1. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
2. In the installer, select the **"Desktop development with C++"** workload.
3. Under "Individual components", make sure the MSVC target architecture(s) you need are
   selected:
   - **x64**: `MSVC v143 - VS 2022 C++ x64/x86 build tools` (this dev machine's target)
   - **ARM64**: `MSVC v143 - VS 2022 C++ ARM64 build tools` (only needed if you'll build for
     ARM64 — see section 11)
4. Also confirm a **Windows 10/11 SDK** component is selected (the installer usually pulls one
   in automatically with the C++ workload).

Tauri also needs [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) — the
Evergreen runtime ships with Windows 11 and most updated Windows 10 installs already, so a
separate install step is usually unnecessary. If `npm run tauri dev` fails complaining about a
missing WebView2 runtime, install the "Evergreen Bootstrapper" from the link above.

### Node.js LTS

Install a current Node.js **LTS** release (from [nodejs.org](https://nodejs.org/) or
`winget install OpenJS.NodeJS.LTS`). `apps/spike` builds with npm scripts (`tsc` + `vite` +
`vitest`) that assume a recent Node LTS.

Verify:

```powershell
node --version
npm --version
```

## 2. Build and run `apps/spike`

From the repo root on Windows (adjust the path to wherever you cloned/checked out the repo):

```powershell
cd apps\spike
npm install
```

### Development (hot reload, dev console)

```powershell
npm run tauri dev
```

This starts the Vite dev server and launches the Tauri window pointed at it. Rust changes under
`src-tauri/` or `crates/winmux-core` trigger a rebuild; frontend changes hot-reload.

### Distributable exe (no installer/bundle)

```powershell
npm run tauri build -- --no-bundle
```

`--no-bundle` skips MSI/NSIS installer packaging (not needed for Spike verification) and leaves
a plain `winmux-spike.exe` under the **workspace root** `target\release\` — same reason as
section 3: the repo root is the cargo workspace, so `target/` lives there. This is the binary
[`scripts/win/measure.ps1`](../scripts/win/measure.ps1) expects by default (`-ProcessName
winmux-spike`, matching `productName` in `src-tauri/tauri.conf.json`).

## 3. Build and run `apps/winmux`

`apps/winmux` is the MVP app (계획 v2 section 17, stage 10 onward) — same Tauri v2 +
Node/npm toolchain as `apps/spike` above, but its Rust glue drives the `winmux-core`
`Dispatcher` over the single `Command` bus instead of spike's thin per-call commands.
Architecture: [`docs/adr/0002`](adr/0002-stage10-architecture.md) and
[`docs/adr/0003`](adr/0003-split-tab-ui-architecture.md).

From the repo root on Windows:

```powershell
cd apps\winmux
npm install
```

### Development (hot reload, dev console)

```powershell
npm run tauri dev
```

Same rebuild behavior as spike: Rust changes under `src-tauri/` or `crates/winmux-core`
trigger a rebuild, frontend changes hot-reload. On boot the app itself dispatches a
single atomic `CreateWorkspace{tab}` (from Tauri `setup`, before the frontend ever
attaches — stage 13 folded the earlier `CreateWorkspace` + `CreateTab` pair into one
command), so a terminal tab is already running when the window opens. Splits/tabs
(stages 11–12) and the workspace sidebar (stage 13) are mouse-driven; commands without
UI yet can still be driven from the WebView dev console via the dev hook
`window.__winmux.dispatch(command)`. See section 6 below for the stage 10 manual
checklist that exercises this.

### Distributable exe (no installer/bundle)

```powershell
npm run tauri build -- --no-bundle
```

Leaves a plain `winmux-app.exe` under the **workspace root** `target\release\` — not under
`apps\winmux\src-tauri\`, because the repo root is the cargo workspace and that is where
cargo puts `target/`. The binary is named after the cargo package (`winmux-app`), not after
`productName`; `--no-bundle` skips the bundling step that would apply the product name.

Cross-compiling adds the triple: `--target aarch64-pc-windows-msvc` writes to
`target\aarch64-pc-windows-msvc\release\winmux-app.exe`. That is the path
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) uploads, verified by a real
`workflow_dispatch` run of the `windows-artifacts` job for both targets.

## 4. `WINMUX_DISTRO` environment variable

Both apps spawn the WSL shell as `wsl.exe [-d $WINMUX_DISTRO] -- bash -l` — spike's glue
reads it directly per spawn (see spike-plan.md section 4.5); winmux threads it through
`winmux-core`'s `Command::CreateWorkspace` → `ShellSpawnReq::distro` (see
`crates/winmux-core/src/command.rs`), same underlying `wsl.exe` invocation either way.
`WINMUX_DISTRO` selects which WSL distribution to spawn into:

- **Unset**: `wsl.exe` uses your default distribution (`wsl -l -v` shows which one has `*`).
- **Set**: `wsl.exe -d <name>` targets that distribution explicitly — useful if you have more
  than one installed (e.g. a plain Ubuntu install alongside an isolated/sandboxed distro for
  agent work) and want the app to consistently target one of them regardless of which is
  marked default.

Set it for the current PowerShell session before launching the app:

```powershell
$env:WINMUX_DISTRO = "Ubuntu-24.04"
npm run tauri dev
```

or persist it for your user account (`setx WINMUX_DISTRO "Ubuntu-24.04"`, new shells only).

## 5. Spike verification checklist (regression reference)

The verification checklist — OSC passthrough, Claude Code/Codex behavior, IME, flow control,
renderer comparison, RAM measurement — is [`docs/plans/spike-plan.md`](plans/spike-plan.md)
section 6 ("Windows Spike 검증 체크리스트"). It was fully executed for the Spike sign-off
(results in ADR-0001) and, now that `apps/spike` is frozen as a measurement harness (section
2), doubles as the regression checklist for MVP-era changes. This runs against `apps/spike`;
`apps/winmux`'s own stage 10 checklist is section 6 below.

Scripts referenced by that checklist:

- [`scripts/wsl/osc-test.sh`](../scripts/wsl/osc-test.sh) — run **inside the WSL terminal that
  the Spike app opened**, from a Windows-side terminal you do *not* need this for. Emits OSC
  0/7/9/777 with both BEL and ST terminators, plus a chunk-split case.
- [`scripts/wsl/flood.sh`](../scripts/wsl/flood.sh) — `yes`-speed output burst (and an optional
  `--random-lines` high-entropy burst) for the flow control / backpressure check.
- [`scripts/wsl/scrollback-test.sh`](../scripts/wsl/scrollback-test.sh) — emits 12,000 lines to
  confirm the 5,000-line scrollback cap actually evicts old lines.
- [`scripts/wsl/claude-hook-example.md`](../scripts/wsl/claude-hook-example.md) — the canonical
  OSC contract (`winmux:` status tokens, title/cwd) plus the Claude Code hook and shell-prompt
  snippets that emit it, for the agent-notification half of the checklist.
- [`scripts/win/measure.ps1`](../scripts/win/measure.ps1) — run from a **Windows** PowerShell
  prompt (not inside WSL) while the Spike app is running, to record private working set (WebView2
  process tree included) over time and export it to CSV:

  ```powershell
  .\scripts\win\measure.ps1 -ProcessName winmux-spike -IntervalSec 5 -Samples 12 -OutCsv .\ram-4pane.csv
  ```

  No administrator privileges are required.

## 6. Stage 10 manual verification checklist

This was stage 10's completion gate on top of the automated gates in `CLAUDE.md`; it
**passed on Windows** (decisions distilled into
[`docs/adr/0002`](adr/0002-stage10-architecture.md)) and remains here as a regression
checklist for later work on the attach protocol and dispatcher.

1. **Boot** — launching the app auto-creates a workspace and a terminal tab (the
   dispatcher issues `CreateWorkspace` + `CreateTab` from Tauri `setup`, dogfooding the
   same `Command` bus the UI will use later); the terminal accepts input immediately.
2. **Reload survives** — type something with a distinguishable marker (e.g. `echo
   RELOAD-MARK-1`), then reload the WebView with **Ctrl+Shift+R** (or `window.__winmux.reload()` from the
   dev console). Plain F5 is *not* a reload key here — with the terminal focused, xterm
   correctly delivers F5 to the shell as `ESC[15~` (TUI apps like htop use it), which is
   why pressing it just prints a stray `~`.
   The session and its printed text must still be there afterward — that's the stage 10
   bar ("세션 생존 + 텍스트 보존"); pixel-perfect redraw of the TUI screen itself is out
   of scope until stage 14 (plan section 0-2).
3. **Dev-hook commands land** — from the WebView dev console, drive
   `window.__winmux.dispatch(...)` with `CreateTab`, `CloseTab`, and `SplitPane` commands.
   Each should update the `state-changed` snapshot, and closing tabs/panes must not leave
   orphaned WSL/shell processes behind (check via Task Manager, or `ps` inside WSL).
4. **IDs are stable across reload** — note the `Pane`/`Tab` ids from `get_state` (or the
   dev hook's command output) before reloading, reload, and confirm they're unchanged
   afterward.
5. **Background tab stays free-running** — create a second terminal tab, start a long
   noisy command in it (e.g. `seq 1000000`), switch back to the first tab, wait a few
   seconds, then check `window.__winmux` dev hook → `get_stats` (or `invoke("get_stats")`):
   the background session must show `paused: false` and keep making progress.
   *(Historical note: when this item was written, tab switching disposed the view and
   detached its channel. Since stage 12 landed keep-alive views, switching tabs keeps the
   hidden view attached and acking — the item still holds, it now verifies the keep-alive
   ack path instead of detach-on-dispose.)*
6. **`root_path` workspace spawns in the right directory** — dispatch
   `createWorkspace` with a `rootPath` (absolute **Linux** path, e.g. `/home/<user>`),
   create a terminal tab in it, and confirm `pwd` prints that path. This is the first
   real-world use of the `wsl.exe --cd <path>` mapping (spike only ever used
   `wsl.exe -- bash -l`); relative or Windows-style paths are not supported there.

## 7. Stage 11–12 manual verification checklist

This was stages 11–12's completion gate on top of the automated gates; it **passed on
Windows 2026-08-09** (decisions distilled into
[`docs/adr/0003`](adr/0003-split-tab-ui-architecture.md)) and remains here as a
regression checklist for split/tab UI work. All UI is mouse-driven; the dev hook is only
needed where noted.

1. **Splits render and nest** — use the pane-header icons to split left/right and
   top/bottom, then split one of the halves again. Layout must match the icon direction
   (horizontal = side by side, vertical = stacked), each new pane opens with a running
   terminal tab (atomic `SplitPane{tab}` — no empty-pane flash), and focus moves to the
   new pane.
2. **Splitter drag survives reload** — drag a splitter (live preview while dragging, no
   command spam), release, then reload (Ctrl+Shift+R). The adjusted ratio must persist (it lives in Rust
   state, not the DOM). Also observe: if a structural change arrives mid-drag (e.g. a
   session exits in another pane), the drag is deliberately abandoned — the preview snaps
   back and no resize command is sent (expected behavior, not a bug).
3. **Tabs: create/switch/close** — multiple terminal tabs per pane via the header icon;
   switching is instant with **no replay flash** (keep-alive views — the terminal content
   must not visibly re-render from scratch); closing a tab kills only that session.
   **A single click on an *inactive* pane's tab must land** (activate the tab, not just
   focus the pane) — regression guard for a mid-click re-render that used to require two
   clicks.
4. **Hidden tab keeps flowing** — run `bash ~/code/winmux/scripts/wsl/flood.sh 10` in a
   tab, switch away, wait, switch back: the buffer shows the latest output and
   `get_stats` shows `paused: false` throughout (hidden views keep acking).
5. **Unvisited tab after reload keeps flowing** — create a second tab, start `seq
   1000000` in it, reload (Ctrl+Shift+R), and do **not** click that tab. Check `get_stats`: the session must
   show `paused: false` (the boot reconcile sweeps `detach_terminal` over unattached
   sessions — the post-reload freeze fix). Then click the tab: latest output appears via
   replay.
6. **Last tab closes the pane** — closing the last tab of a pane collapses the pane
   (sibling takes the space, focus falls back); closing the last tab of the *last* pane
   leaves an empty pane with the placeholder, and the header icons still work from there.
   After any tab close, keyboard input must land in the surviving tab **without an extra
   click** (focus compensation — a removed xterm otherwise drops focus to the body).
7. **2×2 reload** — build a 2×2 layout with running TUIs (e.g. `htop`), reload (Ctrl+Shift+R): all four
   panes re-attach with their sessions and text intact, and the TUIs redraw after the
   resize nudge. Pane/Tab/Split ids unchanged (`get_state`).
8. **Errors surface** — from the dev console, dispatch `resizeSplit` with a stale id
   (e.g. `{ type: "resizeSplit", split: 9999, ratio: 0.5 }`): the status line shows the
   error and the layout stays consistent.
9. **RAM reference** — with the 2×2 layout idle, run `scripts/win/measure.ps1
   -ProcessName winmux` and note the total against the 계획 v2 section 16 budget
   (≤150MB); this is a reference point, not a hard gate for these stages.

## 8. Stage 13 manual verification checklist

This is stage 13's completion gate on top of the automated gates: the workspace sidebar
— create/switch/close workspaces from the UI (계획 v2 section 17 stage 13; design
decisions in [ADR-0004](adr/0004-lifecycle-persistence-reset.md)). All
interactions are mouse-driven in the sidebar; the dev hook is only needed where noted.

1. **Boot workspace card** — on launch the sidebar shows one card for the boot
   workspace with the active highlight, its name, and the status line (`idle` until an
   agent reports otherwise), matching the workspace rendered on the right. (The card
   carries no pane/tab counts and no git branch — see §10 item 11.)
2. **Create via the sidebar form** — click "+ New workspace", enter a name (rootPath
   optional — absolute **Linux** path, e.g. `/home/<user>`), submit. A new card
   appears, the app switches to the new workspace, and a terminal tab is already
   running with keyboard focus (atomic `CreateWorkspace{tab}` — no empty-workspace
   flash). If a `rootPath` was given, `pwd` prints it.
3. **Background workspace keeps flowing** — in the first workspace start a long noisy
   command (e.g. `seq 1000000` or `bash ~/code/winmux/scripts/wsl/flood.sh 10`), switch
   to another workspace via its card, wait a few seconds, then check `get_stats` from
   the dev console: the background session must show `paused: false` and keep making
   progress (leaving a workspace disposes its views; the detach sweep frees the
   channels so nothing sticks at paused).
4. **Switch-back restores** — switch back to the first workspace: layout, tabs, and
   terminal output are restored via replay (lazy attach), and keyboard input lands in
   the active pane **without an extra click** (focus compensation). Pane/Tab ids
   unchanged (`get_state`).
5. **Close kills sessions** — click a card's ×: a confirm dialog appears when the
   workspace has **running** terminal sessions (exited-only workspaces close without
   one); **cancelling the dialog must change nothing** (workspace and sessions stay).
   After confirming, the card disappears and no orphaned WSL/shell processes remain
   (Task Manager, or `ps` inside WSL). Closing the *last* workspace leaves the empty
   state ("no workspace" on the right), and the sidebar form still creates a fresh
   workspace from there.
6. **Reload keeps the workspace list** — with 2+ workspaces, reload (Ctrl+Shift+R):
   the card list, active workspace, and all ids are unchanged (state lives in Rust,
   the WebView is just a view).

## 9. Stage 14–16 / Checkpoint 1 manual verification

This is **checkpoint 1** (roadmap decision in `CLAUDE.md`): the batched manual Windows
verification for stages 13–16 — workspace sidebar (section 8 doubles as its checklist),
replay trim + switch latency tracer (stage 14), persistence (stage 15), and the automatic
UI reset safety net (stage 16, 계획 v2 section 12). It **passed on Windows 2026-08-09**
(decisions and ConPTY field findings distilled into
[`docs/adr/0004`](adr/0004-lifecycle-persistence-reset.md)) and remains here as a
regression checklist.

### Auto-reset environment variables

The reset supervisor reads six environment variables at app start (set them in the
PowerShell session before `npm run tauri dev`, e.g. `$env:WINMUX_RESET_IDLE_SECS = "30"`).
`0` disables the trigger it belongs to; invalid values fall back to the default with a
loud stderr warning. The effective config is printed to stderr on boot
(`[winmux] reset: config ...`).

| Variable | Default | Meaning |
|---|---|---|
| `WINMUX_RESET_IDLE_SECS` | `1800` | Idle reset: fire once this many seconds after the last real user input (`0` = off). Re-arms only on the next real input. |
| `WINMUX_RESET_HIDDEN_SECS` | `600` | Hidden reset: fire after the window stays unfocused **and** invisible for this long continuously (`0` = off). Once per hidden stretch. |
| `WINMUX_RESET_MEM_MB` | `1536` | Memory watchdog: when the WebView2 process tree's private memory exceeds this many MB, schedule a reset for the next safe moment (`0` = off). |
| `WINMUX_RESET_MEM_POLL_SECS` | `60` | Watchdog sampling period. `0` is rejected (would busy-loop) — default + warning. |
| `WINMUX_RESET_SAFE_IDLE_SECS` | `60` | Seconds since the last input for a pending watchdog reset to count as "safe" (`0` = immediately safe). |
| `WINMUX_RESET_COOLDOWN_SECS` | `300` | Suppression window after any reset fires (`0` = no cooldown). |

Reset activity is stderr-only by design (no UI): look for `[winmux] reset: reloading
webview (trigger=...)` lines. A manual reset is available from the dev console as
`window.__winmux.resetUi()` (dev hook / future MCP only — there is deliberately no UI
button, 계획 v2 section 12).

### Checklist

1. **Persistence round-trip** — build 2 workspaces with splits, tabs, and an adjusted
   splitter ratio → quit and restart the app → the full structure (workspaces, panes,
   tabs, ratios, active selections, ids) is restored, and every terminal tab runs a
   **fresh shell** (session content is not persisted — only structure).
2. **Exited tabs stay exited** — exit a shell (`exit`) so the tab shows the exited badge,
   restart the app → the tab is still Exited with empty content (no respawn), and the app
   is otherwise fully functional.
3. **Corrupt state recovers loudly** — corrupt `state.json` (e.g. truncate it) in the app
   data dir, restart → the app starts fresh, keeps the original as
   `state.json.corrupt-<epoch>`, and logs the reason to stderr.
4. **Switch latency readout** — build a 4-pane workspace plus a second workspace, switch
   back and forth → read `window.__winmux.lastSwitch` in the dev console: total should be
   in the ~100ms class, with per-tab replay timings populated.
5. **Replay trim keeps lines whole** — flood 1MB+ of colored output (e.g.
   `bash ~/code/winmux/scripts/wsl/flood.sh`), switch away and back → the top of the
   restored buffer starts at a line boundary with no broken escape sequences /
   half-colored garbage.
6. **Idle reset fires once, invisibly** — set `WINMUX_RESET_IDLE_SECS=30` (and
   `WINMUX_RESET_COOLDOWN_SECS=0` for this item), leave the app alone for 30s → exactly one
   reset fires (stderr `trigger=idle`); sessions and terminal text survive and the reload
   is visually seamless. Keep waiting another 30s **without touching anything**: no
   second reset — the post-reset automatic attach/resize/ack must **not** re-arm the idle
   timer (only real input does). **Repeat the whole item with `vim` (or Claude Code)
   running in a tab**: TUI apps emit terminal queries (DA/DSR/color) that xterm
   auto-answers after the replay, and those synthetic writes must not re-arm idle either
   (review finding — stdin writes deliberately don't count as activity; only the
   frontend's real-gesture ping does).
7. **Hidden reset, and never while typing** — set `WINMUX_RESET_HIDDEN_SECS=30`, minimize
   or fully cover + unfocus the window for 30s → reset fires. Then keep the window
   focused and type continuously for well past 30s → **no** reset ever fires (guards
   against a spurious `Focused(false)` misdetection — real input re-arms the hidden
   countdown too).
8. **Mem watchdog waits for a safe moment** — set `WINMUX_RESET_MEM_MB=100` (trivially
   exceeded) → while typing/scrolling nothing fires; stop touching the app for
   `WINMUX_RESET_SAFE_IDLE_SECS` (or switch workspaces) → the pending reset fires
   (`trigger=memWatchdog` with the sampled bytes in stderr).
9. **Scrollback reading is activity** — with `WINMUX_RESET_IDLE_SECS=30`, read scrollback
   using **wheel only** (no keys) for over 30s → no reset fires (the throttled activity
   ping counts pure viewing as activity).
10. **Kill survives** — force-kill the app from Task Manager, restart → state is restored
    except at most the last ≤500ms of structural mutations (save debounce window).

## 10. Stage 17+ / Checkpoint 2 manual verification

This section collects the batched manual Windows verification for the stages after
checkpoint 1, to be run at **checkpoint 2** (after stage 21). Items are appended per
stage as each lands.

<!-- retired 2026-08-10 — see ADR (agent-facing channel is a v2 follow-up); the checklist below stays as a historical record of what was verified. -->

### Stage 17 — pane-to-pane text send (계획 v2 section 8)

The pane header gains two send icons: `⤷` (send selection) and `⤷⏎` (send & run).
Clicking one captures the current selection of that pane's shown terminal and enters
target-selection mode (status line prompt, crosshair cursor); the next primary-button
mousedown on a pane delivers.

1. **Send selection** — select text in one terminal, click that pane's `⤷` icon: the
   status line shows `send: click a pane to send to (Esc cancels)`; click another pane →
   the text appears on the target's input line **without executing** (no Enter is sent),
   and the prompt clears.
2. **Send & run is a separate gesture** — repeat with `⤷⏎`: the text is pasted *and*
   followed by exactly one CR in the target (a shell command runs once). The two icons
   stay visually and behaviorally distinct (mis-run guard).
3. **Bracketed paste safety (vim target)** — open `vim` in the target pane, enter insert
   mode, and send a multi-line selection with `⤷` (send only): the text lands as a paste —
   no autoindent staircase, no literal `ESC[200~`/`ESC[201~` fragments, and **nothing is
   executed** (send-only must never run anything in the target, TUI or shell).
4. **Multi-line to a non-bracketed target is refused** — with a target whose foreground
   program does *not* enable bracketed paste (e.g. plain `cat` waiting on stdin, or a
   bare shell with bracketed paste off), sending a **multi-line** selection with either
   icon is refused with a status-line error (`cannot send multi-line: target is not in
   bracketed paste mode`), and **nothing** is written to the target. Rationale: without
   bracketed paste the target cannot distinguish pasted newlines from Enter, so the
   intermediate lines would execute — refusing is the only safe behavior. A
   **single-line** send to the same target still works (and with `⤷` never executes).
5. **No selection surfaces an error** — with no selection in the source terminal (or with
   an empty pane / non-terminal placeholder shown), clicking either icon shows a one-shot
   status-line error (`no selection to send` / `cannot send: no terminal shown in this
   pane`) and does **not** enter target-selection mode.
6. **Esc and self-click cancel** — arm send mode, press Esc → the prompt clears, nothing
   is sent, and the next pane click focuses normally (the Esc must not leak into the
   terminal). Arm again and click the **source** pane itself → cancelled the same way
   (self-send is meaningless).
7. **Workspace switch auto-cancels** — arm send mode, then switch to another workspace
   (sidebar click): the prompt clears and the mode is cancelled — a pane click in the new
   workspace focuses normally instead of delivering (cross-workspace send is out of scope
   for v1).
8. **Exited target surfaces an error** — arm send mode and click a pane whose shown
   terminal has **exited** (run `exit` there first): a status-line error appears
   (`cannot send: target terminal has exited`) and nothing is silently dropped.
9. **Send & run ordering** — with `⤷⏎` and a multi-line selection into a bracketed-paste
   shell, the full pasted text always lands **before** the single CR (the command that
   runs is the complete pasted text, never a truncated prefix).

### Stage 18 — OSC notification routing + coalescing + keyed reconcile (계획 v2 section 9; contract in [ADR-0006](adr/0006-osc-notification-routing.md) and [`scripts/wsl/claude-hook-example.md`](../scripts/wsl/claude-hook-example.md))

OSC 777/9 agent notifications and OSC 0 (alias "2")/7 are now routed into the Rust model
through a 100ms-trailing-window coalescer (`OscRouter`), surfacing on three layers: the
tab's unread dot, the pane header's aggregate badge (`●`), and the workspace sidebar card
(status text, message preview, aggregate unread dot). The frontend applies snapshot
updates with keyed in-place reconcile (sidebar cards and tab-strip entries patch by id
instead of rebuilding the DOM) — this is what guards the ADR-0003 d7 mid-click swallow
regression once the model fields are dynamic.

1. **Synthetic OSC routing (`osc-test.sh`)** — with a background tab (not the pane's
   shown terminal, in a non-active workspace), run
   [`scripts/wsl/osc-test.sh`](../scripts/wsl/osc-test.sh)'s OSC 777 cases (7–9; 9 is the
   chunk-split case) and OSC 9 cases (5–6) against it: the tab gets an unread dot, the pane header's `●` badge lights,
   and the workspace's sidebar card shows the aggregate unread dot (OSC 9 / a
   token-mismatched 777 are status-neutral — `agentStatus` on the card must **not**
   change). Repeat while that tab is the pane's **shown** terminal: no dot appears at all
   (visible-tab suppression happens at apply time, not just on next activation).
2. **Real Claude Code + hook 3-tuple** — run Claude Code in a winmux tab with the
   `UserPromptSubmit`/`Notification`/`Stop` hooks from `claude-hook-example.md` wired up:
   submitting a prompt shows `running` (no dot), a permission prompt shows `needsInput`
   with the sidebar preview populated from the hook's message (dot set), and finishing a
   turn shows `idle` (dot set, preview persists — an empty body never clears the previous
   message). Activating the tab clears its dot immediately.
3. **needsInput priority across tabs** — with one tab's session at `needsInput`, trigger
   `winmux:running` on a **different** tab (`osc-test.sh` case 10, or another hook run): the
   workspace's sidebar status stays `needsInput` — only the same tab that raised it
   (`agentStatusSource`) can demote it, which happens naturally once its own
   `UserPromptSubmit` fires `running`.
4. **Click during a live title update (d7 regression guard)** — with a tab emitting OSC
   0/2 titles on a fast loop (or repeated `osc-test.sh` case 1/2 runs) so the tab strip is
   patching in place, click that tab repeatedly: activation must land every time, never
   swallowed by a re-render replacing the clicked element underneath the pointer.
5. **[conditional] cwd restore across restart** — `cd` to a distinct directory in a tab
   (letting the `~/.bashrc` `PROMPT_COMMAND` snippet from `claude-hook-example.md` emit
   OSC 7), quit and restart the app → the respawned shell's `pwd` matches that directory.
   ConPTY's real-world OSC 7 emission is an **unverified precondition** (plan risk,
   [ADR-0006](adr/0006-osc-notification-routing.md); verified at checkpoint 2) — if this
   fails it is **not a stage blocker**: fall back to the reduced scope (title/cwd routing
   stays landed, only the restart-respawn behavior is unverified) and revisit the
   file/socket cwd-passing alternative from 계획 v2 section 2 separately; it is independent
   of the notification path covered by items 1–4 and 6–8.
6. **OSC flood** — run [`scripts/wsl/flood.sh`](../scripts/wsl/flood.sh) in a tab: the UI
   stays responsive throughout (coalescing keeps model updates at the 100ms flush cadence
   regardless of OSC volume — `WINMUX_OSC_FLUSH_MS`), the persistence Saver's debounce cadence
   is undisturbed, and RAM stays stable (no unbounded growth from the flood).
7. **Closing a needsInput tab returns the sidebar to idle** — with a tab at `needsInput`,
   close it via **CloseTab**, then repeat and close via **ClosePane** instead: in both
   cases, if that tab was the workspace's `agentStatusSource`, the sidebar reverts to
   `idle` with no lingering dot.
8. **Restart clears notifications and status (sanitize)** — with a tab left at
   `needsInput`/`idle` with an unread dot and a sidebar preview message, restart the app:
   every workspace's `agentStatus` is `idle` with no `agentStatusSource` or
   `lastAgentMessage`, and every tab's `notification` is cleared — same guarantee as the
   existing `pty_session` reset, extended to the new notification fields.

### Stage 20 — three-tier keyboard navigation (계획 v2 "키보드 모델"; the canonical interception list lives in the [`apps/winmux/src/keys.ts`](../apps/winmux/src/keys.ts) module doc)

One movement key per tier: `Ctrl+1`…`Ctrl+9` (workspace), `Alt+arrows` (pane focus, by
on-screen adjacency), `Ctrl+Tab` / `Ctrl+Shift+Tab` (tab cycle inside the active pane).
All three are window-level capture handlers, so they work with the terminal focused. When
a key has no target (ordinal past the last workspace, already-active workspace, no pane in
that direction, 0–1 tabs) the app does nothing — silently, with no status-line error.

1. **Workspace switch (`Ctrl+1`…`Ctrl+9`)** — with 2+ workspaces and a terminal focused,
   press `Ctrl+2`: the sidebar's **second** card becomes active (1-based, sidebar order)
   and focus lands in that workspace's active pane (typing goes to its terminal
   immediately). Pressing the ordinal of the **already active** workspace, or an ordinal
   past the last card (e.g. `Ctrl+9` with 3 workspaces), does nothing at all.
2. **Pane focus move (`Alt+arrows`)** — in a workspace split into 2x2 panes, `Alt+→` /
   `Alt+↓` / `Alt+←` / `Alt+↑` move the active-pane highlight to the geometrically
   adjacent pane each time, and the newly focused pane's terminal receives typing. At an
   edge (e.g. `Alt+→` from the rightmost pane) and in a single-pane workspace, nothing
   happens and no error appears.
3. **Tab cycle (`Ctrl+Tab` / `Ctrl+Shift+Tab`)** — in a pane with 3 tabs, `Ctrl+Tab`
   advances through them in tab-strip order and **wraps** from the last back to the first;
   `Ctrl+Shift+Tab` walks the same cycle backwards. The activated tab's terminal takes
   focus. In a pane with 0 or 1 tabs, nothing happens.
4. **Intercepted keys never reach the shell** — with a shell prompt focused, press each of
   the keys above (including the no-op cases from items 1–3, such as `Ctrl+9` with fewer
   workspaces): the command line stays empty — no stray digits, no `^I`/tab completion
   triggered, no escape-sequence garbage — and no line is ever submitted. Then confirm the
   keys **not** in the interception list still belong to the terminal: a bare `Tab`
   completes a path, bare arrow keys walk shell history/cursor, and `Ctrl+C` still
   interrupts a running command.
5. **[conditional] `Ctrl+Tab` reaches the page in WebView2** — item 3 depends on WebView2
   delivering `Ctrl+Tab` to the page instead of consuming it as a host-level shortcut,
   which is an **unverified precondition** (plan risk). If `Ctrl+Tab` produces no tab
   change *and* leaves nothing in the terminal, the interception itself is fine but the
   key never arrives: this is **not a stage blocker** — items 1, 2 and 4 stand on their
   own, and the follow-up is to pick a replacement binding (e.g. `Ctrl+PgUp`/`Ctrl+PgDn`)
   in `keys.ts` and re-run item 3. Record which behavior you observed.

### Stage 21 — viewer tabs (folderBrowser / textViewer / markdownViewer; 계획 v2 "탭 타입별 동작"; contract in [ADR-0008](adr/0008-viewer-tabs.md))

Tabs are no longer terminals only. The pane header's `▤` icon opens a **folderBrowser**
tab, and clicking a file row there opens a viewer tab in the same pane — a
**markdownViewer** for `.md`/`.markdown`, a **textViewer** for everything else. All three
are viewers, never editors. The Rust side reads the WSL filesystem through
`\\wsl.localhost\<distro>\...` (`fs_list_dir` / `fs_stat` / `fs_read_chunk`), so these
items exercise a Windows→WSL path that has no equivalent on the Linux dev host — none of
it can be checked before this checkpoint. Two contracts drive most of the items:
navigation is a **dispatcher command** (`navigateFolder`), so the current path is part of
the persisted model; and a viewer tab is mounted **only while it is the active tab of its
pane**, so leaving and returning is a real unmount/remount.

1. **Folder browser opens on the workspace root** — click the pane header's `▤` icon: a
   new tab opens in that pane listing the workspace's `rootPath` (`/` when the workspace
   has none), with **directories first** and each group sorted by name (case-insensitive).
   Directory rows end with `/`, file rows show a size, and a directory with more than
   5,000 entries shows the truncation banner instead of hanging.
2. **Navigation goes through the model** — click into a subdirectory, then use the `..`
   row to come back: the listing and the **tab title** follow the path each time. Now
   restart the app: the tab reopens on the **last visited path**, and the listing is a
   fresh read — create a file in that directory from WSL before restarting and it is there
   without any refresh gesture.
3. **A huge file opens instantly and stays bounded** — from WSL, make a few-hundred-MB log
   (`yes "$(date)" | head -c 400M > /tmp/big.log`) and click it in the folder browser: the
   text tab appears **immediately** (no multi-second freeze), and the bar above the text
   reads `bytes 0–… of …`. With `scripts/win/measure.ps1 -ProcessName winmux` taken before
   and after, the private working set grows by **less than 20MB**. Then walk with `next` /
   `prev` / `last` / `first`: each button loads exactly one 512KiB window, the byte range
   updates, and the working set does **not** grow with the number of jumps — only one
   window is resident, and scrolling to the end of a window never continues automatically.
4. **Scroll position survives unmount and restart** — scroll to the middle of a text tab,
   switch to another tab in that pane and back: the same lines are on screen (the position
   is recorded ~0.5s after scrolling settles, and immediately on leaving the tab). Restart
   the app: the same lines again. Repeat once in a window **other than the first** (jump
   with `next`, scroll, leave, return) — the recorded value is a byte offset, so the
   restored view must land in that window, not back at the top of the file.
5. **Background viewer tabs hold no DOM** — with a viewer tab in the background (another
   tab active in that pane), inspect that pane's `.pane-content` in devtools: there is no
   `.folder-view` / `.text-view` element for the background tab. Activating it again
   re-mounts and re-reads.
6. **Missing and deleted files surface inline** — with a text tab open, delete the file
   from WSL, then leave the tab and come back: the tab **stays open** and shows
   `cannot read <path>: …` where the content was. Same shape for a folder browser whose
   directory was removed (`cannot list <path>: …`), and its `..` row still navigates out.

7. **Markdown renders and reloads live** — click a `.md` file in the folder browser: it
   opens **rendered** (headings, lists, code blocks), not as source. With that tab active,
   append a line from WSL (`echo '## appended' >> notes.md`): the rendered view picks it up
   within **2–4 seconds** without any gesture, and the scroll position stays where it was.
   Now switch to another tab in that pane and, from WSL, append again: nothing polls while
   the viewer is unmounted, and coming back re-reads once and shows the new content.
   Minimize the window (or switch to another app so the WebView reports `document.hidden`)
   and confirm from devtools that no `fs_stat` traffic continues while hidden; restoring
   the window resumes the 2s cycle. Finally, open a `.md` file **larger than 2MiB**: it
   refuses to render, says so in the banner, and offers **open as text** — clicking that
   opens the same path in a textViewer tab.
8. **Raw HTML is inert and links do nothing** — put this in a `.md` file and open it:
   `<script>alert('x')</script>`, `<img src=x onerror=alert(1)>`, `[click](javascript:alert(1))`,
   `![alt](https://example.com/pic.png)`. No dialog ever appears; the script and img tags
   are displayed **as literal text**; the image is a `[image: alt]` placeholder with no
   network request (check devtools Network); and clicking the link does nothing — no
   navigation, no new window, and the anchor has no `href` in the inspector. This is the
   security contract of the markdown viewer: this WebView holds the `dispatch`/`fs_*` IPC,
   so file-borne HTML must never reach the DOM.
9. **Locked-down distro** — repeat items 1–4 in a workspace whose distro has `automount`
   and `interop` disabled in `/etc/wsl.conf` (then `wsl --shutdown`): the viewers still
   work. File access runs Windows→WSL over `\\wsl.localhost`, the direction those settings
   do not gate (계획 v2 section 5).
10. **No edit affordance anywhere** — no viewer offers any way to change the file: no
    editable field, no rename/delete/save control, typing into a focused text or markdown
    view does nothing, and the only file-scoped controls are the text viewer's window
    movement buttons and the markdown viewer's **open as text** button (both read-only).
11. **Terminals stay alive across viewer switches** — start a long-running command (e.g.
    `top`) in a terminal tab, switch to a viewer tab in the same pane and back: the
    terminal is exactly where it was and still running, with **no replay flash** and no
    re-attach. Mounting a viewer must not disturb the keep-alive terminal views.
12. **Unconfigured distro resolves automatically** — with **no** workspace distro and
    **no** `WINMUX_DISTRO` (section 4), open a folder browser and a text file: both work,
    because the glue falls back to the WSL default distro (`wsl.exe -l -q`, cached for the
    process lifetime). Then set `WINMUX_DISTRO` to a second installed distro, restart, and
    confirm the viewers read **that** distro's filesystem. If every resolution path fails
    (e.g. no distro installed), the inline banner must say so loudly and name the fix
    (workspace distro or `WINMUX_DISTRO`) — never a silently empty listing.

### Post-checkpoint-2 fixes and keyboard-first UX — re-verification

Checkpoint 2 (2026-08-09) passed except three field defects; the fixes below plus the
keyboard-first UX batch need one focused re-verification round. Pull, run
`npm install` (no new runtime deps, but the lockfile moved), delete
`scripts/wsl/*.sh` and `git checkout -- scripts` once so the new `.gitattributes`
(`*.sh text eol=lf`) re-materializes them with LF endings, then rebuild.

1. **Hook tty fallback** — rewire the hooks from the updated
   [`scripts/wsl/claude-hook-example.md`](../scripts/wsl/claude-hook-example.md)
   (the canonical script now tries `/dev/tty` first and then walks up to 8 ancestor
   processes for a `/dev/pts/*` fd — the approach you field-tested, formalized). Run a
   real Claude Code session: running → needsInput → idle must route as before, with no
   `/dev/tty: No such device or address` in the hook's stderr.
2. **Markdown polling stops while minimized** — open a markdownViewer tab, minimize the
   window: `fs_stat` polling must stop within one 2s cycle (verify by appending to the
   file while minimized — no re-render happens until restore). Restore: polling resumes
   and the change lands within ~2-4s. Also confirm **no false positives**: dragging the
   window edge to resize and focusing another window (winmux still visible) must NOT stop
   polling — the live-preview-while-editing-elsewhere flow depends on it. (The minimize
   signal is a 0x0-Resized heuristic that cannot be checked on the Linux host.)
3. **Shell scripts run directly** — `bash scripts/wsl/osc-test.sh` works without the
   `sed 's/\r$//'` workaround after the re-checkout above.
4. **Global shortcuts** — `Ctrl+Shift+W` (close active tab, viewer tabs included; on the
   last empty pane it is a quiet no-op), `Ctrl+Shift+T` (new terminal tab),
   `Ctrl+Shift+D` (split top/bottom), `Ctrl+Shift+E` (split left/right),
   `Ctrl+Shift+B` (folder browser tab), `Ctrl+Shift+N` (focus the sidebar's new-workspace
   name input). Each must act **and** leave nothing in the terminal. These are Chromium
   accelerator combos (incognito/reopen-tab/bookmarks-bar/bookmark) — WebView2 usually
   lacks those features, but if any key does nothing at all, record it: the follow-up is
   `AreBrowserAcceleratorKeysEnabled(false)` or a rebinding.
5. **Tooltips show shortcuts** — hovering the pane-header buttons (`+`, `▤`, split pair),
   the tab `×`, and the sidebar's new-workspace button shows the function plus its
   shortcut (single source: `keys.ts shortcutLabel`).
6. **Folder browser keyboard navigation** — with the folder list focused: arrows move the
   selection highlight, `Home`/`End` jump, `PgUp`/`PgDn` move by 10, `Enter` opens the
   selected row (directory navigates, file opens a viewer), `Backspace` goes to the
   parent. `Alt+arrows` must still move pane focus (the view only consumes unmodified
   keys). Verify a mouse click moves the selection too, and that keyboard navigation
   still works right after opening a directory by mouse.
7. **Text viewer windows by keyboard** — with the text view focused: `Ctrl+PgUp`/
   `Ctrl+PgDn` move one 512KiB window, `Ctrl+Home`/`Ctrl+End` jump to the first/last
   window, plain `PgUp`/`PgDn` page by whole lines (no half-cut top line). Window buttons
   disable at the ends (first/prev at offset 0, next/last on the last window) and their
   tooltips name the shortcuts. **Last-line fix**: `Ctrl+End` on a large file must show
   the file's actual last line (a read-length bug used to make it unreachable).
8. **Window restore keeps context** — scroll mid-file in a >512KiB file, restart: the
   same top line is visible **and** you can scroll upward within the window (the window
   is now centered on the saved offset instead of starting at it).
9. **Per-tab shell history** — run distinct commands in two terminal tabs, restart the
   app: each respawned tab's `history` (and up-arrow) shows only its own tab's commands
   (`~/.winmux/history/tab-<id>` in the distro). Then close a tab normally and restart:
   report whether its history survived — bash writes `HISTFILE` on exit, and if the kill
   path skips it we need a `history -a` follow-up.
10. **Reload-while-minimized edge (known, accept)** — if the WebView reloads while the
    window is minimized (auto-reset), polling resumes until the next minimize/restore
    cycle. Accepted narrow window; no action needed unless it bites in practice.
11. **UI cleanup (display only — no feature was removed)** — the top status line is now
    ephemeral: at rest it is collapsed entirely (no `workspace: … · panes: … · rev …`
    log, and the terminal area starts right below the title bar). It appears only while
    send-mode is armed (the prompt) or for a dispatch error (red, gone after ~5s), and
    collapses again afterwards. Sidebar cards are three lines — name (+ unread dot, ×) /
    status text (`running` / `needs input` / `idle`, followed by ` — <last agent message>`
    when there is one, `needs input` being the only accented one) / abbreviated path;
    no more `⚡`/`🔔` icons, pane/tab counts, or branch field. Pane headers no longer show
    the `#<id>` label or the permanently disabled `◎` browser button — the unread `●`
    badge and the six working buttons (`+ ▤ ⤷ ⤷⏎ ◫ ⊟`) stay, with their tooltips intact.
12. **Rename migration (`wmux` → `winmux`)** — the project was renamed (the old name
    collided with an unrelated existing project). This is a one-time, single-developer
    migration handled by hand, not by migration code. On the Windows checkout:
    - **Remote** — GitHub redirects the old repository URL, but update it explicitly:
      `git remote set-url origin git@github.com:sjkwon-1023/winmux.git`. The local folder
      name is free (rename it to `winmux` or leave it — nothing reads it).
    - **App state (do this or you boot fresh)** — the Tauri identifier changed from
      `app.wmux.desktop` to `app.winmux.desktop`, so the state directory moved. Rename
      `%APPDATA%\app.wmux.desktop` to `%APPDATA%\app.winmux.desktop` and the existing
      workspaces/panes/tabs restore exactly as before. Skip it and the app boots with an
      empty state — nothing is lost, the data just sits in the old folder until you move
      it. (The spike's identifier moved `app.wmux.spike` → `app.winmux.spike` the same
      way, but it persists nothing.)
    - **Environment variables** — every `WMUX_*` knob is now `WINMUX_*`. If you had
      `WMUX_DISTRO` set (section 4), set `WINMUX_DISTRO` instead — the old name is no
      longer read, and a stale one silently does nothing. Same for any `WMUX_RESET_*` /
      `WMUX_OSC_FLUSH_MS` you set for the section 9 checks.
    - **Claude Code hooks** — the OSC status token is now `winmux:running` /
      `winmux:needsInput` / `winmux:idle`. Rewire the hooks from the updated
      [`scripts/wsl/claude-hook-example.md`](../scripts/wsl/claude-hook-example.md);
      hooks still emitting `wmux:*` land as status-neutral notifications (unread dot, no
      status change), which is exactly what a missed rewire looks like.
    - **Shell history in WSL** — `mv ~/.wmux ~/.winmux` in the distro keeps every tab's
      history (item 9). Without it each respawned tab starts with an empty history.

### Post re-verification polish — verification

The re-verification round above passed in full on 2026-08-10. The batch below landed
right after it: a `Shift+Enter` rewrite for agents, a sidebar reflow fix, a folder-first
new-workspace flow, and a surface cleanup (send buttons retired, icons redrawn). Rebuild
and check these; nothing here needs a fresh `npm install`.

1. **`Shift+Enter` inserts a newline in Claude Code** — in a terminal tab running Claude
   Code, type a word, press `Shift+Enter`: the prompt must grow a second line instead of
   submitting. Plain `Enter` still submits. The terminal now emits `ESC CR` for that
   combo itself, so this must work **without** running Claude Code's `/terminal-setup`.
   Then confirm it is harmless everywhere else: at a bash prompt `Shift+Enter` must
   behave like a normal `Enter` (runs the line / gives a fresh prompt — no stray escape
   character left on the line), and in `vim` insert mode it must still open a new line.
2. **Sidebar width never moves** — with a workspace whose card shows a long status line
   (run a Claude Code session so the status becomes `needs input — <a long agent
   message>`), watch the sidebar's right edge across `running` → `needs input` → `idle`
   transitions: it must stay pinned at 220px and the terminal area must not reflow. Long
   text is cut with an ellipsis on each of the card's three lines. Also drag the window
   narrower and confirm the sidebar still holds its width.
3. **Folder pick creates the workspace immediately** — the sidebar's new-workspace button
   opens the Windows folder dialog directly; there is no name field to fill in any more.
   Picking a folder creates the workspace **in one step** (name = folder name, root path =
   the converted Linux path) with a terminal tab already in it, and the new tab's shell
   starts in that directory (`pwd`). Cancelling the dialog does nothing at all (no error,
   no empty workspace). Verify both path shapes:
   - **WSL filesystem (UNC)** — pick something under `\\wsl.localhost\<distro>\home\...`:
     the workspace root must come out as the plain Linux path (`/home/...`) and the tab
     must run in **that distro** even when it is not the default one.
   - **Windows drive** — pick e.g. `C:\Users\<you>\code`: the root must come out as
     `/mnt/c/Users/<you>/code` and the tab starts there. (This assumes the distro's
     default automount root; a custom `/etc/wsl.conf` shows up as a `cd` failure.)
   - **Rejected paths** — create one from inside WSL (`mkdir '/tmp/bad:name'`), then pick
     it through the UNC view: the status line must show a loud path error and **no**
     workspace may appear. Names containing `:` or `\`, or ending in a dot or space, are
     refused on purpose — the same rule the viewer tabs use, so a bad name can never be
     assembled into a different Windows path than the one you clicked.
4. **`Ctrl+Shift+N` opens the same picker** — from anywhere (terminal focused included)
   it must open the folder dialog, not focus a sidebar field. Holding the keys down or
   double-pressing must not stack two dialogs. Nothing may leak into the terminal.
5. **`F2` renames the active workspace** — press `F2`: the active sidebar card's name
   turns into an inline text box, pre-filled and fully selected. `Enter` commits; `Esc`
   **and** clicking away both cancel and restore the old name. An all-whitespace name is
   refused — the box stays open and focused instead of sending anything. Committing an
   unchanged name sends nothing. While the box has focus, typing must land in the box and
   not in the terminal.
   Known trade-off: `F2` no longer reaches TUI apps (e.g. `mc`'s Rename) — confirm that
   is the only casualty and that `F1`/`F3`… still reach the terminal.
6. **`Ctrl+Shift+[` / `Ctrl+Shift+]` cycle workspaces** — with three or more workspaces,
   `]` moves to the next in sidebar order and wraps at the end, `[` moves back and wraps
   at the start; with a single workspace both are quiet no-ops. Check them on a keyboard
   layout where `[`/`]` need no modifier and, if you have one handy, on a layout where
   they do — both the bare characters and their shifted forms (`{`/`}`) are matched.
7. **Send buttons are gone** — the pane header now has exactly four buttons: new terminal
   tab, folder browser tab, and the two splits. The `⤷` / `⤷⏎` pair and its
   target-selection mode (crosshair cursor, status-line prompt, `Esc` to cancel) are
   **retired**, so `Esc` now always belongs to the terminal: press `Esc` in `vim` and it
   must leave insert mode on the first press. Clicking a pane always just focuses it.
   Selection + `Ctrl+Shift+C` copy is untouched. (Agent-facing text passing returns in v2
   as a designed channel, not as a manual button.)
8. **Redrawn icons** — the pane header's folder button is now a drawn folder outline, and
   the two split buttons are a matched pair: one rectangle split by a **vertical** line
   (left/right) and the same rectangle split by a **horizontal** line (top/bottom). At a
   glance it must be obvious which is which; click each and confirm the split direction
   matches its picture. The icons must inherit the header's text colour (including the
   hover background) and stay crisp at the OS display scaling you use. Tooltips still
   read `<function> (<shortcut>)`.
9. **App icon** — the exe, its taskbar button, the window's title-bar corner and
   `Alt+Tab` must all show the new icon: a blue `W` on a near-black square. Check it at
   small sizes too (taskbar, 16px file-explorer list view) — the `W` must stay readable,
   not a blue smudge. If Explorer still shows the old icon after a rebuild it is the
   Windows icon cache, not the build: `ie4uinit.exe -show` or a fresh folder view.

## 11. ARM64 cross-build notes

The dev machine that produced this repo's crates is x86_64; the eventual target device policy
(터미널-계획-v2.md section 13) is ARM64. Cross-compiling *from* this x64 Windows machine *for*
ARM64 Windows:

1. Add the ARM64 MSVC build tools individually component in the Visual Studio Build Tools
   installer (see section 1 above) — the x64 build tools alone do not include the ARM64 linker.
2. Add the Rust target:

   ```powershell
   rustup target add aarch64-pc-windows-msvc
   ```

3. Build with `--target`:

   ```powershell
   cd apps\spike
   npm run tauri build -- --target aarch64-pc-windows-msvc --no-bundle
   ```

Cross-compiled ARM64 binaries can only be *built* here — running them and doing the actual
Spike verification (ConPTY OSC passthrough, IME, RAM) requires real ARM64 hardware (or an
ARM64 VM), since this machine cannot execute ARM64 Windows binaries. `crates/winmux-core` itself
has no target-specific code (it's checked against `x86_64-pc-windows-msvc` in the WSL-side gate
per spike-plan.md section 5), so the ARM64-specific risk surface is `portable-pty`'s ConPTY
backend and Tauri/WebView2, not `winmux-core`.

### CI artifacts (stage 22) and device testing (stage 23)

Since stage 22, `.github/workflows/ci.yml` runs the full gate set (including
`cargo clippy --workspace --all-targets --target aarch64-pc-windows-msvc` — check-family
commands never link, so this needs no MSVC libraries and also runs on the Linux dev host)
on every push, and builds **release artifacts for both targets** on a manual
`workflow_dispatch` (GitHub → Actions → CI → Run workflow) or a `v*` tag: download
`winmux-aarch64-pc-windows-msvc` from the run's artifacts for the ARM64 device.

Stage 23 (device verification) runs on the ARM64 machine, WSL2 + ARM64 Ubuntu installed:
1. The artifact runs natively (Task Manager shows no emulation; ARM64 process).
2. Spike-era regression spot: OSC routing (`osc-test.sh`), IME (한글), flood
   responsiveness, copy/paste — sections 5–6 spot checks.
3. Checkpoint-2 spot: one item each from the Stage 17/18/20/21 subsections of §10.
4. RAM: `scripts/win/measure.ps1 -ProcessName winmux-app` with the 4-pane + viewer
   composition from checkpoint 2 — same 100–150MB acceptance band.
5. Claude Code inside ARM64 WSL (and Codex CLI if its Linux ARM64 binary exists — 계획
   v2 section 13 precheck) with the hook contract wired.
