#!/usr/bin/env python3
"""
Advanced fine-tuning with QLoRA optimized for RTX 2060 SUPER (8GB VRAM).
Uses 4-bit quantization, gradient checkpointing, and advanced techniques.
"""

import json
import torch
from pathlib import Path
from typing import List, Dict
import sys

sys.path.insert(0, str(Path(__file__).parent.parent))

try:
    from unsloth import FastLanguageModel
    from unsloth.chat_templates import get_chat_template
    UNSLOTH_AVAILABLE = True
except ImportError:
    UNSLOTH_AVAILABLE = False
    print("⚠ Unsloth not available, falling back to standard PEFT")

from transformers import (
    AutoModelForCausalLM,
    AutoTokenizer,
    TrainingArguments,
    Trainer,
    DataCollatorForLanguageModeling,
    BitsAndBytesConfig
)
from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training, TaskType
from datasets import Dataset, load_dataset
from trl import SFTTrainer
import gc

def load_training_data(data_path: str, max_examples: int = 5000) -> List[Dict]:
    """Load formatted training data, prioritizing recent advanced_training files."""
    examples = []
    data_path_obj = Path(data_path)
    
    # Load JSONL files
    if data_path_obj.is_file():
        files = [data_path_obj]
    else:
        # Prioritize 'advanced_training' files and sort by most recent
        files = sorted(
            list(data_path_obj.glob("advanced_training_*.jsonl")), 
            key=lambda p: p.stat().st_mtime, 
            reverse=True
        )
        
        # Fallback to any jsonl if no advanced_training files
        if not files:
            files = sorted(
                list(data_path_obj.glob("*.jsonl")), 
                key=lambda p: p.stat().st_mtime, 
                reverse=True
            )
    
    print(f"Loading data from {len(files)} files (limiting to {max_examples} examples)...")
    
    for jsonl_file in files:
        if len(examples) >= max_examples:
            break
            
        try:
            with open(jsonl_file, 'r', encoding='utf-8') as f:
                file_examples = []
                for line in f:
                    if line.strip():
                        try:
                            data = json.loads(line)
                            file_examples.append(data)
                        except json.JSONDecodeError:
                            continue
                
                # Add up to the limit
                remaining = max_examples - len(examples)
                examples.extend(file_examples[:remaining])
                print(f"  + Added {mini(len(file_examples), remaining)} examples from {jsonl_file.name}")
        except Exception as e:
            print(f"  ⚠ Error reading {jsonl_file.name}: {e}")
            continue
    
    print(f"Total examples loaded: {len(examples)}")
    return examples

def mini(a, b): return a if a < b else b

def format_prompt_advanced(example: dict) -> str:
    """Format example with advanced prompt engineering."""
    instruction = example.get("instruction", "")
    input_text = example.get("input", "")
    output = example.get("output", "")
    metadata = example.get("metadata", {})
    
    # Build rich context
    context_parts = []
    
    if metadata.get("type") == "function":
        func_meta = metadata.get("function_analysis", {})
        context_parts.append(f"Function: {func_meta.get('name', 'unknown')}")
        context_parts.append(f"Complexity: {func_meta.get('complexity', 1)}")
        if func_meta.get('dependencies'):
            context_parts.append(f"Dependencies: {', '.join(func_meta.get('dependencies', []))}")
    
    elif metadata.get("type") == "class":
        class_meta = metadata.get("class_analysis", {})
        context_parts.append(f"Class: {class_meta.get('name', 'unknown')}")
        if class_meta.get('design_pattern'):
            context_parts.append(f"Pattern: {class_meta.get('design_pattern')}")
    
    context = "\n".join(context_parts) if context_parts else ""
    
    # Build prompt
    prompt = f"""<|system|>
You are an expert AI coding assistant fine-tuned on the PhazeAI ecosystem.
You understand deep coding patterns, architecture, and conventions.
Generate code that follows established patterns and best practices.
<|end|>

<|user|>
{instruction}

{input_text}

{context}
<|end|>

<|assistant|>
{output}
<|end|>
"""
    
    return prompt

def fine_tune_with_unsloth(config: Dict, training_data: List[Dict]):
    """Fine-tune using Unsloth (fastest method)."""
    print("="*60)
    print("Fine-tuning with Unsloth (Optimized)")
    print("="*60)
    
    base_model = config["fine_tuning"]["base_model"]
    model_name = f"unsloth/{base_model}" if "llama" in base_model.lower() else base_model
    
    print(f"Loading model: {model_name}")
    
    # Load model with 4-bit quantization
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name=model_name,
        max_seq_length=config["fine_tuning"]["max_length"],
        dtype=None,  # Auto-detect
        load_in_4bit=True,  # 4-bit quantization for 8GB VRAM
    )
    
    # Add LoRA adapters
    model = FastLanguageModel.get_peft_model(
        model,
        r=16,  # LoRA rank
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj",
                       "gate_proj", "up_proj", "down_proj"],
        lora_alpha=16,
        lora_dropout=0,
        bias="none",
        use_gradient_checkpointing=True,  # Save memory
        random_state=3407,
        use_rslora=False,
        loftq_config=None,
    )
    
    # Format data
    print("Formatting training data...")
    formatted_data = []
    for example in training_data:
        prompt = format_prompt_advanced(example)
        formatted_data.append({"text": prompt})
    
    dataset = Dataset.from_list(formatted_data)
    
    # Training arguments optimized for RTX 2060 SUPER
    trainer = SFTTrainer(
        model=model,
        tokenizer=tokenizer,
        train_dataset=dataset,
        dataset_text_field="text",
        max_seq_length=config["fine_tuning"]["max_length"],
        packing=False,
        args=TrainingArguments(
            per_device_train_batch_size=2,  # Small batch for 8GB VRAM
            gradient_accumulation_steps=4,  # Effective batch size = 8
            warmup_steps=50,
            num_train_epochs=config["fine_tuning"]["epochs"],
            learning_rate=config["fine_tuning"]["learning_rate"],
            fp16=not torch.cuda.is_bf16_supported(),
            bf16=torch.cuda.is_bf16_supported(),
            logging_steps=10,
            optim="adamw_8bit",  # 8-bit optimizer
            weight_decay=0.01,
            lr_scheduler_type="linear",
            seed=3407,
            output_dir=str(Path(__file__).parent.parent / config["fine_tuning"]["output_dir"]),
            save_strategy="epoch",
            save_total_limit=2,
        ),
    )
    
    # Enable gradient checkpointing
    model.enable_input_require_grads()
    
    print("\nStarting training...")
    print(f"Model: {base_model}")
    print(f"Training examples: {len(training_data)}")
    print(f"Batch size: 2 (effective: 8 with gradient accumulation)")
    print(f"Epochs: {config['fine_tuning']['epochs']}")
    print(f"Max length: {config['fine_tuning']['max_length']}")
    print("="*60)
    
    trainer.train()
    
    # Save model
    output_dir = Path(__file__).parent.parent / config["fine_tuning"]["output_dir"]
    model.save_pretrained(str(output_dir))
    tokenizer.save_pretrained(str(output_dir))
    
    print(f"\n✓ Model saved to {output_dir}")
    
    # Merge and save for inference
    print("Merging LoRA adapters...")
    model = FastLanguageModel.merge_and_unload()
    merged_dir = output_dir.parent / "merged"
    merged_dir.mkdir(exist_ok=True)
    model.save_pretrained(str(merged_dir))
    tokenizer.save_pretrained(str(merged_dir))
    
    print(f"✓ Merged model saved to {merged_dir}")
    
    return str(output_dir)

def fine_tune_with_peft(config: Dict, training_data: List[Dict]):
    """Fine-tune using standard PEFT (fallback)."""
    print("="*60)
    print("Fine-tuning with PEFT (Standard)")
    print("="*60)
    
    base_model = config["fine_tuning"]["base_model"]
    hf_model_name = f"meta-llama/{base_model.capitalize()}-8b" if "llama" in base_model.lower() else base_model
    
    print(f"Loading model: {hf_model_name}")
    
    # 4-bit quantization config
    bnb_config = BitsAndBytesConfig(
        load_in_4bit=True,
        bnb_4bit_quant_type="nf4",
        bnb_4bit_compute_dtype=torch.float16,
        bnb_4bit_use_double_quant=True,
    )
    
    # Load model
    model = AutoModelForCausalLM.from_pretrained(
        hf_model_name,
        quantization_config=bnb_config,
        device_map="auto",
        trust_remote_code=True,
    )
    
    tokenizer = AutoTokenizer.from_pretrained(hf_model_name)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
        tokenizer.pad_token_id = tokenizer.eos_token_id
    
    # Prepare for LoRA
    model = prepare_model_for_kbit_training(model)
    
    # LoRA config
    lora_config = LoraConfig(
        r=16,
        lora_alpha=32,
        target_modules=["q_proj", "v_proj", "k_proj", "o_proj",
                       "gate_proj", "up_proj", "down_proj"],
        lora_dropout=0.05,
        bias="none",
        task_type=TaskType.CAUSAL_LM,
    )
    
    model = get_peft_model(model, lora_config)
    
    # Format data
    print("Formatting training data...")
    formatted_data = []
    for example in training_data:
        prompt = format_prompt_advanced(example)
        formatted_data.append({"text": prompt})
    
    dataset = Dataset.from_list(formatted_data)
    
    def tokenize_function(examples):
        return tokenizer(
            examples["text"],
            truncation=True,
            padding="max_length",
            max_length=config["fine_tuning"]["max_length"],
        )
    
    tokenized_dataset = dataset.map(tokenize_function, batched=True, remove_columns=dataset.column_names)
    
    # Training arguments
    training_args = TrainingArguments(
        output_dir=str(Path(__file__).parent.parent / config["fine_tuning"]["output_dir"]),
        num_train_epochs=config["fine_tuning"]["epochs"],
        per_device_train_batch_size=1,  # Very small for 8GB VRAM
        gradient_accumulation_steps=8,  # Effective batch size = 8
        learning_rate=config["fine_tuning"]["learning_rate"],
        logging_steps=10,
        save_steps=100,
        save_total_limit=2,
        fp16=True,
        optim="paged_adamw_8bit",
        warmup_steps=50,
        gradient_checkpointing=True,
    )
    
    # Trainer
    trainer = Trainer(
        model=model,
        args=training_args,
        train_dataset=tokenized_dataset,
        data_collator=DataCollatorForLanguageModeling(tokenizer, mlm=False),
    )
    
    print("\nStarting training...")
    trainer.train()
    
    output_dir = Path(__file__).parent.parent / config["fine_tuning"]["output_dir"]
    model.save_pretrained(str(output_dir))
    tokenizer.save_pretrained(str(output_dir))
    
    print(f"\n✓ Model saved to {output_dir}")
    
    return str(output_dir)

def main():
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    # Load training data
    data_dir = Path(__file__).parent.parent / "data" / "training"
    
    print("Loading training data...")
    training_data = load_training_data(str(data_dir))
    
    if len(training_data) == 0:
        print("ERROR: No training data found!")
        print("Run: python scripts/advanced_collect.py")
        return
    
    print(f"Found {len(training_data)} training examples")
    
    # Choose fine-tuning method
    if UNSLOTH_AVAILABLE:
        print("\nUsing Unsloth (recommended - fastest)")
        output_dir = fine_tune_with_unsloth(config, training_data)
    else:
        print("\nUsing standard PEFT")
        output_dir = fine_tune_with_peft(config, training_data)
    
    print("\n" + "="*60)
    print("Fine-tuning complete!")
    print("="*60)
    print(f"\nModel saved to: {output_dir}")
    print("\nNext steps:")
    print("1. Convert to GGUF format for Ollama")
    print("2. Create Modelfile")
    print("3. Load in Ollama: ollama create phazeai-coder -f models/Modelfile")

if __name__ == "__main__":
    main()

