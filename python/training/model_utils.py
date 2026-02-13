#!/usr/bin/env python3
"""
Utility scripts for model management and conversion.
"""

import json
import subprocess
from pathlib import Path
import sys

def check_ollama_models():
    """Check available Ollama models."""
    try:
        result = subprocess.run(['ollama', 'list'], capture_output=True, text=True)
        print("Available Ollama models:")
        print(result.stdout)
    except Exception as e:
        print(f"Error: {e}")

def create_modelfile(model_name: str, base_model_path: str, output_path: str):
    """Create Ollama Modelfile."""
    modelfile_content = f"""FROM {base_model_path}

# PhazeAI Fine-tuned Model
# Trained on custom codebase patterns

PARAMETER temperature 0.7
PARAMETER top_p 0.9
PARAMETER top_k 40
PARAMETER num_ctx 4096
PARAMETER repeat_penalty 1.1

# System prompt
SYSTEM \"\"\"
You are PhazeAI, an expert AI coding assistant fine-tuned on the PhazeAI ecosystem.
You understand deep coding patterns, architecture, and conventions.
Generate code that follows established patterns and best practices.
\"\"\"
"""
    
    output_path_obj = Path(output_path)
    output_path_obj.parent.mkdir(parents=True, exist_ok=True)
    
    with open(output_path_obj, 'w') as f:
        f.write(modelfile_content)
    
    print(f"Modelfile created at {output_path}")

def convert_to_gguf(model_dir: str, output_file: str):
    """Convert model to GGUF format (requires llama.cpp)."""
    print("Converting to GGUF format...")
    print("Note: This requires llama.cpp to be installed")
    print(f"Model directory: {model_dir}")
    print(f"Output file: {output_file}")
    print("\nTo convert manually:")
    print(f"  cd llama.cpp")
    print(f"  python convert.py {model_dir} --outfile {output_file}")

def main():
    if len(sys.argv) < 2:
        print("Usage:")
        print("  python model_utils.py check          - Check Ollama models")
        print("  python model_utils.py modelfile      - Create Modelfile")
        print("  python model_utils.py convert        - Convert to GGUF")
        return
    
    command = sys.argv[1]
    
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    model_name = config["ollama"]["fine_tuned_model"]
    
    if command == "check":
        check_ollama_models()
    
    elif command == "modelfile":
        merged_dir = Path(__file__).parent.parent / "models" / "merged"
        modelfile_path = Path(__file__).parent.parent / "models" / model_name / "Modelfile"
        
        if merged_dir.exists():
            create_modelfile(model_name, str(merged_dir), str(modelfile_path))
            print(f"\nCreate Ollama model with:")
            print(f"  ollama create {model_name} -f {modelfile_path}")
        else:
            print(f"Merged model not found at {merged_dir}")
            print("Run fine-tuning first!")
    
    elif command == "convert":
        merged_dir = Path(__file__).parent.parent / "models" / "merged"
        output_file = Path(__file__).parent.parent / "models" / f"{model_name}.gguf"
        convert_to_gguf(str(merged_dir), str(output_file))
    
    else:
        print(f"Unknown command: {command}")

if __name__ == "__main__":
    main()

