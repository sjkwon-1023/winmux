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

One batch landed after re-verification and has **not** been through a Windows round yet
(Shift+Enter as ESC CR, sidebar reflow fix, folder-first workspace creation, retired send
buttons, redrawn icons) — its checklist is WINDOWS-BUILD §10's last subsection.

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
  exercised on the Linux dev box).
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
- **`isCommandError`'s variant table is hand-maintained** — the `formatCommandError`
  switch is compile-time exhaustive via `assertNever`, but the type guard above it is a
  literal list, so a new `CommandError` variant falls silently through to the raw-JSON
  path. Force the table from the type.
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
