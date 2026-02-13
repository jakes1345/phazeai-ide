#!/usr/bin/env python3
"""
Post-install patch for torchao to fix torch.int1 compatibility issue.
This patches the torchao source code after installation.
"""

import sys
import os
from pathlib import Path

def patch_torchao_source():
    """Patch torchao's quant_primitives.py to handle missing torch.int1."""
    import site
    import importlib.util
    
    # Find torchao installation
    try:
        import torchao
        torchao_path = Path(torchao.__file__).parent
    except ImportError:
        # Try to find it in site-packages
        for site_pkg in site.getsitepackages():
            torchao_path = Path(site_pkg) / "torchao"
            if torchao_path.exists():
                break
        else:
            print("⚠ torchao not installed, skipping patch")
            return True
    
    # Find the problematic file (from error: quant_primitives.py line 175)
    files_to_patch = [
        torchao_path / "quantization" / "quant_primitives.py",
        torchao_path / "dtypes" / "affine_quantized_tensor.py",
    ]
    
    patched_any = False
    for file_path in files_to_patch:
        if file_path.exists():
            print(f"Patching: {file_path}")
            
            # Read the file
            with open(file_path, 'r') as f:
                content = f.read()
            
            # Check if it needs patching
            if 'torch.int1' in content:
                # Replace torch.int1 with torch.int8 as fallback
                # Be careful to only replace the problematic usage
                original = content
                patched = content.replace('torch.int1:', 'torch.int8:')  # Dictionary key
                patched = patched.replace('torch.int1,', 'torch.int8,')  # Tuple element
                patched = patched.replace('torch.int1)', 'torch.int8)')   # Function arg
                patched = patched.replace('torch.int1 ', 'torch.int8 ')  # Standalone
                
                if patched != original:
                    # Write back
                    with open(file_path, 'w') as f:
                        f.write(patched)
                    
                    print(f"✓ Patched {file_path.name}")
                    patched_any = True
                else:
                    print(f"  (No changes needed in {file_path.name})")
    
    if patched_any:
        print("✓ torchao source code patched successfully")
        return True
    else:
        print("✓ torchao doesn't need patching (or not found)")
        return True

if __name__ == "__main__":
    success = patch_torchao_source()
    sys.exit(0 if success else 1)
