#!/usr/bin/env python3
"""
Advanced training data collection with deep code analysis and semantic understanding.
"""

import os
import json
import hashlib
import re
from pathlib import Path
from typing import List, Dict, Any
from datetime import datetime
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler
import sys

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent))

from core.code_analyzer import AdvancedCodeAnalyzer

# Make embedding system optional
try:
    from core.embedding_system import CodebaseEmbeddingSystem
    EMBEDDING_AVAILABLE = True
except ImportError:
    EMBEDDING_AVAILABLE = False
    CodebaseEmbeddingSystem = None

class AdvancedCodeCollector(FileSystemEventHandler):
    """Advanced code collector with semantic analysis."""
    
    def __init__(self, output_dir: str, projects_config: Dict):
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.projects_config = projects_config
        self.analyzer = AdvancedCodeAnalyzer()
        self.collected_data = []
        self.patterns_db = {}
        
    def on_modified(self, event):
        if event.is_directory:
            return
        
        # Support all languages
        supported_extensions = (
            '.py', '.js', '.ts', '.jsx', '.tsx',
            '.rs', '.go',
            '.cpp', '.c', '.h', '.hpp', '.cc', '.cxx',
            '.java', '.kt',
            '.cs',
            '.html', '.htm', '.css',
            '.xml', '.json', '.yaml', '.yml',
            '.sh', '.bash',
            '.sql'
        )
        
        if event.src_path.endswith(supported_extensions):
            self.process_file(event.src_path)
    
    def process_file(self, filepath: str):
        """Process file with deep analysis."""
        try:
            # Additional security check before processing
            filepath_str = str(filepath).lower()
            filename = Path(filepath).name.lower()
            
            sensitive_keywords = [
                'creds', 'credentials', 'secret', 'password', 'key', 'token', 
                'auth', 'api_key', 'private_key', 'ssh_key', '.env', '.pem', 
                '.key', 'id_rsa', 'master.key'
            ]
            
            # Skip if filename or path contains sensitive keywords
            if any(kw in filename or kw in filepath_str for kw in sensitive_keywords):
                return  # Silently skip (already logged during scan)
            
            with open(filepath, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
            
            # Quick content check for obvious secrets (API keys, tokens, etc.)
            # Skip files that look like they contain secrets
            secret_patterns = [
                r'api[_-]?key\s*[:=]\s*["\']?[a-zA-Z0-9]{20,}',
                r'secret[_-]?key\s*[:=]\s*["\']?[a-zA-Z0-9]{20,}',
                r'password\s*[:=]\s*["\']?[^\s"\']{8,}',
                r'-----BEGIN\s+(RSA\s+)?PRIVATE\s+KEY-----',
                r'-----BEGIN\s+EC\s+PRIVATE\s+KEY-----',
            ]
            
            for pattern in secret_patterns:
                if re.search(pattern, content, re.IGNORECASE):
                    print(f"  🔒 Skipping file with detected secrets: {Path(filepath).name}")
                    return
            
            # Deep analysis
            analysis = self.analyzer.analyze_file(filepath, content)
            
            # Determine project context
            project_context = self.get_project_context(filepath)
            
            # Create rich training examples
            examples = self.create_training_examples(filepath, content, analysis, project_context)
            
            self.collected_data.extend(examples)
            
            # Save incrementally
            if len(self.collected_data) >= 50:
                self.save_data()
                
        except Exception as e:
            print(f"Error processing {filepath}: {e}")
    
    def create_training_examples(self, filepath: str, content: str, analysis: Dict, project: str) -> List[Dict]:
        """Create multiple training examples from a single file."""
        examples = []
        
        # Example 1: Full file context
        examples.append({
            "instruction": f"Generate code for {project} project following established patterns",
            "input": json.dumps({
                "project": project,
                "filepath": filepath,
                "language": analysis.get('language'),
                "patterns": [p.pattern_type for p in analysis.get('patterns', [])],
                "architecture": analysis.get('architecture_patterns', []),
                "complexity": analysis.get('complexity_metrics', {})
            }, indent=2),
            "output": content,
            "metadata": {
                "type": "full_file",
                "analysis": analysis
            }
        })
        
        # Example 2: Function-level examples
        for func in analysis.get('functions', []):
            func_code = self.extract_function_code(content, func.get('name', ''))
            if func_code:
                examples.append({
                    "instruction": f"Write a {func.get('name', 'function')} function for {project} with parameters {func.get('parameters', [])}",
                    "input": json.dumps({
                        "project": project,
                        "function_name": func.get('name'),
                        "parameters": func.get('parameters', []),
                        "return_type": func.get('return_type'),
                        "complexity": func.get('complexity', 1),
                        "dependencies": func.get('dependencies', []),
                        "decorators": func.get('decorators', [])
                    }, indent=2),
                    "output": func_code,
                    "metadata": {
                        "type": "function",
                        "function_analysis": func
                    }
                })
        
        # Example 3: Class-level examples
        for cls in analysis.get('classes', []):
            cls_code = self.extract_class_code(content, cls.get('name', ''))
            if cls_code:
                examples.append({
                    "instruction": f"Create a {cls.get('name', 'class')} class for {project} following {cls.get('design_pattern', 'standard')} pattern",
                    "input": json.dumps({
                        "project": project,
                        "class_name": cls.get('name'),
                        "base_classes": cls.get('base_classes', []),
                        "methods": [m.get('name') for m in cls.get('methods', [])],
                        "attributes": cls.get('attributes', []),
                        "design_pattern": cls.get('design_pattern'),
                        "docstring": cls.get('docstring')
                    }, indent=2),
                    "output": cls_code,
                    "metadata": {
                        "type": "class",
                        "class_analysis": cls
                    }
                })
        
        # Example 4: Pattern-based examples
        for pattern in analysis.get('patterns', []):
            examples.append({
                "instruction": f"Implement {pattern.pattern_type} pattern for {project}",
                "input": json.dumps({
                    "project": project,
                    "pattern_type": pattern.pattern_type,
                    "description": pattern.description,
                    "context": pattern.context
                }, indent=2),
                "output": pattern.code[:1000],  # Limit size
                "metadata": {
                    "type": "pattern",
                    "pattern": pattern.pattern_type
                }
            })
        
        return examples
    
    def extract_function_code(self, content: str, function_name: str) -> str:
        """Extract function code from content."""
        # Simple extraction - can be improved with AST
        lines = content.split('\n')
        in_function = False
        function_lines = []
        indent_level = None
        
        for line in lines:
            if f"def {function_name}" in line or f"function {function_name}" in line:
                in_function = True
                indent_level = len(line) - len(line.lstrip())
                function_lines.append(line)
            elif in_function:
                current_indent = len(line) - len(line.lstrip()) if line.strip() else indent_level
                if line.strip() and current_indent <= indent_level and not line.strip().startswith('#'):
                    break
                function_lines.append(line)
        
        return '\n'.join(function_lines)
    
    def extract_class_code(self, content: str, class_name: str) -> str:
        """Extract class code from content."""
        lines = content.split('\n')
        in_class = False
        class_lines = []
        indent_level = None
        
        for line in lines:
            if f"class {class_name}" in line:
                in_class = True
                indent_level = len(line) - len(line.lstrip())
                class_lines.append(line)
            elif in_class:
                current_indent = len(line) - len(line.lstrip()) if line.strip() else indent_level
                if line.strip() and current_indent <= indent_level and not line.strip().startswith('#'):
                    break
                class_lines.append(line)
        
        return '\n'.join(class_lines)
    
    def get_project_context(self, filepath: str) -> str:
        """Determine which project a file belongs to."""
        filepath_lower = filepath.lower()
        for project_name, project_info in self.projects_config.get("projects", {}).items():
            project_path = project_info.get("path", "")
            if project_path and project_path in filepath:
                return project_name
        return "unknown"
    
    def save_data(self):
        """Save collected data to JSONL format."""
        output_file = self.output_dir / f"advanced_training_{datetime.now().strftime('%Y%m%d_%H%M%S')}.jsonl"
        with open(output_file, 'a', encoding='utf-8') as f:
            for item in self.collected_data:
                f.write(json.dumps(item, default=str) + '\n')
        
        print(f"Saved {len(self.collected_data)} training examples to {output_file}")
        self.collected_data = []

def scan_existing_projects(projects_config: Dict, output_dir: str):
    """Scan existing projects with deep analysis."""
    collector = AdvancedCodeCollector(output_dir, projects_config)
    
    print("Starting advanced code analysis...")
    
    for project_name, project_info in projects_config.get("projects", {}).items():
        project_path = project_info.get("path", "")
        if not project_path or not os.path.exists(project_path):
            print(f"Project path not found: {project_path}")
            continue
        
        print(f"\nAnalyzing {project_name} at {project_path}...")
        
        extensions = [
            '.py', '.js', '.ts', '.jsx', '.tsx',  # Python, JavaScript/TypeScript
            '.rs', '.go',                         # Rust, Go
            '.cpp', '.c', '.h', '.hpp', '.cc', '.cxx',  # C/C++
            '.java', '.kt',                       # Java, Kotlin
            '.cs',                                # C#
            '.html', '.htm', '.css',              # Web
            '.xml', '.json', '.yaml', '.yml',     # Data formats
            '.sh', '.bash', '.zsh',               # Shell scripts
            '.sql',                               # SQL
            '.md'                                 # Documentation
        ]
        file_count = 0
        
        for ext in extensions:
            for filepath in Path(project_path).rglob(f"*{ext}"):
                # Security filter - Comprehensive sensitive file detection
                skip_names = [
                    'node_modules', '.git', '__pycache__', 'target', 'build', 'dist', 
                    'venv', '.env', '.venv', 'env', 'virtualenv', '.pytest_cache',
                    '.mypy_cache', '.tox', '.coverage', 'htmlcov', '.eggs', '*.egg-info'
                ]
                
                # Comprehensive sensitive keywords (filename and path)
                sensitive_keywords = [
                    'creds', 'credentials', 'credential', 'secret', 'secrets', 
                    'password', 'passwords', 'passwd', 'pwd', 'key', 'keys', 
                    'token', 'tokens', 'auth', 'authorization', 'api_key', 
                    'apikey', 'private_key', 'privatekey', 'ssh_key', 'sshkey',
                    'access_token', 'accesstoken', 'bearer', 'oauth', 'oauth2',
                    'jwt', 'session', 'cookie', '.env', 'config', 'config.json',
                    'settings', 'secrets.json', 'secrets.yaml', 'secrets.yml',
                    '.pem', '.key', '.p12', '.pfx', '.jks', '.keystore',
                    'id_rsa', 'id_dsa', 'id_ecdsa', 'id_ed25519', '.pub',
                    'master.key', 'master_key', 'private', 'privkey'
                ]
                
                # Skip large files (> 100KB) to prevent OOM during loading
                if filepath.stat().st_size > 100 * 1024:
                    print(f"  🔒 Skipping large file (>100KB): {filepath.name}")
                    continue

                # Check path parts for skip patterns
                filepath_str = str(filepath).lower()
                
                # Check project-specific exclusions
                project_excludes = project_info.get("exclude", [])
                if any(exclude.lower() in filepath_str for exclude in project_excludes):
                    print(f"  🔒 Skipping excluded path: {filepath.name}")
                    continue

                if any(skip in filepath_str for skip in skip_names):
                    continue
                    
                # Check filename for sensitive keywords
                filename = filepath.name.lower()
                if any(keyword in filename for keyword in sensitive_keywords):
                    print(f"  🔒 Skipping sensitive file: {filepath.name}")
                    continue
                
                # Check path for sensitive keywords (catches files in sensitive directories)
                if any(keyword in filepath_str for keyword in sensitive_keywords):
                    print(f"  🔒 Skipping sensitive file (path): {filepath.name}")
                    continue
                
                # Check for common sensitive file extensions
                sensitive_extensions = ['.pem', '.key', '.p12', '.pfx', '.jks', '.keystore', '.env', '.jsonl']
                if filepath.suffix.lower() in sensitive_extensions:
                    print(f"  🔒 Skipping sensitive/data file (extension): {filepath.name}")
                    continue
                
                file_count += 1
                if file_count % 100 == 0:
                    print(f"  Processed {file_count} files...")
                
                collector.process_file(str(filepath))
        
        print(f"  Completed {project_name}: {file_count} files processed")
    
    collector.save_data()
    print("\n✓ Advanced analysis complete!")

def main():
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    output_dir = Path(__file__).parent.parent / "data" / "training"
    output_dir.mkdir(parents=True, exist_ok=True)
    
    print("="*60)
    print("Advanced Training Data Collection")
    print("="*60)
    print("\nThis will:")
    print("  - Perform deep code analysis (AST, semantic)")
    print("  - Extract functions, classes, patterns")
    print("  - Create rich training examples")
    print("  - Build pattern database")
    print("="*60)
    
    print("\n1. Scanning existing projects with deep analysis...")
    scan_existing_projects(config, str(output_dir))
    
    print("\n2. Building codebase embedding index...")
    if EMBEDDING_AVAILABLE and CodebaseEmbeddingSystem:
        try:
            embedding_system = CodebaseEmbeddingSystem()
            project_paths = [info['path'] for info in config['projects'].values() if os.path.exists(info.get('path', ''))]
            
            if project_paths:
                embedding_dir = Path(__file__).parent.parent / "data" / "embeddings"
                embedding_system.build_index(project_paths, str(embedding_dir))
                print("✓ Embedding index built!")
            else:
                print("⚠ No valid project paths found for embedding")
        except Exception as e:
            print(f"⚠ Embedding system failed (non-critical): {e}")
            print("Continuing without embeddings...")
    else:
        print("⚠ Embedding system not available (sentence-transformers/faiss missing)")
        print("Continuing without embeddings...")
    
    # Only start watcher if not in "scan-only" mode (for training pipeline)
    if os.environ.get("SKIP_WATCHER", "").lower() != "true":
        print("\n3. Starting file watcher...")
        print("The collector will now watch for file changes.")
        print("Press Ctrl+C to stop.")
        
        collector = AdvancedCodeCollector(str(output_dir), config)
        observer = Observer()
        
        for project_name, project_info in config.get("projects", {}).items():
            project_path = project_info.get("path", "")
            if project_path and os.path.exists(project_path):
                observer.schedule(collector, project_path, recursive=True)
                print(f"Watching: {project_path}")
        
        observer.start()
        
        try:
            while True:
                import time
                time.sleep(1)
        except KeyboardInterrupt:
            observer.stop()
            collector.save_data()
        
        observer.join()
        print("\n✓ Data collection stopped.")
    else:
        print("\n3. Skipping file watcher (scan-only mode)...")
        print("✓ Data collection complete!")

if __name__ == "__main__":
    main()

