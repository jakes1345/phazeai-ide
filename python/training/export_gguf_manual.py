#!/usr/bin/env python3
"""
Manual GGUF export script - works around unsloth's uv dependency issue.
"""

import os
import sys
import subprocess
from pathlib import Path

# Fix ptxas path
import shutil
venv_path = Path(sys.executable).parent.parent
triton_ptxas = venv_path / "lib" / "python3.12" / "site-packages" / "triton" / "backends" / "nvidia" / "bin" / "ptxas"
if triton_ptxas.exists():
    tmp_ptxas = Path("/tmp/ptxas_triton")
    shutil.copy2(triton_ptxas, tmp_ptxas)
    tmp_ptxas.chmod(0o755)
    os.environ['TRITON_PTXAS_PATH'] = str(tmp_ptxas)

import torch
from unsloth import FastLanguageModel
from pathlib import Path

def export_gguf():
    model_dir = Path(__file__).parent.parent / "models" / "fine_tuned" / "phazeai-qwen"
    lora_dir = model_dir / "lora"
    gguf_dir = model_dir / "gguf"
    
    if not lora_dir.exists():
        print(f"ERROR: LoRA directory not found: {lora_dir}")
        return
    
    print(f"Loading model from {lora_dir}...")
    
    # Load the fine-tuned model
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name = str(lora_dir),
        max_seq_length = 1024,
        dtype = None,
        load_in_4bit = True,
    )
    
    # Export to GGUF (save_pretrained_gguf automatically merges LoRA)
    print(f"Exporting to GGUF in {gguf_dir}...")
    print("Note: This will merge LoRA adapters automatically and may take 10-20 minutes...")
    print("      Requires ~30GB RAM for merging 7B model...")
    
    # Try to install uv if missing (for unsloth's GGUF export)
    import subprocess
    try:
        subprocess.run(["which", "uv"], check=True, capture_output=True)
        print("✓ uv found")
    except:
        print("⚠ uv not found, trying to install...")
        try:
            subprocess.run([sys.executable, "-m", "pip", "install", "uv"], check=True)
            print("✓ uv installed")
        except:
            print("⚠ uv installation failed, will try without it")
    
    try:
        # save_pretrained_gguf automatically merges LoRA weights
        # This may fail if uv/llama.cpp dependencies are missing
        model.save_pretrained_gguf(
            str(gguf_dir),
            tokenizer,
            quantization_method = "q4_k_m"
        )
        print(f"\n✓ GGUF export complete!")
        print(f"Model saved to: {gguf_dir}")
        
        # Check for Modelfile
        modelfile = gguf_dir / "Modelfile"
        if modelfile.exists():
            print(f"\n✓ Modelfile found. You can now create Ollama model:")
            print(f"  ollama create phazeai -f {modelfile}")
        else:
            # Look for .gguf files
            gguf_files = list(gguf_dir.glob("*.gguf"))
            if gguf_files:
                print(f"\n✓ Found GGUF files:")
                for f in gguf_files:
                    print(f"  - {f.name}")
                print(f"\nYou can create Ollama model manually:")
                print(f"  ollama create phazeai --file {gguf_files[0]}")
            else:
                print(f"\n⚠ No GGUF files found in {gguf_dir}")
            
    except Exception as e:
        print(f"\n❌ GGUF export failed: {e}")
        print("\nThis is usually due to:")
        print("  1. Missing system dependencies (libcurl4-openssl-dev)")
        print("  2. uv/llama.cpp build issues")
        print("  3. Insufficient RAM (~30GB needed for merging)")
        print("\nTrying alternative: Save merged model for manual conversion...")
        
        # Alternative: Merge and save as 16-bit safetensors
        print("\nMerging LoRA adapters...")
        try:
            # Get the base PEFT model and merge
            if hasattr(model, 'merge_and_unload'):
                merged_model = model.merge_and_unload()
            else:
                # Try peft method
                from peft import PeftModel
                if isinstance(model, PeftModel):
                    merged_model = model.merge_and_unload()
                else:
                    print("⚠ Cannot merge - model structure unknown")
                    return
            
            merged_dir = gguf_dir / "merged_16bit"
            merged_dir.mkdir(parents=True, exist_ok=True)
            
            print(f"Saving merged 16-bit model to {merged_dir}...")
            merged_model.save_pretrained(str(merged_dir), safe_serialization=True)
            tokenizer.save_pretrained(str(merged_dir))
            
            print(f"\n✓ Saved merged 16-bit model to {merged_dir}")
            print("\nTo convert to GGUF manually:")
            print("  1. Install llama.cpp: git clone https://github.com/ggerganov/llama.cpp")
            print("  2. Convert: python llama.cpp/convert_hf_to_gguf.py {merged_dir} --outdir {gguf_dir}")
            print("  3. Quantize: ./llama.cpp/quantize {gguf_file} {gguf_file}.q4_k_m q4_K_M")
            
        except Exception as e2:
            print(f"❌ Alternative export also failed: {e2}")
            print("\nYour LoRA adapters are still saved at:")
            print(f"  {lora_dir}")
            print("You can load and use them directly with Unsloth.")

if __name__ == "__main__":
    export_gguf()
