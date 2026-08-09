#!/usr/bin/env bash
# Build and install lvr (LinuxVR) into the current user's home.
# Nothing here needs root and nothing outside $HOME is touched.
set -euo pipefail

BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
APP_DIR="${APP_DIR:-$HOME/.local/share/applications}"
ICON_DIR="${ICON_DIR:-$HOME/.local/share/icons/hicolor/scalable/apps}"
AUTOSTART_DIR="${AUTOSTART_DIR:-$HOME/.config/autostart}"
# Desktop-entry basename. Change it to install alongside another build of lvr
# instead of replacing its menu entry.
DESKTOP_ID="${DESKTOP_ID:-lvr}"
SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

autostart=ask
action=install

usage() {
    cat <<'EOF'
Usage: ./install.sh [OPTIONS]

  --autostart        Also start LinuxVR when you log in
  --no-autostart     Do not touch the autostart entry
  --uninstall        Remove everything this script installed
  -h, --help         Show this help

Environment overrides: BIN_DIR, APP_DIR, ICON_DIR, AUTOSTART_DIR, DESKTOP_ID

DESKTOP_ID is the desktop-entry basename (default "lvr"). Set it to install
alongside another build of lvr rather than replacing its menu entry, e.g.
    DESKTOP_ID=lvr-opus ./install.sh
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --autostart) autostart=yes ;;
        --no-autostart) autostart=no ;;
        --uninstall) action=uninstall ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

refresh_caches() {
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -f -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
    command -v kbuildsycoca6 >/dev/null 2>&1 &&
        kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
    command -v kbuildsycoca5 >/dev/null 2>&1 &&
        kbuildsycoca5 >/dev/null 2>&1 || true
}

if [ "$action" = uninstall ]; then
    echo "Stopping any running instance…"
    pkill -x lvr 2>/dev/null || true
    for target in "$BIN_DIR/lvr" "$APP_DIR/$DESKTOP_ID.desktop" \
                  "$ICON_DIR/lvr.svg" "$AUTOSTART_DIR/$DESKTOP_ID.desktop"; do
        [ -e "$target" ] || continue
        rm -f "$target"
        echo "  removed $target"
        # Put back whatever we displaced when installing.
        restored=$(ls -1t "$target".bak-* 2>/dev/null | head -n1 || true)
        if [ -n "$restored" ]; then
            mv "$restored" "$target"
            echo "  restored the file that was there before ($target)"
        fi
    done
    refresh_caches
    echo "Done. Your config at ~/.config/lvr/config.toml was kept."
    exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
    cat >&2 <<'EOF'
cargo was not found.

Install a Rust toolchain first, either on the host:
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
or inside a distrobox (handy on image-based distros like Bazzite):
    distrobox create --name rust --image registry.fedoraproject.org/fedora:latest
    distrobox enter rust -- sudo dnf install -y cargo rust
Then run this script again (from inside the distrobox if you used one).
EOF
    exit 1
fi

echo "Building lvr (release)…"
cargo build --release --manifest-path "$SOURCE_DIR/Cargo.toml"

# Never clobber a file we did not write: a differing file is copied aside first.
# (An earlier build of lvr, or another project that claimed the same name.)
backups=()
install_file() {
    local mode="$1" source="$2" target="$3"
    if [ -e "$target" ] && ! cmp -s "$source" "$target"; then
        local backup="$target.bak-$(date +%Y%m%d%H%M%S)"
        cp -p "$target" "$backup"
        backups+=("$backup")
    fi
    install -D"m$mode" "$source" "$target"
}

entry="$SOURCE_DIR/target/$DESKTOP_ID.desktop"
if [ "$DESKTOP_ID" = lvr ]; then
    cp "$SOURCE_DIR/assets/lvr.desktop" "$entry"
else
    # A custom id means another lvr is already in the menu, so make the two
    # tellable apart at a glance.
    sed "s|^Name=LinuxVR$|Name=LinuxVR ($DESKTOP_ID)|" \
        "$SOURCE_DIR/assets/lvr.desktop" > "$entry"
fi

echo "Installing…"
install_file 755 "$SOURCE_DIR/target/release/lvr" "$BIN_DIR/lvr"
install_file 644 "$SOURCE_DIR/assets/lvr.svg"     "$ICON_DIR/lvr.svg"
install_file 644 "$entry"                         "$APP_DIR/$DESKTOP_ID.desktop"

if [ "$autostart" = ask ] && [ -t 0 ]; then
    read -r -p "Start LinuxVR automatically when you log in? [Y/n] " reply
    case "${reply:-y}" in [Nn]*) autostart=no ;; *) autostart=yes ;; esac
fi

if [ "$autostart" = yes ]; then
    mkdir -p "$AUTOSTART_DIR"
    sed 's|^Exec=lvr$|Exec=lvr --hidden|' "$entry" \
        > "$SOURCE_DIR/target/$DESKTOP_ID-autostart.desktop"
    install_file 644 "$SOURCE_DIR/target/$DESKTOP_ID-autostart.desktop" \
                     "$AUTOSTART_DIR/$DESKTOP_ID.desktop"
    echo "  autostart enabled ($AUTOSTART_DIR/$DESKTOP_ID.desktop)"
else
    echo "  autostart left alone"
fi

refresh_caches

echo
echo "Installed:"
echo "  $BIN_DIR/lvr"
echo "  $APP_DIR/$DESKTOP_ID.desktop"
echo "  $ICON_DIR/lvr.svg"

if [ ${#backups[@]} -gt 0 ]; then
    echo
    echo "Existing files were replaced; the originals were kept as:"
    printf '  %s\n' "${backups[@]}"
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo; echo "NOTE: $BIN_DIR is not on your PATH." ;;
esac

echo
echo "First run creates ~/.config/lvr/config.toml with sensible defaults."
echo "Check what it will do without starting anything:  lvr --check"
echo "See the live state of your VR stack:              lvr --status"
echo "Start it:                                         lvr"
