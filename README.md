# wmux

A lightweight cmux-style terminal for Windows, centered on WSL2 and coding agents
(Claude Code / Codex). Windows 11 ARM64 first, x64 supported.

- Product plan (Korean): [`터미널-계획-v2.md`](./터미널-계획-v2.md)
- Current phase: **post-Spike → MVP**. The spike verdict (2026-08-08) adopted candidate A
  (Tauri v2 + single WebView2 + xterm.js + ConPTY → wsl.exe): 4 panes at 113MB private
  working set, OSC 777/9/7 passthrough confirmed. See
  [`docs/adr/0001-adopt-tauri-webview2-xterm-stack.md`](./docs/adr/0001-adopt-tauri-webview2-xterm-stack.md).

## Layout

```
crates/wmux-core/   Pure Rust core: PTY session, flow control (backpressure),
                    OSC 777/9/7 scanner, replay buffer. No Tauri dependency;
                    unit/integration tested on Linux.
apps/spike/         Tauri v2 spike app: xterm.js frontend + thin command glue.
scripts/wsl/        Test scripts run inside WSL (OSC emission, output flood, hooks).
scripts/win/        Windows-side measurement (private working set of the process tree).
docs/               Build instructions and plans.
```

## Development (WSL)

Core logic is developed and tested inside WSL:

```bash
cargo test -p wmux-core
cargo clippy -p wmux-core --all-targets -- -D warnings
cargo check -p wmux-core --target x86_64-pc-windows-msvc
cd apps/spike && npm run build && npx vitest run
```

The full workspace (including `src-tauri`) also type-checks for the Windows target from
WSL once `llvm-rc` is available — no sudo required:

```bash
apt-get download llvm-18 libllvm18 && mkdir -p ~/.local/llvm && \
  for d in *.deb; do dpkg -x "$d" ~/.local/llvm; done
export PATH="$HOME/.local/llvm/usr/lib/llvm-18/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.local/llvm/usr/lib/x86_64-linux-gnu:$LD_LIBRARY_PATH"
cargo check --workspace --target x86_64-pc-windows-msvc
```

## Building the spike app (Windows)

The app itself targets Windows (ConPTY + WebView2). See
[`docs/WINDOWS-BUILD.md`](./docs/WINDOWS-BUILD.md) for toolchain setup, build steps,
and the spike verification checklist (OSC passthrough, IME, flow control, RAM budget).
