#!/usr/bin/env python3
"""
PhazeAI Training Data Preparation
==================================
Collects and formats coding data for QLoRA fine-tuning.

Sources:
1. OpenCodeInstruct (5M coding Q&A pairs)
2. Code Alpaca (20K instruction pairs)
3. Custom codebase (your own projects)

Output: JSONL files in ChatML format for Unsloth training.
"""

import json
import os
import sys
import glob
import random
from pathlib import Path
from typing import List, Dict, Optional

# ── Configuration ──────────────────────────────────────────────────────

OUTPUT_DIR = Path(__file__).parent / "datasets"
CODEBASE_DIR = OUTPUT_DIR / "codebase"
INSTRUCT_DIR = OUTPUT_DIR / "code_instruct"

# File extensions to collect from codebases
CODE_EXTENSIONS = {
    ".rs", ".py", ".js", ".jsx", ".ts", ".tsx", ".go",
    ".c", ".cpp", ".h", ".hpp", ".java", ".rb", ".sh",
    ".sql", ".html", ".css", ".toml", ".yaml", ".yml",
    ".json", ".md"
}

# Max file size (skip huge generated files)
MAX_FILE_SIZE = 100_000  # 100KB


def create_chatml_entry(
    instruction: str,
    output: str,
    input_text: str = "",
    system: str = "You are PhazeAI, an expert coding assistant."
) -> Dict:
    """Format a single training example in ChatML format."""
    messages = [{"role": "system", "content": system}]
    
    user_content = instruction
    if input_text:
        user_content += f"\n\n{input_text}"
    
    messages.append({"role": "user", "content": user_content})
    messages.append({"role": "assistant", "content": output})
    
    return {"messages": messages}


def collect_codebase_data(project_dirs: List[str], max_files: int = 5000) -> List[Dict]:
    """
    Collect code files from your projects and create training pairs.
    For each file, generates:
    - "Explain this code" → explanation
    - "What does <function> do" → description
    - File content for completion training
    """
    entries = []
    file_count = 0
    
    for project_dir in project_dirs:
        project_path = Path(project_dir)
        if not project_path.exists():
            print(f"  ⚠️  Skipping {project_dir} (not found)")
            continue
        
        project_name = project_path.name
        print(f"  📂 Scanning {project_name}...")
        
        for ext in CODE_EXTENSIONS:
            for filepath in project_path.rglob(f"*{ext}"):
                if file_count >= max_files:
                    break
                
                # Skip common non-useful dirs
                parts = filepath.parts
                skip_dirs = {"node_modules", "target", "dist", "build", 
                           ".git", "__pycache__", "vendor", ".next"}
                if any(d in parts for d in skip_dirs):
                    continue
                
                try:
                    stat = filepath.stat()
                    if stat.st_size > MAX_FILE_SIZE or stat.st_size < 50:
                        continue
                    
                    content = filepath.read_text(encoding="utf-8", errors="ignore")
                    rel_path = filepath.relative_to(project_path)
                    
                    # Training pair 1: "Read and explain this file"
                    entries.append(create_chatml_entry(
                        instruction=f"Read and explain this {ext[1:].upper()} file from the {project_name} project.",
                        input_text=f"File: {rel_path}\n\n```{ext[1:]}\n{content}\n```",
                        output=f"This file `{rel_path}` in the {project_name} project "
                               f"contains {len(content.splitlines())} lines of {ext[1:].upper()} code. "
                               f"It {_summarize_file(content, ext)}."
                    ))
                    
                    # Training pair 2: "Complete this code"  
                    lines = content.splitlines()
                    if len(lines) > 10:
                        split_point = len(lines) // 2
                        prefix = "\n".join(lines[:split_point])
                        suffix = "\n".join(lines[split_point:])
                        entries.append(create_chatml_entry(
                            instruction=f"Complete the following {ext[1:].upper()} code:",
                            input_text=f"```{ext[1:]}\n{prefix}\n```",
                            output=f"```{ext[1:]}\n{suffix}\n```"
                        ))
                    
                    file_count += 1
                    
                except (UnicodeDecodeError, PermissionError, OSError):
                    continue
    
    print(f"  ✅ Collected {len(entries)} training pairs from {file_count} files")
    return entries


def _summarize_file(content: str, ext: str) -> str:
    """Quick heuristic summary of a code file."""
    lines = content.splitlines()
    
    # Count functions/classes
    fn_count = sum(1 for l in lines if any(
        kw in l for kw in ["def ", "fn ", "func ", "function ", "class ", "struct ", "impl "]
    ))
    
    if fn_count > 0:
        return f"defines {fn_count} functions/classes"
    
    return f"implements project functionality ({len(lines)} lines)"


def download_huggingface_dataset(
    dataset_name: str,
    output_path: Path,
    max_samples: int = 20000,
    split: str = "train"
) -> bool:
    """Download a dataset from HuggingFace and convert to ChatML JSONL."""
    try:
        from datasets import load_dataset
    except ImportError:
        print("  ❌ Install datasets: pip install datasets")
        return False
    
    print(f"  ⬇️  Downloading {dataset_name}...")
    
    try:
        ds = load_dataset(dataset_name, split=split, streaming=True)
        entries = []
        
        for i, item in enumerate(ds):
            if i >= max_samples:
                break
            
            # Handle different dataset formats
            if "instruction" in item and "output" in item:
                # Code Alpaca format
                entry = create_chatml_entry(
                    instruction=item["instruction"],
                    input_text=item.get("input", ""),
                    output=item["output"]
                )
            elif "question" in item and "answer" in item:
                entry = create_chatml_entry(
                    instruction=item["question"],
                    output=item["answer"]
                )
            elif "prompt" in item and "completion" in item:
                entry = create_chatml_entry(
                    instruction=item["prompt"],
                    output=item["completion"]
                )
            else:
                continue
            
            entries.append(entry)
        
        output_path.parent.mkdir(parents=True, exist_ok=True)
        with open(output_path, "w") as f:
            for entry in entries:
                f.write(json.dumps(entry) + "\n")
        
        print(f"  ✅ Saved {len(entries)} samples to {output_path}")
        return True
        
    except Exception as e:
        print(f"  ❌ Failed to download {dataset_name}: {e}")
        return False


def prepare_all_data(
    project_dirs: Optional[List[str]] = None,
    download_public: bool = True,
    max_codebase_files: int = 5000,
    max_public_samples: int = 20000
):
    """
    Master data preparation function.
    Combines codebase data + public datasets into training-ready JSONL.
    """
    print("╔═══════════════════════════════════════════╗")
    print("║   PhazeAI Training Data Preparation       ║")
    print("╚═══════════════════════════════════════════╝")
    print()
    
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    CODEBASE_DIR.mkdir(parents=True, exist_ok=True)
    INSTRUCT_DIR.mkdir(parents=True, exist_ok=True)
    
    all_entries = []
    
    # 1. Collect from your codebases
    if project_dirs:
        print("📦 Step 1: Collecting codebase data...")
        codebase_entries = collect_codebase_data(project_dirs, max_codebase_files)
        all_entries.extend(codebase_entries)
        
        # Save separately
        codebase_out = CODEBASE_DIR / "codebase_train.jsonl"
        with open(codebase_out, "w") as f:
            for entry in codebase_entries:
                f.write(json.dumps(entry) + "\n")
        print(f"  💾 Saved to {codebase_out}")
    
    # 2. Download public datasets
    if download_public:
        print("\n📦 Step 2: Downloading public coding datasets...")
        
        public_datasets = [
            ("sahil2801/CodeAlpaca-20k", INSTRUCT_DIR / "code_alpaca.jsonl", max_public_samples),
            ("iamtarun/python_code_instructions_18k_alpaca", INSTRUCT_DIR / "python_instruct.jsonl", max_public_samples),
        ]
        
        for ds_name, out_path, max_n in public_datasets:
            download_huggingface_dataset(ds_name, out_path, max_n)
    
    # 3. Combine everything
    print("\n📦 Step 3: Combining all data...")
    
    # Load all JSONL files from datasets/
    for jsonl_file in OUTPUT_DIR.rglob("*.jsonl"):
        if jsonl_file.name == "combined_train.jsonl":
            continue
        with open(jsonl_file) as f:
            for line in f:
                try:
                    all_entries.append(json.loads(line.strip()))
                except json.JSONDecodeError:
                    continue
    
    # Deduplicate by instruction
    seen = set()
    unique_entries = []
    for entry in all_entries:
        key = entry.get("messages", [{}])[-1].get("content", "")[:200]
        if key not in seen:
            seen.add(key)
            unique_entries.append(entry)
    
    # Shuffle
    random.shuffle(unique_entries)
    
    # Save combined
    combined_path = OUTPUT_DIR / "combined_train.jsonl"
    with open(combined_path, "w") as f:
        for entry in unique_entries:
            f.write(json.dumps(entry) + "\n")
    
    print(f"\n✅ Total unique training samples: {len(unique_entries)}")
    print(f"💾 Combined dataset: {combined_path}")
    print(f"📊 File size: {combined_path.stat().st_size / 1024 / 1024:.1f} MB")


if __name__ == "__main__":
    # Default: collect from PhazeAI project + download public datasets
    projects = [
        os.path.expanduser("~/phazeai_ide"),
    ]
    
    # Add any extra project dirs from command line
    if len(sys.argv) > 1:
        projects.extend(sys.argv[1:])
    
    prepare_all_data(
        project_dirs=projects,
        download_public=True,
        max_codebase_files=5000,
        max_public_samples=20000
    )
