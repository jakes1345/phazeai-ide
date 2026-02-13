#!/usr/bin/env python3
"""
Collect training data from your coding patterns and project files.
This script watches your projects and extracts code patterns, comments, and context.
"""

import os
import json
import hashlib
from pathlib import Path
from typing import List, Dict, Any
from datetime import datetime
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler
import yaml

class CodeCollector(FileSystemEventHandler):
    """Collects code patterns and context from file changes."""
    
    def __init__(self, output_dir: str, projects_config: Dict):
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.projects_config = projects_config
        self.collected_data = []
        
    def on_modified(self, event):
        if event.is_directory:
            return
        
        if event.src_path.endswith(('.py', '.js', '.ts', '.rs', '.go', '.cpp', '.c', '.h', '.hpp')):
            self.process_file(event.src_path)
    
    def process_file(self, filepath: str):
        """Extract code patterns and context from a file."""
        try:
            with open(filepath, 'r', encoding='utf-8', errors='ignore') as f:
                content = f.read()
            
            # Determine project context
            project_context = self.get_project_context(filepath)
            
            # Extract patterns
            data_point = {
                "filepath": filepath,
                "content": content,
                "project": project_context,
                "timestamp": datetime.now().isoformat(),
                "file_hash": hashlib.md5(content.encode()).hexdigest()
            }
            
            self.collected_data.append(data_point)
            
            # Save incrementally
            if len(self.collected_data) % 10 == 0:
                self.save_data()
                
        except Exception as e:
            print(f"Error processing {filepath}: {e}")
    
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
        output_file = self.output_dir / f"training_data_{datetime.now().strftime('%Y%m%d')}.jsonl"
        with open(output_file, 'a', encoding='utf-8') as f:
            for item in self.collected_data:
                f.write(json.dumps(item) + '\n')
        self.collected_data = []
        print(f"Saved training data to {output_file}")

def scan_existing_projects(projects_config: Dict, output_dir: str):
    """Scan existing project files to build initial training dataset."""
    collector = CodeCollector(output_dir, projects_config)
    
    for project_name, project_info in projects_config.get("projects", {}).items():
        project_path = project_info.get("path", "")
        if not project_path or not os.path.exists(project_path):
            print(f"Project path not found: {project_path}")
            continue
        
        print(f"Scanning {project_name} at {project_path}...")
        
        # Scan common code files
        extensions = ['.py', '.js', '.ts', '.rs', '.go', '.cpp', '.c', '.h', '.hpp', '.java', '.kt']
        for ext in extensions:
            for filepath in Path(project_path).rglob(f"*{ext}"):
                if 'node_modules' in str(filepath) or '.git' in str(filepath):
                    continue
                collector.process_file(str(filepath))
    
    collector.save_data()
    print("Initial scan complete!")

def main():
    config_path = Path(__file__).parent.parent / "config" / "projects.json"
    with open(config_path, 'r') as f:
        config = json.load(f)
    
    output_dir = Path(__file__).parent.parent / "data" / "training"
    output_dir.mkdir(parents=True, exist_ok=True)
    
    print("Starting training data collection...")
    print("1. Scanning existing projects...")
    scan_existing_projects(config, str(output_dir))
    
    print("\n2. Starting file watcher...")
    print("The collector will now watch for file changes.")
    print("Press Ctrl+C to stop.")
    
    collector = CodeCollector(str(output_dir), config)
    observer = Observer()
    
    # Watch all project directories
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
    print("\nData collection stopped.")

if __name__ == "__main__":
    main()

