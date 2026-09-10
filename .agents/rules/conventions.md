# lvr — Agent Conventions

## Commits and code structure

Make frequent, atomic commits after meaningful verified changes. Use `<type>: <description>`.
Keep unrelated user edits intact. No dead code, commented-out implementations, or runtime
aliases for removed application formats. `build.rs` enforces the existing forbidden-word
policy and the 1000-line maximum for Rust source files; prefer focused modules well below it.

## Domain categories and policy

Canonical category names are Videos, Images, Strings, Shared, and verbatim custom names.
Convert official API field names at their input boundary; do not add aliases to from_name.
Validate domains and protect VRChat core/asset domains before writing blocklists.

Desired BlockState is kept in memory and persisted in configuration. Existing prefix state
can seed a first run. Hosts files are enforcement output, not a recurring source that may
erase desired policy when Steam resets them. Surface partial failures and retry; do not
report successful enforcement when a required filesystem operation failed.

## One DNS implementation

Use **getaddrinfo-rs only**, as explicitly requested by the user. LinuxVR does not build,
embed, or maintain a separate C interceptor. The build script verifies the sibling project
and deployment installs its Rust library as liblvr_dns_shield.so. Domain rules are written
to the Proton prefix hosts file; stale shield_rules.txt paths are not used.

File replacements must be atomic. Preserve unrelated hosts entries, other LD_PRELOAD
libraries, quoting in launch options, and a usable yt-dlp backup. Never truncate a loaded
shared library in place. All Steam integration belongs in src/steam/.

## UI and lifecycle

Keep primary VR actions large. Category buttons show names with red/green policy colors;
counts belong in summaries and describe rules, not observed traffic. DNS installation
controls belong in Settings. Close-to-tray is allowed only when a tray is available.
Only remove displays/process resources owned by LinuxVR. Logging must explain failures.

## Verification and deployment

Run scripts/build.sh --test-only for both projects' checks and the cross-project native
DNS flow. Wine tests require --wine-test and use a disposable prefix; skipped hardware or
Wine coverage must be reported. Do not test against a live game prefix or deploy merely to
verify an audit. Use scripts/build.sh --deploy when deployment is requested.
