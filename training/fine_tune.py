#!/usr/bin/env python3
"""
PhazeAI Fine-Tuning with Unsloth QLoRA
=======================================
Fine-tunes a base coding model on PhazeAI training data using 4-bit QLoRA.

Requirements:
    pip install unsloth transformers datasets trl peft bitsandbytes

GPU Requirements:
    - Minimum: 8GB VRAM (RTX 3060)
    - Recommended: 16-24GB VRAM (RTX 4090, A6000)
    
Output: LoRA adapter weights that can be exported to GGUF for Ollama.
"""

import os
import sys
import json
import torch
from pathlib import Path

# ── Configuration ──────────────────────────────────────────────────────

# Base model to fine-tune
BASE_MODEL = "unsloth/Qwen2.5-Coder-7B-Instruct-bnb-4bit"

# Alternative base models (uncomment to use):
# BASE_MODEL = "unsloth/llama-3.2-3b-instruct-bnb-4bit"      # Fastest, smallest
# BASE_MODEL = "unsloth/Qwen2.5-Coder-14B-Instruct-bnb-4bit" # Best quality but needs 16GB+ VRAM
# BASE_MODEL = "unsloth/deepseek-coder-v2-lite-instruct-bnb-4bit"

# Training parameters
MAX_SEQ_LENGTH = 4096
LEARNING_RATE = 2e-4
NUM_EPOCHS = 3
BATCH_SIZE = 2           # Reduce to 1 if OOM
GRADIENT_ACCUMULATION = 4
WARMUP_STEPS = 50
LORA_RANK = 32           # Higher = more capacity but slower
LORA_ALPHA = 64
LORA_DROPOUT = 0.05

# Paths
DATASET_PATH = Path(__file__).parent / "datasets" / "combined_train.jsonl"
OUTPUT_DIR = Path(__file__).parent / "output"
ADAPTER_DIR = OUTPUT_DIR / "lora_adapter"


def check_gpu():
    """Check GPU availability and VRAM."""
    if not torch.cuda.is_available():
        print("❌ No CUDA GPU detected. QLoRA requires a CUDA-capable GPU.")
        print("   Supported: NVIDIA RTX 3060+ / A100 / L4 / etc.")
        sys.exit(1)
    
    gpu_name = torch.cuda.get_device_name(0)
    vram_gb = torch.cuda.get_device_properties(0).total_mem / (1024**3)
    print(f"🖥️  GPU: {gpu_name} ({vram_gb:.1f} GB VRAM)")
    
    if vram_gb < 6:
        print("⚠️  Low VRAM. Consider reducing MAX_SEQ_LENGTH and BATCH_SIZE.")
    
    return vram_gb


def load_training_data(dataset_path: Path):
    """Load ChatML-formatted JSONL training data."""
    from datasets import Dataset
    
    if not dataset_path.exists():
        print(f"❌ Dataset not found: {dataset_path}")
        print(f"   Run prepare_data.py first!")
        sys.exit(1)
    
    entries = []
    with open(dataset_path) as f:
        for line in f:
            try:
                entry = json.loads(line.strip())
                # Convert to the format Unsloth expects
                if "messages" in entry:
                    text = format_chatml(entry["messages"])
                    entries.append({"text": text})
            except json.JSONDecodeError:
                continue
    
    print(f"📊 Loaded {len(entries)} training samples")
    
    dataset = Dataset.from_list(entries)
    return dataset


def format_chatml(messages):
    """Format messages into ChatML template string."""
    text = ""
    for msg in messages:
        role = msg["role"]
        content = msg["content"]
        text += f"<|im_start|>{role}\n{content}<|im_end|>\n"
    return text


def main():
    print("╔═══════════════════════════════════════════╗")
    print("║   PhazeAI QLoRA Fine-Tuning               ║")
    print("║   Powered by Unsloth                      ║")
    print("╚═══════════════════════════════════════════╝")
    print()
    
    # 1. Check GPU
    vram_gb = check_gpu()
    
    # Adjust batch size for available VRAM
    batch_size = BATCH_SIZE
    if vram_gb < 10:
        batch_size = 1
        print("  📉 Reduced batch size to 1 for low VRAM")
    
    # 2. Load model with Unsloth
    print("\n📦 Loading base model with 4-bit quantization...")
    
    try:
        from unsloth import FastLanguageModel
    except ImportError:
        print("❌ Unsloth not installed. Run:")
        print("   pip install 'unsloth[colab-new] @ git+https://github.com/unslothai/unsloth.git'")
        sys.exit(1)
    
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name=BASE_MODEL,
        max_seq_length=MAX_SEQ_LENGTH,
        dtype=None,  # Auto-detect (float16 for most GPUs)
        load_in_4bit=True,
    )
    
    print(f"  ✅ Loaded {BASE_MODEL}")
    
    # 3. Add LoRA adapters
    print("\n🔧 Adding LoRA adapters...")
    
    model = FastLanguageModel.get_peft_model(
        model,
        r=LORA_RANK,
        lora_alpha=LORA_ALPHA,
        lora_dropout=LORA_DROPOUT,
        target_modules=[
            "q_proj", "k_proj", "v_proj", "o_proj",
            "gate_proj", "up_proj", "down_proj",
        ],
        bias="none",
        use_gradient_checkpointing="unsloth",
        random_state=42,
    )
    
    trainable, total = model.get_nb_trainable_parameters()
    print(f"  📊 Trainable: {trainable:,} / {total:,} parameters ({100*trainable/total:.1f}%)")
    
    # 4. Load training data
    print("\n📂 Loading training data...")
    dataset = load_training_data(DATASET_PATH)
    
    # 5. Train
    print(f"\n🚀 Starting training ({NUM_EPOCHS} epochs, batch={batch_size})...")
    
    from trl import SFTTrainer
    from transformers import TrainingArguments
    
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    
    trainer = SFTTrainer(
        model=model,
        tokenizer=tokenizer,
        train_dataset=dataset,
        dataset_text_field="text",
        max_seq_length=MAX_SEQ_LENGTH,
        dataset_num_proc=2,
        packing=True,  # Pack short sequences together for efficiency
        args=TrainingArguments(
            output_dir=str(OUTPUT_DIR / "checkpoints"),
            per_device_train_batch_size=batch_size,
            gradient_accumulation_steps=GRADIENT_ACCUMULATION,
            warmup_steps=WARMUP_STEPS,
            num_train_epochs=NUM_EPOCHS,
            learning_rate=LEARNING_RATE,
            fp16=not torch.cuda.is_bf16_supported(),
            bf16=torch.cuda.is_bf16_supported(),
            logging_steps=10,
            save_steps=200,
            save_total_limit=3,
            optim="adamw_8bit",
            weight_decay=0.01,
            lr_scheduler_type="cosine",
            seed=42,
            report_to="none",
        ),
    )
    
    # Start training
    stats = trainer.train()
    
    print(f"\n✅ Training complete!")
    print(f"   Loss: {stats.training_loss:.4f}")
    print(f"   Runtime: {stats.metrics['train_runtime']:.0f}s")
    
    # 6. Save LoRA adapter
    print(f"\n💾 Saving LoRA adapter to {ADAPTER_DIR}...")
    ADAPTER_DIR.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(str(ADAPTER_DIR))
    tokenizer.save_pretrained(str(ADAPTER_DIR))
    
    print(f"\n╔═══════════════════════════════════════════╗")
    print(f"║  ✅ Fine-tuning complete!                 ║")
    print(f"║                                           ║")
    print(f"║  Next step: Run export_gguf.py to         ║")
    print(f"║  convert to GGUF for Ollama               ║")
    print(f"╚═══════════════════════════════════════════╝")


if __name__ == "__main__":
    main()
