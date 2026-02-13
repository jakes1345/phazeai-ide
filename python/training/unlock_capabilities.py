import os
import subprocess
import sys

def run_command(cmd):
    print(f"🚀 Running: {cmd}")
    try:
        subprocess.check_call(cmd, shell=True)
    except subprocess.CalledProcessError:
        print(f"❌ Failed: {cmd}")

def unlock_capabilities():
    print("🔓 UNLOCKING GOOGLE ANTIGRAVITY LOCAL CAPABILITIES 🔓")
    print("-------------------------------------------------------")

    # 1. The Smart Brain (Uncensored, Logic, Coding)
    print("\n🧠 Installing 'Dolphin-Llama3' (Uncensored Logic)...")
    run_command("ollama pull dolphin-llama3")

    # 2. The Vision (Seeing Images/Screenshots)
    print("\n👁️ Installing 'LLaVA' (Vision/Multimodal)...")
    run_command("ollama pull llava-llama3")

    # 3. The Voice (Text-to-Speech)
    print("\n🗣️ Installing 'Coqui TTS' (High-Fi Speech)...")
    run_command(f"{sys.executable} -m pip install TTS")
    
    # 4. Game Making Knowledge (Unreal/Unity)
    # We trigger the Researcher to pre-load this knowledge
    print("\n🎮 Absorbing Game Development Knowledge...")
    run_command(f"{sys.executable} scripts/research_and_learn.py 'Unreal Engine 5 C++ Ultimate Guide'")
    run_command(f"{sys.executable} scripts/research_and_learn.py 'Unity C# Advanced Architecture'")

    print("\n✅ INSTALLATION COMPLETE")
    print("   - Brain: dolphin-llama3")
    print("   - Eyes:  llava-llama3")
    print("   - Voice: TTS Library")
    print("   - Knowledge: Game Dev")
    print("\nRestart the IDE to integrate these new organs.")

if __name__ == "__main__":
    unlock_capabilities()
