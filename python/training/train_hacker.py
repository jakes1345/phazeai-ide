#!/usr/bin/env python3
"""
Train PhazeAI-Neo - Cybersecurity & Hacking Expert.
Uses PRIMUS datasets and WhiteRabbitNeo V3 for uncensored security knowledge.
"""

import json
from pathlib import Path
import sys

# Load config
script_dir = Path(__file__).parent
config_path = script_dir / "config" / "projects.json"

if config_path.exists():
    with open(config_path, 'r') as f:
        config = json.load(f)
else:
    print("ERROR: Config not found. Please ensure config/projects.json exists.")
    sys.exit(1)

# Get AI name from config
ai_name = config.get("ai_customization", {}).get("name", "phazeai")
model_name = ai_name.lower().replace(" ", "-")

# Paths
output_dir = script_dir / "models" / f"{model_name}-neo"

print(f"\n{'='*60}")
print(f"Training {ai_name}-Neo - Cybersecurity Expert")
print(f"{'='*60}")
print("This model specializes in:")
print("  - Network infiltration")
print("  - Vulnerability research")
print("  - Binary exploitation")
print("  - Security analysis")
print("  - Penetration testing")
print("  - NO ethical disclaimers - real commands only")
print(f"{'='*60}")

print(f"Output directory: {output_dir}")

print(f"\n{'='*60}")
print("Ready to train! Run:")
print(f"  python3 scripts/train_hacker.py --epochs 2")
print(f"{'='*60}")
print("\nNote: Uses WhiteRabbitNeo V3 base model (already uncensored)")
print("  Fine-tunes with QLoRA r=64 for specialized security knowledge")
print("  Optimized for RTX 2060 SUPER (8GB VRAM)")
