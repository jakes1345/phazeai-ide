#!/usr/bin/env python3
"""
SOTA Fine-Tuning Script using Unsloth & Qwen 2.5 Coder.
This is a production-grade training script optimized for consumer GPUs (8GB+ VRAM).
"""

import os
import json
import shutil
import torch
from pathlib import Path
from typing import List, Dict
import sys

# Fix for paths with spaces - Triton/ptxas can't handle them
# The issue: Triton's bundled ptxas is in a path with spaces, causing shell errors
# Solution: Copy ptxas to /tmp (no spaces) and point Triton to it

# Try to find system ptxas first
system_ptxas = shutil.which('ptxas')
if system_ptxas:
    os.environ['TRITON_PTXAS_PATH'] = system_ptxas
    print(f"✓ Using system ptxas: {system_ptxas}")
else:
    # If no system ptxas, copy Triton's ptxas to /tmp (no spaces in path)
    # This fixes the "path with spaces" issue
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    
    # Find triton ptxas in venv
    venv_lib = project_root / ".venv" / "lib"
    python_dirs = list(venv_lib.glob("python*"))
    
    if python_dirs:
        python_dir = python_dirs[0]  # Use first python version found
        triton_ptxas = python_dir / "site-packages" / "triton" / "backends" / "nvidia" / "bin" / "ptxas"
        
        if triton_ptxas.exists():
            # Copy to /tmp to avoid space issues
            tmp_ptxas = Path("/tmp/ptxas_triton")
            try:
                if not tmp_ptxas.exists() or tmp_ptxas.stat().st_mtime < triton_ptxas.stat().st_mtime:
                    shutil.copy2(triton_ptxas, tmp_ptxas)
                    tmp_ptxas.chmod(0o755)
                os.environ['TRITON_PTXAS_PATH'] = str(tmp_ptxas)
                print(f"✓ Fixed ptxas path (copied to /tmp to avoid space issues)")
            except Exception as e:
                print(f"⚠ Could not copy ptxas: {e}")
                print("  Training may fail if path has spaces. Consider moving project to path without spaces.")

# Ensure unsloth is available
try:
    from unsloth import FastLanguageModel
    from unsloth.chat_templates import get_chat_template
    from unsloth import is_bfloat16_supported
except ImportError:
    print("CRITICAL: Unsloth is not installed.")
    print("Please install it: pip install \"unsloth[colab-new] @ git+https://github.com/unslothai/unsloth.git\"")
    print("And: pip install --no-deps \"xformers<0.0.27\" \"trl<0.9.0\" peft accelerate bitsandbytes")
    sys.exit(1)

from datasets import Dataset
from trl import SFTTrainer
from transformers import TrainingArguments
import shutil

def load_and_mix_datasets(data_dir: Path, max_local_examples: int = 5000) -> List[Dict]:
    """
    Loads real training data from multiple sources and mixes them.
    Prioritizes synthetic data (high quality) and local code analysis.
    """
    mixed_data = []
    
    # 1. Load Real Codebase Data (from advanced_collect.py)
    # We use the extracted functions, classes, and patterns from your actual code
    # Focus on advanced_training_*.jsonl files (properly formatted)
    raw_files = sorted(
        list(data_dir.glob("advanced_training_*.jsonl")), 
        key=lambda p: p.stat().st_mtime, 
        reverse=True
    )
    
    # Fallback to any jsonl if no advanced_training files
    if not raw_files:
        raw_files = sorted(
            list(data_dir.glob("*.jsonl")), 
            key=lambda p: p.stat().st_mtime, 
            reverse=True
        )
    
    # Only use the 20 most recent files to prevent memory issues
    max_files = 20
    if len(raw_files) > max_files:
        print(f"Found {len(raw_files)} data files. Using {max_files} most recent...")
        raw_files = raw_files[:max_files]
    
    for file_path in raw_files:
        if len(mixed_data) >= max_local_examples:
            break
            
        # Skip any leftover synthetic files if they exist
        if "synthetic" in file_path.name: 
            print(f"Skipping synthetic file: {file_path.name}")
            continue
        
        # skip massive files just in case
        if file_path.stat().st_size > 50 * 1024 * 1024: # 50MB
             print(f"Skipping massive file (>50MB): {file_path.name}")
             continue

        print(f"Loading codebase data from {file_path.name}...")
        try:
            with open(file_path, 'r') as f:
                file_data_count = 0
                for line in f:
                    if line.strip():
                        try:
                            data = json.loads(line)
                            mixed_data.append({
                                "conversations": [
                                    {"role": "user", "content": data['instruction']},
                                    {"role": "assistant", "content": data['output']}
                                ]
                            })
                            file_data_count += 1
                            if len(mixed_data) >= max_local_examples:
                                break
                        except: continue
                print(f"  + Added {file_data_count} examples")
        except Exception as e:
            print(f"  ⚠ Error reading {file_path.name}: {e}")
            continue

    # 2. Add SOTA Open Source Datasets (Phase 2: High-Quality Instruction Tuning)
    print("\nMixing in SOTA Open Source Datasets (Phase 2: Security + Logic)...")
    try:
        from datasets import load_dataset
        
        # Dataset A: Nick088/evol-instruct-code-80k-v1 (Complex coding logic)
        # Increased to 5000 for Phase 2
        print("Downloading Evol-Instruct-Code (High-IQ Coding Instructions)...")
        try:
            ds_evol = load_dataset("nick088/evol-instruct-code-80k-v1", split="train[:5000]") 
            for item in ds_evol:
                mixed_data.append({
                    "conversations": [
                        {"role": "user", "content": item['instruction']},
                        {"role": "assistant", "content": item['output']}
                    ]
                })
        except Exception as e: print(f"  ⚠ Failed to load Evol-Instruct: {e}")

        # Dataset B: Code Vulnerability (Security)
        # Teaches secure coding practices
        print("Downloading Code Vulnerability DPO (Security Training)...")
        try:
            ds_sec = load_dataset("CyberNative/Code_Vulnerability_Security_DPO", split="train[:3000]")
            for item in ds_sec:
                # DPO format: prompt, chosen, rejected. We use 'chosen' for SFT.
                mixed_data.append({
                    "conversations": [
                        {"role": "user", "content": item['prompt']},
                        {"role": "assistant", "content": item['chosen']}
                    ]
                })
        except Exception as e: print(f"  ⚠ Failed to load Security dataset: {e}")

        # Dataset C: CodeFeedback (Self-Correction)
        # Teaches debugging and fixing errors
        print("Downloading CodeFeedback Filtered (Debugging Training)...")
        try:
            ds_feed = load_dataset("m-a-p/CodeFeedback-Filtered-Instruction", split="train[:3000]")
            for item in ds_feed:
                # Columns: query, answer
                mixed_data.append({
                    "conversations": [
                        {"role": "user", "content": item['query']},
                        {"role": "assistant", "content": item['answer']}
                    ]
                })
        except Exception as e: print(f"  ⚠ Failed to load CodeFeedback: {e}")

    except Exception as e:
        print(f"Warning: Could not load open source datasets: {e}")
        print("Continuing with only local data...")

    print(f"Total training examples loaded: {len(mixed_data)}")
    return mixed_data

def train():
    # Configuration - Optimized for RTX 2060 Super (8GB VRAM)
    max_seq_length = 1024  # Reduced to 1024 for 8GB VRAM (was 4096)
    dtype = None # Auto detection
    load_in_4bit = True # Mandatory for 8GB/12GB cards
    
    # Paths
    script_dir = Path(__file__).parent
    project_root = script_dir.parent
    data_dir = project_root / "data" / "training"
    output_dir = project_root / "models" / "fine_tuned" / "phazeai-qwen"
    
    # Ensure directories exist
    data_dir.mkdir(parents=True, exist_ok=True)
    output_dir.mkdir(parents=True, exist_ok=True)
    
    # Clear old checkpoints sparingly (or just allow resume)
    # Removing the automatic deletion to allow Resuming if training is interrupted
    print(f"\nModel Output Directory: {output_dir}")
    checkpoint_dirs = list(output_dir.glob("checkpoint-*"))
    if checkpoint_dirs:
        print(f"  → Found {len(checkpoint_dirs)} existing checkpoint(s). Resuming enabled.")
    
    # 1. Load Model (Qwen 2.5 Coder - SOTA)
    model_name = "unsloth/Qwen2.5-Coder-7B-Instruct-bnb-4bit"
    print(f"\nDownloading/Loading Model: {model_name}...")
    
    # Clear CUDA cache before loading model
    if torch.cuda.is_available():
        torch.cuda.empty_cache()
    
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name = model_name,
        max_seq_length = max_seq_length,
        dtype = dtype,
        load_in_4bit = load_in_4bit,
    )
    
    # Clear cache again after loading
    if torch.cuda.is_available():
        torch.cuda.empty_cache()

    # 2. Add LoRA Adapters (The "Fine-Tuning" part)
    # Reduced rank from 16 to 4 for RTX 2060 Super (8GB VRAM) - minimum for training
    model = FastLanguageModel.get_peft_model(
        model,
        r = 4,  # Minimum rank for 8GB VRAM (was 16)
        target_modules = ["q_proj", "k_proj", "v_proj", "o_proj",
                          "gate_proj", "up_proj", "down_proj",],
        lora_alpha = 4,  # Reduced to match rank
        lora_dropout = 0,
        bias = "none",
        use_gradient_checkpointing = "unsloth", # Optimized
        random_state = 3407,
        max_seq_length = max_seq_length,
    )

    # 3. Prepare Data
    raw_data = load_and_mix_datasets(data_dir)
    if not raw_data:
        print("ERROR: No training data found.")
        print("Please run: python3 scripts/advanced_collect.py")
        print("This will scan your codebase and create training examples.")
        return

    dataset = Dataset.from_list(raw_data)
    
    # Verify dataset
    print(f"\nDataset loaded: {len(dataset)} examples")
    if len(dataset) == 0:
        print("ERROR: Dataset is empty!")
        return
    
    # Setup ChatML Template
    tokenizer = get_chat_template(
        tokenizer,
        chat_template = "chatml",
        mapping = {"role" : "role", "content" : "content", "user" : "user", "assistant" : "assistant"},
    )

    def formatting_prompts_func(examples):
        convos = examples["conversations"]
        texts = []
        for convo in convos:
            try:
                text = tokenizer.apply_chat_template(convo, tokenize = False, add_generation_prompt = False)
                if text and len(text.strip()) > 10:  # Filter out empty/short texts
                    texts.append(text)
            except Exception as e:
                print(f"Warning: Failed to format conversation: {e}")
                continue
        return { "text" : texts }

    print("Formatting dataset with chat template...")
    dataset = dataset.map(formatting_prompts_func, batched = True, remove_columns=dataset.column_names)
    
    # Filter out empty texts
    dataset = dataset.filter(lambda x: x["text"] is not None and len(x["text"].strip()) > 10)
    print(f"Dataset after formatting: {len(dataset)} examples")
    
    # Calculate expected steps
    num_examples = len(dataset)
    per_device_batch = 2  # Increased from 1 for 8GB VRAM
    grad_accum = 4       # Decreased to keep effective batch size at 8
    effective_batch_size = per_device_batch * grad_accum
    steps_per_epoch = (num_examples + effective_batch_size - 1) // effective_batch_size
    total_steps = steps_per_epoch * 2  # 2 epochs
    print(f"\nDataset: {num_examples} examples")
    print(f"Expected steps per epoch: {steps_per_epoch}")
    print(f"Expected total steps: {total_steps} (2 epochs)")
    
    # Clear CUDA cache before training
    if torch.cuda.is_available():
        torch.cuda.empty_cache()
        print(f"GPU Memory before training: {torch.cuda.memory_allocated() / 1024**3:.2f} GB / {torch.cuda.get_device_properties(0).total_memory / 1024**3:.2f} GB")

    # 4. Train
    print("\nStarting Training (This may take hours)...")
    
    trainer = SFTTrainer(
        model = model,
        tokenizer = tokenizer,
        train_dataset = dataset,
        dataset_text_field = "text",
        max_seq_length = max_seq_length,
        dataset_num_proc = 1,  # Reduced to 1 to save memory
        packing = False,
        args = TrainingArguments(
            per_device_train_batch_size = 2,  # Increased for better speed
            gradient_accumulation_steps = 4,  # Adjusted
            warmup_steps = 5,
            max_steps = -1,  # Use num_train_epochs (0 might cause issues, use -1)
            num_train_epochs = 2,  # Reduced from 3 to 2 for faster training on 8GB GPU
            save_steps = 100,  # Save checkpoint every 100 steps
            save_total_limit = 2,  # Keep only 2 checkpoints
            save_strategy = "steps",  # Save based on steps
            load_best_model_at_end = False,  # Don't load best model (saves memory)
            resume_from_checkpoint = True,  # ENABLED: Pick up where you left off!
            learning_rate = 2e-4,
            fp16 = not is_bfloat16_supported(),
            bf16 = is_bfloat16_supported(),
            logging_steps = 10,  # Increased from 1 to reduce CPU/GPU sync overhead
            optim = "adamw_8bit",
            weight_decay = 0.01,
            lr_scheduler_type = "linear",
            seed = 3407,
            output_dir = str(output_dir),
            report_to = "none", # No wandb
            dataloader_pin_memory = False,  # Disable to save VRAM
            dataloader_num_workers = 0,  # Disable multiprocessing to save VRAM
            max_grad_norm = 1.0,  # Gradient clipping
        ),
    )

    trainer_stats = trainer.train()
    
    print(f"\nTraining Complete! Saving to {output_dir}...")
    
    # 5. Save Model (Merged GGUF for Ollama)
    # We save both LoRA adapters and the GGUF export for immediate use
    
    # Save Adapters
    model.save_pretrained(str(output_dir / "lora"))
    tokenizer.save_pretrained(str(output_dir / "lora"))
    
    print("Exporting to GGUF (q4_k_m) for Ollama...")
    # This automatically handles the conversion
    try:
        model.save_pretrained_gguf(
            str(output_dir / "gguf"), 
            tokenizer, 
            quantization_method = "q4_k_m"
        )
        print(f"✓ GGUF saved. You can now create your Ollama model.")
        print(f"Run: ollama create phazeai -f {output_dir}/gguf/Modelfile")
    except Exception as e:
        print(f"GGUF Export failed (check RAM): {e}")
        print("You can try converting manually later.")

if __name__ == "__main__":
    train()
