#!/usr/bin/env python3
"""Download and quantize the paraphrase-multilingual-MiniLM-L12-v2 model.

This script downloads the model from HuggingFace, quantizes it to FP16
safetensors format, and places the result in core/models/ for embedding
into the Rust binary via include_bytes!.

Usage:
    python scripts/download_model.py

Requirements:
    pip install torch transformers safetensors
"""

import os
import sys
from pathlib import Path

MODEL_NAME = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2"
OUTPUT_DIR = Path(__file__).resolve().parent.parent / "core" / "models"


def main():
    try:
        import torch
        from transformers import AutoModel, AutoTokenizer
        from safetensors.torch import save_file
    except ImportError:
        print(
            "Missing dependencies. Install with:\n"
            "  pip install torch transformers safetensors",
            file=sys.stderr,
        )
        sys.exit(1)

    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)

    print(f"Downloading {MODEL_NAME} ...")
    model = AutoModel.from_pretrained(MODEL_NAME)
    tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME)

    # Convert to FP16 to halve size (~225 MB → ~112 MB)
    print("Quantizing to FP16 ...")
    state_dict = {k: v.half() for k, v in model.state_dict().items()}

    model_path = OUTPUT_DIR / "model.safetensors"
    tokenizer_path = OUTPUT_DIR / "tokenizer.json"

    print(f"Saving model to {model_path} ...")
    save_file(state_dict, str(model_path))

    print(f"Saving tokenizer to {tokenizer_path} ...")
    tokenizer.save_pretrained(str(OUTPUT_DIR))
    # The save_pretrained call writes multiple files; we only need tokenizer.json.
    # Clean up extras.
    for f in OUTPUT_DIR.iterdir():
        if f.name not in ("model.safetensors", "tokenizer.json"):
            f.unlink()
            print(f"  Removed {f.name}")

    model_size = model_path.stat().st_size / (1024 * 1024)
    tokenizer_size = tokenizer_path.stat().st_size / 1024
    print(
        f"\nDone! Model: {model_size:.1f} MB, Tokenizer: {tokenizer_size:.1f} KB"
    )
    print(f"Output directory: {OUTPUT_DIR}")


if __name__ == "__main__":
    main()
