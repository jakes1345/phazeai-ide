#!/bin/bash
# PhazeAI Professional Launcher - Aesthetic & Efficient
# 1. Launches the IDE immediately if binary exists.
# 2. Rebuilds in the background to ensure 'Evergreen' updates.
# 3. Provides clean terminal output.

set -e

PROJECT_DIR="/home/jack/phazeai_ide"
BINARY_PATH="$PROJECT_DIR/target/release/phazeai-ide"
cd "$PROJECT_DIR"

echo "----------------------------------------------------"
echo "  🔥 PHAZEAI IDE - EVOLUTIONARY CODING"
echo "----------------------------------------------------"

if [ -f "$BINARY_PATH" ]; then
    echo "✨ Binary found. Launching PhazeAI immediately..."
    # Launch in background and disown so terminal closure doesn't kill it
    "$BINARY_PATH" & disown
    
    echo "🔄 Checking for background updates..."
    # Perform a quiet build in the background. Next launch will use it.
    (cargo build --release --quiet && echo "✅ Updates built for next launch.") &
else
    echo "🚀 Initial setup detected. Performing first build..."
    cargo build --release
    echo "✨ Launching PhazeAI..."
    "$BINARY_PATH" & disown
fi

echo "----------------------------------------------------"
echo "  Window will close in 3 seconds..."
sleep 3
exit 0
