# lvr — LinuxVR

A tray app and GUI that holds a Linux VR session together. It watches the
[WiVRn](https://github.com/WiVRn/WiVRn) server, follows headset connect/disconnect
with your audio routing, and starts/stops the companion apps you want around
VRChat — with buttons big enough to hit from inside the headset.

Built for Bazzite + KDE + WiVRn + PipeWire, but nothing is hard-coded: every
command, device and process pattern lives in a config file you can edit from the
GUI.

## What it does

- **Keeps WiVRn running.** If the server disappears — crash, accidental close,
  anything — it is started again. Stopping WiVRn from `lvr` pauses the watchdog,
  so it never fights you.
- **Follows the headset with your audio.** When a headset connects, the default
  output and microphone switch to the WiVRn devices; when it disconnects they go
  back. Existing streams can be dragged along, so Discord moves too.
- **Runs your companion apps.** Each entry has a trigger (VRChat running, WiVRn
  running, headset connected, any process you name, or manual only) and a grace
  period: stop immediately, stop after N seconds, or keep running forever.
- **One click to stop everything VR** — every managed app, WiVRn itself, and
  audio back on your desktop devices.
- **Restart WiVRn** from the tray or the dashboard when it gets stuck.

## Install

```bash
./install.sh
```

That builds a release binary and installs `lvr`, its icon and a desktop entry
into `~/.local`. It offers to add an autostart entry; `--autostart` /
`--no-autostart` skip the question and `--uninstall` reverses everything (your
config is kept).

Nothing is clobbered: if a file it wants to write already exists and differs, the
original is copied to `<file>.bak-<timestamp>` and listed at the end —
`--uninstall` puts it back.

Requirements: a Rust toolchain (`cargo`), `pactl` (ships with pipewire-pulse) and
a session D-Bus. On image-based distros where you would rather not install Rust
on the host, build inside a distrobox and run `install.sh` from there — the
binary it produces runs on the host.

## Using it

`lvr` starts in the tray. Click the tray icon to open the window; closing the
window hides it back to the tray (configurable). The tray icon is grey when
nothing runs, blue when WiVRn is up and green when a headset is connected.

The window has five tabs:

| Tab | What's there |
| --- | --- |
| **Dashboard** | Live status and the big buttons: restart/stop WiVRn, disconnect headset, switch audio, stop everything VR |
| **Autostart** | The table of managed apps — enable, reorder, start/stop by hand, edit or delete |
| **Audio** | Which devices "VR" and "desktop" mean, plus manual routing |
| **Settings** | UI scale, watchdog, timings, VRChat detection patterns, config file location |
| **Logs** | What the supervisor has been doing, filterable |

Config changes save themselves a moment after you stop editing.

### Command line

```bash
lvr                      # tray + GUI
lvr --hidden             # start in the tray without a window
lvr --show --tab logs    # open (or raise) the window on a tab
lvr --status             # print the live VR status and exit
lvr --check              # print what the config would do and exit
lvr --audio vr           # route audio to the headset and exit
lvr --audio desktop      # route it back
```

Only one `lvr` runs at a time: a second launch raises the first one's window
(that is what the desktop entry's right-click actions use). `--audio`, `--status`
and `--check` are one-shots that work whether or not an instance is running, so
they are easy to bind to a keyboard shortcut.

## Configuration

`~/.config/lvr/config.toml` (override with `$LVR_CONFIG` or `--config`). The
GUI writes the same file, so editing either way is fine.

```toml
[general]
ui_scale = 1.25                  # bigger = easier to hit with a VR controller
start_hidden = true
vrchat_match = ["vrchat.exe"]    # what "VRChat is running" means
terminal = ""                    # "" = auto-detect; "{cmd}" is the app
relaunch_debounce_secs = 30      # slow starters get time to appear
stop_grace_secs = 5              # SIGTERM → SIGKILL

[wivrn]
watchdog = true
start_command = "flatpak run --branch=stable --arch=x86_64 --command=/app/bin/wivrn-dashboard io.github.wivrn.wivrn"
restart_delay_secs = 5
max_consecutive_failures = 5     # 0 = keep trying forever
flatpak_id = "io.github.wivrn.wivrn"

[audio]
enabled = true
vr_sink = "wivrn.sink"
vr_source = "wivrn.source"
desktop_sink = ""                # "" = whatever was active before connecting
desktop_source = ""
move_streams = true

[[autostart]]
id = "vrcvideocacher"
name = "VRCVideoCacher"
enabled = true
trigger = "vrchat"               # vrchat | wivrn_running | headset_connected | manual
                                 # or: trigger = { process = "Something.exe" }
command = "/home/you/Desktop/VRCVideoCacher"
working_dir = "/home/you/Desktop"
console = true                   # run in a terminal so its output stays visible
match_patterns = ["vrcvideocacher"]
grace_secs = 120                 # -1 = never stop it, 0 = stop immediately
start_delay_secs = 0
restart_on_exit = false
stop_command = ""                # optional, e.g. flatpak kill dev.slimevr.SlimeVR
include_in_stop_all = true
```

The defaults ship with VRCVideoCacher, VRCOSC, VRCX and VRCX-Extras on the
VRChat trigger, SlimeVR on the WiVRn trigger (5 minute grace), and WayVR present
but disabled — WiVRn's own XR-plugin autostart normally handles that one.

### How triggers and grace periods behave

- An app starts once per trigger activation. If you close it yourself it stays
  closed, unless `restart_on_exit = true`.
- When the trigger goes away the grace timer starts. If the trigger comes back
  before it expires, the app is left alone.
- Stopping an app by hand suppresses auto-start until the trigger cycles off and
  on again.
- An app that was **already running before `lvr` ever saw its trigger** is never
  stopped automatically. Starting `lvr` will not kill things it did not start.

### About `match_patterns`

Patterns are case-insensitive substrings matched against each process' name,
executable path and full command line — and everything that matches is what gets
signalled when the app is stopped. Keep them specific (`vrcosc.exe`, not
`vrc`). The Autostart tab shows the pids each entry currently matches, so you can
check a pattern before relying on it. Threads are never counted, so an Electron
app shows its real handful of processes rather than a hundred.

## Development

```bash
cargo test                              # unit tests, no VR hardware needed
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --release
```

Layout: `config.rs` (persistence) · `state.rs` (shared state, commands, logs) ·
`procs.rs` (process discovery, launching, stopping) · `audio.rs` (`pactl`) ·
`wivrn.rs` (D-Bus) · `engine.rs` (the supervisor) · `tray.rs` (StatusNotifierItem)
· `ui/` (egui) · `icon.rs` (procedural icon) · `ipc.rs` (single instance).

The autostart decision logic lives in one pure function, `EntryRuntime::plan`,
which is where the interesting tests are.
