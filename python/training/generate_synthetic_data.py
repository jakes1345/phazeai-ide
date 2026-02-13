#!/usr/bin/env python3
"""
Synthetic Data Generator using DeepSeek V3.
This script reads your local code and asks DeepSeek V3 to generate "Instruction/Response" pairs
that teach the model how to reason about your specific codebase.
"""

import os
import json
import time
import requests
from pathlib import Path
from typing import List, Dict
import concurrent.futures

# DeepSeek Configuration
DEEPSEEK_API_KEY = "YOUR_API_KEY"  # Will be loaded from QSettings or env
DEEPSEEK_URL = "https://api.deepseek.com/v1/chat/completions"

def load_settings_api_key():
    """Try to load API key from QSettings config file if available"""
    try:
        config_path = Path.home() / ".config/Phaze/PhazeIDE.conf"
        # This is a simplified check, in reality we might need to parse INI
        # For now, we rely on environment variable or manual input if this fails
        return os.environ.get("DEEPSEEK_API_KEY", "")
    except:
        return ""

def generate_synthetic_data(code_snippet: str, file_path: str) -> List[Dict]:
    """Ask DeepSeek to generate Q&A about this code."""
    
    prompt = f"""
    Analyze the following code from file '{file_path}'.
    Generate 3 high-quality instruction-response pairs for training a coding assistant.
    
    The pairs should cover:
    1. Explaining the code's purpose.
    2. Suggesting a refactor or optimization.
    3. Writing a unit test for this code.
    
    Format the output strictly as a JSON list of objects with 'instruction' and 'output' keys.
    
    Code:
    ```
    {code_snippet[:4000]}  # Truncate to avoid context limit issues
    ```
    """
    
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {load_settings_api_key() or os.environ.get('DEEPSEEK_API_KEY')}"
    }
    
    data = {
        "model": "deepseek-chat",
        "messages": [
            {"role": "system", "content": "You are an expert AI data generator. You create training data for coding models. Output only valid JSON."},
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.7
    }
    
    try:
        response = requests.post(DEEPSEEK_URL, headers=headers, json=data)
        response.raise_for_status()
        result = response.json()
        content = result['choices'][0]['message']['content']
        
        # Clean up markdown code blocks if present
        if "```json" in content:
            content = content.split("```json")[1].split("```")[0]
        elif "```" in content:
            content = content.split("```")[1].split("```")[0]
            
        return json.loads(content)
    except Exception as e:
        print(f"Error generating data for {file_path}: {e}")
        return []

def main():
    print("="*60)
    print("Synthetic Data Generator (Powered by DeepSeek V3)")
    print("="*60)
    
    api_key = load_settings_api_key() or os.environ.get("DEEPSEEK_API_KEY")
    if not api_key:
        print("Error: DEEPSEEK_API_KEY not found. Please export it:")
        print("export DEEPSEEK_API_KEY='your-key-here'")
        return

    # 1. Load existing collected data (to get file contents)
    data_dir = Path(__file__).parent.parent / "data" / "training"
    input_files = list(data_dir.glob("*.jsonl"))
    
    if not input_files:
        print("No training data found. Run scripts/advanced_collect.py first.")
        return

    all_synthetic_data = []
    
    # Process each collected file
    print("Processing code files to generate synthetic instructions...")
    
    with concurrent.futures.ThreadPoolExecutor(max_workers=5) as executor:
        futures = []
        
        for jsonl_file in input_files:
            with open(jsonl_file, 'r') as f:
                for line in f:
                    try:
                        item = json.loads(line)
                        if item.get("metadata", {}).get("type") == "full_file":
                            code = item.get("output", "")
                            path = item.get("input", "unknown_file")
                            if len(code) > 100:  # Only meaningful files
                                futures.append(executor.submit(generate_synthetic_data, code, path))
                    except:
                        continue
        
        # Collect results
        for i, future in enumerate(concurrent.futures.as_completed(futures)):
            result = future.result()
            if result:
                all_synthetic_data.extend(result)
                if i % 10 == 0:
                    print(f"Generated {len(all_synthetic_data)} pairs so far...")

    # Save
    output_path = data_dir / "synthetic_deepseek_data.jsonl"
    with open(output_path, 'w') as f:
        for item in all_synthetic_data:
            f.write(json.dumps(item) + "\n")
            
    print(f"\n✓ Saved {len(all_synthetic_data)} synthetic examples to {output_path}")

if __name__ == "__main__":
    main()
