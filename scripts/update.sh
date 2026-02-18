#!/bin/bash
# PhazeAI Update Script
# Usage: ./scripts/update.sh

set -e

echo "🔄 Updating PhazeAI IDE..."

# 1. Pull latest changes
git pull origin main

# 2. Rebuild the project
echo "🛠 Rebuilding workspace..."
cargo build --release

# 3. Update Modelfiles if needed
echo "🧠 Checking local models..."
cd modelfiles && bash install.sh

echo "✅ PhazeAI is up to date!"
