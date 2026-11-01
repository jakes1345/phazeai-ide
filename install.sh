#!/usr/bin/env bash
# Build PhazeAI from this checkout and install it for the current user.
#
#   ./install.sh                 build (release) and install to ~/.local
#   ./install.sh --prefix DIR    install to DIR/bin and DIR/share instead
#   ./install.sh --debug         build the dev profile (faster, slower to run)
#   ./install.sh --no-build      install binaries already built in target/
#   ./install.sh --uninstall     remove what this script installed
#
# Installs:
#   <prefix>/bin/phazeai-ui     the desktop IDE
#   <prefix>/bin/phazeai-ide    a link to it (the name older launchers use)
#   <prefix>/bin/phazeai        the terminal UI / CLI
#   <prefix>/share/applications/phazeai-ide.desktop
#   <prefix>/share/icons/hicolor/256x256/apps/phazeai-ide.png
#
# Never uses sudo. If a system library is missing it prints the command to
# install it and stops.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="$HOME/.local"
PROFILE="release"
BUILD=1
UNINSTALL=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) PREFIX="${2:?--prefix needs a directory}"; shift 2 ;;
        --prefix=*) PREFIX="${1#*=}"; shift ;;
        --debug) PROFILE="debug"; shift ;;
        --no-build) BUILD=0; shift ;;
        --uninstall) UNINSTALL=1; shift ;;
        -h|--help) sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
done

BIN="$PREFIX/bin"
APPS="$PREFIX/share/applications"
ICONS="$PREFIX/share/icons/hicolor/256x256/apps"
DESKTOP="$APPS/phazeai-ide.desktop"
ICON="$ICONS/phazeai-ide.png"

refresh_desktop_caches() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$APPS" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null || true
}

if [[ $UNINSTALL -eq 1 ]]; then
    rm -f "$BIN/phazeai-ui" "$BIN/phazeai-ide" "$BIN/phazeai" "$DESKTOP" "$ICON"
    refresh_desktop_caches
    echo "Removed PhazeAI from $PREFIX. Settings in ~/.config/phazeai were kept."
    exit 0
fi

if [[ $BUILD -eq 1 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo was not found. Install Rust with rustup (https://rustup.rs), then run this again." >&2
        exit 1
    fi
    # System libraries the build links against (pkg-config names -> Debian/Ubuntu/Mint packages).
    missing=()
    if ! command -v pkg-config >/dev/null 2>&1; then
        missing+=(pkg-config)
    else
        # Only what the build links against; the window system libraries
        # (wayland, xkbcommon) are loaded at run time and come with a desktop.
        for pair in dbus-1:libdbus-1-dev openssl:libssl-dev; do
            pkg="${pair%%:*}"; deb="${pair##*:}"
            pkg-config --exists "$pkg" || missing+=("$deb")
        done
    fi
    if [[ ${#missing[@]} -gt 0 ]]; then
        echo "Missing build dependencies. On Linux Mint / Ubuntu / Debian run:" >&2
        echo "  sudo apt install build-essential ${missing[*]}" >&2
        exit 1
    fi
    echo "==> Building PhazeAI ($PROFILE)..."
    if [[ $PROFILE == release ]]; then
        cargo build --release -p phazeai-ui -p phazeai-cli --manifest-path "$ROOT/Cargo.toml"
    else
        cargo build -p phazeai-ui -p phazeai-cli --manifest-path "$ROOT/Cargo.toml"
    fi
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
OUT="$TARGET_DIR/$PROFILE"
for b in phazeai-ui phazeai; do
    if [[ ! -x "$OUT/$b" ]]; then
        echo "$OUT/$b was not built. Run without --no-build, or check the build output above." >&2
        exit 1
    fi
done

echo "==> Installing to $PREFIX..."
mkdir -p "$BIN" "$APPS" "$ICONS"
install -m 0755 "$OUT/phazeai-ui" "$BIN/phazeai-ui"
install -m 0755 "$OUT/phazeai" "$BIN/phazeai"
ln -sf phazeai-ui "$BIN/phazeai-ide"
install -m 0644 "$ROOT/assets/branding/icon_256.png" "$ICON"

cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=PhazeAI IDE
GenericName=AI-Powered IDE
Comment=Local-first AI code editor
Exec=$BIN/phazeai-ui %F
Icon=phazeai-ide
Terminal=false
Categories=Development;IDE;TextEditor;
Keywords=ide;editor;ai;rust;coding;
StartupWMClass=phazeai-ui
EOF
refresh_desktop_caches

echo "==> Installed:"
echo "    $BIN/phazeai-ui   (the IDE; also in your application menu as PhazeAI IDE)"
echo "    $BIN/phazeai      (terminal UI and CLI)"
case ":$PATH:" in
    *":$BIN:"*) ;;
    *) echo "    Note: $BIN is not on your PATH. Add to ~/.bashrc:  export PATH=\"$BIN:\$PATH\"" ;;
esac
echo "    Uninstall with: $0 --uninstall${PREFIX:+ --prefix $PREFIX}"
