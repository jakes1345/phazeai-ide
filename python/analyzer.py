"""
Advanced code analyzer with AST parsing, semantic analysis, and pattern extraction.
"""

import ast
import re
from pathlib import Path
from typing import Dict, List, Any, Optional, Tuple
from dataclasses import dataclass

try:
    from tree_sitter import Language, Parser

    try:
        import tree_sitter_python as tspython

        TREE_SITTER_AVAILABLE = True
    except ImportError:
        # Try alternative import
        try:
            from tree_sitter_languages import get_language

            TREE_SITTER_AVAILABLE = True
            tspython = None
        except ImportError:
            TREE_SITTER_AVAILABLE = False
            tspython = None
except ImportError:
    TREE_SITTER_AVAILABLE = False
    tspython = None

try:
    import networkx as nx

    NETWORKX_AVAILABLE = True
except ImportError:
    NETWORKX_AVAILABLE = False
    nx = None


@dataclass
class CodePattern:
    """Represents a coding pattern."""

    pattern_type: str
    description: str
    code: str
    context: Dict[str, Any]
    frequency: int = 1


@dataclass
class FunctionSignature:
    """Function signature with metadata."""

    name: str
    parameters: List[str]
    return_type: Optional[str]
    docstring: Optional[str]
    decorators: List[str]
    complexity: int
    dependencies: List[str]


@dataclass
class ClassDefinition:
    """Class definition with metadata."""

    name: str
    base_classes: List[str]
    methods: List[FunctionSignature]
    attributes: List[str]
    docstring: Optional[str]
    design_pattern: Optional[str]


class AdvancedCodeAnalyzer:
    """Deep code analysis with semantic understanding."""

    def __init__(self):
        self.parsers = {}
        self._init_parsers()
        self.patterns = []
        self.dependency_graph = nx.DiGraph() if NETWORKX_AVAILABLE else None

    def _init_parsers(self):
        """Initialize tree-sitter parsers for different languages."""
        if TREE_SITTER_AVAILABLE and tspython is not None:
            try:
                PY_LANGUAGE = Language(tspython.language())
                self.parsers["python"] = Parser(PY_LANGUAGE)
            except Exception:
                pass  # Fallback to AST for Python

    def analyze_file(self, filepath: str, content: str) -> Dict[str, Any]:
        """Perform deep analysis of a code file."""
        ext = Path(filepath).suffix.lower()

        analysis = {
            "filepath": filepath,
            "language": self._detect_language(ext),
            "patterns": [],
            "functions": [],
            "classes": [],
            "imports": [],
            "dependencies": [],
            "complexity_metrics": {},
            "architecture_patterns": [],
            "code_quality": {},
            "semantic_embedding": None,
        }

        if ext == ".py":
            analysis.update(self._analyze_python(content, filepath))
        elif ext in [".js", ".ts", ".jsx", ".tsx"]:
            analysis.update(self._analyze_javascript(content, filepath))
        elif ext == ".rs":
            analysis.update(self._analyze_rust(content, filepath))
        elif ext == ".go":
            analysis.update(self._analyze_go(content, filepath))
        elif ext in [".cpp", ".c", ".h", ".hpp"]:
            analysis.update(self._analyze_cpp(content, filepath))
        elif ext in [".java"]:
            analysis.update(self._analyze_java(content, filepath))
        elif ext in [".cs"]:
            analysis.update(self._analyze_csharp(content, filepath))
        elif ext in [".html", ".htm"]:
            analysis.update(self._analyze_html(content, filepath))

        # Extract patterns
        analysis["patterns"] = self._extract_patterns(content, analysis)

        # Calculate complexity
        analysis["complexity_metrics"] = self._calculate_complexity(content, analysis)

        # Detect architecture patterns
        analysis["architecture_patterns"] = self._detect_architecture_patterns(analysis)

        return analysis

    def _analyze_python(self, content: str, filepath: str) -> Dict[str, Any]:
        """Deep Python analysis using AST."""
        try:
            tree = ast.parse(content)
        except SyntaxError:
            return {}

        functions = []
        classes = []
        imports = []
        dependencies = []

        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef):
                func = self._extract_function_signature(node, content)
                functions.append(func)

            elif isinstance(node, ast.ClassDef):
                cls = self._extract_class_definition(node, tree, content)
                classes.append(cls)

            elif isinstance(node, (ast.Import, ast.ImportFrom)):
                imports.extend(self._extract_imports(node))
                if isinstance(node, ast.ImportFrom) and node.module:
                    dependencies.append(node.module)

        return {
            "functions": [self._function_to_dict(f) for f in functions],
            "classes": [self._class_to_dict(c) for c in classes],
            "imports": imports,
            "dependencies": list(set(dependencies)),
        }

    def _extract_function_signature(
        self, node: ast.FunctionDef, content: str
    ) -> FunctionSignature:
        """Extract detailed function signature."""
        params = [arg.arg for arg in node.args.args]

        # Get return type annotation
        return_type = None
        if node.returns:
            return_type = (
                ast.unparse(node.returns)
                if hasattr(ast, "unparse")
                else str(node.returns)
            )

        # Get docstring
        docstring = ast.get_docstring(node)

        # Get decorators
        decorators = [
            ast.unparse(d) if hasattr(ast, "unparse") else str(d)
            for d in node.decorator_list
        ]

        # Calculate complexity (simplified cyclomatic complexity)
        complexity = self._calculate_function_complexity(node)

        # Extract dependencies (function calls)
        dependencies = []
        for child in ast.walk(node):
            if isinstance(child, ast.Call):
                if isinstance(child.func, ast.Name):
                    dependencies.append(child.func.id)
                elif isinstance(child.func, ast.Attribute):
                    dependencies.append(child.func.attr)

        return FunctionSignature(
            name=node.name,
            parameters=params,
            return_type=return_type,
            docstring=docstring,
            decorators=decorators,
            complexity=complexity,
            dependencies=dependencies,
        )

    def _extract_class_definition(
        self, node: ast.ClassDef, tree: ast.AST, content: str
    ) -> ClassDefinition:
        """Extract detailed class definition."""
        base_classes = []
        for base in node.bases:
            if isinstance(base, ast.Name):
                base_classes.append(base.id)

        methods = []
        attributes = []

        for item in node.body:
            if isinstance(item, ast.FunctionDef):
                methods.append(self._extract_function_signature(item, content))
            elif isinstance(item, ast.Assign):
                for target in item.targets:
                    if isinstance(target, ast.Name):
                        attributes.append(target.id)

        docstring = ast.get_docstring(node)
        design_pattern = self._detect_design_pattern(node, methods)

        return ClassDefinition(
            name=node.name,
            base_classes=base_classes,
            methods=methods,
            attributes=attributes,
            docstring=docstring,
            design_pattern=design_pattern,
        )

    def _calculate_function_complexity(self, node: ast.FunctionDef) -> int:
        """Calculate cyclomatic complexity."""
        complexity = 1  # Base complexity

        for child in ast.walk(node):
            if isinstance(child, (ast.If, ast.While, ast.For, ast.ExceptHandler)):
                complexity += 1
            elif isinstance(child, ast.BoolOp):
                complexity += len(child.values) - 1

        return complexity

    def _detect_design_pattern(
        self, node: ast.ClassDef, methods: List[FunctionSignature]
    ) -> Optional[str]:
        """Detect common design patterns."""
        method_names = [m.name for m in methods]

        # Singleton
        if "__new__" in method_names or "__init__" in method_names:
            if any("instance" in m.name.lower() for m in methods):
                return "singleton"

        # Factory
        if any(
            "create" in m.name.lower() or "factory" in m.name.lower() for m in methods
        ):
            return "factory"

        # Observer
        if "subscribe" in method_names or "notify" in method_names:
            return "observer"

        # Strategy
        if (
            len(
                [
                    m
                    for m in methods
                    if "strategy" in m.name.lower() or "execute" in m.name.lower()
                ]
            )
            > 0
        ):
            return "strategy"

        return None

    def _extract_imports(self, node: ast.Import | ast.ImportFrom) -> List[str]:
        """Extract import statements."""
        imports = []
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
        elif isinstance(node, ast.ImportFrom):
            module = node.module or ""
            for alias in node.names:
                imports.append(f"{module}.{alias.name}" if module else alias.name)
        return imports

    def _analyze_javascript(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze JavaScript/TypeScript files."""
        # Extract functions using regex (simplified)
        functions = []
        function_pattern = (
            r"(?:export\s+)?(?:async\s+)?function\s+(\w+)\s*\([^)]*\)\s*\{"
        )
        for match in re.finditer(function_pattern, content):
            functions.append({"name": match.group(1), "type": "function"})

        # Extract classes
        classes = []
        class_pattern = r"class\s+(\w+)(?:\s+extends\s+(\w+))?\s*\{"
        for match in re.finditer(class_pattern, content):
            classes.append(
                {
                    "name": match.group(1),
                    "extends": match.group(2) if match.group(2) else None,
                }
            )

        # Extract imports
        imports = []
        import_pattern = r'import\s+(?:.*?\s+from\s+)?[\'"]([^\'"]+)[\'"]'
        for match in re.finditer(import_pattern, content):
            imports.append(match.group(1))

        return {
            "functions": functions,
            "classes": classes,
            "imports": imports,
            "dependencies": imports,
        }

    def _analyze_rust(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze Rust files."""
        functions = []
        function_pattern = r"(?:pub\s+)?fn\s+(\w+)\s*\([^)]*\)(?:\s*->\s*[^{]+)?\s*\{"
        for match in re.finditer(function_pattern, content):
            functions.append({"name": match.group(1), "type": "function"})

        structs = []
        struct_pattern = r"(?:pub\s+)?struct\s+(\w+)"
        for match in re.finditer(struct_pattern, content):
            structs.append({"name": match.group(1), "type": "struct"})

        imports = []
        use_pattern = r"use\s+([^;]+);"
        for match in re.finditer(use_pattern, content):
            imports.append(match.group(1).strip())

        return {
            "functions": functions,
            "classes": structs,
            "imports": imports,
            "dependencies": imports,
        }

    def _analyze_go(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze Go files."""
        functions = []
        function_pattern = r"func\s+(?:\([^)]+\)\s+)?(\w+)\s*\([^)]*\)"
        for match in re.finditer(function_pattern, content):
            functions.append({"name": match.group(1), "type": "function"})

        structs = []
        struct_pattern = r"type\s+(\w+)\s+struct"
        for match in re.finditer(struct_pattern, content):
            structs.append({"name": match.group(1), "type": "struct"})

        imports = []
        import_pattern = r'import\s+(?:\(([^)]+)\)|"([^"]+)")'
        for match in re.finditer(import_pattern, content):
            imports.extend([i.strip().strip('"') for i in match.groups() if i])

        return {
            "functions": functions,
            "classes": structs,
            "imports": imports,
            "dependencies": imports,
        }

    def _analyze_cpp(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze C/C++ files."""
        functions = []
        function_pattern = (
            r"(?:inline\s+)?(?:\w+\s+)*(\w+)\s*\([^)]*\)\s*(?:const)?\s*\{"
        )
        for match in re.finditer(function_pattern, content):
            functions.append({"name": match.group(1), "type": "function"})

        classes = []
        class_pattern = (
            r"class\s+(\w+)(?:\s*:\s*(?:public|private|protected)\s+(\w+))?\s*\{"
        )
        for match in re.finditer(class_pattern, content):
            classes.append(
                {
                    "name": match.group(1),
                    "extends": match.group(2) if match.group(2) else None,
                }
            )

        includes = []
        include_pattern = r'#include\s+[<"]([^>"]+)[>"]'
        for match in re.finditer(include_pattern, content):
            includes.append(match.group(1))

        return {
            "functions": functions,
            "classes": classes,
            "imports": includes,
            "dependencies": includes,
        }

    def _analyze_java(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze Java files."""
        functions = []
        # Match methods: public/private/protected returnType methodName(params) { or throws
        method_pattern = r"(?:public|private|protected|static|\s)*\s*(?:\w+\s+)*(\w+)\s*\([^)]*\)(?:\s+throws\s+[\w\s,]+)?\s*\{"
        for match in re.finditer(method_pattern, content):
            func_name = match.group(1)
            # Skip if it's a class declaration
            if func_name not in ["class", "interface", "enum", "abstract"]:
                functions.append({"name": func_name, "type": "method"})

        classes = []
        class_pattern = r"(?:public\s+)?(?:abstract\s+)?(?:final\s+)?class\s+(\w+)(?:\s+extends\s+(\w+))?(?:\s+implements\s+([^{]+))?\s*\{"
        for match in re.finditer(class_pattern, content):
            extends = match.group(2) if match.group(2) else None
            implements = match.group(3).strip().split(",") if match.group(3) else []
            classes.append(
                {
                    "name": match.group(1),
                    "extends": extends,
                    "implements": [i.strip() for i in implements],
                }
            )

        interfaces = []
        interface_pattern = r"(?:public\s+)?interface\s+(\w+)"
        for match in re.finditer(interface_pattern, content):
            interfaces.append({"name": match.group(1), "type": "interface"})

        imports = []
        import_pattern = r"import\s+(?:static\s+)?([\w.]+)\s*;"
        for match in re.finditer(import_pattern, content):
            imports.append(match.group(1))

        packages = []
        package_pattern = r"package\s+([\w.]+)\s*;"
        for match in re.finditer(package_pattern, content):
            packages.append(match.group(1))

        return {
            "functions": functions,
            "classes": classes + interfaces,
            "imports": imports,
            "dependencies": imports + packages,
        }

    def _analyze_csharp(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze C# files."""
        functions = []
        # Match methods: [attributes] access_modifier returnType MethodName(params) { or async
        method_pattern = r"(?:public|private|protected|internal|static|async|\s)*\s*(?:\w+\s+)*(\w+)\s*\([^)]*\)(?:\s*:\s*base\([^)]*\))?\s*\{"
        for match in re.finditer(method_pattern, content):
            func_name = match.group(1)
            # Skip keywords
            if func_name not in [
                "class",
                "interface",
                "struct",
                "enum",
                "namespace",
                "using",
                "get",
                "set",
            ]:
                functions.append({"name": func_name, "type": "method"})

        classes = []
        class_pattern = r"(?:public\s+)?(?:abstract\s+)?(?:sealed\s+)?(?:partial\s+)?class\s+(\w+)(?:\s*:\s*([^{]+))?\s*\{"
        for match in re.finditer(class_pattern, content):
            base_classes = match.group(2).strip().split(",") if match.group(2) else []
            classes.append(
                {"name": match.group(1), "extends": [b.strip() for b in base_classes]}
            )

        interfaces = []
        interface_pattern = r"(?:public\s+)?interface\s+(\w+)"
        for match in re.finditer(interface_pattern, content):
            interfaces.append({"name": match.group(1), "type": "interface"})

        namespaces = []
        namespace_pattern = r"namespace\s+([\w.]+)\s*\{"
        for match in re.finditer(namespace_pattern, content):
            namespaces.append(match.group(1))

        imports = []
        using_pattern = r"using\s+(?:static\s+)?([\w.]+)\s*;"
        for match in re.finditer(using_pattern, content):
            imports.append(match.group(1))

        return {
            "functions": functions,
            "classes": classes + interfaces,
            "imports": imports,
            "dependencies": imports + namespaces,
        }

    def _analyze_html(self, content: str, filepath: str) -> Dict[str, Any]:
        """Analyze HTML files."""
        # Extract script tags (JavaScript)
        scripts = []
        script_pattern = r"<script[^>]*>(.*?)</script>"
        for match in re.finditer(script_pattern, content, re.DOTALL | re.IGNORECASE):
            script_content = match.group(1)
            if script_content.strip():
                scripts.append(
                    {"type": "inline_script", "content_length": len(script_content)}
                )

        # Extract style tags (CSS)
        styles = []
        style_pattern = r"<style[^>]*>(.*?)</style>"
        for match in re.finditer(style_pattern, content, re.DOTALL | re.IGNORECASE):
            style_content = match.group(1)
            if style_content.strip():
                styles.append(
                    {"type": "inline_style", "content_length": len(style_content)}
                )

        # Extract links (CSS, JS files)
        links = []
        link_pattern = r'<link[^>]+href=["\']([^"\']+)["\']'
        for match in re.finditer(link_pattern, content, re.IGNORECASE):
            links.append(match.group(1))

        # Extract script src
        script_srcs = []
        script_src_pattern = r'<script[^>]+src=["\']([^"\']+)["\']'
        for match in re.finditer(script_src_pattern, content, re.IGNORECASE):
            script_srcs.append(match.group(1))

        # Extract forms
        forms = []
        form_pattern = r"<form[^>]*>"
        forms = [m.group(0) for m in re.finditer(form_pattern, content, re.IGNORECASE)]

        # Extract elements
        elements = {}
        element_pattern = r"<(\w+)"
        for match in re.finditer(element_pattern, content):
            tag = match.group(1).lower()
            elements[tag] = elements.get(tag, 0) + 1

        return {
            "scripts": scripts,
            "styles": styles,
            "links": links + script_srcs,
            "forms": len(forms),
            "elements": elements,
            "dependencies": links + script_srcs,
        }

    def _extract_patterns(self, content: str, analysis: Dict) -> List[CodePattern]:
        """Extract coding patterns from code."""
        patterns = []

        # Error handling patterns
        if re.search(r"try\s*\{", content):
            patterns.append(
                CodePattern(
                    pattern_type="error_handling",
                    description="Try-catch error handling",
                    code=content,
                    context={"language": analysis.get("language")},
                )
            )

        # Async/await patterns
        if re.search(r"async\s+def|await\s+", content):
            patterns.append(
                CodePattern(
                    pattern_type="async",
                    description="Async/await pattern",
                    code=content,
                    context={"language": analysis.get("language")},
                )
            )

        # Decorator patterns
        if "@" in content and analysis.get("language") == "python":
            patterns.append(
                CodePattern(
                    pattern_type="decorator",
                    description="Python decorator usage",
                    code=content,
                    context={"language": "python"},
                )
            )

        return patterns

    def _calculate_complexity(self, content: str, analysis: Dict) -> Dict[str, Any]:
        """Calculate code complexity metrics."""
        lines = content.split("\n")
        code_lines = [l for l in lines if l.strip() and not l.strip().startswith("#")]

        return {
            "lines_of_code": len(code_lines),
            "total_lines": len(lines),
            "function_count": len(analysis.get("functions", [])),
            "class_count": len(analysis.get("classes", [])),
            "average_function_complexity": self._avg_complexity(
                analysis.get("functions", [])
            ),
        }

    def _avg_complexity(self, functions: List[Any]) -> float:
        """Calculate average function complexity."""
        if not functions:
            return 0.0

        complexities = []
        for func in functions:
            if isinstance(func, FunctionSignature):
                complexities.append(func.complexity)
            elif isinstance(func, dict) and "complexity" in func:
                complexities.append(func["complexity"])
            else:
                complexities.append(1)  # Default

        return sum(complexities) / len(complexities) if complexities else 0.0

    def _detect_architecture_patterns(self, analysis: Dict) -> List[str]:
        """Detect architecture patterns."""
        patterns = []

        # MVC pattern
        if any(
            "controller" in f.get("name", "").lower()
            for f in analysis.get("classes", [])
        ):
            patterns.append("MVC")

        # Repository pattern
        if any(
            "repository" in f.get("name", "").lower()
            for f in analysis.get("classes", [])
        ):
            patterns.append("Repository")

        # Service pattern
        if any(
            "service" in f.get("name", "").lower() for f in analysis.get("classes", [])
        ):
            patterns.append("Service")

        return patterns

    def _detect_language(self, ext: str) -> str:
        """Detect programming language from extension."""
        lang_map = {
            ".py": "python",
            ".js": "javascript",
            ".ts": "typescript",
            ".jsx": "javascript",
            ".tsx": "typescript",
            ".rs": "rust",
            ".go": "go",
            ".cpp": "cpp",
            ".c": "c",
            ".h": "c",
            ".hpp": "cpp",
            ".java": "java",
            ".cs": "csharp",
            ".html": "html",
            ".htm": "html",
            ".css": "css",
            ".xml": "xml",
            ".json": "json",
        }
        return lang_map.get(ext, "unknown")

    def _function_to_dict(self, func: FunctionSignature) -> Dict:
        """Convert FunctionSignature to dict."""
        return {
            "name": func.name,
            "parameters": func.parameters,
            "return_type": func.return_type,
            "docstring": func.docstring if func.docstring is not None else "",
            "decorators": func.decorators,
            "complexity": func.complexity,
            "dependencies": func.dependencies,
        }

    def _class_to_dict(self, cls: ClassDefinition) -> Dict:
        """Convert ClassDefinition to dict."""
        return {
            "name": cls.name,
            "base_classes": cls.base_classes,
            "methods": [self._function_to_dict(m) for m in cls.methods],
            "attributes": cls.attributes,
            "docstring": cls.docstring,
            "design_pattern": cls.design_pattern,
        }
