# Full audit of LinuxVR and getaddrinfo-rs

Status: completed source audit and isolated verification, 2026-09-10. Each file checkbox records review and relevant verification, not a guarantee against all defects. Live hardware limitations are listed below.

Scope: project-owned source, configuration, documentation, assets, tests, and distribution artifacts. Generated target directories and Git internals are excluded; vendored reference projects are supporting material, not code to modify.

Existing user changes are part of the starting baseline; do not discard them. No deployment or live VR/Steam changes during testing.

## Cross-project verification

- [x] Establish formatting, Clippy, and test baselines.
- [x] Verify native DNS interception, Wine resolver behavior, rule precedence, hot reload, and failure handling.
- [x] Verify LinuxVR lifecycle, process ownership, audio/display restoration logic, persistence, and shutdown through source review and isolated tests (live hardware excluded).
- [x] Verify build, installation, CI, documentation, and integration with the sole Rust DNS implementation.

## lvr

- [x] `AUDIT.md`
- [x] `.github/workflows/ci.yml`
- [x] `assets/lvr.service`
- [x] `src/files.rs`
- [x] `src/systemd.rs`
- [x] `src/domain_block/data.rs`
- [x] `src/domain_block/prefix.rs`
- [x] `src/domain_block/tests.rs`
- [x] `tests/test_domain_scripts.py`
- [x] `c_src/liblvr_dns_shield.so` — removed obsolete ignored C build artifact.

- [x] `.agents/prompt.md`
- [x] `.agents/rules/conventions.md`
- [x] `.github/workflows/build_hosts.yml`
- [x] `.gitignore`
- [x] `Cargo.lock`
- [x] `Cargo.toml`
- [x] `Prefix hosts file,` — empty pre-existing untracked artifact; preserved.
- [x] `Prefix hosts.d dir,` — empty pre-existing untracked artifact; preserved.
- [x] `README.md`
- [x] `assets/lists/blocklist.schema.json`
- [x] `assets/lists/community.json`
- [x] `assets/lists/domains.json`
- [x] `assets/lvr.desktop`
- [x] `assets/lvr.svg`
- [x] `assets/vrchat_config_fallback.json`
- [x] `build.rs`
- [x] `c_src/dns_shield.c` — removed; getaddrinfo-rs is the sole interceptor.
- [x] `clippy.toml`
- [x] `scripts/build.sh`
- [x] `scripts/build_hosts_files.py`
- [x] `scripts/convert_urls.py`
- [x] `scripts/extract_urls.py`
- [x] `src/audio.rs`
- [x] `src/config.rs`
- [x] `src/display.rs`
- [x] `src/domain_block/dns_shield.rs`
- [x] `src/domain_block/mod.rs`
- [x] `src/domain_block/vrchat_config.rs`
- [x] `src/engine/audio_routing.rs`
- [x] `src/engine/entries.rs`
- [x] `src/engine/mod.rs`
- [x] `src/engine/tests.rs`
- [x] `src/engine/watchdog.rs`
- [x] `src/icon.rs`
- [x] `src/ipc.rs`
- [x] `src/main.rs`
- [x] `src/procs.rs`
- [x] `src/state.rs`
- [x] `src/steam/launch_options.rs`
- [x] `src/steam/mod.rs`
- [x] `src/steam/paths.rs`
- [x] `src/steam/process.rs`
- [x] `src/steam/vdf.rs`
- [x] `src/tray.rs`
- [x] `src/ui/audio_tab.rs`
- [x] `src/ui/autostart.rs`
- [x] `src/ui/dashboard.rs`
- [x] `src/ui/logs.rs`
- [x] `src/ui/mod.rs`
- [x] `src/ui/settings.rs`
- [x] `src/ui/widgets.rs`
- [x] `src/wivrn.rs`
- [x] `tests/dns_test/Program.cs`
- [x] `tests/dns_test/dns_test.csproj`
- [x] `todo.md`

## getaddrinfo-rs

- [x] `src/hooks.rs`
- [x] `src/packet.rs`
- [x] `tests/preload.py`
- [x] `tests/wine.c`

- [x] `.github/workflows/ci.yml`
- [x] `.gitignore`
- [x] `Cargo.lock`
- [x] `Cargo.toml`
- [x] `README.md`
- [x] `dist/getaddrinfo-rs-linux-i686`
- [x] `dist/getaddrinfo-rs-linux-x86_64`
- [x] `dist/getaddrinfo-rs-v0.1.0-linux-i686.tar.gz`
- [x] `dist/getaddrinfo-rs-v0.1.0-linux-x86_64.tar.gz`
- [x] `scripts/build.sh`
- [x] `src/discovery.rs`
- [x] `src/lib.rs`
- [x] `src/main.rs`
- [x] `src/parser.rs`
- [x] `src/rules.rs`
- [x] `tests/integration.rs`

- [x] `dist/libgetaddrinfo-linux-i686.so` — inspected pre-audit binary; not rebuilt or published.

- [x] `dist/libgetaddrinfo-linux-x86_64.so` — inspected pre-audit binary; not rebuilt or published.

## Findings and verification log

- Baseline: lvr has five modified source files and two untracked files; getaddrinfo-rs has three modified source files and untracked distribution artifacts. No initial edits were discarded.
- Reconciled historical lvr conventions with hosts-based policy and the sole Rust interceptor.

### Completed findings

- Both original Clippy baselines failed; original lvr tests passed 111/111 outside socket sandbox, resolver tests 10/10.
- Removed lvr C interceptor and build-time network fetch. Deployment uses the single Rust resolver and replaces library inodes atomically.
- Resolver fixes: local A/AAAA packets, correct thread-local resolver errors, preservation of address mappings/redirects, redirect-cycle denial, root-dot handling, throttled reload, serialized initial loading, and actual exported-hook tests.
- Lifecycle fixes: single-instance file lock, no unsafe startup after guard failure, unexpected supervisor exit propagates failure, service-manager watchdog, no-tray visibility, command timeouts, manual audio transition semantics, retained child reaping, owned-only display cleanup.
- File safety: atomic hosts/config/VDF writes; recoverable yt-dlp backup; persisted desired policy; no false success without a prefix; bounded retries and contextual errors.
- Steam: preserve other LD_PRELOAD libraries and quoted arguments, re-read after Steam shutdown, restart on edit failure, support inserting missing LaunchOptions, UTF-8/empty-path parsing, discover additional libraries, avoid arbitrary multi-account fallback.
- Data tooling: missing input cannot erase community categories, database access is explicit/read-only, publishing does not extract personal history, core-domain protection and category deduplication, daily workflow stages only files it generates.
- Python regression suite: 5/5 pass. .NET diagnostic builds with 0 warnings/errors. ShellCheck, desktop-entry validation, and systemd user-unit verification pass.
- Existing distribution archives have safe relative members but predate these changes; they are not release evidence and were not published.

### Technical reference

Resolver error handling was checked against the [GNU libc host-name documentation](https://sourceware.org/glibc/manual/2.23/html_node/Host-Names.html): host lookup errors use h_errno, not errno.

### Final verification and limits

- LinuxVR: 128 Rust tests passed, including headless rendering of all five tabs at two widths, policy transitions, atomic file replacement, IPC, and prefix backup/restore. Resolver: 14 Rust tests passed (10 unit, 4 integration). Python domain tooling: 5 tests passed.
- Both projects pass formatting, Clippy with warnings denied, release builds, and Git whitespace checks. Native LD_PRELOAD tests cover actual exported hooks, IPv4/IPv6, redirect cycles, errors, concurrent calls, reload, atomic replacement, and snippet deletion. A cross-project test resolves domains from the hosts file written by LinuxVR.
- An isolated 64-bit Wine Winsock executable passed wildcard blocking and redirected-address checks with the Rust library. A disposable prefix was used. The 32-bit helper loader rejected the 64-bit preload during prefix initialization; 32-bit interception remains unverified.
- Temporary installation passed with spaces in paths, identical deployed Rust libraries, CLI checks, desktop validation, startup-file backup, and generated systemd-unit validation. Service commands were mocked; no live service or game prefix was changed.
- Actual headset transitions, desktop tray integration, audio devices, KDE output creation/restoration, and live Steam shutdown/relaunch require a hardware session. Source-level safeguards and isolated tests do not substitute for that coverage.
- GitHub workflows were reviewed; local checks passed, but hosted CI was not run. The LinuxVR workflow checks out the resolver's remote main branch, so the resolver changes must be published first. No commits were pushed and no releases were published. Existing distribution artifacts remain pre-audit versions.
- The resolver checkout began two commits behind its cached remote (Renovate-only changes); unrelated remote changes were not merged.
- Local verification logs: `/tmp/lvr-audit-install.log`, `/tmp/lvr-wine-audit.log`, and `/tmp/lvr-audit-clippy.log`. These temporary logs are not repository artifacts.
