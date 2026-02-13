#!/usr/bin/env python3
"""
Compatibility patch for torchao's torch.int1 issue.
This patches torchao before unsloth tries to import it.
"""

import sys
import os

def patch_torchao():
    """Patch torchao to handle missing torch.int1."""
    try:
        import torch
        
        # Check if torch.int1 exists
        if not hasattr(torch, 'int1'):
            # Create a dummy int1 attribute
            # This is a workaround for torchao compatibility
            torch.int1 = torch.int8  # Use int8 as fallback
            print("✓ Patched torch.int1 compatibility")
    except Exception as e:
        print(f"⚠ Could not patch torch: {e}")

# Apply patch before any imports
patch_torchao()

# Now try to import unsloth
try:
    import unsloth
    print("✓ Unsloth imported successfully")
    sys.exit(0)
except Exception as e:
    print(f"❌ Unsloth import failed: {e}")
    sys.exit(1)
