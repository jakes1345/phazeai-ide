#!/usr/bin/env python3
"""
DPO (Direct Preference Optimization) Script.
This script aligns the fine-tuned model to prefer "better" answers, reducing hallucinations.
"""

import os
import json
import torch
from pathlib import Path
from typing import List, Dict
import sys

try:
    from unsloth import FastLanguageModel, PatchDPOTrainer
    from unsloth import is_bfloat16_supported
    PatchDPOTrainer() # Enable memory optimizations
except ImportError:
    print("Unsloth not found.")
    sys.exit(1)

from datasets import Dataset
from trl import DPOTrainer, DPOConfig

def load_preference_data(data_dir: Path) -> List[Dict]:
    """
    Load DPO data (Chosen vs Rejected).
    Currently looks for 'dpo_pairs.jsonl'.
    """
    data = []
    dpo_path = data_dir / "dpo_pairs.jsonl"
    
    if dpo_path.exists():
        print(f"Loading DPO data from {dpo_path}...")
        with open(dpo_path, 'r') as f:
            for line in f:
                try:
                    item = json.loads(line)
                    # Expected format: {"prompt": "...", "chosen": "...", "rejected": "..."}
                    if "chosen" in item and "rejected" in item:
                        data.append(item)
                except: continue
    
    return data

def train_dpo():
    # Paths
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    data_dir = project_root / "data" / "training"
    
    # Input model (The SFT model we just trained)
    # We load the merged model or the base model + adapters
    # For simplicity in DPO, we often start with the SFT checkpoint
    model_name = "unsloth/Qwen2.5-Coder-7B-Instruct-bnb-4bit" # Base
    adapter_path = project_root / "models" / "fine_tuned" / "phazeai-qwen" / "lora"
    
    output_dir = project_root / "models" / "fine_tuned" / "phazeai-dpo"
    
    print("Loading model for DPO alignment...")
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name = str(adapter_path) if adapter_path.exists() else model_name,
        max_seq_length = 4096,
        load_in_4bit = True,
    )

    # DPO requires LoRA configuration again usually
    model = FastLanguageModel.get_peft_model(
        model,
        r = 16,
        target_modules = ["q_proj", "k_proj", "v_proj", "o_proj",
                          "gate_proj", "up_proj", "down_proj",],
        lora_alpha = 16,
        lora_dropout = 0,
        bias = "none",
        use_gradient_checkpointing = "unsloth",
        random_state = 3407,
    )

    # Load Data
    raw_data = load_preference_data(data_dir)
    if not raw_data:
        print("No DPO data found (dpo_pairs.jsonl). Skipping DPO step.")
        print("To run DPO, you need pairs of (prompt, chosen_answer, rejected_answer).")
        return

    dataset = Dataset.from_list(raw_data)

    print(f"Starting DPO Training on {len(raw_data)} pairs...")
    
    dpo_trainer = DPOTrainer(
        model = model,
        ref_model = None, # Unsloth handles this efficiently
        tokenizer = tokenizer,
        beta = 0.1,
        train_dataset = dataset,
        max_length = 2048,
        max_prompt_length = 1024,
        args = DPOConfig(
            per_device_train_batch_size = 2,
            gradient_accumulation_steps = 4,
            warmup_ratio = 0.1,
            num_train_epochs = 1,
            learning_rate = 5e-6, # Very low LR for DPO
            fp16 = not is_bfloat16_supported(),
            bf16 = is_bfloat16_supported(),
            logging_steps = 1,
            optim = "adamw_8bit",
            output_dir = str(output_dir),
            report_to = "none",
        ),
    )

    dpo_trainer.train()
    
    print(f"DPO Complete! Saving to {output_dir}")
    model.save_pretrained(str(output_dir))
    tokenizer.save_pretrained(str(output_dir))

    # GGUF Export
    try:
        model.save_pretrained_gguf(
            str(output_dir / "gguf"), 
            tokenizer, 
            quantization_method = "q4_k_m"
        )
        print("✓ DPO GGUF saved.")
    except Exception as e:
        print(f"GGUF Export failed: {e}")

if __name__ == "__main__":
    train_dpo()
