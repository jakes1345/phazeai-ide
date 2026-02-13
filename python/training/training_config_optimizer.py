#!/usr/bin/env python3
"""
Optimized Training Config for RTX 2060 Super (8GB VRAM)
Smart resource management for parallel OS builds
"""

import os
import json
from pathlib import Path

# Hardware constraints
GPU_VRAM_GB = 8
CURRENT_RAM_GB = 16  # Estimate, will be 32GB after upgrade
BATCH_SIZE_LIMIT = 2  # Max for 8GB VRAM with 4-bit quantization
GRAD_ACCUM_STEPS = 4  # Effective batch size = 8

# Training parameters optimized for your setup
TRAINING_CONFIG = {
    "model": {
        "name": "unsloth/Qwen2.5-Coder-7B-Instruct-bnb-4bit",
        "max_seq_length": 1024,  # Reduced from 2048 to save VRAM
        "load_in_4bit": True,
        "dtype": None  # Auto-detect
    },
    
    "lora": {
        "r": 4,  # Minimum rank for 8GB VRAM
        "lora_alpha": 4,
        "lora_dropout": 0,
        "target_modules": [
            "q_proj", "k_proj", "v_proj", "o_proj",
            "gate_proj", "up_proj", "down_proj"
        ],
        "bias": "none",
        "use_gradient_checkpointing": "unsloth"
    },
    
    "training": {
        "per_device_train_batch_size": BATCH_SIZE_LIMIT,
        "gradient_accumulation_steps": GRAD_ACCUM_STEPS,
        "num_train_epochs": 2,  # Quick training
        "learning_rate": 2e-4,
        "warmup_steps": 5,
        "save_steps": 100,
        "save_total_limit": 2,  # Save disk space
        "logging_steps": 10,
        "optim": "adamw_8bit",
        "weight_decay": 0.01,
        "lr_scheduler_type": "linear",
        "max_grad_norm": 1.0,
        "dataloader_num_workers": 0,  # Disable to save RAM
        "dataloader_pin_memory": False,  # Disable to save RAM
        "resume_from_checkpoint": True
    },
    
    "data": {
        "max_local_examples": 5000,  # Your codebase
        "evol_instruct_examples": 5000,  # Complex coding
        "security_examples": 3000,  # Secure coding
        "codefeedback_examples": 3000,  # Debugging
        "total_target": 16000  # Total training examples
    },
    
    "output": {
        "model_name": "phazeai-gamedev",
        "gguf_quantization": "q4_k_m",
        "ollama_modelfile": True
    }
}

def print_resource_estimate():
    """Print estimated resource usage"""
    print("\n📊 RESOURCE ESTIMATES (RTX 2060 Super):")
    print("=" * 50)
    
    # VRAM
    model_vram = 4.7
    optimizer_vram = 1.5
    activations_vram = 1.5
    total_vram = model_vram + optimizer_vram + activations_vram
    
    print(f"\n🎮 VRAM Usage:")
    print(f"  Model (4-bit):     {model_vram:.1f} GB")
    print(f"  Optimizer:         {optimizer_vram:.1f} GB")
    print(f"  Activations:       {activations_vram:.1f} GB")
    print(f"  Total:             {total_vram:.1f} GB / {GPU_VRAM_GB} GB")
    print(f"  Status:            {'✅ FITS' if total_vram <= GPU_VRAM_GB else '❌ TOO BIG'}")
    
    # Training time
    examples = TRAINING_CONFIG["data"]["total_target"]
    batch_size = BATCH_SIZE_LIMIT * GRAD_ACCUM_STEPS
    epochs = TRAINING_CONFIG["training"]["num_train_epochs"]
    steps_per_epoch = examples // batch_size
    total_steps = steps_per_epoch * epochs
    seconds_per_step = 2.5  # Estimate for RTX 2060 Super
    total_hours = (total_steps * seconds_per_step) / 3600
    
    print(f"\n⏱️  Training Time:")
    print(f"  Examples:          {examples:,}")
    print(f"  Batch Size:        {batch_size}")
    print(f"  Steps/Epoch:       {steps_per_epoch}")
    print(f"  Total Steps:       {total_steps}")
    print(f"  Est. Time:         {total_hours:.1f} hours ({total_hours/8:.1f} nights)")
    
    print("\n" + "=" * 50)

if __name__ == "__main__":
    print_resource_estimate()
