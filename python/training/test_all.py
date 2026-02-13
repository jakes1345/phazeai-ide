#!/usr/bin/env python3
"""Comprehensive test suite for PhazeAI IDE."""

import sys
from pathlib import Path

def print_header(title: str):
    print(f"\n{'='*60}")
    print(f"{title}")
    print(f"{'='*60}")

def print_success(msg: str):
    print(f"✅ {msg}")

def print_error(msg: str):
    print(f"❌ {msg}")

def test_data_collection():
    print_header("Testing Data Collection")
    try:
        from scripts.advanced_collect import AdvancedCodeCollector
        print_success("Advanced collector imports successfully")
        return True
    except ImportError as e:
        print_error(f"Failed to import collector: {e}")
        return False
    except Exception as e:
        print_error(f"Failed to test data collection: {e}")
        return False

def test_embeddings():
    print_header("Testing Embedding System")
    try:
        from core.embedding_system import CodebaseEmbeddingSystem
        print_success("Embedding system imports successfully")
        system = CodebaseEmbeddingSystem()
        print_success("Embedding system initialized")
        return True
    except ImportError as e:
        print_error(f"Failed to import embedding system: {e}")
        return False
    except Exception as e:
        print_error(f"Failed to test embeddings: {e}")
        return False

def test_training_scripts():
    print_header("Testing Training Scripts")
    scripts_to_test = [
        "sota_fine_tune.py",
        "advanced_fine_tune.py",
        "dpo_align.py",
        "fine_tune_lora.py",
        "export_to_gguf.py",
        "train_pipeline.py",
    ]
    
    all_passed = True
    for script_name in scripts_to_test:
        script_path = Path(__file__).parent.parent / "scripts" / script_name
        if not script_path.exists():
            print_error(f"Script not found: {script_name}")
            all_passed = False
            continue
        
        import py_compile
        try:
            py_compile.compile(str(script_path), doraise=True)
            print_success(f"{script_name}: syntax OK")
        except py_compile.PyCompileError as e:
            print_error(f"{script_name}: syntax error - {e}")
            all_passed = False
        except Exception as e:
            print_error(f"{script_name}: error - {e}")
            all_passed = False
    
    return all_passed

def test_gguf_export():
    print_header("Testing GGUF Export")
    try:
        from unsloth import FastLanguageModel
        print_success("Unsloth available - GGUF export supported")
        return True
    except ImportError:
        print_error("Unsloth not available")
        return False

def test_ollama():
    print_header("Testing Ollama Integration")
    import subprocess
    result = subprocess.run(
        ["ollama", "list"],
        capture_output=True,
        text=True
    )
    
    if result.returncode == 0:
        print_success("Ollama is installed")
        return True
    else:
        print_error("Ollama not found")
        return False

def test_config():
    print_header("Testing Configuration")
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    if not config_path.exists():
        print_error("Config file not found")
        return False
    
    try:
        import json
        with open(config_path, 'r') as f:
            config = json.load(f)
        
        required_fields = ['projects', 'ollama', 'fine_tuning', 'hardware', 'ai_customization']
        missing_fields = [f for f in required_fields if f not in config]
        
        if missing_fields:
            print_error(f"Missing config fields: {', '.join(missing_fields)}")
        else:
            print_success("Config structure is valid")
        
        projects = config.get('projects', {})
        if projects:
            print_success(f"Configured projects: {len(projects)}")
        else:
            print_error("No projects configured")
        
        return True
    except json.JSONDecodeError as e:
        print_error(f"Config JSON is invalid: {e}")
        return False
    except Exception as e:
        print_error(f"Error reading config: {e}")
        return False

def test_vram():
    print_header("Testing GPU & VRAM")
    try:
        import torch
        if torch.cuda.is_available():
            gpu_name = torch.cuda.get_device_name(0)
            total_memory = torch.cuda.get_device_properties(0).total_memory / 1024**3
            print_success(f"GPU: {gpu_name}")
            print_success(f"VRAM: {total_memory:.1f} GB")
            return True
        else:
            print_error("CUDA not available")
            return False
    except Exception as e:
        print_error(f"Error checking GPU: {e}")
        return False

def run_all_tests():
    print(f"\n{'='*60}")
    print("PHASEAI IDE - COMPREHENSIVE TEST SUITE")
    print(f"{'='*60}")
    
    tests = {
        "Data Collection": test_data_collection,
        "Embeddings": test_embeddings,
        "Training Scripts": test_training_scripts,
        "GGUF Export": test_gguf_export,
        "Ollama Integration": test_ollama,
        "Configuration": test_config,
        "GPU & VRAM": test_vram,
    }
    
    results = {}
    for test_name, test_func in tests.items():
        try:
            result = test_func()
            results[test_name] = result
        except Exception as e:
            print_error(f"Test '{test_name}' crashed: {e}")
            results[test_name] = False
    
    print(f"\n{'='*60}")
    print("TEST RESULTS SUMMARY")
    print(f"{'='*60}")
    
    passed = sum(1 for v in results.values() if v)
    total = len(results)
    
    for test_name, result in results.items():
        status = "✅ PASSED" if result else "❌ FAILED"
        print(f"{test_name:<25}: {status}")
    
    print(f"{'='*60}")
    print(f"\nSummary: {passed}/{total} tests passed")
    
    if passed == total:
        print("\n🎉 ALL TESTS PASSED! System is ready.")
        return 0
    else:
        print(f"\n⚠️  {total - passed} test(s) failed. Please review above.")
        return 1

if __name__ == "__main__":
    sys.exit(run_all_tests())
