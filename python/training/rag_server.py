#!/usr/bin/env python3
import os
import json
import glob
import logging
from typing import List, Dict
from flask import Flask, request, jsonify
from sentence_transformers import SentenceTransformer
import faiss
import numpy as np

# Configuration
DATA_DIR = "../data/training"
MODEL_NAME = 'all-MiniLM-L6-v2' # Lightweight, fast, efficient
PORT = 5002

# Setup Logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

app = Flask(__name__)

class VectorEngine:
    def __init__(self):
        self.model = None
        self.index = None
        self.documents = [] # Metadata for retrieved chunks
        self.is_ready = False

    def load(self):
        logger.info(f"Loading Embedding Model: {MODEL_NAME}...")
        self.model = SentenceTransformer(MODEL_NAME)
        
        # Load Documents
        self.documents = []
        corpus = []
        
        jsonl_files = glob.glob(os.path.join(DATA_DIR, "*.jsonl"))
        logger.info(f"Found {len(jsonl_files)} training files.")
        
        for fpath in jsonl_files:
            try:
                with open(fpath, 'r') as f:
                    for line in f:
                        if not line.strip(): continue
                        data = json.loads(line)
                        # Construct a meaningful chunk
                        # Format: "File: <path>\nCode:\n..."
                        # We use the 'input' or 'prompt' field usually, but let's check structure
                        # Typical collector structure: { "prompt": "...", "response": "..." }
                        # For RAG, we want to index the CODE (response) or the CONTEXT.
                        
                        text_content = ""
                        if 'response' in data:
                            text_content += data['response']
                        if 'prompt' in data:
                            text_content += "\n" + data['prompt']
                            
                        if len(text_content) > 50: # Skip noise
                            self.documents.append(data)
                            corpus.append(text_content)
            except Exception as e:
                logger.error(f"Error reading {fpath}: {e}")

        if not corpus:
            logger.warning("No data found to index.")
            self.is_ready = True
            return

        logger.info(f"Encoding {len(corpus)} documents...")
        embeddings = self.model.encode(corpus, convert_to_numpy=True)
        
        # Initialize FAISS
        dimension = embeddings.shape[1]
        self.index = faiss.IndexFlatL2(dimension)
        self.index.add(embeddings)
        
        logger.info(f"Indexed {self.index.ntotal} vectors.")
        self.is_ready = True

    def search(self, query: str, k: int = 5):
        if not self.is_ready or self.index is None or self.index.ntotal == 0:
            return []
            
        vector = self.model.encode([query], convert_to_numpy=True)
        distances, indices = self.index.search(vector, k)
        
        results = []
        for i, idx in enumerate(indices[0]):
            if idx != -1 and idx < len(self.documents):
                results.append({
                    "score": float(distances[0][i]),
                    "data": self.documents[idx]
                })
        return results

engine = VectorEngine()

@app.route('/refresh', methods=['POST'])
def refresh():
    engine.load()
    return jsonify({"success": True, "count": engine.index.ntotal if engine.index else 0})

@app.route('/search', methods=['POST'])
def search():
    if not engine.is_ready:
        return jsonify({"error": "Engine loading"}), 503
        
    data = request.json
    query = data.get("query", "")
    k = data.get("k", 5)
    
    if not query:
        return jsonify({"error": "No query provided"}), 400
        
    results = engine.search(query, k)
    return jsonify({"results": results})

@app.route('/health', methods=['GET'])
def health():
    return jsonify({"status": "ok", "ready": engine.is_ready})

if __name__ == "__main__":
    # Initial load
    engine.load()
    app.run(host='0.0.0.0', port=PORT)
