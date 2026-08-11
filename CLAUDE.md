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
- **≤100MB RAM** — ~129MB at checkpoint 2 sits inside the 100–150MB adoption band
  (ADR-0001); getting under 100MB is a v2 optimization.
- **Per-tab shell history GC** — `~/.winmux/history/tab-<id>` files outlive the tabs that
  created them and nothing prunes them. Open alongside it: whether a tab closed through
  the kill path writes its `HISTFILE` at all (a `history -a` follow-up if it does not).
- **`isCommandError`'s variant table is hand-maintained** — the `formatCommandError`
  switch is compile-time exhaustive via `assertNever`, but the type guard above it is a
  literal list, so a new `CommandError` variant falls silently through to the raw-JSON
  path. Force the table from the type.
- **Splitter resize is mouse-drag only** — no keyboard equivalent for the drag handle.
- **1MiB-replay workspace switch is ~236ms with visible flicker** (ADR-0004) — candidates:
  smaller replay cap, progressive replay, hide-until-parsed.
- **Per-pane split-button affordance** (ADR-0004) — a first-time user read the header
  buttons as "split the *selected* pane".
- **Windows toast notifications** — excluded from stage 18 on dependency/ARM64 scope; the
  OSC flush point is the natural hook (ADR-0006).
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
