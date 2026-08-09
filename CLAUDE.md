# CLAUDE.md

winmux — a lightweight cmux-style terminal for Windows, centered on WSL2 and coding agents
(Claude Code / Codex). Product plan: `터미널-계획-v2.md` (Korean). Decisions: `docs/adr/`.

## Current state

Spike executed and verified on Windows (2026-08-08); **candidate A adopted** — see
ADR-0001. The paste bug from ADR-0001 "Known issues" is resolved (Windows Terminal
copy/paste convention; interception list in `apps/spike/src/terminal-tile.ts`).
MVP stages 10–12 are **done and Windows-verified** (2026-08-09): stage 10 (data model +
command dispatcher + stable IDs — ADR-0002) and stages 11–12 (split/tab UI — ADR-0003:
SplitId addressing, atomic SplitPane{tab}, splitter drag, keep-alive tab views, CloseTab
auto-collapse, self-healing detach, Ctrl+Shift+R reload). The Windows checklists in
`docs/WINDOWS-BUILD.md` sections 6–7 stay as regression references.

Stages 13–16 are **done and Windows-verified** (checkpoint 1 passed 2026-08-09 after
two field-fix rounds — ADR-0004): workspace sidebar, replay trim + switch tracer,
persistence (manage-first restore boot, atomic debounced saves), automatic reset
(gesture-only activity, hidden = unfocused OR invisible, glue `reset_ui`). The hard-won
ConPTY findings live in ADR-0004: no EOF on child exit (waiter thread) and cursor
probes in replays (first-attach flag in the attach protocol). Sections 8–9 of
`docs/WINDOWS-BUILD.md` stay as regression references. UX backlog: per-pane split
button affordance; 1MiB-replay switch flicker.

Roadmap (user decision 2026-08-09): proceed straight through stage 21, with **stage 19
(git branch display) deferred to v2** — the model fields (`git_branch`/`git_dirty`) stay
reserved and the sidebar hides them while null. Manual Windows testing is batched into
two checkpoints: after stage 16 (sidebar + teardown latency + persistence + auto reset)
and after stage 21 (text passing + notifications + keyboard + viewer tabs + full
regression). Stages 22–23 (ARM64 CI / device testing) come after checkpoint 2.

Stages 17–21 are **code-complete, Windows verification pending** (2026-08-09, awaiting
**checkpoint 2** — `docs/WINDOWS-BUILD.md` §10): stage 17 (inter-pane text passing:
send/send&run icons, target-pick mode, bracketed-paste delivery guards), stage 18 (OSC
routing: `notify.rs` merge-cell coalescing + glue `OscRouter` 100ms flush,
`winmux:<status>` hook contract in `scripts/wsl/claude-hook-example.md`, keyed reconcile
for sidebar/tab strip with node-identity tests), stage 20 (keyboard navigation:
Ctrl+1–9 / Alt+arrows / Ctrl+Tab; intercept list canonical in `apps/winmux/src/keys.ts`),
stage 21 (viewer tabs: folderBrowser/textViewer/markdownViewer, `wslpath` UNC
validation, fs_* invokes with default-distro resolution, viewer unmount lifecycle).
Plan files `docs/plans/mvp-stage{17,18,20,21}-plan.md` stay until checkpoint 2 passes,
then distill into ADRs per ADR-0001's docs governance.

Checkpoint 2 ran 2026-08-09: **passed** except three field defects, all fixed and
pushed the same day (hook tty fallback, minimize-aware markdown polling,
.gitattributes LF) together with the keyboard-first UX batch (global Ctrl+Shift
shortcuts + tooltips from `keys.ts shortcutLabel`, folder/text viewer keyboard
navigation, per-tab HISTFILE, window-restore centering) — the batched
**re-verification checklist is WINDOWS-BUILD §10 last subsection** (user will run it
together with later verification). RAM at checkpoint 2: ~129MB (inside the 100–150MB
adoption band; ≤100MB stays a v2 optimization backlog item). Stage 22 (CI) is live:
first run green, gates on every push, x64+ARM64 artifacts via workflow_dispatch.
Stage 23 (ARM64 device checklist, WINDOWS-BUILD §11) awaits hardware.

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
