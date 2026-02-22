#!/usr/bin/env bash
# Build a portable AppImage for PhazeAI IDE (Linux x86_64)
# Requirements: cargo, appimagetool (download from https://github.com/AppImage/AppImageKit/releases)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$PROJECT_ROOT/build/appimage"
APPDIR="$BUILD_DIR/PhazeAI-IDE.AppDir"
VERSION="${1:-$(grep '^version' "$PROJECT_ROOT/Cargo.toml" | head -1 | awk -F'"' '{print $2}')}"

echo "==> Building PhazeAI IDE v$VERSION AppImage"

# 1. Compile release binary
echo "==> Compiling release binary..."
cargo build --release -p phazeai-ide --manifest-path "$PROJECT_ROOT/Cargo.toml"

BINARY="$PROJECT_ROOT/target/release/phazeai-ide"
if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: Binary not found at $BINARY" >&2
    exit 1
fi

# 2. Set up AppDir structure
echo "==> Setting up AppDir..."
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/lib"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"
mkdir -p "$APPDIR/usr/share/icons/hicolor/scalable/apps"

# Copy binary
cp "$BINARY" "$APPDIR/usr/bin/phazeai-ide"
chmod +x "$APPDIR/usr/bin/phazeai-ide"

# Copy icon
ICON_PNG="$PROJECT_ROOT/assets/icon-256.png"
ICON_SVG="$PROJECT_ROOT/assets/icon.svg"
if [[ -f "$ICON_PNG" ]]; then
    cp "$ICON_PNG" "$APPDIR/usr/share/icons/hicolor/256x256/apps/phazeai-ide.png"
    cp "$ICON_PNG" "$APPDIR/phazeai-ide.png"
else
    echo "WARNING: No icon found at $ICON_PNG, using placeholder"
    # Create a minimal 1x1 placeholder PNG
    printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde\x00\x00\x00\x0cIDATx\x9cc\xf8\x0f\x00\x00\x01\x01\x00\x05\x18\xd8N\x00\x00\x00\x00IEND\xaeB`\x82' > "$APPDIR/phazeai-ide.png"
fi
if [[ -f "$ICON_SVG" ]]; then
    cp "$ICON_SVG" "$APPDIR/usr/share/icons/hicolor/scalable/apps/phazeai-ide.svg"
fi

# Desktop entry
cat > "$APPDIR/usr/share/applications/phazeai-ide.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=PhazeAI IDE
GenericName=AI-Powered IDE
Comment=Local-first AI-native IDE with multi-model support
Exec=phazeai-ide %F
Icon=phazeai-ide
Terminal=false
Categories=Development;IDE;TextEditor;
MimeType=text/plain;text/x-rust;text/x-python;text/javascript;application/json;
Keywords=ide;editor;ai;rust;python;coding;
StartupWMClass=phazeai-ide
EOF
cp "$APPDIR/usr/share/applications/phazeai-ide.desktop" "$APPDIR/phazeai-ide.desktop"

# AppRun launcher script
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/bash
SELF=$(readlink -f "$0")
HERE="${SELF%/*}"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
exec "$HERE/usr/bin/phazeai-ide" "$@"
EOF
chmod +x "$APPDIR/AppRun"

# 3. Bundle required shared libraries (optional but improves portability)
echo "==> Bundling shared libraries..."
EXCLUDE_LIBS="libGL|libEGL|libX11|libXext|libGLdispatch|libGLX|libxcb|libdbus|libglib|libgobject|ld-linux|libc.so|libm.so|libpthread|libdl.so|librt.so"
ldd "$BINARY" | awk '/=> \// {print $3}' | while read -r lib; do
    libname=$(basename "$lib")
    if ! echo "$libname" | grep -qE "$EXCLUDE_LIBS"; then
        if [[ -f "$lib" ]]; then
            cp -n "$lib" "$APPDIR/usr/lib/" 2>/dev/null || true
        fi
    fi
done

# 4. Build AppImage
echo "==> Building AppImage..."
mkdir -p "$PROJECT_ROOT/dist"
OUTPUT="$PROJECT_ROOT/dist/PhazeAI-IDE-$VERSION-x86_64.AppImage"

if command -v appimagetool &>/dev/null; then
    ARCH=x86_64 appimagetool "$APPDIR" "$OUTPUT"
elif [[ -f "$PROJECT_ROOT/tools/appimagetool" ]]; then
    ARCH=x86_64 "$PROJECT_ROOT/tools/appimagetool" "$APPDIR" "$OUTPUT"
else
    echo "WARNING: appimagetool not found. Download from:"
    echo "  https://github.com/AppImage/AppImageKit/releases"
    echo "  Place in $PROJECT_ROOT/tools/appimagetool or install in PATH"
    echo ""
    echo "AppDir prepared at: $APPDIR"
    echo "Run manually: ARCH=x86_64 appimagetool $APPDIR $OUTPUT"
    exit 0
fi

chmod +x "$OUTPUT"
echo ""
echo "==> AppImage built: $OUTPUT"
echo "    Size: $(du -sh "$OUTPUT" | cut -f1)"
