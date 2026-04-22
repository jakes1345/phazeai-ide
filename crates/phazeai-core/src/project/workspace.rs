use std::path::{Path, PathBuf};

/// Information about a detected workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    pub root: PathBuf,
    pub project_type: ProjectType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectType {
    Rust,
    Node,
    Python,
    Go,
    Git,
    Unknown,
}

/// Walk up from the given path to find the workspace root.
/// Looks for common project markers: .git, Cargo.toml, package.json, etc.
pub fn find_workspace_root(start: &Path) -> Option<WorkspaceInfo> {
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        // Check for project markers in priority order
        if current.join("Cargo.toml").exists() {
            return Some(WorkspaceInfo {
                root: current,
                project_type: ProjectType::Rust,
            });
        }
        if current.join("package.json").exists() {
            return Some(WorkspaceInfo {
                root: current,
                project_type: ProjectType::Node,
            });
        }
        if current.join("pyproject.toml").exists() || current.join("setup.py").exists() {
            return Some(WorkspaceInfo {
                root: current,
                project_type: ProjectType::Python,
            });
        }
        if current.join("go.mod").exists() {
            return Some(WorkspaceInfo {
                root: current,
                project_type: ProjectType::Go,
            });
        }
        if current.join(".git").exists() {
            return Some(WorkspaceInfo {
                root: current,
                project_type: ProjectType::Git,
            });
        }

        if !current.pop() {
            break;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_rust_workspace() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        let sub = tmp.path().join("src");
        std::fs::create_dir_all(&sub).unwrap();

        let info = find_workspace_root(&sub).unwrap();
        assert_eq!(info.root, tmp.path());
        assert_eq!(info.project_type, ProjectType::Rust);
    }

    #[test]
    fn test_no_workspace() {
        let tmp = TempDir::new().unwrap();
        let result = find_workspace_root(tmp.path());
        assert!(result.is_none());
    }
}
