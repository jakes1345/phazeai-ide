#!/usr/bin/env python3
"""
Fine-tune Ollama models on your coding patterns and project context.
"""

import os
import json
import subprocess
from pathlib import Path
from typing import List, Dict

def prepare_training_data(data_dir: str, output_file: str):
    """Convert collected data into fine-tuning format."""
    data_dir = Path(data_dir)
    training_examples = []
    
    # Load all JSONL files
    for jsonl_file in data_dir.glob("*.jsonl"):
        with open(jsonl_file, 'r', encoding='utf-8') as f:
            for line in f:
                if line.strip():
                    data = json.loads(line)
                    training_examples.append(data)
    
    # Convert to instruction format
    formatted_data = []
    for example in training_examples:
        project = example.get("project", "unknown")
        filepath = example.get("filepath", "")
        content = example.get("content", "")
        
        # Create instruction-following format
        instruction = f"Generate code for {project} project. File: {filepath}"
        response = content
        
        formatted_data.append({
            "instruction": instruction,
            "input": f"Project: {project}\nFilepath: {filepath}",
            "output": response
        })
    
    # Save formatted data
    output_path = Path(output_file)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    
    with open(output_path, 'w', encoding='utf-8') as f:
        for item in formatted_data:
            f.write(json.dumps(item) + '\n')
    
    print(f"Prepared {len(formatted_data)} training examples")
    return output_path

def create_modelfile(training_data_path: str, base_model: str, output_path: str):
    """Create Ollama Modelfile for fine-tuning."""
    modelfile_content = f"""FROM {base_model}

# Fine-tuned for PhazeAI IDE
# Trained on custom project patterns

PARAMETER temperature 0.7
PARAMETER top_p 0.9
PARAMETER top_k 40
PARAMETER num_ctx 4096

# Training data will be loaded separately
"""
    
    with open(output_path, 'w') as f:
        f.write(modelfile_content)
    
    print(f"Created Modelfile at {output_path}")

def fine_tune_with_ollama(base_model: str, training_data_path: str, model_name: str):
    """
    Fine-tune model using Ollama.
    Note: Ollama doesn't directly support fine-tuning, so we'll use a workaround
    with LoRA adapters or provide instructions for using external tools.
    """
    print(f"\nFine-tuning {base_model} -> {model_name}")
    print(f"Training data: {training_data_path}")
    
    # Check if Ollama is running
    try:
        import requests
        response = requests.get("http://localhost:11434/api/tags")
        if response.status_code != 200:
            print("ERROR: Ollama is not running. Start it with: ollama serve")
            return False
    except:
        print("ERROR: Cannot connect to Ollama. Make sure it's running.")
        return False
    
    # For actual fine-tuning, you'll need to use:
    # 1. Unsloth + Ollama (recommended)
    # 2. Transformers + PEFT (LoRA)
    # 3. llama.cpp fine-tuning
    
    print("\n" + "="*60)
    print("FINE-TUNING OPTIONS:")
    print("="*60)
    print("\nOption 1: Use Unsloth (Fastest)")
    print("  pip install unsloth")
    print("  python scripts/fine_tune_unsloth.py")
    print("\nOption 2: Use Transformers + PEFT (LoRA)")
    print("  python scripts/fine_tune_lora.py")
    print("\nOption 3: Use llama.cpp")
    print("  Follow: https://github.com/ggerganov/llama.cpp/blob/master/examples/finetune/README.md")
    print("\nAfter fine-tuning, create Ollama model:")
    print(f"  ollama create {model_name} -f models/{model_name}/Modelfile")
    print("="*60)
    
    return True

def main():
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    data_dir = Path(__file__).parent.parent / "data" / "training"
    training_data_path = Path(__file__).parent.parent / "data" / "formatted_training.jsonl"
    models_dir = Path(__file__).parent.parent / "models"
    models_dir.mkdir(parents=True, exist_ok=True)
    
    base_model = config["fine_tuning"]["base_model"]
    model_name = config["ollama"]["fine_tuned_model"]
    
    print("Preparing training data...")
    prepare_training_data(str(data_dir), str(training_data_path))
    
    print("\nCreating Modelfile...")
    modelfile_path = models_dir / model_name / "Modelfile"
    modelfile_path.parent.mkdir(parents=True, exist_ok=True)
    create_modelfile(str(training_data_path), base_model, str(modelfile_path))
    
    print("\nStarting fine-tuning process...")
    fine_tune_with_ollama(base_model, str(training_data_path), model_name)

if __name__ == "__main__":
    main()

