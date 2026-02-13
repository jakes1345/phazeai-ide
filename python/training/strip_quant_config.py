#!/usr/bin/env python3
"""Remove quantization_config from a HuggingFace config.json so llama.cpp treats it as a plain fp16 model."""

import json
from pathlib import Path

cfg_path = Path("/media/jack/New Volume/ide/models/fine_tuned/phazeai-qwen/gguf/merged_16bit/config.json")

data = json.loads(cfg_path.read_text())

if "quantization_config" in data:
    print("Found quantization_config; removing it so convert_hf_to_gguf ignores bitsandbytes.")
    data.pop("quantization_config")
    cfg_path.write_text(json.dumps(data, indent=2))
    print("✓ Updated config.json (quantization_config removed)")
else:
    print("No quantization_config key found; nothing to do.")
