#!/usr/bin/env bash
# Build a .deb package for PhazeAI IDE (Debian/Ubuntu)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VERSION="${1:-0.1.0}"
ARCH="amd64"
PKG_NAME="phazeai-ide"
BUILD_DIR="$PROJECT_ROOT/build/deb/${PKG_NAME}_${VERSION}_${ARCH}"

echo "==> Building PhazeAI IDE v$VERSION .deb package"

# Compile release binary
echo "==> Compiling release binary..."
cargo build --release -p phazeai-ide --manifest-path "$PROJECT_ROOT/Cargo.toml"

BINARY="$PROJECT_ROOT/target/release/phazeai-ide"

# Set up package structure
echo "==> Setting up package structure..."
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/DEBIAN"
mkdir -p "$BUILD_DIR/usr/bin"
mkdir -p "$BUILD_DIR/usr/share/applications"
mkdir -p "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$BUILD_DIR/usr/share/doc/$PKG_NAME"

# Copy binary
cp "$BINARY" "$BUILD_DIR/usr/bin/phazeai-ide"
chmod 0755 "$BUILD_DIR/usr/bin/phazeai-ide"

# Icon
ICON_PNG="$PROJECT_ROOT/assets/icon-256.png"
if [[ -f "$ICON_PNG" ]]; then
    cp "$ICON_PNG" "$BUILD_DIR/usr/share/icons/hicolor/256x256/apps/phazeai-ide.png"
fi

# Desktop file
cat > "$BUILD_DIR/usr/share/applications/phazeai-ide.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=PhazeAI IDE
GenericName=AI-Powered IDE
Comment=Local-first AI-native IDE with multi-model support
Exec=phazeai-ide %F
Icon=phazeai-ide
Terminal=false
Categories=Development;IDE;TextEditor;
MimeType=text/plain;
Keywords=ide;editor;ai;rust;python;coding;
StartupWMClass=phazeai-ide
EOF

# Changelog
cat > "$BUILD_DIR/usr/share/doc/$PKG_NAME/changelog.Debian" <<EOF
$PKG_NAME ($VERSION) unstable; urgency=low

  * Initial release.

 -- PhazeAI Technologies <dev@phazeai.com>  $(date -R)
EOF
gzip -9 "$BUILD_DIR/usr/share/doc/$PKG_NAME/changelog.Debian"

# Copyright
cat > "$BUILD_DIR/usr/share/doc/$PKG_NAME/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: phazeai-ide
Upstream-Contact: PhazeAI Technologies <dev@phazeai.com>
Source: https://github.com/phazeai/ide

Files: *
Copyright: 2024 PhazeAI Technologies
License: MIT
EOF

# Control file
INSTALLED_SIZE=$(du -sk "$BUILD_DIR" | awk '{print $1}')
cat > "$BUILD_DIR/DEBIAN/control" <<EOF
Package: $PKG_NAME
Version: $VERSION
Section: devel
Priority: optional
Architecture: $ARCH
Installed-Size: $INSTALLED_SIZE
Depends: libc6 (>= 2.17), libglib2.0-0, libx11-6, libxcb1
Recommends: rust-analyzer, python3-pyright
Maintainer: PhazeAI Technologies <dev@phazeai.com>
Homepage: https://github.com/phazeai/ide
Description: AI-powered local-first IDE
 PhazeAI IDE is an open-source, AI-native development environment.
 Features multi-model AI support (Claude, OpenAI, Ollama), integrated
 terminal, LSP support, git integration, and a powerful agent system.
EOF

# Post-install script to update icon cache
cat > "$BUILD_DIR/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -qtf /usr/share/icons/hicolor 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications 2>/dev/null || true
fi
EOF
chmod 0755 "$BUILD_DIR/DEBIAN/postinst"

# Fix permissions
find "$BUILD_DIR" -type f -name "*.desktop" -exec chmod 0644 {} \;
find "$BUILD_DIR" -type f -name "*.png" -exec chmod 0644 {} \;

# Build .deb
echo "==> Building .deb package..."
mkdir -p "$PROJECT_ROOT/dist"
OUTPUT="$PROJECT_ROOT/dist/${PKG_NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --build "$BUILD_DIR" "$OUTPUT"

echo ""
echo "==> Package built: $OUTPUT"
echo "    Size: $(du -sh "$OUTPUT" | cut -f1)"
echo ""
echo "Install with:  sudo dpkg -i $OUTPUT"
echo "Or:            sudo apt install $OUTPUT"
