# winmux

[![CI](https://github.com/sjkwon-1023/winmux/actions/workflows/ci.yml/badge.svg)](https://github.com/sjkwon-1023/winmux/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A lightweight terminal for Windows, built around WSL2 and coding agents.

winmux gives you split panes, tabs inside each pane, and a workspace sidebar that tells you
at a glance which agent is running, which one is waiting for your input, and which one just
finished — without switching to it. Split a pane, run Claude Code in one and Codex in
another, send a prompt from one pane to another, and let the sidebar do the watching.

## Why this exists

I used cmux on macOS and wanted the same workflow when I moved to Windows. Nothing quite fit,
so I built the parts I actually used.

That shaped two decisions the whole project follows:

- **Only the essentials.** Every feature here is one I reach for daily. There is no plugin
  system, no theme gallery, no config language. Configuration is close to zero on purpose.
- **Stay small.** The design budget is **100MB, with a hard ceiling of 150MB** — measured as
  private working set across the entire WebView2 process tree, not the flattering
  shared-DLL number. Blowing past the ceiling is treated as a reason to reconsider the
  rendering stack, not as something to live with. The most recent measurement sits inside
  that band at ~129MB.

While migrating I also added a few things to taste. Having a file tree next to the terminal
turned out to be worth it, so winmux has a folder browser tab, plus a text viewer for reading
code and a markdown viewer for reading docs — all inside the same window, and all deliberately
minimal. They are for *reading* while an agent works, not for editing.

## Requirements

winmux runs **WSL2 only**. This is a design decision, not a missing feature: the app never
offers a Windows shell. Opening a terminal always means ConPTY → `wsl.exe` → a login shell in
your distribution. There is no PowerShell or CMD profile, and none is planned.

If you don't use WSL2, winmux is not the tool for you.

- Windows 11 (x64 or ARM64)
- WSL2 with at least one installed distribution
- WebView2 runtime — already present on Windows 11

## Features

- **Split panes** — split any pane horizontally or vertically, drag the splitter to resize,
  focus with `Alt`+arrow keys. Closing the last tab in a pane collapses the split.
- **Tabs inside panes** — each pane holds its own tab strip, so one screen position can hold
  several contexts. Tab views stay alive in the background instead of being torn down.
- **Workspace sidebar** — workspaces are the big unit of work. Each one is a status card
  showing its agent's state and the last message it sent, so you can tell what needs you
  without opening it.
- **Agent status and notifications** — winmux reads OSC 777/9 escape sequences out of the
  terminal stream, so a Claude Code hook or a shell prompt can drive the unread dot, the pane
  badge, and the sidebar status. No IPC server, no helper daemon, no named pipe. See
  [`scripts/wsl/claude-hook-example.md`](./scripts/wsl/claude-hook-example.md).
- **Pane-to-pane text passing** — send the text you have to another pane, optionally running
  it on arrival. Handy for handing an agent's output to another agent.
- **Viewer tabs** — folder browser, text viewer, and markdown viewer, reading the WSL
  filesystem over `\\wsl.localhost`. Large files are paged in 512KiB windows rather than
  loaded whole.
- **Layout persistence** — closing and reopening restores your workspaces, splits, and tabs.
  Processes are *not* resurrected: you get your layout back, and shells respawn in the
  directory they were last in.
- **Automatic UI reset** — every piece of durable state (PTY sessions, layout, scrollback)
  lives in Rust, so the WebView can be reloaded wholesale to reclaim memory without losing a
  session. That happens when you have been idle, when the window has been hidden for a while,
  or when a memory watchdog trips — and in that last case only at the next safe moment, never
  mid-keystroke.
- **Keyboard-first** — every mouse-driven action has a shortcut, all on `Ctrl+Shift` so plain
  `Ctrl` combinations stay with your shell.
- **x64 and ARM64** — both are build targets and both are gated in CI from day one.

## Installing

There are no prebuilt binaries or releases yet, so you build from source. The full toolchain
setup — rustup with the MSVC ABI, Visual Studio Build Tools with the C++ workload, Node.js LTS
— is in [`docs/WINDOWS-BUILD.md`](./docs/WINDOWS-BUILD.md), which also tells you where the
resulting `.exe` is written.

```powershell
git clone https://github.com/sjkwon-1023/winmux.git
cd winmux\apps\winmux
npm install
npm run tauri build -- --no-bundle
```

`--no-bundle` skips MSI/NSIS installer packaging and leaves a plain executable you can run
directly. To develop against it instead, `npm run tauri dev` starts the Vite dev server and
opens the app pointed at it, rebuilding on Rust changes and hot-reloading the frontend.

## Setup

### Choosing a distribution

By default winmux spawns into the WSL default distribution. To point it somewhere else, set
`WINMUX_DISTRO`:

```powershell
$env:WINMUX_DISTRO = "Ubuntu-24.04"      # current shell
setx WINMUX_DISTRO "Ubuntu-24.04"        # persist for your user account
```

This matters if you keep a locked-down distribution for agent work. winmux does not filter
commands — an app-level filter is bypassed by a single alias and only breaks honest input.
What it does instead is never hand you a Windows shell, and leave the real boundary to the
distribution's own `/etc/wsl.conf` (`interop.enabled=false`, `automount.enabled=false`). Point
a workspace at that distribution and an agent has no path to `C:\` at all. The viewer tabs
still work, because they read in the other direction, from Windows into WSL.

### Agent status

Status in the sidebar comes from OSC sequences your shell and agent emit. Copy the hook script
and the `settings.json` snippet from
[`scripts/wsl/claude-hook-example.md`](./scripts/wsl/claude-hook-example.md); it also covers
emitting the tab title and cwd from your `~/.bashrc`.

## Keyboard shortcuts

Global shortcuts are all `Ctrl+Shift`, deliberately: plain `Ctrl+W`, `Ctrl+D`, and `Ctrl+E`
belong to your shell, and taking them would break the terminal. Anything not in this list goes
straight through to the PTY.

| Key | Action |
|---|---|
| `Ctrl+1` … `Ctrl+9` | Switch workspace by sidebar position |
| `Alt+↑ ↓ ← →` | Move focus to the adjacent pane |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Cycle tabs in the active pane |
| `Ctrl+Shift+T` | New terminal tab |
| `Ctrl+Shift+B` | New folder browser tab |
| `Ctrl+Shift+W` | Close the active tab |
| `Ctrl+Shift+D` | Split the pane top/bottom |
| `Ctrl+Shift+E` | Split the pane left/right |
| `Ctrl+Shift+N` | New workspace |
| `Ctrl+Shift+R` | Reload the WebView |
| `Ctrl+V` / `Ctrl+Shift+V` / `Shift+Insert` | Paste |
| `Ctrl+C` / `Ctrl+Shift+C` | Copy when there is a selection — a bare `Ctrl+C` with no selection still sends SIGINT |

The canonical list, including viewer-local keys, lives in the module doc of
[`apps/winmux/src/keys.ts`](./apps/winmux/src/keys.ts).

## Project status

winmux is usable and I use it daily, but it is early and has one maintainer.

- **Tested on an x64 Windows 11 desktop only.** ARM64 is a first-class target — every push
  type-checks and lints against it, and the release workflow can produce ARM64 binaries on
  demand — but no ARM64 build has ever been run on real hardware. Device testing is planned
  once I have an ARM64 Windows laptop.
- **Git branch display** is deferred. The data model reserves the fields and the sidebar hides
  them while empty.
- **A built-in browser tab is not planned right now.** If I find I need one, it goes in then.

## Architecture

The terminal logic is pure Rust with no Tauri dependency, so it is unit- and integration-tested
on Linux; the Windows-specific glue is deliberately thin.

```
crates/winmux-core/   PTY sessions, flow control, OSC scanner, replay buffer,
                      state model and command dispatcher. No Tauri dependency.
apps/winmux/          The app: Tauri v2 + WebView2 + xterm.js frontend driving
                      winmux-core over a single serializable command bus.
apps/spike/           Frozen stack-validation harness, kept compiling as a
                      regression reference.
scripts/              WSL-side test scripts and Windows-side RAM measurement.
docs/                 Build guide and architecture decision records.
```

Terminal output stays raw binary end to end — no JSON on the hot path — and flow control pauses
the PTY *read* so backpressure reaches the OS pipe rather than piling up in memory.

Design decisions are recorded in [`docs/adr/`](./docs/adr/).

## License

MIT — see [LICENSE](./LICENSE).
