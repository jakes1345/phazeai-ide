#!/bin/bash
# PhazeAI Model Installer
# Creates all custom PhazeAI models in Ollama from Modelfiles.
# Run this once after installing Ollama.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "╔═══════════════════════════════════════════╗"
echo "║     PhazeAI Custom Model Installer        ║"
echo "║     100% Local — Zero Cloud Dependency    ║"
echo "╚═══════════════════════════════════════════╝"
echo ""

# Check if Ollama is running
if ! command -v ollama &> /dev/null; then
    echo "❌ Ollama not found. Install it first:"
    echo "   curl -fsSL https://ollama.ai/install.sh | sh"
    exit 1
fi

if ! curl -s http://localhost:11434/api/tags > /dev/null 2>&1; then
    echo "⚠️  Ollama isn't running. Starting it..."
    ollama serve &
    sleep 3
fi

echo "📦 Step 1/4: Pulling base models (this may take a while on first run)..."
echo ""

# Pull base models if not already present
for model in "qwen2.5-coder:14b" "llama3.2:3b" "deepseek-coder-v2:16b"; do
    if ollama list | grep -q "$(echo $model | cut -d: -f1)"; then
        echo "  ✅ $model already pulled"
    else
        echo "  ⬇️  Pulling $model..."
        ollama pull "$model"
    fi
done

echo ""
echo "🔨 Step 2/4: Creating phaze-coder (primary coding model)..."
ollama create phaze-coder -f "$SCRIPT_DIR/Modelfile.coder"
echo "  ✅ phaze-coder ready"

echo ""
echo "🔨 Step 3/4: Creating phaze-planner (fast planning model)..."
ollama create phaze-planner -f "$SCRIPT_DIR/Modelfile.planner"
echo "  ✅ phaze-planner ready"

echo ""
echo "🔨 Step 4/4: Creating phaze-reviewer (code review model)..."
ollama create phaze-reviewer -f "$SCRIPT_DIR/Modelfile.reviewer"
echo "  ✅ phaze-reviewer ready"

echo ""
echo "╔═══════════════════════════════════════════╗"
echo "║  ✅ All PhazeAI models installed!         ║"
echo "║                                           ║"
echo "║  Models available:                        ║"
echo "║    • phaze-coder    (code generation)     ║"
echo "║    • phaze-planner  (planning)            ║"
echo "║    • phaze-reviewer (code review)         ║"
echo "║                                           ║"
echo "║  Test: ollama run phaze-coder             ║"
echo "╚═══════════════════════════════════════════╝"
