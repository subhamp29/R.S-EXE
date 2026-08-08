# R.S EXE

## About

**R.S EXE** (short for *Rancho Sanctuary EXE*) is a cross-platform desktop application that provides a modern, intuitive graphical interface for the Android SDK Command-line Tools. Built with the Tauri framework — combining a Rust backend with a web-based frontend — it delivers native desktop performance while keeping the install footprint extremely small compared to Electron-based alternatives.

Instead of memorising dozens of `sdkmanager`, `avdmanager`, `emulator`, and `adb` subcommands, users can manage their entire Android development environment from one unified dashboard. Whether you need to install platform-tools, create a virtual device with specific hardware profiles, boot an emulator with custom GPU settings, or capture a screenshot from a running AVD, R.S EXE orchestrates every operation behind a clean, responsive UI.

### Key capabilities

- **SDK package management** — Install/uninstall Android SDK components (JDK, command-line tools, system images, build-tools, platform-tools) with real-time progress tracking.
- **AVD lifecycle control** — Create, edit, boot, stop, force-stop, and snapshot Android Virtual Devices — all from within the app.
- **Emulator configuration** — Fine-tune GPU mode (host / host-only / swiftshader / software / none), RAM allocation, and boot options before launching an emulator.
- **APK management** — Drag-and-drop APK installation, list and launch installed apps, and uninstall packages across connected devices or emulators.
- **Snapshot management** — Save, restore, and delete emulator snapshots so you can quickly revert to known-good states.
- **System diagnostics** — Real-time system info panel showing RAM, CPU, GPU details, and hypervisor availability — no command line needed.
- **Single-instance** — A running instance detects duplicate launches and simply brings the existing window to the foreground.
- **Portable / offline-ready** — Once the SDK is installed, the app works without an internet connection for day-to-day AVD management.

### Who is this for?

- **Android developers** who want a faster, lighter alternative to Android Studio's built-in AVD Manager.
- **QA engineers** who need to spin up multiple emulators with different configurations.
- **CI/CD pipelines** that benefit from scripted SDK and AVD setup in headless environments.
- **Educators and students** learning Android development who prefer a GUI over the command line.

### Technology stack

| Layer        | Technology                              |
|--------------|-----------------------------------------|
| Backend      | Rust 1.85+, Tauri 2, tokio, reqwest     |
| Frontend     | React / Vite / TypeScript               |
| Build system | cargo (release profile: LTO fat, strip) |
| Bundling     | Tauri — NSIS installer, MSI, standalone EXE |
| Platform     | Windows x64 (primary), with macOS/Linux support via Tauri portability |

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
