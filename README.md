# lvr — LinuxVR

A lightweight tray app and GUI for managing Linux VR sessions (WiVRn, automatic audio routing, headless virtual displays, and companion apps).

## Features

- **WiVRn Watchdog:** Keeps the WiVRn server running and recovers from crashes.
- **Audio Switching:** Automatically switches default PipeWire audio output and microphone to/from the headset on connect/disconnect.
- **Companion App Supervisor:** Starts and stops companion tools (e.g. VRCX, SlimeVR, VRCOSC, VRCVideoCacher) based on customizable triggers (VRChat, WiVRn, headset connection) and grace periods.
- **Virtual Display Management:** Manages virtual displays via KWin / `kscreen-doctor` for headless or VR streaming use.
- **VRChat Prefix & Domain Blocking:** Selective blocking of video players, images, strings, and REST domains inside the VRChat prefix with shared asset protection and multi-list community blocklists.
- **VR Dashboard:** VR-tailored UI with big touch/laser targets for toggling audio, restarting WiVRn, and stopping all VR processes in one click.

## Quick Start

### Install

```bash
./scripts/build.sh --deploy
```

Installs binary, icon, and desktop entry to `~/.local`. Pass `--autostart` or `--no-autostart` to control login startup.

### Usage

```bash
lvr                      # launch GUI + tray
lvr --hidden             # start minimized to tray
lvr --status             # query live VR status
lvr --audio [vr|desktop] # route audio to headset or desktop
lvr --tab [name]         # open or raise window to a specific tab
```

## Configuration

Configured via GUI or `~/.config/lvr/config.toml` (overridable with `$LVR_CONFIG` or `-c`). All companion apps, triggers, process patterns, audio sinks, display settings, and Proton profiles can be configured directly in the GUI.

## Development

```bash
cargo build --release
cargo test
```

