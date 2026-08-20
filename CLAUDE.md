# CLAUDE.md

winmux — a lightweight cmux-style terminal for Windows, centered on WSL2 and coding agents
(Claude Code / Codex). Decisions: `docs/adr/`.

The product plan `터미널-계획-v2.md` (Korean) is **no longer in the tree** — it was removed
when the repo went public. Roughly 120 "계획 v2 <n>장 / section <n>" citations across the
source comments, ADRs, and the remaining plan docs still point at it; read it out of git
history when one of them matters (`git show HEAD~1:터미널-계획-v2.md`, or any commit before
its removal).

## Current state

**MVP stages 10–22 are complete and Windows-verified**: checkpoint 2 passed 2026-08-09
(three field defects fixed the same day) and its re-verification round passed in full
2026-08-10. The manual checklists in `docs/WINDOWS-BUILD.md` sections 6–10 stay as
regression references. Stage 22 (CI) is live — the gates run on every push, x64 + ARM64
artifacts build on `workflow_dispatch`. **Remaining: stage 23**, ARM64 device testing
(`docs/WINDOWS-BUILD.md` §11), which awaits hardware.

Every decision behind those stages lives in `docs/adr/`: stack adoption (0001),
stage-10 architecture (0002), split/tab UI (0003), lifecycle + persistence + reset with
the hard-won ConPTY findings (0004), inter-pane text passing and its UI retirement
(0005), OSC notification routing (0006), the keyboard model and the canonical
interception list (0007), viewer tabs (0008). Stage 19 (git branch display) is deferred
to v2 with `git_branch`/`git_dirty` reserved on the model.

Every release through **v0.3.8 is field-verified** (2026-08-13 round): the toast pipeline
end to end (AUMID registration, OS-signal focus gate, direct WinRT delivery, the widened
focused-but-other-workspace case), viewer fonts and viewer zoom, Codex resume hints, and
the earlier post-re-verification batch (Shift+Enter as ESC CR, sidebar reflow, workspace
creation, icons). The WINDOWS-BUILD §10 subsections per release stay as regression
checklists. One field lesson worth keeping: synthetic needs-input tests must reset state
first (`winmux:idle` then `winmux:needsInput`) — the onset only fires on a transition, and
two rounds were burned on stale-state and wrong-token test artifacts that looked like app
defects.

### Backlog

Accepted deferrals, one line each. None of these block the MVP.

- **Codex composer pill: closed as out-of-app (2026-08-12)** — the field probe showed
  conhost consuming OSC 11 queries without answering anyone, so no app-side lever
  exists; upstream is openai/codex#19741. Responder + THEME_SYNC stay as no-cost
  coverage for other conhost versions.
- **Resume-hint ↑ integration is bash-only** — a `.bashrc` that execs zsh/fish would
  read its own history (hint line still shows, ↑ would not). Informational: the
  2026-08-12 field failure was NOT this — it was the `wsl.exe --` double evaluation,
  fixed by `--exec` in v0.3.3 and field-confirmed.
- **Ctrl+= / Ctrl+- terminal zoom — landed 2026-08-12** as **session-only** (no write-back:
  relaunch returns to the `settings.json` size, Ctrl+0 resets to it), window-wide across all
  tabs, clamped to the backend's 6-72 range; Ctrl+- shadows the terminal's C-_ (emacs undo)
  as an accepted trade-off, same class as Ctrl+1-9. Verification: WINDOWS-BUILD §10 v0.3.4.
  - **Extended to the viewers — landed 2026-08-13** (user request, v0.3.8), reversing the
    v0.3.7 decision to keep zoom terminal-only. One key moves **both** surfaces by the same
    step — no per-surface zoom, because the moment you have to remember which surface grew,
    Ctrl+0 stops having one meaning. The two surfaces keep separate effective sizes and
    separate baselines (terminal 13px, viewers 12px) and clamp independently, so at 6/72 one
    can stop while the other still moves. What made this more than a CSS-variable write is that
    two viewers hold coordinates the resize invalidates, so `viewer-font.ts` now owns a live-view
    registry (the counterpart of `terminal-view.ts`'s `liveViews`) driven in **two phases** —
    every view anchors its position *before* the variable changes, then re-seats itself after.
    `TextView` re-lays row height, spacer height and `scrollTop` around the **topmost visible
    line** (model coordinates are byte offsets, so holding the line holds the position and
    nothing goes back to the model). `MarkdownView` holds a **relative** anchor instead — its
    scroll coordinate is px and the prose reflows — and deliberately does not write the post-zoom
    px back, since a relaunch renders at the `settings.json` size where that px means a different
    place. The folder listing needs neither. The "never call this again after boot" warning on
    `viewer-font.ts` is gone with the hazard. Markdown prose now follows the **size** too (its
    face is still not the code font) — otherwise zooming a document left the prose behind, which
    partly supersedes a v0.3.7 decision. Verification: WINDOWS-BUILD §10 v0.3.8.
- **Syntax highlighting in the text viewer — landed 2026-08-12** (user request 2026-08-11)
  with highlight.js 11 behind **dynamic import only**: the entry bundle carries zero
  highlighter bytes (verified on the build output — core, one chunk per language and the
  vs2015 theme CSS are separate assets), so start-up and non-code files pay nothing.
  Opening a file always renders plain first and the colours are overlaid when the module
  lands (stale callbacks dropped on dispose / window change). The window is tokenized once
  and cached per line as the design note required, capped at 256 KiB per window
  (measured ~2 MB/s, so a full 512 KiB window would block the main thread for ~250 ms);
  over the cap the window stays plain. Language comes from an explicit extension map — no
  `highlightAuto` — filtered by `settings.json`'s `highlightLanguages` (default python,
  javascript, typescript, rust, json, toml, css, html; `[]` disables; unknown names are
  rejected loudly like `fontSize`). Supported names are exactly the shipped loader set,
  mirrored in `commands.rs::HIGHLIGHT_LANGUAGES` — widening it means adding a loader on
  both sides. Verification: WINDOWS-BUILD §10 v0.3.6 item 2.
- **Windows toast notifications on needsInput — landed 2026-08-12** via the official
  `tauri-plugin-notification` (+ the `notify_toast` glue command), fired **only while the
  window is unfocused** (`document.hasFocus()` false) on the chime's own onset rule — one
  toast per transitioning workspace, chime alone when focused. Still a field check: toast
  sender identity for an unsigned standalone exe (WINDOWS-BUILD §10 v0.3.4).
  **Superseded by the v0.3.7 entry below** — the plugin, that focus source and the chime are
  all gone.
- **Toasts do not appear at all in the field — landed 2026-08-12** (user report 2026-08-12,
  v0.3.5): the card-style Windows notification never showed, focused or not. Root cause
  **confirmed 2026-08-12**: Windows Settings › Notifications had no winmux entry at all
  (screenshot checked), i.e. the shell had never seen an app identity to attribute toasts to —
  an unpackaged, unsigned exe registers no AppUserModelID / Start-menu shortcut, and WinRT
  drops toasts from unregistered senders silently. Fixed by `src-tauri/src/app_identity.rs`,
  called at the very top of `main()` (before the webview and plugin init): it calls
  `SetCurrentProcessExplicitAppUserModelID` and creates/refreshes
  `%AppData%\...\Start Menu\Programs\winmux.lnk` with `PKEY_AppUserModel_ID`, idempotently —
  same target + AUMID means no write, a moved exe rewrites the target (the version-swap case).
  The AUMID is **`app.winmux.desktop`**, i.e. `tauri.conf.json`'s `identifier`, because that is
  what the plugin puts on the toast (`tauri-plugin-notification` 2.3.3 `desktop.rs:27` takes
  `app.config().identifier`, `desktop.rs:195-206` sets it as the app_id unless the exe sits in
  `target\{debug,release}`); a `const` assert against `tauri.conf.json` breaks the build if the
  two ever drift, since a mismatch would fail silently. The dev-directory exception is mirrored
  in `plugin_uses_our_aumid` so dev builds do not litter the Start menu — they keep the
  plugin's PowerShell-sender fallback. Failure logs one loud line and never blocks boot.
  Verification: WINDOWS-BUILD §10 v0.3.6 item 3 (all of it is field-only — none of it could be
  exercised on the Linux dev box). The AUMID-derivation and dev-exception halves of this entry
  were rewritten in v0.3.7 (next entry); the registration mechanism itself is unchanged.
- **Toasts still did not appear once the identity was registered — redesigned 2026-08-13**
  (field diagnosis, v0.3.6). The identity work above was *correct*: `Get-StartApps` lists
  `winmux app.winmux.desktop`, and a hand-run
  `CreateToastNotifier("app.winmux.desktop").Show(...)` in PowerShell puts a card on screen, so
  the OS pipeline is proven. Inside the app the onset fired (the chime rang) and no toast
  followed, leaving **two suspects that could not be told apart from inside the app**: (a)
  WebView2's `document.hasFocus()` staying `true` while the window is unfocused, which would
  make the front-end suppress every toast, and (b) `tauri-plugin-notification` swallowing send
  errors — 2.3.3 `desktop.rs:216` is literally
  `tauri::async_runtime::spawn(async move { let _ = notification.show(); })`. Rather than guess,
  **both layers were removed**:
  - Focus comes from the OS: `main.rs` already had `WindowEvent::Focused` for the reset policy
    and now also emits `window-focus` (bool) to the front-end, which keeps a `windowFocused`
    flag. `document.hasFocus()` is no longer used anywhere. The flag is **subscribed and then
    seeded**: window events only fire on a *change*, but this front-end also restarts on the
    automatic webview reload — whose main trigger is "hidden/unfocused for N minutes", i.e. the
    reloaded page comes up unfocused with the transition long past. Assuming focus there would
    suppress exactly the toast the user stepped away to receive, so `installWindowFocus`
    subscribes first, then asks `getCurrentWindow().isFocused()` once and applies the answer only
    if no event beat it (review finding 2026-08-13; the query is inside `core:default`, so the
    capability did not change).
  - Sending is direct: `notify_toast` calls `tauri-winrt-notification`'s
    `Toast::new(app_identity::APP_USER_MODEL_ID)` itself — **sender AUMID = registered AUMID =
    one constant** — and returns the `show()` error instead of dropping it. The plugin, its
    `notification:default` capability and its lock entries are gone; the crate that used to sit
    at the end of that chain is now a direct dependency, so the graph shrank. Because we now
    always send under our own AUMID, `plugin_uses_our_aumid`'s dev-build exception lost its
    basis and was deleted — dev builds register the Start-menu shortcut like any other, or their
    toasts would die silently.
  - Diagnosis has a field-visible window: every attempt appends one timestamped `ok`/`err` line
    to `%AppData%\app.winmux.desktop\toast.log` (best-effort, body not logged, truncated past
    64 KiB), so the next round can separate "never called" from "sent but not shown" from
    "WinRT refused" without a dev console.
  - The rule also widened, since the old one leaned on the chime: a toast is suppressed **only**
    when the window is focused *and* the workspace is the active one (it is already on screen).
    Unfocused, or focused-but-another-workspace, both toast. The judgment is the pure
    `chime.ts::needsInputToastTargets`, locked by vitest.
  Verification: WINDOWS-BUILD §10 v0.3.7 item 2 (field-only, as before).
- **needs-input chime removed — decided 2026-08-13** (user decision): the signal is the toast
  alone. The chime could not say *which* workspace was waiting, and its existence was the
  argument for suppressing toasts whenever the window had focus — the rule that hid a second
  project going quiet. `Chime`/`installChimeUnlock` and their tests stay in `chime.ts` as
  **dormant** code (the send-mode precedent: entry point unwired, contract still tested, reason
  recorded in the module header); only the wiring in `main.ts` was cut. `detectNeedsInputOnset`
  stays as the onset engine, minus its now-meaningless `chime` derived field — a **contract
  change**: `NeedsInputOnset` is `{ onsets, next }`.
- **Query-reply `/tmp` confinement is string-level only** — a pre-planted symlink
  (`/tmp/x → $HOME`) routes the reply write outside; blocking it needs a
  canonicalize-at-write recheck whose 9P semantics are unverified on real hardware
  (review finding 2026-08-11; docs state the honest contract).

- **Agent-facing pane-send channel** — **landed 2026-08-11 as a shell CLI**, not MCP (user
  decision: MCP is heavy, and it is a v2 browser-surface question instead). `winmux send`
  addresses a target by stable tab id (`'#181'`) as well as by title, and `winmux ls`
  enumerates the tabs over a query channel (`OSC 777;winmux-query`, reply written to a
  `/tmp` file the caller names). Contract in `scripts/wsl/claude-hook-example.md`, agent
  surface in `scripts/wsl/skills/winmux-send/SKILL.md`, verification in WINDOWS-BUILD §10.
  Still open: **reading a pane's scrollback** (enumeration is metadata only — an opt-in
  design is needed before any output leaves a pane), and `winmux ls`'s **`COMMAND` column
  showing `?` for a tab whose shell is in another distro or a Windows shell** (it is read
  from this distro's `/proc`). Keyboard targeting for the old manual send mode stays absorbed
  by the stage-17 retirement — it is not coming back.
- **`winmux send` submits to shells but not to TUI agents** — found in the field 2026-08-15,
  not fixed. `cmd_send` appends **LF** (`printf '%s\n' "$text"`, the CLI heredoc in
  `provision.rs`). A shell runs the line because its line discipline takes LF as end-of-line,
  but a raw-mode TUI does not — the terminal sends **CR** for Enter, so Codex and Claude Code
  take the text into their prompt and then sit there, never submitting. Confirmed both ways: a
  send to a Codex tab pre-filled but never ran, and a bare CR to that same tab started it
  immediately. The contract already carries the assumption in its wording — "Include the
  newline if the target **shell** should run the line"
  (`scripts/wsl/claude-hook-example.md`) — while the channel exists precisely to hand work to
  *agents*, which is what the skill advertises. Fix is to append `\r` instead: a shell's
  `ICRNL` turns CR into NL, so the shell case is unaffected. That is a `provision.rs` heredoc
  edit plus a `SETUP_VERSION` bump, with the same correction in the contract doc and in
  `scripts/wsl/skills/winmux-send/SKILL.md`.
- **Cross-workspace send/`ls` would need an explicit opt-in** — both halves of the agent
  channel stop at the requester's own workspace (2026-08-11 decision, ADR-0005 addendum); if
  reaching another project's pane ever becomes a real need it arrives as a named opt-in, never
  as the default radius.
- **≤100MB RAM** — ~129MB at checkpoint 2 sits inside the 100–150MB adoption band
  (ADR-0001); getting under 100MB is a v2 optimization.
- **Per-tab shell history GC** — `~/.winmux/history/tab-<id>` files outlive the tabs that
  created them and nothing prunes them. `~/.winmux/resume/tab-<id>` (the agent resume hint)
  has the same shape and the same gap, so one sweep should take both — including any
  `tab-<id>.tmp.<pid>` a hook killed mid-write left behind. Open alongside it:
  whether a tab closed through the kill path writes its `HISTFILE` at all (a `history -a`
  follow-up if it does not).
- **The resume hint covers Codex too — landed 2026-08-12** (setup v7). A new
  `~/.winmux/bin/winmux-codex-notify.sh` reads Codex's notify payload from `$1` (it arrives as
  the final **argv** element, not on stdin), records `codex resume <thread-id>` in the same
  per-tab file the Claude hook writes, and delegates the `winmux:idle` emission to
  `winmux-notify.sh` — whose body is now Codex's last message rather than a fixed string.
  Both agents write one file, so **the last agent to finish a turn in a tab wins**, and the
  spawn wrapper's read guard is a whitelist that now takes `codex resume <token>` as well.
  The exclusion that blocked this was the "never rewrite an existing `notify`" rule; it is
  resolved by a **self-migration** narrow enough to keep the rule: the *only* value ever
  replaced is one byte-for-byte identical to the line winmux itself wrote (unchanged from
  setup v2 through v6; re-parsed with `tomllib` and value-checked before the write when
  `tomllib` exists — without it the in-place swap proceeds unverified). Every other `notify`, hand-edited variants of
  our own line included, is left untouched with a log line naming the replacement. Payload
  keys are kebab-case (`thread-id`, `last-assistant-message`) as of `codex-cli 0.147`, with
  the snake-cased spellings accepted as a fallback; no version probe — a payload we cannot
  read notifies without a hint. Contract: `scripts/wsl/claude-hook-example.md`. Verification:
  WINDOWS-BUILD §10 v0.3.5.
- **A tab's cwd never advances, so a restart reopens every shell at the workspace root** (user
  report 2026-08-15). The restore path itself is fine: `respawn_tab` (`command.rs:803`) reads the
  tab's stored `cwd` and spawns there, which is exactly the point of the feature. What is missing
  is anything that ever *updates* that value. The contract already says `OSC 7 file://host/path`
  carries "Tab cwd (respawn location on restart)", the scanner parses it and `notify.rs:71`
  applies it — but **provisioning never wires a shell to emit it**: `provision.rs` installs the
  OSC 777 channels only, and the OSC 0/7 prompt snippet in `scripts/wsl/claude-hook-example.md`
  is manual-install advice. Field state confirms the consequence — every terminal tab in
  `state.json` carries the same `cwd` as its workspace `rootPath` regardless of where the shell
  actually went. The fix is a provisioning line (and a `SETUP_VERSION` bump), not a code change.
- **Agent coverage beyond Claude Code and Codex — Antigravity CLI and opencode** (user
  request 2026-08-15, not started). Both would reuse the `winmux:running` /
  `winmux:needsInput` / `winmux:idle` tokens and `winmux-notify.sh` **unchanged**: nothing in
  `winmux-core` (`osc.rs`, `notify.rs`) or the front-end (`chime.ts`, `main.ts`) is
  agent-specific. The work is `provision.rs` — the single source of truth for every notify
  script and every auto-wiring block — plus a `SETUP_VERSION` bump, a resume-command entry in
  `host.rs::bash_argv`'s whitelist (today `claude --resume` and `codex resume` only), and a
  matching section in `scripts/wsl/claude-hook-example.md`. What is *not* settled, per agent:
  - **Antigravity**: only the **CLI** is in scope. The IDE's agents do not run in a winmux
    tab, so there is no pts to emit into and no tab to attribute a toast to. The CLI's hooks
    are near-isomorphic to Claude Code's — `hooks.json` under `.agents/` per workspace or
    `~/.gemini/config/` globally, the same `{"matcher": …, "hooks": [{"type": "command",
    "command": …, "timeout": …}]}` shape, payload on stdin — so the Claude half's Python
    merge is the template rather than new machinery. The gap is the **event mapping**: the
    documented events are `PreToolUse` / `PostToolUse` / `PreInvocation` / `PostInvocation` /
    `Stop`, so `Stop → winmux:idle` is obvious, but there is **no `Notification` equivalent
    to carry `winmux:needsInput`** — the one state the toast exists for. Until that is
    answered the integration is idle-only, which is half the feature. Payload keys are
    camelCase (`conversationId`, `transcriptPath`, `terminationReason`, `fullyIdle`); unlike
    Codex no snake-case fallback is in evidence. Version risk: hook delivery has been in flux
    (a field report of `Stop`/`PostToolUse` never firing on IDE 1.107.0, later addressed by
    running hooks.json hooks ahead of the built-in termination checks), so a field check must
    name the CLI version it passed on.
  - **opencode**: it has **no shell-command hook at all** — extension is TypeScript/JS
    plugins under `.opencode/plugins/` (project) or `~/.config/opencode/plugins/` (global), a
    default-exported async function returning an event-hook object. The mapping is the better
    of the two (`session.idle → winmux:idle`, `permission.asked → winmux:needsInput`,
    `permission.replied → winmux:running` covers all three states where Antigravity covers
    one), and the plugin context hands over Bun's `$` shell, so it can call
    `~/.winmux/bin/winmux-notify.sh` verbatim — no third notify script. The blocker is **tty
    attribution**: opencode plugins run in the server process with no controlling terminal,
    so `winmux_emit` would always fall through to its ancestor-pts walk, and it is unverified
    whether that server sits in the tab's ancestor chain at all — or whether one server is
    shared across tabs, in which case a needsInput toast lands on the wrong tab or nowhere.
    Answer that before writing any provisioning. Writing a plugin file is also a different
    discipline from the "never rewrite an existing key" rule the Claude/Codex halves follow:
    a plugin file we create is ours to upgrade, one that already exists is not.
- **A shell that never starts is flagged now, but input that stops reaching one is not** —
  the 2026-08-15 field incident, partly landed in v0.3.9. WSL could not allocate the vsock ring
  buffer new interop channels need, so `wsl.exe` started while no shell was ever created inside
  it; separately, the same memory pressure left already-running agents unresponsive. Full
  analysis and the decisions are in [ADR-0009](docs/adr/0009-startup-marker-and-spawn-deadline.md).

  **Landed**: a startup marker (`OSC 777;winmux-started`) emitted first thing by the wrapper, a
  20s watchdog that marks the tab `NotStarted` **without killing the session** (a late marker
  clears it), a pane banner naming WSL as the likely cause with a Retry that cleans up the
  session the tab still held, and a 5s spawn deadline so one tab cannot hold the dispatcher
  lock indefinitely.

  **Still open**: the symptom that actually hurt most — **an agent that was working and stops
  accepting input**. Silence alone cannot decide it (an idle shell is silent), but a
  `write_stdin` that does not return within seconds is a sound signal, and the front end
  serialises writes per session so one stuck write kills that tab's input entirely. Left out of
  v0.3.9 as too heavy for its value at the time: it needs another status, another surface and
  its own false-positive rules. Note it shares a fix with the unbounded blocking task below.
  Also open: failures *after* the marker (the wrapper's file I/O, the final `exec bash -l`), and
  whether killing `wsl.exe` actually reaps the Linux-side relay — the incident's zombies were
  `/init` relays that survived with `PPID=1`, and WINDOWS-BUILD §10 v0.3.9 item 2 measures it.
- **A tab whose shell died stayed dead forever — fixed 2026-08-20** (user report, v0.3.9).
  Field diagnosis: the machine slept at 07:28 with winmux running, WSL went down with it, and
  the app recorded all ten `SessionExited` events (`code: 1073807364` = `0x40010004`,
  `DBG_TERMINATE_PROCESS`) into `state.json`. `Exited` was an **absorbing, persisted** state —
  `sanitize` kept the status while clearing `pty_session`, the boot respawn enumerates
  `Running` tabs only, `respawn_tab` rejected `Exited`, and v0.3.8 had no front-end respawn
  binding at all — so every relaunch produced an empty pane with an `exited` badge and no way
  back. A normal app quit never caused it (nothing kills the PTYs before the exit flush, and
  Tauri leaves via `std::process::exit`); the trigger is a shell dying *while the app is up*:
  sleep/shutdown, `wsl --shutdown`, WSL OOM, or typing `exit`. Landed: restore normalizes
  `Exited` → `Running` so a restart revives every tab in its stored `cwd`, `respawn_tab`
  accepts `Exited` (killing the stale replay session first), and the ADR-0009 pane banner now
  covers `exited` with a **Restart** button. Revival keeps the tab id, so `HISTFILE` and the
  per-tab resume hint come back with it — `↑` gives `claude --resume <id>`. Decisions and the
  rejected alternatives: [ADR-0010](docs/adr/0010-restart-dead-terminal-tabs.md). Verification:
  WINDOWS-BUILD §10 v0.3.9 item 4. **Follow-up the same day** (v0.3.10): the first boot that used
  it revived 11 tabs at once and 6 shells never started — 13 `wsl.exe` inside one second lost the
  race with a cold VM (no zombie relays; live `bash -l` matched the 5 `running` tabs exactly). Boot
  now warms each distro once and paces respawns (`boot.rs`, `WINMUX_RESPAWN_STAGGER_MS`), off the
  setup thread, and `NotStarted` is normalized on restore too so a restart retries a partly failed
  wave. See the ADR-0010 amendment. **Still open alongside it**: the tab `cwd` gap above means a
  revived tab reopens at the workspace root rather than where the shell had moved to.
- **Sessions do not survive a severed relay, and that is a deliberate limit** (considered
  2026-08-15, not planned). Putting a detach layer (`dtach`, ~50KB installed and <1MB per
  server) between the terminal and the shell would let a session live through a broken vsock
  channel, an app restart, even a winmux crash — the agent process keeps running and reattaches
  where it left off, making the resume-hint feature unnecessary. It would not survive
  `wsl --shutdown` or a reboot, and output produced while detached is lost. Rejected for now on
  complexity, not cost: socket lifetime, attach-vs-new arbitration, resize forwarding and
  provisioning all grow. Sessions dying with the app is intended, memory pressure belongs to
  `.wslconfig`, and users who need more can run tmux themselves.
- **No runtime log file** — everything the app says at runtime goes to `eprintln!`, and the
  release build is `windows_subsystem = "windows"` (`main.rs:15`), so there is no console for it
  to land in. `~/.winmux/setup.log` is provisioning-only and `toast.log` records toast sends;
  neither sees the backend. Reconstructing the bug above took `dmesg` plus forensics on relay
  processes that happened to still be alive — winmux itself had recorded nothing, and two
  hypotheses were chased and discarded before the process tree settled it. A rolling file log
  next to `state.json` (spawn start/finish with duration, exit, write errors) is what turns that
  into a five-minute diagnosis. Keep it **off by default** with an opt-in switch, and never put
  terminal output in it.
- **`isCommandError`'s variant table is hand-maintained** — the `formatCommandError`
  switch is compile-time exhaustive via `assertNever`, but the type guard above it is a
  literal list, so a new `CommandError` variant falls silently through to the raw-JSON
  path. Force the table from the type.
- **No way to split *around* an existing split** (user request 2026-08-15, not started) —
  with a pane already split top/bottom, nothing adds a pane spanning the full left or right
  side; only one of the two halves can be split again. The tree already **represents** the
  wanted shape (`Split{horizontal, first: Split{vertical, A, B}, second: Leaf{C}}`) and
  renders, resizes and persists it — what is missing is a command that reaches it.
  `SplitTree::split` matches a `Leaf` by `PaneId` and replaces it in place
  (`model.rs:250-274`), and `SplitPane` is the only split command, so every surface (the two
  header buttons, `Ctrl+Shift+D` / `Ctrl+Shift+E`) can only ever target one leaf; `ResizeSplit`
  is the sole command addressing a `SplitId` and it only moves a ratio. ADR-0003 neither
  decided nor deferred this — it was never raised. The smallest useful shape is a **root
  wrap** (`SplitRoot { direction, tab }`: the whole workspace tree becomes one side of a new
  `Split`, the new pane the other), because "the entire left/right side" is a root-level
  request in practice and the target is then unambiguous in the UI — which a general
  `SplitNode { node: SplitId, … }` is not, since nothing lets a user point at a subtree three
  levels down. One contract question rides along: `split` always puts the new pane in
  `second` (right/bottom), so a root wrap either takes the insertion side as a parameter or
  is right/bottom-only.
- **Splitter resize is mouse-drag only** — no keyboard equivalent for the drag handle.
- **1MiB-replay workspace switch is ~236ms with visible flicker** (ADR-0004) — candidates:
  smaller replay cap, progressive replay, hide-until-parsed.
- **Per-pane split-button affordance** (ADR-0004) — a first-time user read the header
  buttons as "split the *selected* pane".
- **The `◎` browser tab button** was removed from the pane header (permanently disabled,
  taking up space); it returns in v2 with the feature behind it.
- **Diagnostic stderr/console logging** kept from checkpoint-1 debugging (activity source,
  focus/visible, rebuild order) — revisit the noise now that the repo is public.
- **Reload while minimized** resumes markdown polling until the next minimize/restore
  cycle — accepted narrow window.
- **OSC scanner C0 handling** — CAN/SUB abort is implemented; the remaining C0 cases were
  never reviewed against real terminal behavior (carried from ADR-0001).

## Layout

- `crates/winmux-core` — pure Rust core (PTY session, flow control, OSC scanner, replay
  buffer, and the `model`/`command` state + dispatcher). No Tauri dependency; this is
  where unit/integration tests live.
- `apps/winmux` — the MVP app (계획 v2 section 17, stage 10 onward): Tauri v2 + vanilla TS
  frontend driving the `winmux-core` `Dispatcher` over a single serializable `Command` bus.
  Architecture: ADR-0002 (state/bus/attach), ADR-0003 (split/tab UI).
- `apps/spike` — **frozen as the measurement harness** (ADR-0001 reproduction rig):
  feature work stops here, only compiling is maintained going forward. Its checklist and
  scripts keep serving as the MVP-era regression check (`docs/plans/spike-plan.md`
  sections 4 and 6).
- `scripts/wsl`, `scripts/win` — verification scripts (OSC emission, flood, RAM
  measurement).

## Gates (run before committing)

```bash
export PATH="$HOME/.local/node/bin:$HOME/.cargo/bin:$PATH"
cargo test -p winmux-core
cargo clippy --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
cargo clippy --workspace --all-targets --target aarch64-pc-windows-msvc -- -D warnings
cargo check --workspace --target x86_64-pc-windows-msvc
cd apps/spike && npm run build && npx vitest run
cd apps/winmux && npm run build && npx vitest run
```

The ARM64 clippy works on the Linux dev host because check-family commands never link —
no MSVC import libraries needed (계획 v2 section 13: x64 + ARM64 from day one). CI
(`.github/workflows/ci.yml`) runs the same gates on every push and builds x64 + ARM64
release artifacts on `workflow_dispatch` or a `v*` tag (kept off the per-push path —
Windows runners bill at 2x).

- `src-tauri` cannot compile for the Linux host (no webkit2gtk) — the Windows-target
  check/clippy IS the compile gate for the glue. It needs `llvm-rc` on PATH; the
  sudo-free setup (apt-get download + dpkg -x into `~/.local/llvm`) is in README
  "Development".
- Windows build/run and the manual verification flow: `docs/WINDOWS-BUILD.md`.
- The app spawns `wsl.exe [-d $WINMUX_DISTRO] -- bash -l` on Windows, `$SHELL -l` on Unix.

## Conventions

- Code comments are Korean prose; identifiers, commit messages, and tracked reference
  docs are English (this file, README, ADRs). Korean domain/plan docs keep their names.
- User-facing strings (UI text, script output, warnings, error messages) are English; code
  comments and test names may be Korean.
- The terminal output hot path stays raw binary end to end (`ipc::Channel` +
  `InvokeResponseBody::Raw`; xterm gets `Uint8Array`). No JSON on that path — JSON is
  fine for low-frequency events (`state-changed`, `terminal-exit`, stats). OSC no longer
  crosses the IPC boundary in `apps/winmux` — it is routed into the model in Rust (stage 18);
  the `osc-event` emit survives only in the frozen `apps/spike`.
- Lock discipline in the glue: never hold the session-registry mutex across a blocking
  PTY call. Write/resize/spawn go through `spawn_blocking`; `ack_output` stays sync and
  cheap. See the module docs in `apps/spike/src-tauri/src/commands.rs`.
- Flow control must pause the PTY *read* (backpressure into the OS pipe), never just the
  delivery. See `winmux-core::session` reader loop.

## Docs

- `docs/adr/` — decision records, English, numbered (`0001-...`).
- `docs/plans/` — Korean working plans for in-flight work. When a plan is executed,
  distill the outcome into an ADR and delete the plan file. Current exception:
  `spike-plan.md` stays because its section 4 is the module-contract reference that code
  comments point to; delete it when the MVP refactor replaces those contracts.
