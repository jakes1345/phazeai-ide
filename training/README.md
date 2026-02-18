# PhazeAI Custom Model Training Pipeline

> Train your own coding AI — 100% local, zero cloud dependency.

## Quick Start

```bash
# 1. Install Python dependencies
pip install unsloth transformers datasets trl peft bitsandbytes

# 2. Prepare training data
python training/prepare_data.py ~/my-project1 ~/my-project2

# 3. Fine-tune (needs NVIDIA GPU with 8GB+ VRAM)  
python training/fine_tune.py

# 4. Export to GGUF and register with Ollama
python training/export_gguf.py

# 5. Test your custom model
ollama run phaze-coder-custom "Write a Rust function to merge two sorted vectors"
```

## Architecture

```
Your Code + Public Datasets
        ↓
   prepare_data.py (ChatML JSONL)
        ↓
   fine_tune.py (Unsloth QLoRA 4-bit)
        ↓
   LoRA Adapter Weights
        ↓
   export_gguf.py (GGUF + Ollama registration)
        ↓
   ollama run phaze-coder-custom
```

## Training Data Sources

| Source | Command | Samples |
|---|---|---|
| Your codebase | `python prepare_data.py ~/myproject` | Varies |
| Code Alpaca | Auto-downloaded | 20,000 |
| Python Instruct | Auto-downloaded | 18,000 |

Want more data? Add HuggingFace datasets in `prepare_data.py`:
- `bigcode/the-stack-v2-train-smol-ids` (The Stack v2)
- `nvidia/OpenCodeInstruct` (5M samples)
- `sahil2801/CodeAlpaca-20k` (already included)

## GPU Requirements

| GPU | VRAM | Estimated Training Time |
|---|---|---|
| RTX 3060 | 12GB | ~4-6 hours |
| RTX 4070 Ti | 16GB | ~2-3 hours |
| RTX 4090 | 24GB | ~1-2 hours |
| A100 | 80GB | ~30 minutes |

## Base Models

Edit `BASE_MODEL` in `fine_tune.py`:

| Model | Size | Best For |
|---|---|---|
| `Qwen2.5-Coder-7B` | 7B | **Default** — great balance |
| `Qwen2.5-Coder-14B` | 14B | Best quality (needs 16GB+) |
| `llama-3.2-3b` | 3B | Fast planning model |
| `deepseek-coder-v2-lite` | 16B | Strong at debugging |

## File Structure

```
training/
├── prepare_data.py      # Collect + format training data
├── fine_tune.py          # QLoRA training with Unsloth
├── export_gguf.py        # Convert to GGUF for Ollama
├── README.md             # This file
├── datasets/             # Training data (generated)
│   ├── codebase/         # Your project code pairs
│   ├── code_instruct/    # Public instruction datasets
│   └── combined_train.jsonl  # Everything combined
└── output/               # Training output (generated)
    ├── checkpoints/      # Training checkpoints
    ├── lora_adapter/     # LoRA weights
    └── gguf/             # GGUF model files
```
