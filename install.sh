#!/bin/bash

# PhazeAI Installation Script
# This script builds PhazeAI in release mode and installs it to ~/.local/bin

set -e

echo "🚀 Starting PhazeAI Installation..."

# 1. Build in release mode
echo "📦 Building PhazeAI (Release Mode)..."
cargo build --release --workspace

# 2. Create local bin directory if it doesn't exist
mkdir -p ~/.local/bin

# 3. Copy binaries
echo "🚚 Installing binaries to ~/.local/bin/..."
cp target/release/phazeai ~/.local/bin/phazeai
cp target/release/phazeai-ide ~/.local/bin/phazeai-ide

# 4. Set up desktop entry
echo "🖥️ Setting up desktop integration..."
APP_DIR="/home/jack/phazeai_ide"
ICON_PATH="$APP_DIR/phazeai.png"
DESKTOP_FILE="$HOME/.local/share/applications/phazeai.desktop"

# Copy icon if it exists (assuming it's in the root)
if [ -f "$ICON_PATH" ]; then
    mkdir -p ~/.local/share/icons
    cp "$ICON_PATH" ~/.local/share/icons/phazeai.png
    ICON_REF="phazeai"
else
    ICON_REF="utilities-terminal"
fi

cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Name=PhazeAI
Comment=AI-powered coding assistant
Exec=$HOME/.local/bin/phazeai-ide
Icon=$ICON_REF
Terminal=false
Type=Application
Categories=Development;IDE;
Keywords=AI;Coding;Rust;
EOF

chmod +x "$DESKTOP_FILE"

# 5. Final message
echo "✅ Installation Complete!"
echo ""
echo "You can now run:"
echo "  - 'phazeai' from your terminal to start the CLI."
echo "  - 'PhazeAI' from your application menu to start the IDE."
echo ""
echo "Note: If 'phazeai' is not found, make sure ~/.local/bin is in your PATH."
echo "Add this to your .bashrc or .zshrc if needed:"
echo "  export PATH=\$PATH:\$HOME/.local/bin"
