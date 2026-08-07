# R.S EXE v0.1.0 — Windows x64

Production build artifacts for the R.S EXE Android Virtual Device (AVD) Manager.

## Contents

| File | Description | Size |
|------|-------------|------|
| `rs-exe.exe` | Standalone executable — double-click to run (no installer needed) | ~7.5 MB |
| `R.S EXE_0.1.0_x64-setup.exe` | NSIS installer with Start Menu shortcut and uninstaller | ~2.4 MB |
| `R.S EXE_0.1.0_x64_en-US.msi` | MSI installer for enterprise/group deployment | ~3.4 MB |

## Running on Other Devices

1. **Standalone EXE (simplest):** Copy `rs-exe.exe` to any Windows x64 machine and double-click to launch. No installation required.

2. **Installer (recommended for distribution):** Run `R.S EXE_0.1.0_x64-setup.exe` on the target machine to install with a Start Menu shortcut and uninstall entry.

3. **Prerequisites:** Windows 10/11 x64 with WebView2 runtime (usually pre-installed on modern Windows). If WebView2 is missing, the installer will prompt to download it automatically.

## Source Code

Full source code is in the repository root. The build uses Tauri 2 (Rust backend + Vite/React frontend).
