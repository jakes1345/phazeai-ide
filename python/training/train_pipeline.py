#!/usr/bin/env python3
"""
AI Training Pipeline for PhazeAI IDE.
Handles the complete workflow: data collection -> processing -> fine-tuning -> deployment.
"""

import os
import sys
import json
import time
import shutil
from pathlib import Path
from datetime import datetime
from typing import Dict, List, Any, Optional
import subprocess

class AITrainingPipeline:
    """Complete pipeline for training PhazeAI on your codebase."""
    
    def __init__(self, config_path: Optional[str] = None):
        self.base_dir = Path(__file__).parent.parent
        
        if config_path is None:
            config_path = self.base_dir / "config" / "projects.json"
        
        with open(config_path, 'r') as f:
            self.config = json.load(f)
        
        self.data_dir = self.base_dir / "data"
        self.models_dir = self.base_dir / "models"
        self.training_dir = self.data_dir / "training"
        self.embeddings_dir = self.data_dir / "embeddings"
        
        # Create directories
        for d in [self.data_dir, self.models_dir, self.training_dir, self.embeddings_dir]:
            d.mkdir(parents=True, exist_ok=True)
    
    def collect_training_data(self, verbose: bool = True) -> Dict[str, Any]:
        """Collect training data from all projects."""
        if verbose:
            print("📚 Step 1: Collecting Training Data")
            print("=" * 60)
        
        sys.path.insert(0, str(self.base_dir))
        from scripts.advanced_collect import scan_existing_projects
        
        scan_existing_projects(self.config, str(self.training_dir))
        
        # Get stats
        training_files = list(self.training_dir.glob("*.jsonl"))
        total_examples = 0
        
        for f in training_files:
            with open(f, 'r') as file:
                total_examples += sum(1 for _ in file)
        
        stats = {
            "files": len(training_files),
            "total_examples": total_examples,
            "output_dir": str(self.training_dir)
        }
        
        if verbose:
            print(f"   ✓ Collected {total_examples} training examples")
            print(f"   ✓ Saved to {self.training_dir}")
        
        return stats
    
    def build_embeddings(self, verbose: bool = True) -> Dict[str, Any]:
        """Build embedding index for RAG."""
        if verbose:
            print("\n🔢 Step 2: Building Embedding Index")
            print("=" * 60)
        
        sys.path.insert(0, str(self.base_dir))
        from core.embedding_system import CodebaseEmbeddingSystem
        
        embedding_system = CodebaseEmbeddingSystem()
        
        # Get valid project paths
        project_paths = []
        for name, info in self.config.get("projects", {}).items():
            path = info.get("path", "")
            if path and Path(path).exists():
                project_paths.append(path)
                if verbose:
                    print(f"   Adding: {name} ({path})")
        
        if not project_paths:
            if verbose:
                print("   ⚠ No valid project paths found!")
            return {"error": "No projects to index"}
        
        # Build index
        embedding_system.build_index(project_paths, str(self.embeddings_dir))
        
        stats = {
            "projects_indexed": len(project_paths),
            "index_path": str(self.embeddings_dir)
        }
        
        if verbose:
            print(f"   ✓ Indexed {len(project_paths)} projects")
        
        return stats
    
    def prepare_dataset(self, verbose: bool = True) -> Dict[str, Any]:
        """Prepare dataset for fine-tuning."""
        if verbose:
            print("\n📦 Step 3: Preparing Dataset")
            print("=" * 60)
        
        # Combine all JSONL files
        combined_file = self.training_dir / "combined_dataset.jsonl"
        
        all_examples = []
        for f in self.training_dir.glob("advanced_training_*.jsonl"):
            with open(f, 'r') as file:
                for line in file:
                    try:
                        example = json.loads(line.strip())
                        all_examples.append(example)
                    except json.JSONDecodeError:
                        continue
        
        # Deduplicate by hashing content
        seen_hashes = set()
        unique_examples = []
        
        for ex in all_examples:
            content_hash = hash(json.dumps(ex, sort_keys=True))
            if content_hash not in seen_hashes:
                seen_hashes.add(content_hash)
                unique_examples.append(ex)
        
        # Write combined dataset
        with open(combined_file, 'w') as f:
            for ex in unique_examples:
                f.write(json.dumps(ex) + '\n')
        
        # Create train/val split
        import random
        random.shuffle(unique_examples)
        
        split_idx = int(len(unique_examples) * 0.9)
        train_examples = unique_examples[:split_idx]
        val_examples = unique_examples[split_idx:]
        
        train_file = self.training_dir / "train.jsonl"
        val_file = self.training_dir / "val.jsonl"
        
        with open(train_file, 'w') as f:
            for ex in train_examples:
                f.write(json.dumps(ex) + '\n')
        
        with open(val_file, 'w') as f:
            for ex in val_examples:
                f.write(json.dumps(ex) + '\n')
        
        stats = {
            "total_examples": len(unique_examples),
            "train_examples": len(train_examples),
            "val_examples": len(val_examples),
            "combined_file": str(combined_file),
            "train_file": str(train_file),
            "val_file": str(val_file)
        }
        
        if verbose:
            print(f"   ✓ Total examples: {len(unique_examples)}")
            print(f"   ✓ Training: {len(train_examples)}")
            print(f"   ✓ Validation: {len(val_examples)}")
        
        return stats
    
    def fine_tune_model(self, verbose: bool = True, use_unsloth: bool = True) -> Dict[str, Any]:
        """Run fine-tuning pipeline."""
        if verbose:
            print("\n🎯 Step 4: Fine-Tuning Model")
            print("=" * 60)
        
        ft_config = self.config.get("fine_tuning", {})
        hw_config = self.config.get("hardware", {})
        
        if verbose:
            print(f"   Base Model: {ft_config.get('base_model', 'mistral')}")
            print(f"   GPU: {hw_config.get('gpu', 'Unknown')}")
            print(f"   VRAM: {hw_config.get('vram_gb', 'Unknown')} GB")
            print(f"   Epochs: {ft_config.get('epochs', 3)}")
            print()
        
        # Choose training script
        if use_unsloth:
            script = self.base_dir / "scripts" / "advanced_fine_tune.py"
        else:
            script = self.base_dir / "scripts" / "fine_tune_lora.py"
        
        if not script.exists():
            return {"error": f"Training script not found: {script}"}
        
        # Run training
        if verbose:
            print("   Running fine-tuning (this may take a while)...")
        
        env = os.environ.copy()
        env["CUDA_VISIBLE_DEVICES"] = "0"  # Use first GPU
        
        try:
            result = subprocess.run(
                [sys.executable, str(script)],
                cwd=str(self.base_dir),
                env=env,
                capture_output=True,
                text=True,
                timeout=7200  # 2 hour timeout
            )
            
            if result.returncode == 0:
                if verbose:
                    print("   ✓ Fine-tuning completed!")
                return {"success": True, "output": result.stdout}
            else:
                if verbose:
                    print(f"   ✗ Fine-tuning failed: {result.stderr}")
                return {"success": False, "error": result.stderr}
                
        except subprocess.TimeoutExpired:
            return {"error": "Fine-tuning timed out (2 hours)"}
        except Exception as e:
            return {"error": str(e)}
    
    def create_ollama_model(self, verbose: bool = True) -> Dict[str, Any]:
        """Create Ollama model from fine-tuned weights."""
        if verbose:
            print("\n🏗️  Step 5: Creating Ollama Model")
            print("=" * 60)
        
        ai_name = self.config.get("ai_customization", {}).get("name", "phazeeco-ai")
        model_name = ai_name.lower().replace(" ", "-")
        
        # Check for fine-tuned model
        ft_output = self.models_dir / "fine_tuned"
        
        if not ft_output.exists():
            if verbose:
                print("   ⚠ No fine-tuned model found, using base model")
            
            # Create Modelfile for base model customization
            modelfile_content = f"""FROM mistral
SYSTEM You are {ai_name}, an expert AI coding assistant fine-tuned on the Phaze Eco ecosystem.
You deeply understand:
- PhazeVPN (secure-vpn) - VPN service
- PhazeOS - Custom operating system
- All components and architecture

You generate precise, production-ready code following established patterns.
You always validate your code for correctness and security.

PARAMETER temperature 0.3
PARAMETER top_p 0.9
PARAMETER top_k 40
"""
        else:
            # Use fine-tuned model
            modelfile_content = f"""FROM {ft_output}/merged

SYSTEM You are {ai_name}, an expert AI coding assistant fine-tuned on YOUR codebase.
You generate precise, production-ready code following YOUR established patterns.

PARAMETER temperature 0.3
PARAMETER top_p 0.9
PARAMETER top_k 40
"""
        
        # Write Modelfile
        modelfile_path = self.models_dir / "Modelfile"
        with open(modelfile_path, 'w') as f:
            f.write(modelfile_content)
        
        if verbose:
            print(f"   Creating model: {model_name}")
        
        # Create Ollama model
        try:
            result = subprocess.run(
                ["ollama", "create", model_name, "-f", str(modelfile_path)],
                capture_output=True,
                text=True,
                timeout=600
            )
            
            if result.returncode == 0:
                if verbose:
                    print(f"   ✓ Model '{model_name}' created!")
                return {"success": True, "model_name": model_name}
            else:
                if verbose:
                    print(f"   ✗ Failed: {result.stderr}")
                return {"success": False, "error": result.stderr}
                
        except Exception as e:
            return {"error": str(e)}
    
    def run_full_pipeline(self, skip_finetune: bool = False):
        """Run the complete training pipeline."""
        ai_name = self.config.get("ai_customization", {}).get("name", "PhazeAI")
        
        print("=" * 60)
        print(f"🚀 {ai_name} Training Pipeline")
        print("=" * 60)
        print()
        print(f"Started at: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
        print()
        
        start_time = time.time()
        
        # Step 1: Collect data
        collect_stats = self.collect_training_data()
        
        # Step 2: Build embeddings
        embed_stats = self.build_embeddings()
        
        # Step 3: Prepare dataset
        dataset_stats = self.prepare_dataset()
        
        # Step 4: Fine-tune (optional)
        if not skip_finetune:
            ft_stats = self.fine_tune_model()
        else:
            print("\n⏭️  Skipping fine-tuning (--skip-finetune)")
            ft_stats = {"skipped": True}
        
        # Step 5: Create Ollama model
        model_stats = self.create_ollama_model()
        
        elapsed = time.time() - start_time
        
        print()
        print("=" * 60)
        print("✅ Pipeline Complete!")
        print("=" * 60)
        print(f"Total time: {elapsed / 60:.1f} minutes")
        print()
        print("Summary:")
        print(f"   • Training examples: {dataset_stats.get('total_examples', 0)}")
        print(f"   • Projects indexed: {embed_stats.get('projects_indexed', 0)}")
        print(f"   • Model created: {model_stats.get('model_name', 'N/A')}")
        print()
        print("You can now use your AI with:")
        print(f"   ollama run {model_stats.get('model_name', 'phazeeco-ai')}")
        print("   python launcher.py")
        print("=" * 60)
        
        return {
            "collection": collect_stats,
            "embeddings": embed_stats,
            "dataset": dataset_stats,
            "fine_tuning": ft_stats,
            "model": model_stats,
            "elapsed_seconds": elapsed
        }


def main():
    import argparse
    
    parser = argparse.ArgumentParser(description="PhazeAI Training Pipeline")
    parser.add_argument("--collect-only", action="store_true", help="Only collect data")
    parser.add_argument("--embed-only", action="store_true", help="Only build embeddings")
    parser.add_argument("--skip-finetune", action="store_true", help="Skip fine-tuning step")
    parser.add_argument("--create-model", action="store_true", help="Only create Ollama model")
    
    args = parser.parse_args()
    
    pipeline = AITrainingPipeline()
    
    if args.collect_only:
        pipeline.collect_training_data()
    elif args.embed_only:
        pipeline.build_embeddings()
    elif args.create_model:
        pipeline.create_ollama_model()
    else:
        pipeline.run_full_pipeline(skip_finetune=args.skip_finetune)


if __name__ == "__main__":
    main()
