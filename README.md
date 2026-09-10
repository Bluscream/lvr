# lvr — LinuxVR

Rust tray app and GUI for WiVRn supervision, headset audio routing, companion apps,
and KDE virtual displays. VRChat domain blocking uses the separate
[getaddrinfo-rs](https://github.com/Bluscream/getaddrinfo-rs) Rust resolver.
There is no embedded C interceptor.

## Build and verify

Rust **1.95 or newer**, a Linux desktop build environment, GNU coreutils (`timeout`), and Python 3 are required.
Keep `getaddrinfo-rs` beside this checkout, or set `GETADDRINFO_DIR`.

```sh
./scripts/build.sh --test-only       # Clippy, Rust/Python tests, native preload tests
./scripts/build.sh                   # checks plus release build
./scripts/build.sh --deploy          # installs under ~/.local; no live app restart
./scripts/build.sh --deploy --autostart
```

`--autostart` installs/enables a systemd user service for the next graphical login
and disables an existing LinuxVR XDG autostart entry after backing it up.
`--no-autostart` leaves existing startup settings alone. Service notifications allow
systemd to restart a failed/stalled supervisor. A standalone launch also exits with
an error if its supervisor thread dies. The service's control group includes apps
it launches, so stopping the service can stop those apps.

`--wine-test` additionally runs a Windows DNS test in a disposable prefix; it needs
Wine (`WINE_BIN`) and MinGW. Wine failures fail verification. Without this flag the
script explicitly reports Wine coverage as skipped. The optional .NET diagnostic
is built with `dotnet build tests/dns_test/dns_test.csproj`; it requires network DNS
for its control domains and treats unresolved queries as inconclusive failures.

## Run

```sh
lvr
lvr --hidden
lvr --no-tray                    # visible window; closing exits
lvr --check                     # validate/load config without starting VR
lvr --status                    # read-only live status probe
lvr --audio vr                  # one-shot output/microphone routing
lvr --audio desktop
lvr --tab settings
```

Configuration is `~/.config/lvr/config.toml`, or `LVR_CONFIG` / `-c PATH`.
Missing configuration is created with defaults. Review the companion-app commands
in the Autostart tab before enabling them; bundled defaults are examples for the
original workstation and may need different paths on another installation.
Logs appear in the GUI and stderr (`LVR_LOG` / `RUST_LOG`).

## Behavior

- WiVRn status comes from its session D-Bus service. Deliberate stops pause its watchdog.
- Audio follows headset transitions. Manual GUI/tray routing lasts until the next
  transition; incomplete automatic switches retry with a delay. Audio uses `pactl`.
- Companion apps use process patterns, start delays, shutdown grace periods, and
  optional restart-on-exit. Negative grace keeps an app running after its trigger ends.
- Virtual display cleanup affects only a child/connector created or enabled by this
  LinuxVR process. Missing display telemetry is not treated as an unplug event.
- Single-instance locking prevents concurrent supervisors from competing over apps.

## Domain blocking

Settings accepts an explicit VRChat prefix (app directory or `pfx`) and otherwise
searches Steam's library folders. Desired block state is persisted in configuration.
LinuxVR atomically updates its tagged prefix hosts sections and retries failures;
unrelated hosts entries are retained. Video blocking also backs up and replaces
`yt-dlp.exe`; unblocking restores that backup. Counts represent configured rules,
not a measurement of network traffic or proof that every client uses system DNS.

The sole resolver is `getaddrinfo-rs`. Installation deploys it as
`~/.local/share/lvr/liblvr_dns_shield.so`; enabling it in Settings updates VRChat's
Steam `LD_PRELOAD` option while preserving other preload libraries and game arguments.
`LVR_DNS_LIBRARY` can point at a custom-built Rust library. Steam is shut down before
editing its configuration and restarted afterward if it was running. Applications
already using an older library must restart to load a new build.

Default lists use a cached/downloaded domain bundle, with a bundled offline fallback.
Disabling community lists uses only official VRChat configuration; additional enabled
community sources are loaded from their configured URLs. Core VRChat domains and
shared-domain categorization are validated before writing hosts entries.

`python3 scripts/build_hosts_files.py` refreshes the public domain bundle.
Local-history extraction is a separate explicit operation:

```sh
python3 scripts/extract_urls.py --database /path/to/VRCX.sqlite3 --log '/path/to/output_log*.txt'
python3 scripts/convert_urls.py
```

Missing URL inputs leave existing community categories unchanged. CI never extracts
local browsing or game history. See [AUDIT.md](AUDIT.md) for file-by-file findings,
verification evidence, and remaining environment-dependent checks.
