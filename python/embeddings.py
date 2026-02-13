"""
Advanced embedding system with RAG (Retrieval Augmented Generation) for codebase knowledge.
"""

import json
import numpy as np
from pathlib import Path
from typing import List, Dict, Any, Optional, Tuple
from dataclasses import dataclass
from sentence_transformers import SentenceTransformer
import faiss
import pickle
from .code_analyzer import AdvancedCodeAnalyzer


@dataclass
class CodeEmbedding:
    """Code embedding with metadata."""

    filepath: str
    content: str
    embedding: np.ndarray
    metadata: Dict[str, Any]
    chunk_index: int = 0


class CodebaseEmbeddingSystem:
    """RAG system for codebase knowledge retrieval."""

    def __init__(self, model_name: str = "all-MiniLM-L6-v2"):
        """Initialize with embedding model."""
        self.embedder = SentenceTransformer(model_name)
        self.analyzer = AdvancedCodeAnalyzer()
        self.index = None
        self.embeddings: List[CodeEmbedding] = []
        self.chunk_size = 512  # tokens
        self.chunk_overlap = 50
        self.index_built = False

    def build_index(self, project_paths: List[str], output_dir: str):
        """Build embedding index from codebase."""
        print("Building codebase embedding index...")
        all_chunks = []

        for project_path in project_paths:
            if not Path(project_path).exists():
                continue

            print(f"Processing {project_path}...")
            chunks = self._process_project(project_path)
            all_chunks.extend(chunks)

        if not all_chunks:
            print("No code found to embed!")
            return

        # Generate embeddings
        print(f"Generating embeddings for {len(all_chunks)} chunks...")
        texts = [chunk["text"] for chunk in all_chunks]
        embeddings = self.embedder.encode(texts, show_progress_bar=True, batch_size=32)

        # Store embeddings
        self.embeddings = []
        for i, (chunk, embedding) in enumerate(zip(all_chunks, embeddings)):
            self.embeddings.append(
                CodeEmbedding(
                    filepath=chunk["filepath"],
                    content=chunk["text"],
                    embedding=embedding,
                    metadata=chunk["metadata"],
                    chunk_index=chunk.get("chunk_index", 0),
                )
            )

        # Build FAISS index
        dimension = embeddings.shape[1]
        self.index = faiss.IndexFlatL2(dimension)

        # Convert to float32 for FAISS and add vectors
        embeddings_f32 = embeddings.astype("float32")
        self.index.add(embeddings_f32)

        print(f"Index built with {len(self.embeddings)} embeddings")

        # Save index
        output_path = Path(output_dir)
        output_path.mkdir(parents=True, exist_ok=True)

        faiss.write_index(self.index, str(output_path / "codebase.index"))

        # Save embeddings metadata
        with open(output_path / "embeddings_metadata.pkl", "wb") as f:
            pickle.dump(self.embeddings, f)

        print(f"Index saved to {output_path}")

    def load_index(self, index_dir: str):
        """Load existing index."""
        index_path = Path(index_dir)

        self.index = faiss.read_index(str(index_path / "codebase.index"))

        with open(index_path / "embeddings_metadata.pkl", "rb") as f:
            self.embeddings = pickle.load(f)

        print(f"Loaded index with {len(self.embeddings)} embeddings")

    def search(self, query: str, top_k: int = 5) -> List[Dict[str, Any]]:
        """Search codebase using semantic similarity."""
        if self.index is None or not self.embeddings:
            return []

        # Encode query
        query_embedding = self.embedder.encode([query])
        query_embedding_f32 = query_embedding.astype("float32")

        # Search
        k = min(top_k, len(self.embeddings))
        distances, indices = self.index.search(query_embedding_f32, k)

        results = []
        for idx, distance in zip(indices[0], distances[0]):
            if idx < len(self.embeddings):
                embedding = self.embeddings[idx]
                results.append(
                    {
                        "filepath": embedding.filepath,
                        "content": embedding.content,
                        "metadata": embedding.metadata,
                        "similarity": float(
                            1 / (1 + distance)
                        ),  # Convert distance to similarity
                        "distance": float(distance),
                    }
                )

        return results

    def get_relevant_context(self, query: str, max_chars: int = 2000) -> str:
        """Get relevant code context for a query."""
        results = self.search(query, top_k=10)

        context_parts = []
        total_chars = 0

        for result in results:
            if total_chars >= max_chars:
                break

            part = f"File: {result['filepath']}\n"
            part += f"```\n{result['content']}\n```\n"
            part += f"Metadata: {json.dumps(result['metadata'], indent=2)}\n\n"

            if total_chars + len(part) <= max_chars:
                context_parts.append(part)
                total_chars += len(part)

        return "\n".join(context_parts)

    def _process_project(self, project_path: str) -> List[Dict[str, Any]]:
        """Process a project and extract chunks."""
        chunks = []
        project_path_obj = Path(project_path)

        # Supported extensions
        extensions = [
            ".py",
            ".js",
            ".ts",
            ".jsx",
            ".tsx",
            ".rs",
            ".go",
            ".cpp",
            ".c",
            ".h",
            ".hpp",
            ".cc",
            ".cxx",
            ".java",
            ".kt",
            ".cs",
            ".html",
            ".htm",
            ".css",
            ".xml",
            ".json",
            ".yaml",
            ".yml",
            ".sh",
            ".bash",
            ".sql",
        ]

        for ext in extensions:
            for filepath in project_path_obj.rglob(f"*{ext}"):
                # Skip common directories
                if any(
                    skip in str(filepath)
                    for skip in [
                        "node_modules",
                        ".git",
                        "__pycache__",
                        "target",
                        "build",
                    ]
                ):
                    continue

                try:
                    with open(filepath, "r", encoding="utf-8", errors="ignore") as f:
                        content = f.read()

                    # Analyze file
                    analysis = self.analyzer.analyze_file(str(filepath), content)

                    # Chunk content
                    file_chunks = self._chunk_content(content, str(filepath), analysis)
                    chunks.extend(file_chunks)

                except Exception as e:
                    print(f"Error processing {filepath}: {e}")

        return chunks

    def _chunk_content(
        self, content: str, filepath: str, analysis: Dict
    ) -> List[Dict[str, Any]]:
        """Chunk content intelligently."""
        chunks = []

        # Strategy 1: Chunk by functions/classes
        if analysis.get("functions") or analysis.get("classes"):
            # Extract function/class blocks
            lines = content.split("\n")
            current_chunk = []
            current_chunk_start = 0

            for i, line in enumerate(lines):
                # Check if this is a function/class start
                is_start = any(
                    f"def {f['name']}" in line or f"class {c['name']}" in line
                    for f in analysis.get("functions", [])
                    for c in analysis.get("classes", [])
                )

                if is_start and current_chunk:
                    # Save previous chunk
                    chunk_text = "\n".join(current_chunk)
                    if len(chunk_text.strip()) > 50:  # Minimum chunk size
                        chunks.append(
                            {
                                "text": chunk_text,
                                "filepath": filepath,
                                "metadata": {
                                    "start_line": current_chunk_start,
                                    "end_line": i,
                                    "analysis": {
                                        k: v
                                        for k, v in analysis.items()
                                        if k != "patterns"
                                    },
                                },
                                "chunk_index": len(chunks),
                            }
                        )
                    current_chunk = []
                    current_chunk_start = i

                current_chunk.append(line)

            # Add remaining chunk
            if current_chunk:
                chunk_text = "\n".join(current_chunk)
                if len(chunk_text.strip()) > 50:
                    chunks.append(
                        {
                            "text": chunk_text,
                            "filepath": filepath,
                            "metadata": {
                                "start_line": current_chunk_start,
                                "end_line": len(lines),
                                "analysis": {
                                    k: v for k, v in analysis.items() if k != "patterns"
                                },
                            },
                            "chunk_index": len(chunks),
                        }
                    )

        # Strategy 2: If no functions/classes, use sliding window
        if not chunks:
            words = content.split()
            for i in range(0, len(words), self.chunk_size - self.chunk_overlap):
                chunk_words = words[i : i + self.chunk_size]
                chunk_text = " ".join(chunk_words)

                if len(chunk_text.strip()) > 50:
                    chunks.append(
                        {
                            "text": chunk_text,
                            "filepath": filepath,
                            "metadata": {
                                "start_word": i,
                                "end_word": min(i + self.chunk_size, len(words)),
                                "analysis": {
                                    k: v for k, v in analysis.items() if k != "patterns"
                                },
                            },
                            "chunk_index": len(chunks),
                        }
                    )

        return chunks
