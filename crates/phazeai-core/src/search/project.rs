//! Whether a folder is a project worth indexing without being asked.
//!
//! The IDE indexes the workspace in the background when it opens. That
//! workspace is the git root of the working directory, or the working directory
//! itself, and a desktop launcher starts the process in `/` (macOS Dock) or in
//! the home directory (Linux `.desktop` files). Walking either is minutes of
//! disk activity, on macOS a round of permission prompts for Desktop, Documents
//! and external volumes, and an index of whatever sorts first.

use super::paths::{absolutize, path_key};
use std::path::Path;

/// Directories whose presence marks the root of a version-controlled tree.
const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn", ".jj"];

/// Files that say "a build or package manifest lives here".
const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "CMakeLists.txt",
    "meson.build",
    "Makefile",
    "composer.json",
    "Gemfile",
    "mix.exs",
    "deno.json",
    "tsconfig.json",
    "pubspec.yaml",
    "Package.swift",
    // PhazeAI's own project state.
    ".phazeai",
];

/// Whether `dir` is, or lies inside, a version-controlled tree, or holds a
/// build or package manifest. A filesystem root never is, and a home directory
/// is only when it is itself under version control (a dotfiles repository): a
/// stray `package.json` there, left by an `npm install` in the wrong place,
/// does not make a person's whole home a project.
pub fn looks_like_project(dir: &Path) -> bool {
    looks_like_project_with(dir, dirs::home_dir().as_deref())
}

fn looks_like_project_with(dir: &Path, home: Option<&Path>) -> bool {
    let dir = absolutize(dir);
    if dir.parent().is_none() {
        return false;
    }
    if dir
        .ancestors()
        .any(|a| VCS_DIRS.iter().any(|v| a.join(v).exists()))
    {
        return true;
    }
    let is_home = home.is_some_and(|h| path_key(&absolutize(h)) == path_key(&dir));
    !is_home && MANIFESTS.iter().any(|m| dir.join(m).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_controlled_trees_and_manifests_mark_projects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("repo/src/deep")).unwrap();
        std::fs::create_dir_all(root.join("crate")).unwrap();
        std::fs::write(root.join("crate/Cargo.toml"), "[package]\n").unwrap();
        std::fs::create_dir_all(root.join("plain/sub")).unwrap();
        std::fs::create_dir_all(root.join("worktree")).unwrap();
        // A worktree or submodule has a `.git` file, not a directory.
        std::fs::write(root.join("worktree/.git"), "gitdir: ../repo/.git\n").unwrap();

        assert!(looks_like_project(&root.join("repo")));
        assert!(looks_like_project(&root.join("repo/src/deep")));
        assert!(looks_like_project(&root.join("crate")));
        assert!(looks_like_project(&root.join("worktree")));
        assert!(!looks_like_project(&root.join("plain")));
        assert!(!looks_like_project(&root.join("plain/sub")));
    }

    #[test]
    fn a_manifest_counts_only_in_the_folder_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("outer/inner")).unwrap();
        std::fs::write(root.join("outer/package.json"), "{}\n").unwrap();
        assert!(looks_like_project(&root.join("outer")));
        assert!(
            !looks_like_project(&root.join("outer/inner")),
            "a folder below a manifest is not a project root"
        );
    }

    #[test]
    fn a_home_directory_is_a_project_only_under_version_control() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("package.json"), "{}\n").unwrap();
        assert!(!looks_like_project_with(&home, Some(&home)));
        // The same folder is a project when it is not the home directory...
        assert!(looks_like_project_with(&home, None));
        // ...or when the home directory is a dotfiles repository.
        std::fs::create_dir_all(home.join(".git")).unwrap();
        assert!(looks_like_project_with(&home, Some(&home)));
    }

    #[test]
    fn a_filesystem_root_is_never_a_project() {
        let top = std::env::temp_dir()
            .ancestors()
            .last()
            .unwrap()
            .to_path_buf();
        assert!(!looks_like_project(&top));
    }
}
