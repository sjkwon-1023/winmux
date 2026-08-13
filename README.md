# winmux

[![CI](https://github.com/sjkwon-1023/winmux/actions/workflows/ci.yml/badge.svg)](https://github.com/sjkwon-1023/winmux/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A lightweight terminal for Windows, built around WSL2 and coding agents.

Split panes, tabs inside each pane, and a workspace sidebar that shows which agent is running,
which one is waiting for your input, and which one just finished — without switching to it.

I built it after moving from cmux on macOS to Windows: only the features I actually use, kept
as small as I could (100MB target, 150MB ceiling, ~129MB measured), plus a folder browser and
text and markdown viewers for reading while an agent works. It is Tauri v2 and xterm.js over a
pure Rust core — the terminal logic has no UI framework dependency and is tested on Linux.

## Requirements

winmux is **WSL2 only**, by design rather than omission. Opening a terminal always means
ConPTY → `wsl.exe` → a login shell. There is no PowerShell or CMD profile.

- Windows 11 (x64 or ARM64)
- WSL2 with at least one distribution installed
- WebView2 runtime — ships with Windows 11

## Features

- **Split panes** — split in either direction, drag to resize, `Alt`+arrows to move focus.
- **Tabs inside panes** — every pane has its own tab strip; background tabs stay alive.
- **Workspace sidebar** — one status card per workspace: agent state and its last message.
- **Agent status** — driven by OSC 777/9 from a Claude Code hook or your shell prompt. No
  daemon, no IPC server, no named pipe.
- **Pane-to-pane text passing** — send text to another pane, optionally running it on arrival.
- **Viewer tabs** — folder browser, text viewer, and markdown viewer reading over
  `\\wsl.localhost`. Large files page in 512KiB windows.
- **Layout persistence** — workspaces, splits, and tabs come back; processes don't. Shells
  respawn in the directory they were last in.
- **Automatic UI reset** — durable state lives in Rust, so the WebView can reload to reclaim
  memory without losing a session.
- **x64 and ARM64** — both gated in CI.

## Installing

winmux ships as a single portable executable for x64 and ARM64 — no installer, no setup.
Download `winmux-x64.exe` or `winmux-arm64.exe` from the
[latest release](https://github.com/sjkwon-1023/winmux/releases/latest), matching your CPU
(WSL2 with a distribution installed is still required, see [Requirements](#requirements)).
It's unsigned, so Windows SmartScreen will warn on first launch — "More info" → "Run
anyway".

To build from source, the toolchain setup — rustup on the MSVC ABI, Visual Studio Build Tools
with the C++ workload, Node.js LTS — is in
[`docs/WINDOWS-BUILD.md`](./docs/WINDOWS-BUILD.md).

```powershell
git clone https://github.com/sjkwon-1023/winmux.git
cd winmux\apps\winmux
npm install
npm run tauri build -- --no-bundle
```

That leaves `winmux-app.exe` in the repo's `target\release\`. Use `npm run tauri dev` instead
to run it with hot reload.

Settings are edited by hand — there is no settings screen. Write
`%AppData%\app.winmux.desktop\settings.json` and restart; any key may be left out, and a broken
file reports itself in the status line instead of being silently ignored.

```json
{
  "fontFamily": "Cascadia Code, monospace",
  "fontSize": 15,
  "highlightLanguages": ["python", "javascript", "typescript", "rust", "json", "toml", "css", "html"]
}
```

`fontFamily`/`fontSize` set the font for the terminal **and** for the viewers' monospace
content — the text viewer's lines, the folder listing, and markdown code spans and blocks.
Markdown prose keeps its own face but follows the *size*, so a larger `fontSize` scales the
whole document. The rest of the UI (sidebar, tab bars, status line) is never affected.
`Ctrl+=`/`Ctrl+-`/`Ctrl+0` zoom moves the terminal and all three viewer surfaces together. Zoom
is session-only: a relaunch comes back at the size set here, and `Ctrl+0` returns to it.

`highlightLanguages` picks which languages the text viewer syntax-highlights; the list above is
also what you get when the key is absent, and those eight names are the entire supported set —
an unknown name is reported rather than ignored. The language comes from the file extension
(`.jsx` highlights as `javascript`, `.tsx` as `typescript`), and anything outside the set stays
plain text. `[]` turns highlighting off. The highlighter is loaded on demand, so a session that
never opens a matching file never pays for it.

## Setup

### Choosing a distribution

winmux spawns into the WSL default distribution. To point it elsewhere:

```powershell
$env:WINMUX_DISTRO = "Ubuntu-24.04"      # current shell
setx WINMUX_DISTRO "Ubuntu-24.04"        # persist for your user account
```

This matters if you keep a locked-down distribution for agent work. winmux never filters
commands — it just never hands you a Windows shell, and leaves the real boundary to that
distribution's own `/etc/wsl.conf`. Viewer tabs keep working there, because they read in the
other direction, from Windows into WSL.

### Agent status

Copy the hook script and the `settings.json` snippet from
[`scripts/wsl/claude-hook-example.md`](./scripts/wsl/claude-hook-example.md). It also covers
emitting the tab title and cwd from your `~/.bashrc`.

## Keyboard shortcuts

Global shortcuts are all `Ctrl+Shift`, so plain `Ctrl` combinations stay with your shell.
Anything not listed goes straight to the PTY.

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

The full list, including viewer-local keys, is in
[`apps/winmux/src/keys.ts`](./apps/winmux/src/keys.ts).

## Status

Early, one maintainer, but I use it daily.

- **Tested on an x64 Windows 11 desktop only.** ARM64 is type-checked and linted on every
  push, but has never run on real hardware — device testing waits on an ARM64 laptop.
- **Git branch display** and a **built-in browser tab** are both planned, not built. The data
  model already reserves the git fields, and the sidebar hides them while they are empty.

## License

MIT — see [LICENSE](./LICENSE).
