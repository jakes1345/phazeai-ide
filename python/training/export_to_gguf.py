#!/usr/bin/env python3
"""
Manual GGUF export script - completes export that failed during training.
FIXED: Python detection, error handling, dynamic paths.
"""

import os
import sys
from pathlib import Path
import json

# Fix ptxas path (same as other scripts)
import shutil
try:
    venv_path = Path(sys.executable).parent.parent
    # Find Python directory dynamically
    python_dirs = list(venv_path.glob("lib/python*"))
    if python_dirs:
        python_dir = python_dirs[0]
        triton_ptxas = python_dir / "site-packages" / "triton" / "backends" / "nvidia" / "bin" / "ptxas"
        
        if triton_ptxas.exists():
            tmp_ptxas = Path("/tmp/ptxas_triton")
            try:
                if not tmp_ptxas.exists() or tmp_ptxas.stat().st_mtime < triton_ptxas.stat().st_mtime:
                    shutil.copy2(triton_ptxas, tmp_ptxas)
                    tmp_ptxas.chmod(0o755)
                os.environ['TRITON_PTXAS_PATH'] = str(tmp_ptxas)
                    print(f"✓ Fixed ptxas path (copied to /tmp to avoid space issues)")
            except Exception as e:
                print(f"⚠ Could not copy ptxas: {e}")
except Exception as e:
    print(f"⚠ Could not detect Python directory: {e}")

# Import after ptxas fix
import torch
try:
    from unsloth import FastLanguageModel
    UNSLOTH_AVAILABLE = True
except ImportError:
    print("❌ Unsloth not installed")
    print("Install: pip install \"unsloth[colab-new] @ git+https://github.com/unslothai/unsloth.git\"")
    sys.exit(1)

script_dir = Path(__file__).parent
project_root = script_dir.parent

# Read config to get output directory
config_path = project_root / "config" / "projects.json"
if config_path.exists():
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    # Get AI name for dynamic output directory
    ai_name = config.get("ai_customization", {}).get("name", "phazeai")
    output_dir_name = ai_name.lower().replace(" ", "-")
    output_dir = project_root / "models" / "fine_tuned" / output_dir_name
else:
    print("⚠ Config not found, using default output directory")
    output_dir = project_root / "models" / "fine_tuned" / "phazeai-qwen"

print(f"Loading trained model from: {output_dir}")
print(f"Model output directory: {output_dir}")

# Check if model exists
model_path = output_dir / "lora"
if not model_path.exists():
    print(f"❌ ERROR: Model not found at {model_path}")
    print(f"   Please run fine-tuning first to create the model.")
    print(f"   Example: python3 scripts/sota_fine_tune.py")
    sys.exit(1)

# Load model with LoRA adapters
print("Loading model with LoRA adapters...")
try:
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name = str(model_path),
        max_seq_length = 1024,
        dtype = None,  # Auto-detect
        load_in_4bit = True,
    )
    print("✓ Model loaded successfully")
except Exception as e:
    print(f"❌ ERROR: Failed to load model: {e}")
    print(f"   Check if the model was fine-tuned properly.")
    sys.exit(1)

# Export to GGUF
print("Exporting to GGUF (q4_k_m)...")
gguf_dir = output_dir / "gguf"
gguf_dir.mkdir(parents=True, exist_ok=True)

try:
    model.save_pretrained_gguf(
        str(gguf_dir), 
        tokenizer, 
        quantization_method = "q4_k_m"  # Fixed keyword name
    )
    print(f"✓ GGUF export complete!")
    print(f"   GGUF files saved to: {gguf_dir}")
    
    # List created files
    gguf_files = list(gguf_dir.glob("*.gguf"))
    if gguf_files:
        print(f"   Created files:")
        for gf in gguf_files:
            print(f"     - {gf.name} ({gf.stat().st_size / 1024 / 1024:.1f} MB)")
    
    # Check for Modelfile
    modelfile = gguf_dir / "Modelfile"
    if modelfile.exists():
        print(f"✓ Modelfile created: {modelfile}")
        print(f"\nTo create Ollama model, run:")
        print(f"  ollama create {output_dir_name} -f {modelfile}")
    else:
        print(f"⚠ Modelfile not found at {gguf_dir}")
        print(f"   You may need to create it manually.")
        print(f"   Example Modelfile:")
        print(f"   FROM {gguf_dir}/*.gguf")
        print(f"   PARAMETER temperature 0.3")
        print(f"   PARAMETER top_p 0.9")
        print(f"   SYSTEM You are {ai_name}, an expert AI assistant fine-tuned on your codebase.")
        
except Exception as e:
    print(f"❌ ERROR: GGUF export failed: {e}")
    print(f"   Possible causes:")
    print(f"   - Out of memory (try smaller model or lower quantization)")
    print(f"   - Model corruption (run fine-tuning again)")
    print(f"   - Missing dependencies: pip install --upgrade llama-cpp-python")
    sys.exit(1)

print(f"\n✅ Export process complete!")
