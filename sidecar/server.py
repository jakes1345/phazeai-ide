#!/usr/bin/env python3
"""
JSON-RPC server for PhazeAI IDE sidecar.
Provides code search, indexing, and analysis over stdio.
"""

import sys
import json
import os
import re
from pathlib import Path
from typing import Dict, List, Any, Optional, Set
from collections import defaultdict
from math import log, sqrt


# Logging to stderr only
def log_error(msg: str) -> None:
    print(f"[ERROR] {msg}", file=sys.stderr, flush=True)


def log_info(msg: str) -> None:
    print(f"[INFO] {msg}", file=sys.stderr, flush=True)


# File extensions to index
SOURCE_EXTENSIONS = {
    '.rs', '.py', '.js', '.ts', '.jsx', '.tsx', '.go', '.java',
    '.c', '.cpp', '.cc', '.cxx', '.h', '.hpp', '.md', '.toml',
    '.json', '.yaml', '.yml', '.txt'
}

# Directories to skip
SKIP_DIRS = {
    '.git', 'node_modules', 'target', '__pycache__', 'dist',
    'build', '.next', '.venv', 'venv', 'vendor'
}


class TfidfIndex:
    """Simple TF-IDF based search index using only stdlib."""

    def __init__(self):
        self.documents: List[Dict[str, Any]] = []
        self.term_doc_freq: Dict[str, int] = defaultdict(int)
        self.doc_term_freq: List[Dict[str, int]] = []
        self.idf_cache: Dict[str, float] = {}

    def tokenize(self, text: str) -> List[str]:
        """Simple tokenization: lowercase, split on non-alphanumeric."""
        text = text.lower()
        tokens = re.findall(r'\b\w+\b', text)
        return tokens

    def compute_term_freq(self, tokens: List[str]) -> Dict[str, int]:
        """Count term frequencies."""
        tf = defaultdict(int)
        for token in tokens:
            tf[token] += 1
        return dict(tf)

    def compute_idf(self, term: str) -> float:
        """Compute inverse document frequency."""
        if term in self.idf_cache:
            return self.idf_cache[term]

        doc_count = len(self.documents)
        if doc_count == 0:
            return 0.0

        term_doc_count = self.term_doc_freq.get(term, 0)
        if term_doc_count == 0:
            idf = 0.0
        else:
            idf = log(doc_count / term_doc_count)

        self.idf_cache[term] = idf
        return idf

    def compute_tfidf(self, tf: Dict[str, int]) -> Dict[str, float]:
        """Compute TF-IDF scores."""
        tfidf = {}
        for term, freq in tf.items():
            tfidf[term] = freq * self.compute_idf(term)
        return tfidf

    def cosine_similarity(self, vec1: Dict[str, float], vec2: Dict[str, float]) -> float:
        """Compute cosine similarity between two sparse vectors."""
        dot_product = sum(vec1.get(term, 0) * vec2.get(term, 0) for term in set(vec1) | set(vec2))

        mag1 = sqrt(sum(v * v for v in vec1.values()))
        mag2 = sqrt(sum(v * v for v in vec2.values()))

        if mag1 == 0 or mag2 == 0:
            return 0.0

        return dot_product / (mag1 * mag2)

    def add_document(self, doc_id: int, text: str, metadata: Dict[str, Any]) -> None:
        """Add a document to the index."""
        tokens = self.tokenize(text)
        tf = self.compute_term_freq(tokens)

        # Update global term document frequency
        for term in tf:
            self.term_doc_freq[term] += 1

        self.documents.append({
            'id': doc_id,
            'text': text,
            'metadata': metadata,
            'tokens': tokens
        })
        self.doc_term_freq.append(tf)

        # Invalidate IDF cache
        self.idf_cache.clear()

    def search(self, query: str, top_k: int = 5) -> List[Dict[str, Any]]:
        """Search the index and return top_k results."""
        if not self.documents:
            return []

        query_tokens = self.tokenize(query)
        query_tf = self.compute_term_freq(query_tokens)
        query_tfidf = self.compute_tfidf(query_tf)

        results = []
        for doc_idx, doc in enumerate(self.documents):
            doc_tfidf = self.compute_tfidf(self.doc_term_freq[doc_idx])
            score = self.cosine_similarity(query_tfidf, doc_tfidf)

            if score > 0:
                results.append({
                    'doc': doc,
                    'score': score
                })

        # Sort by score descending
        results.sort(key=lambda x: x['score'], reverse=True)

        return results[:top_k]


class CodeAnalyzer:
    """Simple code analyzer to extract function/struct/class names."""

    # Patterns for different languages
    PATTERNS = {
        'rust_fn': re.compile(r'\bfn\s+(\w+)\s*[<(]'),
        'rust_struct': re.compile(r'\bstruct\s+(\w+)\s*[<{]'),
        'rust_enum': re.compile(r'\benum\s+(\w+)\s*[<{]'),
        'rust_trait': re.compile(r'\btrait\s+(\w+)\s*[<{]'),
        'rust_impl': re.compile(r'\bimpl\s+(?:<[^>]+>\s+)?(\w+)'),

        'python_def': re.compile(r'\bdef\s+(\w+)\s*\('),
        'python_class': re.compile(r'\bclass\s+(\w+)\s*[(:)]'),

        'js_function': re.compile(r'\bfunction\s+(\w+)\s*\('),
        'js_class': re.compile(r'\bclass\s+(\w+)\s*[{]'),
        'js_const_fn': re.compile(r'\bconst\s+(\w+)\s*=\s*(?:async\s+)?(?:function|\()'),

        'go_func': re.compile(r'\bfunc\s+(?:\([^)]+\)\s+)?(\w+)\s*\('),
        'go_struct': re.compile(r'\btype\s+(\w+)\s+struct\s*{'),
        'go_interface': re.compile(r'\btype\s+(\w+)\s+interface\s*{'),

        'java_method': re.compile(r'\b(?:public|private|protected|static|\s)+\w+\s+(\w+)\s*\('),
        'java_class': re.compile(r'\bclass\s+(\w+)\s*[{<]'),

        'c_function': re.compile(r'\b\w+\s+(\w+)\s*\([^)]*\)\s*{'),
    }

    @staticmethod
    def analyze(content: str, file_path: str = '') -> Dict[str, Any]:
        """Analyze code content and extract symbols."""
        symbols = {
            'functions': [],
            'classes': [],
            'structs': [],
            'traits': [],
            'enums': [],
            'other': []
        }

        ext = Path(file_path).suffix if file_path else ''

        # Apply patterns based on file extension
        for pattern_name, pattern in CodeAnalyzer.PATTERNS.items():
            matches = pattern.findall(content)

            if 'fn' in pattern_name or 'def' in pattern_name or 'func' in pattern_name or 'function' in pattern_name or 'method' in pattern_name:
                symbols['functions'].extend(matches)
            elif 'class' in pattern_name:
                symbols['classes'].extend(matches)
            elif 'struct' in pattern_name:
                symbols['structs'].extend(matches)
            elif 'trait' in pattern_name or 'interface' in pattern_name:
                symbols['traits'].extend(matches)
            elif 'enum' in pattern_name:
                symbols['enums'].extend(matches)
            else:
                symbols['other'].extend(matches)

        # Remove duplicates while preserving order
        for key in symbols:
            seen = set()
            unique = []
            for item in symbols[key]:
                if item not in seen:
                    seen.add(item)
                    unique.append(item)
            symbols[key] = unique

        # Count lines
        lines = content.split('\n')
        line_count = len(lines)

        return {
            'symbols': symbols,
            'line_count': line_count,
            'char_count': len(content)
        }


class CodeIndex:
    """Main code indexing system."""

    def __init__(self):
        self.index = TfidfIndex()
        self.indexed_files: Set[str] = set()
        self.doc_counter = 0

    def should_index_file(self, path: Path) -> bool:
        """Determine if a file should be indexed."""
        if path.suffix not in SOURCE_EXTENSIONS:
            return False

        # Skip if in excluded directory
        for part in path.parts:
            if part in SKIP_DIRS:
                return False

        # Skip hidden files
        if path.name.startswith('.'):
            return False

        return True

    def read_file_safe(self, path: Path) -> Optional[str]:
        """Read file content, return None if binary or error."""
        try:
            with open(path, 'r', encoding='utf-8') as f:
                content = f.read()
                return content
        except (UnicodeDecodeError, PermissionError, OSError):
            return None

    def get_snippet(self, content: str, max_length: int = 200) -> str:
        """Extract a snippet from content."""
        lines = content.split('\n')
        snippet_lines = []
        char_count = 0

        for line in lines:
            stripped = line.strip()
            if stripped and not stripped.startswith(('/', '#', '*')):
                snippet_lines.append(stripped)
                char_count += len(stripped)
                if char_count > max_length:
                    break

        snippet = ' '.join(snippet_lines)
        if len(snippet) > max_length:
            snippet = snippet[:max_length] + '...'

        return snippet or content[:max_length]

    def build_index(self, paths: List[str]) -> Dict[str, Any]:
        """Build index from given paths."""
        indexed_count = 0
        skipped_count = 0
        error_count = 0

        for path_str in paths:
            path = Path(path_str).resolve()

            if not path.exists():
                log_error(f"Path does not exist: {path}")
                error_count += 1
                continue

            if path.is_file():
                files_to_index = [path]
            else:
                files_to_index = []
                try:
                    for root, dirs, files in os.walk(path):
                        # Filter out skip directories
                        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]

                        for file in files:
                            file_path = Path(root) / file
                            if self.should_index_file(file_path):
                                files_to_index.append(file_path)
                except OSError as e:
                    log_error(f"Error walking directory {path}: {e}")
                    error_count += 1
                    continue

            for file_path in files_to_index:
                content = self.read_file_safe(file_path)
                if content is None:
                    skipped_count += 1
                    continue

                # Index the file
                self.doc_counter += 1
                self.index.add_document(
                    doc_id=self.doc_counter,
                    text=content,
                    metadata={
                        'path': str(file_path),
                        'name': file_path.name,
                        'ext': file_path.suffix
                    }
                )
                self.indexed_files.add(str(file_path))
                indexed_count += 1

        log_info(f"Indexed {indexed_count} files, skipped {skipped_count}, errors {error_count}")

        return {
            'indexed': indexed_count,
            'skipped': skipped_count,
            'errors': error_count,
            'total_files': len(self.indexed_files)
        }

    def search(self, query: str, top_k: int = 5) -> List[Dict[str, Any]]:
        """Search the index."""
        results = self.index.search(query, top_k)

        matches = []
        for result in results:
            doc = result['doc']
            score = result['score']

            matches.append({
                'file': doc['metadata']['path'],
                'score': round(score, 4),
                'snippet': self.get_snippet(doc['text'])
            })

        return matches


class JsonRpcServer:
    """JSON-RPC 2.0 server over stdio."""

    def __init__(self):
        self.code_index = CodeIndex()

    def handle_ping(self, params: Optional[Dict]) -> str:
        """Handle ping request."""
        return "pong"

    def handle_build_index(self, params: Dict) -> Dict[str, Any]:
        """Handle build_index request."""
        paths = params.get('paths', [])
        if not paths:
            raise ValueError("Missing 'paths' parameter")

        return self.code_index.build_index(paths)

    def handle_search(self, params: Dict) -> Dict[str, Any]:
        """Handle search request."""
        query = params.get('query')
        if not query:
            raise ValueError("Missing 'query' parameter")

        top_k = params.get('top_k', 5)

        if not self.code_index.indexed_files:
            raise RuntimeError("Index not built")

        matches = self.code_index.search(query, top_k)

        return {'matches': matches}

    def handle_analyze(self, params: Dict) -> Dict[str, Any]:
        """Handle analyze request."""
        content = params.get('content')
        if content is None:
            raise ValueError("Missing 'content' parameter")

        path = params.get('path', '')

        return CodeAnalyzer.analyze(content, path)

    def handle_request(self, request: Dict) -> Dict[str, Any]:
        """Handle a JSON-RPC request."""
        jsonrpc = request.get('jsonrpc')
        if jsonrpc != '2.0':
            return {
                'jsonrpc': '2.0',
                'error': {
                    'code': -32600,
                    'message': 'Invalid Request: jsonrpc must be "2.0"'
                },
                'id': request.get('id')
            }

        method = request.get('method')
        params = request.get('params', {})
        request_id = request.get('id')

        try:
            if method == 'ping':
                result = self.handle_ping(params)
            elif method == 'build_index':
                result = self.handle_build_index(params)
            elif method == 'search':
                result = self.handle_search(params)
            elif method == 'analyze':
                result = self.handle_analyze(params)
            else:
                return {
                    'jsonrpc': '2.0',
                    'error': {
                        'code': -32601,
                        'message': f'Method not found: {method}'
                    },
                    'id': request_id
                }

            return {
                'jsonrpc': '2.0',
                'result': result,
                'id': request_id
            }

        except Exception as e:
            log_error(f"Error handling {method}: {e}")
            return {
                'jsonrpc': '2.0',
                'error': {
                    'code': -1,
                    'message': str(e)
                },
                'id': request_id
            }

    def run(self):
        """Main server loop: read from stdin, write to stdout."""
        log_info("JSON-RPC server started")

        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue

            try:
                request = json.loads(line)
                response = self.handle_request(request)
                print(json.dumps(response), flush=True)

            except json.JSONDecodeError as e:
                log_error(f"Invalid JSON: {e}")
                error_response = {
                    'jsonrpc': '2.0',
                    'error': {
                        'code': -32700,
                        'message': 'Parse error: Invalid JSON'
                    },
                    'id': None
                }
                print(json.dumps(error_response), flush=True)

            except Exception as e:
                log_error(f"Unexpected error: {e}")


def main():
    server = JsonRpcServer()
    try:
        server.run()
    except KeyboardInterrupt:
        log_info("Server stopped by user")
    except Exception as e:
        log_error(f"Fatal error: {e}")
        sys.exit(1)


if __name__ == '__main__':
    main()
