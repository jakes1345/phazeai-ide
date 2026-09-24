#!/usr/bin/env bash
# PhazeAI — build from source and install for the current user (Linux).
#
# Installs the IDE (phazeai-ui) and CLI (phazeai) into ~/.local/bin and adds
# a desktop entry. Run from anywhere inside a clone of the repository.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${PHAZEAI_BIN_DIR:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found. Install Rust from https://rustup.rs and re-run." >&2
    exit 1
fi

echo "Building PhazeAI (release)..."
cargo build --release --locked --manifest-path "$REPO_DIR/Cargo.toml" -p phazeai-ui -p phazeai-cli

echo "Installing binaries to $BIN_DIR..."
mkdir -p "$BIN_DIR"
install -m 755 "$REPO_DIR/target/release/phazeai-ui" "$BIN_DIR/phazeai-ui"
install -m 755 "$REPO_DIR/target/release/phazeai" "$BIN_DIR/phazeai"

echo "Adding desktop entry..."
ICON_SRC="$REPO_DIR/assets/branding/icon_256.png"
ICON_REF="utilities-terminal"
if [ -f "$ICON_SRC" ]; then
    mkdir -p "$DATA_DIR/icons/hicolor/256x256/apps"
    install -m 644 "$ICON_SRC" "$DATA_DIR/icons/hicolor/256x256/apps/phazeai.png"
    ICON_REF="phazeai"
fi

mkdir -p "$DATA_DIR/applications"
cat > "$DATA_DIR/applications/phazeai.desktop" <<EOF
[Desktop Entry]
Name=PhazeAI IDE
Comment=AI-powered code editor
Exec=$BIN_DIR/phazeai-ui %F
Icon=$ICON_REF
Terminal=false
Type=Application
Categories=Development;IDE;
Keywords=AI;Coding;Editor;
StartupWMClass=phazeai-ui
EOF

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DATA_DIR/applications" >/dev/null 2>&1 || true
fi

echo
echo "Installed:"
echo "  phazeai-ui  - desktop IDE (also in your application menu as 'PhazeAI IDE')"
echo "  phazeai     - terminal UI"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo
       echo "Note: $BIN_DIR is not on your PATH. Add this to your shell profile:"
       echo "  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
