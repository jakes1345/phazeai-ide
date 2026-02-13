#!/usr/bin/env python3
"""
PhazeAI Sidecar Server - JSON-RPC over stdio.

The Rust CLI/IDE spawns this as a child process and communicates
via JSON-RPC 2.0 messages over stdin/stdout (one JSON object per line).

Methods:
  - ping: Health check
  - search: Semantic search over codebase embeddings
  - build_index: Build embedding index from project paths
  - analyze: Analyze a source file
"""

import json
import sys
import traceback
from typing import Any, Dict, Optional

# Lazy imports for optional dependencies
embedding_system = None
analyzer = None


def get_embedding_system():
    global embedding_system
    if embedding_system is None:
        try:
            from embeddings import CodebaseEmbeddingSystem
            embedding_system = CodebaseEmbeddingSystem()
        except ImportError:
            return None
    return embedding_system


def get_analyzer():
    global analyzer
    if analyzer is None:
        try:
            from analyzer import AdvancedCodeAnalyzer
            analyzer = AdvancedCodeAnalyzer()
        except ImportError:
            return None
    return analyzer


def handle_ping(params: Optional[Dict]) -> Any:
    return {"status": "ok"}


def handle_search(params: Dict) -> Any:
    es = get_embedding_system()
    if es is None:
        return {"error": "Embedding system not available", "results": []}

    query = params.get("query", "")
    top_k = params.get("top_k", 5)
    results = es.search(query, top_k=top_k)
    return {"results": results}


def handle_build_index(params: Dict) -> Any:
    es = get_embedding_system()
    if es is None:
        return {"error": "Embedding system not available"}

    paths = params.get("paths", [])
    output_dir = params.get("output_dir", ".phazeai/index")
    es.build_index(paths, output_dir)
    return {"status": "ok", "paths_indexed": len(paths)}


def handle_analyze(params: Dict) -> Any:
    az = get_analyzer()
    if az is None:
        return {"error": "Analyzer not available"}

    path = params.get("path", "")
    content = params.get("content", "")
    result = az.analyze_file(path, content)
    return result


METHODS = {
    "ping": handle_ping,
    "search": handle_search,
    "build_index": handle_build_index,
    "analyze": handle_analyze,
}


def process_request(request: Dict) -> Dict:
    """Process a JSON-RPC 2.0 request and return a response."""
    req_id = request.get("id", 0)
    method = request.get("method", "")
    params = request.get("params")

    if method not in METHODS:
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {
                "code": -32601,
                "message": f"Method not found: {method}",
            },
        }

    try:
        result = METHODS[method](params)
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "result": result,
        }
    except Exception as e:
        return {
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {
                "code": -32000,
                "message": str(e),
                "data": traceback.format_exc(),
            },
        }


def main():
    """Main loop: read JSON-RPC requests from stdin, write responses to stdout."""
    # Ensure stderr goes to a log file, not stdout
    sys.stderr = open("/tmp/phazeai_sidecar.log", "a")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
        except json.JSONDecodeError as e:
            response = {
                "jsonrpc": "2.0",
                "id": 0,
                "error": {
                    "code": -32700,
                    "message": f"Parse error: {e}",
                },
            }
            sys.stdout.write(json.dumps(response) + "\n")
            sys.stdout.flush()
            continue

        response = process_request(request)
        sys.stdout.write(json.dumps(response) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
