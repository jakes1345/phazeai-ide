#!/usr/bin/env python3
import os
import sys
from unsloth import FastLanguageModel
import torch

def recover_gguf():
    print("Starting GGUF Recovery...")
    
    # Paths
    project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    adapter_path = os.path.join(project_root, "models/fine_tuned/phazeai-qwen/lora")
    output_path = os.path.join(project_root, "models/fine_tuned/phazeai-qwen/gguf")
    
    if not os.path.exists(adapter_path):
        print(f"Error: Adapter path not found: {adapter_path}")
        return

    print(f"Loading adapter from: {adapter_path}")
    
    try:
        model, tokenizer = FastLanguageModel.from_pretrained(
            model_name = adapter_path,
            max_seq_length = 2048,
            dtype = None,
            load_in_4bit = True,
        )
        
        print("Model loaded. Starting GGUF export (q4_k_m)...")
        
        # Ensure output directory exists
        os.makedirs(output_path, exist_ok=True)
        
        model.save_pretrained_gguf(
            output_path, 
            tokenizer, 
            quantization_method = "q4_k_m"
        )
        print("✓ GGUF Export Successful!")
        print(f"Model saved to: {output_path}")
        
    except Exception as e:
        print(f"❌ Recovery failed: {e}")
        # Fallback to direct llama.cpp usage hints?
        sys.exit(1)

if __name__ == "__main__":
    recover_gguf()
