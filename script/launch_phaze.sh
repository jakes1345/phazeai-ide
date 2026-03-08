#!/bin/bash
# PhazeAI IDE Launcher — launches immediately, rebuilds in background for next time

PROJECT_DIR="/home/jack/phazeai_ide"
BINARY_PATH="$PROJECT_DIR/target/release/phazeai-ui"
cd "$PROJECT_DIR"

if [ -f "$BINARY_PATH" ]; then
    "$BINARY_PATH" & disown
    # Silent background rebuild so next launch gets latest
    (cargo build --release -p phazeai-ui --quiet 2>/dev/null && \
     cp "$BINARY_PATH" ~/.local/bin/phazeai-ui) &
else
    echo "Building PhazeAI IDE (first run)..."
    cargo build --release -p phazeai-ui
    "$BINARY_PATH" & disown
fi
