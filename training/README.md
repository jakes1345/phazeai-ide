# Model training scripts

Python scripts for fine-tuning a coding model with QLoRA (via Unsloth) and loading the result into Ollama as `phaze-coder-custom`. They're separate from the editor: nothing in the Rust workspace runs them or refers to `phaze-coder-custom`, and PhazeAI doesn't need them. The built-in local model in the IDE uses ready-made GGUF files from a fixed catalog instead.

`python/training/` holds a separate, larger set of training experiments with no README. This folder is the shorter pipeline.

## What you need

- An NVIDIA GPU with CUDA. `fine_tune.py` exits if it can't find one. Its own notes say 8 GB of VRAM at minimum and 16 to 24 GB recommended.
- `pip install unsloth transformers datasets trl peft bitsandbytes`
- Ollama, if you want the model registered at the end.
- Network access. The base model and the public datasets come from Hugging Face.

## Steps

```bash
# 1. Collect training data and combine it into training/datasets/combined_train.jsonl
python training/prepare_data.py ~/my-project ~/another-project

# 2. Optional: add tool-calling examples (tool_calling_train.jsonl, merged into combined_train.jsonl)
python training/prepare_tool_data.py

# 3. QLoRA fine-tune; the LoRA adapter goes to training/output/lora_adapter/
python training/fine_tune.py

# 4. Export to GGUF (training/output/gguf/) and run `ollama create phaze-coder-custom`
python training/export_gguf.py --quant q4_k_m    # or q5_k_m, q8_0, f16

ollama run phaze-coder-custom
```

`prepare_data.py` reads code from the directories you pass, plus `~/phazeai_ide`, which is hard-coded in the script. It also downloads up to 20,000 samples each from `sahil2801/CodeAlpaca-20k` and `iamtarun/python_code_instructions_18k_alpaca`. Then it merges every `.jsonl` under `training/datasets/` into `combined_train.jsonl`, removes duplicates and shuffles. `prepare_tool_data.py` appends its examples to that combined file when it exists, and its own output is picked up by the next `prepare_data.py` run, so the order of steps 1 and 2 doesn't matter much.

## Base model

`fine_tune.py` defaults to `unsloth/Qwen2.5-Coder-7B-Instruct-bnb-4bit`. Commented-out alternatives in the same file are the Llama 3.2 3B, Qwen2.5-Coder 14B and DeepSeek-Coder-V2-Lite 4-bit builds. `export_gguf.py` sets `BASE_MODEL` separately, so if you change the model, change it in both files.

Training settings (sequence length 4096, 3 epochs, LoRA rank 32, batch size 2 with 4 accumulation steps) are constants at the top of `fine_tune.py`.

## Output

```
training/
├── datasets/   codebase/, code_instruct/, tool_calling_train.jsonl, combined_train.jsonl
└── output/     checkpoints/, lora_adapter/, gguf/
```

Both folders are created when the scripts run.
