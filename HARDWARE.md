# PhazeAI Hardware Reference

This document serves as a reference for a tested and verified hardware configuration used to develop and run PhazeAI IDE, including the local multi-agent system and fine-tuning pipeline.

## Tested Configuration

| Component | specification | Notes |
|---|---|---|
| **CPU** | AMD Ryzen 5 3600 (6-Core) | Solid performance for local orchestration and workspace indexing. |
| **GPU** | NVIDIA GeForce RTX 2060 SUPER (8GB VRAM) | **Critical for Local AI.** 8GB VRAM is the sweet spot for running 7B-14B models at 4-bit and performing QLoRA fine-tuning. |
| **RAM** | 46GB DDR4 | More than enough for many parallel agents and complex IDE operations. |
| **OS** | Linux Mint 22.3 | Provides a stable environment for CUDA and Rust development. |
| **Kernel** | 6.14.0-37-generic | Latest performance optimizations. |

## What You Can Do With This Setup

With an **8GB NVIDIA GPU** like the 2060 Super, you can:

1.  **Run Multi-Agent Pipelines**: Smooth execution of `phaze-coder` (14B), `phaze-planner` (3B), and `phaze-reviewer` (16B) using 4-bit quantization in Ollama.
2.  **Fine-Tune Custom Models**: The `training/fine_tune.py` script is optimized for 8GB+ VRAM. You can train on your own codebase in ~2-6 hours.
3.  **Low Latency**: Expect fast response times for chat and code generation without needing cloud APIs.

## Minimum vs. Recommended

| Requirement | Minimum | Recommended (Our Setup) |
|---|---|---|
| **GPU** | 6GB VRAM (NVIDIA) | 8GB+ VRAM (NVIDIA) |
| **RAM** | 16GB | 32GB+ |
| **CPU** | 4-Core Modern CPU | 6-Core+ |
| **Storage** | 50GB Free (for models) | 100GB+ SSD |

If your specs match or exceed the "Tested Configuration" above, your experience with PhazeAI will be seamless.
