# R.S EXE

**Desktop Android Virtual Device (AVD) Manager** — A cross-platform Tauri desktop application for managing Android SDK packages, creating and launching AVDs, and controlling emulators with advanced features like APK installation, snapshots, and system optimization.

## Features

- **Dashboard** — System information (RAM, CPU, GPU, Hypervisor status) with live polling
- **Devices** — Full AVD management: create, edit, delete, boot, stop, force-stop virtual devices
- **SDK Manager** — Install JDK, command-line tools, system images, build tools, and platform tools
- **Emulator Control** — Boot AVDs with custom options, capture screenshots, optimize installed apps
- **APK Management** — Drag-and-drop APK installation, list/installed/uninstall/launch apps
- **Snapshots** — Save, list, and delete emulator snapshots
- **GPU Settings** — Configure GPU mode (host, host-only, swiftshader, software, none)
- **Settings** — Configure SDK paths, preferences, and advanced options

## Build Instructions

### Prerequisites

- Rust 1.85+ (with `rustup`)
- Node.js 20+
- npm 11+

### Development

```bash
# Clone the repository
git clone https://github.com/subhamp29/R.S-EXE.git
cd R.S-EXE

# Install npm dependencies
npm install

# Run in development mode (Tauri dev server)
npm run tauri:dev

# Or run the frontend only (Vite dev server on localhost:1420)
npm run dev
```

### Production Build

```bash
# Build the frontend and native binary
npm run tauri:build

# Build artifacts are placed in:
#   src-tauri/target/release/rs-exe.exe
#   src-tauri/target/release/bundle/nsis/
#   src-tauri/target/release/bundle/msi/
```

## Distribution

Pre-built binaries are available on the [Releases page](https://github.com/subhamp29/R.S-EXE/releases).

- **Standalone EXE** (`rs-exe.exe`) — Copy to any Windows 10/11 x64 machine and double-click to launch
- **NSIS Installer** (`R.S EXE_0.1.0_x64-setup.exe`) — Interactive installer with Start Menu shortcuts
- **MSI Installer** (`R.S EXE_0.1.0_x64_en-US.msi`) — For enterprise/group deployment

## License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file for details.

## Authors

- **Subham Mahapatra** — [subhamp29](https://github.com/subhamp29)
