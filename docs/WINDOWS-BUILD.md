# Windows Build Guide

How to set up a Windows machine to build and run wmux, and how to run the Windows-side
verification. Two apps share this guide:

- **`apps/wmux`** — the MVP app, active development from 계획 v2 section 17 stage 10
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
`src-tauri/` or `crates/wmux-core` trigger a rebuild; frontend changes hot-reload.

### Distributable exe (no installer/bundle)

```powershell
npm run tauri build -- --no-bundle
```

`--no-bundle` skips MSI/NSIS installer packaging (not needed for Spike verification) and leaves
a plain `wmux-spike.exe` under `apps\spike\src-tauri\target\release\`. This is the binary
[`scripts/win/measure.ps1`](../scripts/win/measure.ps1) expects by default (`-ProcessName
wmux-spike`, matching `productName` in `src-tauri/tauri.conf.json`).

## 3. Build and run `apps/wmux`

`apps/wmux` is the MVP app (계획 v2 section 17, stage 10 onward) — same Tauri v2 +
Node/npm toolchain as `apps/spike` above, but its Rust glue drives the `wmux-core`
`Dispatcher` over the single `Command` bus instead of spike's thin per-call commands.
Architecture: [`docs/adr/0002`](adr/0002-stage10-architecture.md) and
[`docs/adr/0003`](adr/0003-split-tab-ui-architecture.md).

From the repo root on Windows:

```powershell
cd apps\wmux
npm install
```

### Development (hot reload, dev console)

```powershell
npm run tauri dev
```

Same rebuild behavior as spike: Rust changes under `src-tauri/` or `crates/wmux-core`
trigger a rebuild, frontend changes hot-reload. On boot the app itself dispatches a
single atomic `CreateWorkspace{tab}` (from Tauri `setup`, before the frontend ever
attaches — stage 13 folded the earlier `CreateWorkspace` + `CreateTab` pair into one
command), so a terminal tab is already running when the window opens. Splits/tabs
(stages 11–12) and the workspace sidebar (stage 13) are mouse-driven; commands without
UI yet can still be driven from the WebView dev console via the dev hook
`window.__wmux.dispatch(command)`. See section 6 below for the stage 10 manual
checklist that exercises this.

### Distributable exe (no installer/bundle)

```powershell
npm run tauri build -- --no-bundle
```

Leaves a plain `wmux.exe` under `apps\wmux\src-tauri\target\release\` (distinct from
spike's `wmux-spike.exe` — the two apps' binaries don't collide).

## 4. `WMUX_DISTRO` environment variable

Both apps spawn the WSL shell as `wsl.exe [-d $WMUX_DISTRO] -- bash -l` — spike's glue
reads it directly per spawn (see spike-plan.md section 4.5); wmux threads it through
`wmux-core`'s `Command::CreateWorkspace` → `ShellSpawnReq::distro` (see
`crates/wmux-core/src/command.rs`), same underlying `wsl.exe` invocation either way.
`WMUX_DISTRO` selects which WSL distribution to spawn into:

- **Unset**: `wsl.exe` uses your default distribution (`wsl -l -v` shows which one has `*`).
- **Set**: `wsl.exe -d <name>` targets that distribution explicitly — useful if you have more
  than one installed (e.g. a plain Ubuntu install alongside an isolated/sandboxed distro for
  agent work) and want the app to consistently target one of them regardless of which is
  marked default.

Set it for the current PowerShell session before launching the app:

```powershell
$env:WMUX_DISTRO = "Ubuntu-24.04"
npm run tauri dev
```

or persist it for your user account (`setx WMUX_DISTRO "Ubuntu-24.04"`, new shells only).

## 5. Spike verification checklist (regression reference)

The verification checklist — OSC passthrough, Claude Code/Codex behavior, IME, flow control,
renderer comparison, RAM measurement — is [`docs/plans/spike-plan.md`](plans/spike-plan.md)
section 6 ("Windows Spike 검증 체크리스트"). It was fully executed for the Spike sign-off
(results in ADR-0001) and, now that `apps/spike` is frozen as a measurement harness (section
2), doubles as the regression checklist for MVP-era changes. This runs against `apps/spike`;
`apps/wmux`'s own stage 10 checklist is section 6 below.

Scripts referenced by that checklist:

- [`scripts/wsl/osc-test.sh`](../scripts/wsl/osc-test.sh) — run **inside the WSL terminal that
  the Spike app opened**, from a Windows-side terminal you do *not* need this for. Emits OSC
  0/7/9/777 with both BEL and ST terminators, plus a chunk-split case.
- [`scripts/wsl/flood.sh`](../scripts/wsl/flood.sh) — `yes`-speed output burst (and an optional
  `--random-lines` high-entropy burst) for the flow control / backpressure check.
- [`scripts/wsl/scrollback-test.sh`](../scripts/wsl/scrollback-test.sh) — emits 12,000 lines to
  confirm the 5,000-line scrollback cap actually evicts old lines.
- [`scripts/wsl/claude-hook-example.md`](../scripts/wsl/claude-hook-example.md) — the canonical
  OSC contract (`wmux:` status tokens, title/cwd) plus the Claude Code hook and shell-prompt
  snippets that emit it, for the agent-notification half of the checklist.
- [`scripts/win/measure.ps1`](../scripts/win/measure.ps1) — run from a **Windows** PowerShell
  prompt (not inside WSL) while the Spike app is running, to record private working set (WebView2
  process tree included) over time and export it to CSV:

  ```powershell
  .\scripts\win\measure.ps1 -ProcessName wmux-spike -IntervalSec 5 -Samples 12 -OutCsv .\ram-4pane.csv
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
   RELOAD-MARK-1`), then reload the WebView with **Ctrl+Shift+R** (or `window.__wmux.reload()` from the
   dev console). Plain F5 is *not* a reload key here — with the terminal focused, xterm
   correctly delivers F5 to the shell as `ESC[15~` (TUI apps like htop use it), which is
   why pressing it just prints a stray `~`.
   The session and its printed text must still be there afterward — that's the stage 10
   bar ("세션 생존 + 텍스트 보존"); pixel-perfect redraw of the TUI screen itself is out
   of scope until stage 14 (plan section 0-2).
3. **Dev-hook commands land** — from the WebView dev console, drive
   `window.__wmux.dispatch(...)` with `CreateTab`, `CloseTab`, and `SplitPane` commands.
   Each should update the `state-changed` snapshot, and closing tabs/panes must not leave
   orphaned WSL/shell processes behind (check via Task Manager, or `ps` inside WSL).
4. **IDs are stable across reload** — note the `Pane`/`Tab` ids from `get_state` (or the
   dev hook's command output) before reloading, reload, and confirm they're unchanged
   afterward.
5. **Background tab stays free-running** — create a second terminal tab, start a long
   noisy command in it (e.g. `seq 1000000`), switch back to the first tab, wait a few
   seconds, then check `window.__wmux` dev hook → `get_stats` (or `invoke("get_stats")`):
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
4. **Hidden tab keeps flowing** — run `bash ~/code/wmux/scripts/wsl/flood.sh 10` in a
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
   -ProcessName wmux` and note the total against the 계획 v2 section 16 budget
   (≤150MB); this is a reference point, not a hard gate for these stages.

## 8. Stage 13 manual verification checklist

This is stage 13's completion gate on top of the automated gates: the workspace sidebar
— create/switch/close workspaces from the UI (계획 v2 section 17 stage 13; design
decisions in [`docs/plans/mvp-stage13-plan.md`](plans/mvp-stage13-plan.md)). All
interactions are mouse-driven in the sidebar; the dev hook is only needed where noted.

1. **Boot workspace card** — on launch the sidebar shows one card for the boot
   workspace with the active highlight, a status icon, and pane/tab counts, matching
   the workspace rendered on the right. (Agent status/message and git branch stay
   blank/idle until stages 18–19 — the card just omits them.)
2. **Create via the sidebar form** — click "+ New workspace", enter a name (rootPath
   optional — absolute **Linux** path, e.g. `/home/<user>`), submit. A new card
   appears, the app switches to the new workspace, and a terminal tab is already
   running with keyboard focus (atomic `CreateWorkspace{tab}` — no empty-workspace
   flash). If a `rootPath` was given, `pwd` prints it.
3. **Background workspace keeps flowing** — in the first workspace start a long noisy
   command (e.g. `seq 1000000` or `bash ~/code/wmux/scripts/wsl/flood.sh 10`), switch
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
PowerShell session before `npm run tauri dev`, e.g. `$env:WMUX_RESET_IDLE_SECS = "30"`).
`0` disables the trigger it belongs to; invalid values fall back to the default with a
loud stderr warning. The effective config is printed to stderr on boot
(`[wmux] reset: config ...`).

| Variable | Default | Meaning |
|---|---|---|
| `WMUX_RESET_IDLE_SECS` | `1800` | Idle reset: fire once this many seconds after the last real user input (`0` = off). Re-arms only on the next real input. |
| `WMUX_RESET_HIDDEN_SECS` | `600` | Hidden reset: fire after the window stays unfocused **and** invisible for this long continuously (`0` = off). Once per hidden stretch. |
| `WMUX_RESET_MEM_MB` | `1536` | Memory watchdog: when the WebView2 process tree's private memory exceeds this many MB, schedule a reset for the next safe moment (`0` = off). |
| `WMUX_RESET_MEM_POLL_SECS` | `60` | Watchdog sampling period. `0` is rejected (would busy-loop) — default + warning. |
| `WMUX_RESET_SAFE_IDLE_SECS` | `60` | Seconds since the last input for a pending watchdog reset to count as "safe" (`0` = immediately safe). |
| `WMUX_RESET_COOLDOWN_SECS` | `300` | Suppression window after any reset fires (`0` = no cooldown). |

Reset activity is stderr-only by design (no UI): look for `[wmux] reset: reloading
webview (trigger=...)` lines. A manual reset is available from the dev console as
`window.__wmux.resetUi()` (dev hook / future MCP only — there is deliberately no UI
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
   back and forth → read `window.__wmux.lastSwitch` in the dev console: total should be
   in the ~100ms class, with per-tab replay timings populated.
5. **Replay trim keeps lines whole** — flood 1MB+ of colored output (e.g.
   `bash ~/code/wmux/scripts/wsl/flood.sh`), switch away and back → the top of the
   restored buffer starts at a line boundary with no broken escape sequences /
   half-colored garbage.
6. **Idle reset fires once, invisibly** — set `WMUX_RESET_IDLE_SECS=30` (and
   `WMUX_RESET_COOLDOWN_SECS=0` for this item), leave the app alone for 30s → exactly one
   reset fires (stderr `trigger=idle`); sessions and terminal text survive and the reload
   is visually seamless. Keep waiting another 30s **without touching anything**: no
   second reset — the post-reset automatic attach/resize/ack must **not** re-arm the idle
   timer (only real input does). **Repeat the whole item with `vim` (or Claude Code)
   running in a tab**: TUI apps emit terminal queries (DA/DSR/color) that xterm
   auto-answers after the replay, and those synthetic writes must not re-arm idle either
   (review finding — stdin writes deliberately don't count as activity; only the
   frontend's real-gesture ping does).
7. **Hidden reset, and never while typing** — set `WMUX_RESET_HIDDEN_SECS=30`, minimize
   or fully cover + unfocus the window for 30s → reset fires. Then keep the window
   focused and type continuously for well past 30s → **no** reset ever fires (guards
   against a spurious `Focused(false)` misdetection — real input re-arms the hidden
   countdown too).
8. **Mem watchdog waits for a safe moment** — set `WMUX_RESET_MEM_MB=100` (trivially
   exceeded) → while typing/scrolling nothing fires; stop touching the app for
   `WMUX_RESET_SAFE_IDLE_SECS` (or switch workspaces) → the pending reset fires
   (`trigger=memWatchdog` with the sampled bytes in stderr).
9. **Scrollback reading is activity** — with `WMUX_RESET_IDLE_SECS=30`, read scrollback
   using **wheel only** (no keys) for over 30s → no reset fires (the throttled activity
   ping counts pure viewing as activity).
10. **Kill survives** — force-kill the app from Task Manager, restart → state is restored
    except at most the last ≤500ms of structural mutations (save debounce window).

## 10. Stage 17+ / Checkpoint 2 manual verification

This section collects the batched manual Windows verification for the stages after
checkpoint 1, to be run at **checkpoint 2** (after stage 21). Items are appended per
stage as each lands.

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
ARM64 VM), since this machine cannot execute ARM64 Windows binaries. `crates/wmux-core` itself
has no target-specific code (it's checked against `x86_64-pc-windows-msvc` in the WSL-side gate
per spike-plan.md section 5), so the ARM64-specific risk surface is `portable-pty`'s ConPTY
backend and Tauri/WebView2, not `wmux-core`.
