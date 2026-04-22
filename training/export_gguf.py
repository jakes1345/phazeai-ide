#!/usr/bin/env python3
"""
PhazeAI GGUF Export
====================
Converts a fine-tuned LoRA adapter to GGUF format for Ollama deployment.

This script:
1. Loads the fine-tuned LoRA adapter from training/output/
2. Merges it with the base model
3. Quantizes to various GGUF formats (Q4_K_M, Q8_0, F16)
4. Creates an Ollama Modelfile pointing to the GGUF
5. Registers the model with Ollama

Usage:
    python export_gguf.py                    # Default Q4_K_M quantization
    python export_gguf.py --quant q8_0       # Higher quality, more VRAM
    python export_gguf.py --quant f16        # Full precision (largest)
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path

# ── Configuration ──────────────────────────────────────────────────────

BASE_MODEL = "unsloth/Qwen2.5-Coder-7B-Instruct-bnb-4bit"
ADAPTER_DIR = Path(__file__).parent / "output" / "lora_adapter"
GGUF_DIR = Path(__file__).parent / "output" / "gguf"
MODELFILE_DIR = Path(__file__).parent.parent / "modelfiles"


QUANTIZATION_METHODS = {
    "q4_k_m":  "Good balance of quality and size (recommended)",
    "q5_k_m":  "Higher quality, slightly larger",
    "q8_0":    "Near-lossless quality, large",
    "f16":     "Full precision, largest",
}

PHAZEAI_SYSTEM_PROMPT = """You are PhazeAI, a custom-trained AI coding assistant built into the PhazeAI IDE.
You have been fine-tuned on high-quality coding data to excel at:
- Writing production-quality code in any language
- Understanding and explaining complex codebases
- Finding and fixing bugs
- Code review with actionable feedback

RULES: Write complete code. No placeholders. Include error handling. Match codebase style."""


def check_prerequisites():
    """Verify all required tools are installed."""
    if not ADAPTER_DIR.exists():
        print(f"❌ LoRA adapter not found at {ADAPTER_DIR}")
        print(f"   Run fine_tune.py first!")
        sys.exit(1)
    
    # Check for Ollama
    result = subprocess.run(["which", "ollama"], capture_output=True, text=True)
    if result.returncode != 0:
        print("⚠️  Ollama not found. The GGUF will be exported but not registered.")
        return False
    return True


def export_gguf(quant_method: str = "q4_k_m"):
    """Export fine-tuned model to GGUF format using Unsloth."""
    print("╔═══════════════════════════════════════════╗")
    print("║   PhazeAI GGUF Export                     ║")
    print("╚═══════════════════════════════════════════╝")
    print()
    
    has_ollama = check_prerequisites()
    
    print(f"📦 Quantization: {quant_method} — {QUANTIZATION_METHODS.get(quant_method, 'Custom')}")
    
    # 1. Load model + adapter
    print("\n🔧 Loading base model + LoRA adapter...")
    
    try:
        from unsloth import FastLanguageModel
    except ImportError:
        print("❌ Unsloth not installed. Run:")
        print("   pip install 'unsloth[colab-new] @ git+https://github.com/unslothai/unsloth.git'")
        sys.exit(1)
    
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name=str(ADAPTER_DIR),
        max_seq_length=4096,
        dtype=None,
        load_in_4bit=True,
    )
    
    # 2. Export to GGUF
    GGUF_DIR.mkdir(parents=True, exist_ok=True)
    gguf_filename = f"phaze-coder-custom-{quant_method}.gguf"
    gguf_path = GGUF_DIR / gguf_filename
    
    print(f"\n📤 Exporting to GGUF ({quant_method})...")
    print(f"   Output: {gguf_path}")
    
    model.save_pretrained_gguf(
        str(GGUF_DIR),
        tokenizer,
        quantization_method=quant_method,
    )
    
    # Find the actual output file (Unsloth names it differently)
    gguf_files = list(GGUF_DIR.glob("*.gguf"))
    if not gguf_files:
        print("❌ GGUF export failed — no .gguf file found")
        sys.exit(1)
    
    actual_gguf = gguf_files[0]
    gguf_size_mb = actual_gguf.stat().st_size / (1024 * 1024)
    print(f"\n  ✅ GGUF exported: {actual_gguf.name} ({gguf_size_mb:.0f} MB)")
    
    # 3. Create Ollama Modelfile
    modelfile_content = f"""FROM {actual_gguf.resolve()}
SYSTEM \"\"\"{PHAZEAI_SYSTEM_PROMPT}\"\"\"
PARAMETER temperature 0.3
PARAMETER top_p 0.9
PARAMETER num_ctx 32768
PARAMETER repeat_penalty 1.1
"""
    
    modelfile_path = MODELFILE_DIR / "Modelfile.custom"
    MODELFILE_DIR.mkdir(parents=True, exist_ok=True)
    modelfile_path.write_text(modelfile_content)
    print(f"  📝 Modelfile written: {modelfile_path}")
    
    # 4. Register with Ollama
    if has_ollama:
        print("\n🔨 Registering with Ollama as 'phaze-coder-custom'...")
        result = subprocess.run(
            ["ollama", "create", "phaze-coder-custom", "-f", str(modelfile_path)],
            capture_output=True,
            text=True
        )
        
        if result.returncode == 0:
            print("  ✅ Registered as 'phaze-coder-custom'")
        else:
            print(f"  ❌ Registration failed: {result.stderr}")
    
    print(f"\n╔═══════════════════════════════════════════╗")
    print(f"║  ✅ GGUF Export complete!                 ║")
    print(f"║                                           ║")
    print(f"║  Model: phaze-coder-custom                ║")
    print(f"║  Size:  {gguf_size_mb:.0f} MB                            ║")
    print(f"║                                           ║")
    print(f"║  Test: ollama run phaze-coder-custom      ║")
    print(f"╚═══════════════════════════════════════════╝")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Export PhazeAI fine-tuned model to GGUF")
    parser.add_argument(
        "--quant", 
        default="q4_k_m",
        choices=list(QUANTIZATION_METHODS.keys()),
        help="Quantization method (default: q4_k_m)"
    )
    args = parser.parse_args()
    
    export_gguf(args.quant)
