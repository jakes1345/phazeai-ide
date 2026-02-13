#!/usr/bin/env python3
"""
Fine-tune using Transformers + PEFT (LoRA) - Works with Ollama-compatible models.
"""

import json
import torch
from pathlib import Path
from transformers import (
    AutoModelForCausalLM,
    AutoTokenizer,
    TrainingArguments,
    Trainer,
    DataCollatorForLanguageModeling
)
from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
from datasets import Dataset

def load_training_data(data_path: str):
    """Load formatted training data."""
    examples = []
    with open(data_path, 'r', encoding='utf-8') as f:
        for line in f:
            if line.strip():
                examples.append(json.loads(line))
    return examples

def format_prompt(example: dict) -> str:
    """Format example as prompt for training."""
    instruction = example.get("instruction", "")
    input_text = example.get("input", "")
    output = example.get("output", "")
    
    prompt = f"""### Instruction:
{instruction}

### Input:
{input_text}

### Response:
{output}

"""
    return prompt

def tokenize_function(examples, tokenizer, max_length=2048):
    """Tokenize examples for training."""
    prompts = [format_prompt(ex) for ex in examples]
    tokenized = tokenizer(
        prompts,
        truncation=True,
        padding="max_length",
        max_length=max_length,
        return_tensors="pt"
    )
    tokenized["labels"] = tokenized["input_ids"].clone()
    return tokenized

def main():
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    # Paths
    training_data_path = Path(__file__).parent.parent / "data" / "formatted_training.jsonl"
    output_dir = Path(__file__).parent.parent / config["fine_tuning"]["output_dir"]
    output_dir.mkdir(parents=True, exist_ok=True)
    
    # Model config
    base_model = config["fine_tuning"]["base_model"]
    # Use a HuggingFace equivalent (e.g., meta-llama/Llama-3-8b)
    hf_model_name = f"meta-llama/{base_model.capitalize()}-8b" if "llama" in base_model.lower() else base_model
    
    print(f"Loading base model: {hf_model_name}")
    print("Note: You may need to adjust the model name based on what's available on HuggingFace")
    
    # Load tokenizer and model
    tokenizer = AutoTokenizer.from_pretrained(hf_model_name)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    
    model = AutoModelForCausalLM.from_pretrained(
        hf_model_name,
        torch_dtype=torch.float16,
        device_map="auto",
        load_in_8bit=True
    )
    
    # Prepare for LoRA
    model = prepare_model_for_kbit_training(model)
    
    lora_config = LoraConfig(
        r=16,
        lora_alpha=32,
        target_modules=["q_proj", "v_proj", "k_proj", "o_proj"],
        lora_dropout=0.05,
        bias="none",
        task_type="CAUSAL_LM"
    )
    
    model = get_peft_model(model, lora_config)
    
    # Load training data
    print("Loading training data...")
    examples = load_training_data(str(training_data_path))
    
    if len(examples) == 0:
        print("ERROR: No training data found. Run collect_training_data.py first.")
        return
    
    # Create dataset
    dataset = Dataset.from_list(examples)
    tokenized_dataset = dataset.map(
        lambda x: tokenize_function([x], tokenizer)[0],
        remove_columns=dataset.column_names
    )
    
    # Training arguments
    training_args = TrainingArguments(
        output_dir=str(output_dir),
        num_train_epochs=config["fine_tuning"]["epochs"],
        per_device_train_batch_size=config["fine_tuning"]["batch_size"],
        learning_rate=config["fine_tuning"]["learning_rate"],
        logging_steps=10,
        save_steps=100,
        save_total_limit=3,
        fp16=True,
        optim="adamw_torch",
        warmup_steps=100,
    )
    
    # Trainer
    trainer = Trainer(
        model=model,
        args=training_args,
        train_dataset=tokenized_dataset,
        data_collator=DataCollatorForLanguageModeling(tokenizer, mlm=False)
    )
    
    print("Starting training...")
    trainer.train()
    
    print(f"Saving model to {output_dir}")
    model.save_pretrained(str(output_dir))
    tokenizer.save_pretrained(str(output_dir))
    
    print("\nTraining complete!")
    print(f"\nTo use with Ollama:")
    print(f"1. Convert the model to GGUF format")
    print(f"2. Create Modelfile: ollama create {config['ollama']['fine_tuned_model']} -f models/Modelfile")

if __name__ == "__main__":
    main()

