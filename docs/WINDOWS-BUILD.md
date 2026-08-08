# Windows Build Guide

How to set up a Windows machine to build and run `apps/spike` (Tauri v2 + xterm.js), and how
to run the Windows-side Spike verification. This machine's WSL host has no Rust/MSVC toolchain
for Windows targets, so the build and the ConPTY/IME/RAM checks in
[`docs/plans/spike-plan.md`](plans/spike-plan.md) section 6 must be done on Windows directly.

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
     ARM64 — see section 5)
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

## 3. `WMUX_DISTRO` environment variable

The spike app spawns the WSL shell as `wsl.exe [-d $WMUX_DISTRO] -- bash -l` (see spike-plan.md
section 4.5). `WMUX_DISTRO` selects which WSL distribution to spawn into:

- **Unset**: `wsl.exe` uses your default distribution (`wsl -l -v` shows which one has `*`).
- **Set**: `wsl.exe -d <name>` targets that distribution explicitly — useful if you have more
  than one installed (e.g. a plain Ubuntu install alongside an isolated/sandboxed distro for
  agent work) and want the Spike app to consistently target one of them regardless of which is
  marked default.

Set it for the current PowerShell session before launching the app:

```powershell
$env:WMUX_DISTRO = "Ubuntu-24.04"
npm run tauri dev
```

or persist it for your user account (`setx WMUX_DISTRO "Ubuntu-24.04"`, new shells only).

## 4. Verification

Once the app builds and runs, the actual Spike sign-off checklist — OSC passthrough, Claude
Code/Codex behavior, IME, flow control, renderer comparison, RAM measurement — is
[`docs/plans/spike-plan.md`](plans/spike-plan.md) section 6 ("Windows Spike 검증 체크리스트").
Run through it in order; step 1 (OSC passthrough) is the highest-priority gate because it can
force a switch to the file/socket fallback path (see spike-plan.md section 4.1 and
터미널-계획-v2.md section 2's "단일 실패점").

Scripts referenced by that checklist:

- [`scripts/wsl/osc-test.sh`](../scripts/wsl/osc-test.sh) — run **inside the WSL terminal that
  the Spike app opened**, from a Windows-side terminal you do *not* need this for. Emits OSC
  0/7/9/777 with both BEL and ST terminators, plus a chunk-split case.
- [`scripts/wsl/flood.sh`](../scripts/wsl/flood.sh) — `yes`-speed output burst (and an optional
  `--random-lines` high-entropy burst) for the flow control / backpressure check.
- [`scripts/wsl/scrollback-test.sh`](../scripts/wsl/scrollback-test.sh) — emits 12,000 lines to
  confirm the 5,000-line scrollback cap actually evicts old lines.
- [`scripts/wsl/claude-hook-example.md`](../scripts/wsl/claude-hook-example.md) — Claude Code
  `Stop`/`Notification` hook example that emits OSC 777 to `/dev/tty`, for the agent-notification
  half of the checklist.
- [`scripts/win/measure.ps1`](../scripts/win/measure.ps1) — run from a **Windows** PowerShell
  prompt (not inside WSL) while the Spike app is running, to record private working set (WebView2
  process tree included) over time and export it to CSV:

  ```powershell
  .\scripts\win\measure.ps1 -ProcessName wmux-spike -IntervalSec 5 -Samples 12 -OutCsv .\ram-4pane.csv
  ```

  No administrator privileges are required.

## 5. ARM64 cross-build notes

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
