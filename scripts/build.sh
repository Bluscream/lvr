#!/usr/bin/env bash
# Verify both projects and optionally install user-local files. No live VR operations.
set -euo pipefail
SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GETADDRINFO_DIR="${GETADDRINFO_DIR:-$SOURCE_DIR/../getaddrinfo-rs}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
LIB_DIR="${LIB_DIR:-$HOME/.local/lib}"
APP_DIR="${APP_DIR:-$HOME/.local/share/applications}"
ICON_DIR="${ICON_DIR:-$HOME/.local/share/icons/hicolor/scalable/apps}"
AUTOSTART_DIR="${AUTOSTART_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/autostart}"
UNIT_DIR="${UNIT_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user}"
DESKTOP_ID="${DESKTOP_ID:-lvr}"
deploy=no
autostart=no
test_only=no
uninstall=no
wine_arg=--skip-wine-test
for arg in "$@"; do
    case "$arg" in
        --deploy) deploy=yes ;;
        --autostart) autostart=yes ;;
        --no-autostart) autostart=no ;;
        --test-only) test_only=yes ;;
        --wine-test) wine_arg=--wine-test ;;
        --skip-wine-test) wine_arg=--skip-wine-test ;;
        --uninstall) uninstall=yes ;;
        -h|--help)
            echo 'Usage: scripts/build.sh [--deploy] [--autostart|--no-autostart] [--test-only] [--wine-test|--skip-wine-test] [--uninstall]'
            echo 'Overrides: GETADDRINFO_DIR, BIN_DIR, LIB_DIR, APP_DIR, ICON_DIR, UNIT_DIR, AUTOSTART_DIR, DESKTOP_ID'
            exit 0 ;;
        *) echo "Unknown option: $arg" >&2; exit 2 ;;
    esac
done
case "$DESKTOP_ID" in *[!a-zA-Z0-9._-]*|'') echo 'Invalid DESKTOP_ID' >&2; exit 2 ;; esac
if [ "$uninstall" = yes ]; then
    if [ -f "$UNIT_DIR/$DESKTOP_ID.service" ]; then
        systemctl --user disable --now "$DESKTOP_ID.service"
        rm -- "$UNIT_DIR/$DESKTOP_ID.service"
        systemctl --user daemon-reload
    fi
    rm -f -- "$BIN_DIR/lvr" "$APP_DIR/$DESKTOP_ID.desktop" "$ICON_DIR/lvr.svg" "$AUTOSTART_DIR/$DESKTOP_ID.desktop"
    echo 'Removed LinuxVR installation. Config and the independently installed resolver were kept.'
    exit 0
fi
cd "$SOURCE_DIR"
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 -m unittest discover -s tests -p 'test_*.py'
if [ ! -f "$GETADDRINFO_DIR/scripts/build.sh" ]; then
    echo "getaddrinfo-rs is required at $GETADDRINFO_DIR (override GETADDRINFO_DIR)." >&2
    exit 1
fi
"$GETADDRINFO_DIR/scripts/build.sh" --test "$wine_arg"
resolver_target="$(cargo metadata --manifest-path "$GETADDRINFO_DIR/Cargo.toml" --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
LVR_TEST_DNS_LIBRARY="$resolver_target/release/libgetaddrinfo.so" cargo test --locked block_unblock_is_repeatable
if [ "$test_only" = yes ]; then exit 0; fi
cargo build --locked --release
if [ "$deploy" != yes ]; then echo 'Build and checks passed. Use --deploy to install.'; exit 0; fi
# Resolver installation uses the same verified Rust implementation as standalone installs.
BIN_DIR="$BIN_DIR" LIB_DIR="$LIB_DIR" "$GETADDRINFO_DIR/scripts/build.sh" --deploy --skip-wine-test

target_dir="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
install_atomic() {
    install -Dm"$1" "$2" "$3.new"
    mv -f -- "$3.new" "$3"
}
install_atomic 755 "$target_dir/release/lvr" "$BIN_DIR/lvr"
install_atomic 644 "$SOURCE_DIR/assets/lvr.svg" "$ICON_DIR/lvr.svg"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/lvr"
install_atomic 755 "$LIB_DIR/libgetaddrinfo.so" "$data_dir/liblvr_dns_shield.so"
# Generate quoted desktop and service commands without sed replacement expansion.
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
python3 - "$SOURCE_DIR" "$work" "$BIN_DIR/lvr" <<'PY'
from pathlib import Path
import sys
source, work, binary = map(Path, sys.argv[1:])
def quote(value):
    return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$') + '"'
desktop = (source / 'assets/lvr.desktop').read_text().replace('Exec=lvr', 'Exec=' + quote(binary).replace('%', '%%'))
(work / 'lvr.desktop').write_text(desktop)
service_binary = '"' + str(binary).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace('$', '$$') + '"'
service = (source / 'assets/lvr.service').read_text().replace('%h/.local/bin/lvr', service_binary)
(work / 'lvr.service').write_text(service)
PY
install_atomic 644 "$work/lvr.desktop" "$APP_DIR/$DESKTOP_ID.desktop"
if [ "$autostart" = yes ]; then
    install_atomic 644 "$work/lvr.service" "$UNIT_DIR/$DESKTOP_ID.service"
    # Keep a copy of an old XDG entry, then disable it to prevent competing launchers.
    if [ -f "$AUTOSTART_DIR/$DESKTOP_ID.desktop" ]; then
        cp -p -- "$AUTOSTART_DIR/$DESKTOP_ID.desktop" "$AUTOSTART_DIR/$DESKTOP_ID.desktop.bak"
        printf '[Desktop Entry]\nType=Application\nName=LinuxVR\nHidden=true\n' > "$AUTOSTART_DIR/$DESKTOP_ID.desktop"
    fi
    systemctl --user daemon-reload
    systemctl --user enable "$DESKTOP_ID.service"
    echo 'Systemd login startup enabled. The running session was left alone.'
fi
if command -v update-desktop-database >/dev/null; then update-desktop-database "$APP_DIR"; fi
echo "Installed $BIN_DIR/lvr and the Rust DNS resolver. Restart running applications to load the new library."
