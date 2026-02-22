#!/usr/bin/env bash
# Build a .dmg installer for PhazeAI IDE (macOS)
# Requirements: cargo, create-dmg (brew install create-dmg)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERSION="${1:-0.1.0}"
APP_NAME="PhazeAI IDE"
BUNDLE_ID="com.phazeai.ide"
BUILD_DIR="$PROJECT_ROOT/build/macos"
APP_DIR="$BUILD_DIR/$APP_NAME.app"

echo "==> Building $APP_NAME v$VERSION for macOS"

# 1. Build release binary
echo "==> Compiling release binary..."
cargo build --release -p phazeai-ide --manifest-path "$PROJECT_ROOT/Cargo.toml"

BINARY="$PROJECT_ROOT/target/release/phazeai-ide"

# 2. Create .app bundle
echo "==> Creating .app bundle..."
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"

cp "$BINARY" "$APP_DIR/Contents/MacOS/phazeai-ide"
chmod +x "$APP_DIR/Contents/MacOS/phazeai-ide"

# Copy icon if available (must be .icns for macOS)
ICNS="$PROJECT_ROOT/assets/icon.icns"
if [[ -f "$ICNS" ]]; then
    cp "$ICNS" "$APP_DIR/Contents/Resources/AppIcon.icns"
fi

# Info.plist
cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>phazeai-ide</string>
    <key>CFBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>CFBundleName</key>
    <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>
    <string>$APP_NAME</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2024 PhazeAI Technologies. MIT License.</string>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Source Code</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.source-code</string>
                <string>public.plain-text</string>
            </array>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
        </dict>
    </array>
</dict>
</plist>
EOF

# 3. Code sign (optional - requires Developer ID certificate)
if [[ -n "${APPLE_IDENTITY:-}" ]]; then
    echo "==> Code signing with identity: $APPLE_IDENTITY"
    codesign --force --deep --sign "$APPLE_IDENTITY" \
        --options runtime \
        --entitlements "$SCRIPT_DIR/entitlements.plist" \
        "$APP_DIR"
else
    echo "WARNING: APPLE_IDENTITY not set. Skipping code signing."
    echo "  The app will show a security warning on first launch."
    echo "  Users can override with: xattr -d com.apple.quarantine '/Applications/$APP_NAME.app'"
fi

# 4. Create DMG
echo "==> Creating DMG..."
mkdir -p "$PROJECT_ROOT/dist"
OUTPUT="$PROJECT_ROOT/dist/PhazeAI-IDE-$VERSION-macos.dmg"

if command -v create-dmg &>/dev/null; then
    create-dmg \
        --volname "$APP_NAME $VERSION" \
        --window-pos 200 120 \
        --window-size 600 400 \
        --icon-size 128 \
        --icon "$APP_NAME.app" 150 185 \
        --hide-extension "$APP_NAME.app" \
        --app-drop-link 450 185 \
        --no-internet-enable \
        "$OUTPUT" \
        "$BUILD_DIR"
else
    echo "WARNING: create-dmg not found. Install with: brew install create-dmg"
    echo "Creating simple DMG with hdiutil instead..."
    STAGING="$BUILD_DIR/dmg_staging"
    mkdir -p "$STAGING"
    cp -r "$APP_DIR" "$STAGING/"
    hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING" \
        -ov -format UDZO "$OUTPUT"
fi

echo ""
echo "==> DMG built: $OUTPUT"
echo "    Size: $(du -sh "$OUTPUT" | cut -f1)"
