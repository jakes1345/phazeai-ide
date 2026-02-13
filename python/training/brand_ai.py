#!/usr/bin/env python3
"""
Google Antigravity Branding System.
Customize AI name, replace big AI references.
"""

import sys
import re
from pathlib import Path

def print_header(title: str):
    print(f"\n{'='*60}")
    print(f"{title}")
    print(f"{'='*60}")

def print_success(msg: str):
    print(f"OK {msg}")

def print_error(msg: str):
    print(f"ERROR {msg}")

def customize_ai(name: str, description: str = None):
    print(f"\n🎨 Branding AI as: {name}")
    print("="*60)
    
    script_dir = Path(__file__).parent
    config_path = script_dir / "config" / "projects.json"
    
    if not config_path.exists():
        print(f"ERROR Config file not found at {config_path}")
        print("   Please ensure config/projects.json exists.")
        return False
    
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    if "ai_customization" not in config:
        config["ai_customization"] = {}
    
    config["ai_customization"]["name"] = name
    if description:
        config["ai_customization"]["description"] = description
    
    # Update model names
    config["ollama"]["fine_tuned_model"] = name.lower().replace(" ", "-")
    config["ollama"]["default_model"] = name.lower().replace(" ", "-") + ":latest"
    
    # Save config
    with open(config_path, 'w') as f:
        json.dump(config, f, indent=2)
    
    print(f"OK Config updated with AI name: {name}")
    print(f"OK Fine-tuned model: {config['ollama']['fine_tuned_model']}")
    print(f"OK Default model: {config['ollama']['default_model']}")
    
    # Update README
    update_readme(name)
    
    # Update all Modelfiles
    update_modelfiles(name)
    
    print("="*60)
    print(f"OK Branding complete! Your AI is now '{name}'")
    print("="*60)
    return True

def update_readme(ai_name: str):
    readme_path = Path(__file__).parent / "README.md"
    
    if not readme_path.exists():
        print("WARNING README.md not found, skipping")
        return
    
    with open(readme_path, 'r') as f:
        readme = f.read()
    
    # Replace all references
    readme = re.sub(r'PhazeAI', ai_name, readme)
    readme = re.sub(r'PhazeEco', f'{ai_name} Eco', readme, flags=re.IGNORECASE)
    readme = re.sub(r'phazeai', ai_name.lower().replace(' ', '-'), readme, flags=re.IGNORECASE)
    
    with open(readme_path, 'w') as f:
        f.write(readme)
    
    print(f"OK README.md updated with {ai_name} branding")
    return True

def update_modelfiles(ai_name: str):
    models_dir = Path(__file__).parent / "models"
    
    if not models_dir.exists():
        print(f"WARNING Models directory not found: {models_dir}")
        return
    
    modelfiles = list(models_dir.rglob("*.Modelfile"))
    if not modelfiles:
        print(f"WARNING No Modelfiles found")
        return
    
    updated_count = 0
    
    for modelfile in modelfiles:
        with open(modelfile, 'r') as f:
            content = f.read()
        
        # Replace PhazeAI
        content = re.sub(r'PhazeAI', ai_name, content)
        content = re.sub(r'PhazeEco', f'{ai_name} Eco', content, flags=re.IGNORECASE)
        content = re.sub(r'phazeai', ai_name.lower().replace(' ', '-'), content, flags=re.IGNORECASE)
        
        with open(modelfile, 'w') as f:
            f.write(content)
        
        updated_count += 1
    
    print(f"OK Updated {updated_count} Modelfile(s) with {ai_name} branding")
    return True

def main():
    import argparse
    
    parser = argparse.ArgumentParser(
        description="Google Antigravity Branding System",
        formatter_class=argparse.RawDescriptionHelpFormatter
    )
    
    parser.add_argument(
        "--name",
        required=True,
        help="AI name (e.g., 'Antigravity', 'Nexus AI', 'Zenith')"
    )
    parser.add_argument(
        "--description",
        help="AI description (optional, for branding purposes)"
    )
    
    args = parser.parse_args()
    
    default_descriptions = {
        "antigravity": "A sovereign AI entity by Google DeepMind team - Advanced Agentic Coding",
        "nexus": "Next-generation AI architecture - Scalable and efficient",
        "zenith": "Peak performance AI - Optimized for maximum speed",
        "omega": "Ultimate AI - Complete and comprehensive knowledge",
    }
    
    description = args.description or default_descriptions.get(args.name.lower(), "AI assistant")
    
    # Run branding
    success = customize_ai(args.name, description)
    
    if not success:
        print("\nFAILED!")
        sys.exit(1)
    else:
        print("\n🎉 SUCCESS!")
        print(f"You can now use your AI: ollama run {args.name.lower().replace(' ', '-')}")
