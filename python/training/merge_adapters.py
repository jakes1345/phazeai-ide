#!/usr/bin/env python3
"""
Merge LoRA adapters back into base model for Ollama conversion.
"""

import json
import torch
from pathlib import Path
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import PeftModel

def main():
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    base_model_name = config["fine_tuning"]["base_model"]
    hf_model_name = f"meta-llama/{base_model_name.capitalize()}-8b" if "llama" in base_model_name.lower() else base_model_name
    
    adapter_path = Path(__file__).parent.parent / config["fine_tuning"]["output_dir"]
    merged_path = Path(__file__).parent.parent / "models" / "merged"
    merged_path.mkdir(parents=True, exist_ok=True)
    
    print(f"Loading base model: {hf_model_name}")
    print(f"Loading adapters from: {adapter_path}")
    
    # Load base model
    tokenizer = AutoTokenizer.from_pretrained(hf_model_name)
    base_model = AutoModelForCausalLM.from_pretrained(
        hf_model_name,
        torch_dtype=torch.float16,
        device_map="auto"
    )
    
    # Load and merge adapters
    print("Loading LoRA adapters...")
    model = PeftModel.from_pretrained(base_model, str(adapter_path))
    
    print("Merging adapters...")
    model = model.merge_and_unload()
    
    print(f"Saving merged model to {merged_path}")
    model.save_pretrained(str(merged_path))
    tokenizer.save_pretrained(str(merged_path))
    
    print("Done! You can now convert this to GGUF format for Ollama.")

if __name__ == "__main__":
    main()

