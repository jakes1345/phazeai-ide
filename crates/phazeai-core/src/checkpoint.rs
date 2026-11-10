//! Checkpoints: undo the file edits of an agent run.
//!
//! Each run of an agent that has [`Checkpoints`] records, for every file it
//! writes with `write_file` or `edit_file`, what the file held before the
//! run first touched it and what the run last wrote. [`Checkpoints::undo_last`]
//! puts the files of the latest run with edits back as they were, newest run
//! first, so pressing it again steps back another run.
//!
//! Your own edits are never undone: a file whose content is no longer what
//! the agent wrote (you saved it in the editor since, or another tool changed
//! it) is left as it is and reported. Changes made by shell commands are not
//! recorded.

use std::path::{Path, PathBuf};

/// One file a run wrote.
#[derive(Debug, Clone)]
struct FileEdit {
    path: PathBuf,
    /// The file's bytes before the run first wrote it; `None`: the run
    /// created it.
    original: Option<Vec<u8>>,
    /// What the run last wrote there.
    written: Vec<u8>,
}

/// The file edits of one run.
#[derive(Debug, Clone)]
struct Run {
    /// What the run was asked, shortened, for showing.
    label: String,
    files: Vec<FileEdit>,
}

/// The runs of a conversation, oldest first.
#[derive(Debug, Default)]
pub struct Checkpoints {
    runs: Vec<Run>,
}

/// What [`Checkpoints::undo_last`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoReport {
    /// What the undone run was asked.
    pub label: String,
    /// Files put back as they were before the run.
    pub restored: Vec<PathBuf>,
    /// Files the run created, removed again.
    pub removed: Vec<PathBuf>,
    /// Files that changed after the run wrote them, left as they are.
    pub kept: Vec<PathBuf>,
    /// Files that could not be put back, with the error.
    pub failed: Vec<(PathBuf, String)>,
}

impl UndoReport {
    /// One line for the user, e.g. "Undid 2 files from “add tests”; left
    /// main.rs, which you changed since."
    pub fn summary(&self) -> String {
        let name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        };
        let undone = self.restored.len() + self.removed.len();
        let files = |n: usize| if n == 1 { "file" } else { "files" };
        let mut out = format!(
            "Undid {undone} {} from \u{201c}{}\u{201d}",
            files(undone),
            self.label
        );
        if !self.kept.is_empty() {
            let names: Vec<String> = self.kept.iter().map(name).collect();
            out.push_str(&format!("; left {}, changed since", names.join(", ")));
        }
        if !self.failed.is_empty() {
            let names: Vec<String> = self
                .failed
                .iter()
                .map(|(p, e)| format!("{} ({e})", name(p)))
                .collect();
            out.push_str(&format!("; couldn't restore {}", names.join(", ")));
        }
        out.push('.');
        out
    }
}

impl Checkpoints {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start recording a run asked `request`.
    pub fn begin_run(&mut self, request: &str) {
        let first_line = request.lines().next().unwrap_or("").trim();
        let label = if first_line.chars().count() > 48 {
            let cut: String = first_line.chars().take(47).collect();
            format!("{cut}…")
        } else {
            first_line.to_string()
        };
        // A run that wrote nothing is no checkpoint.
        if self.runs.last().is_some_and(|r| r.files.is_empty()) {
            self.runs.pop();
        }
        self.runs.push(Run {
            label,
            files: Vec::new(),
        });
    }

    /// The current run wrote `written` to `path`, which held `before` (`None`:
    /// no file) just before.
    pub fn record(&mut self, path: &Path, before: Option<&[u8]>, written: &[u8]) {
        let Some(run) = self.runs.last_mut() else {
            return;
        };
        match run.files.iter_mut().find(|f| f.path == path) {
            Some(file) => file.written = written.to_vec(),
            None => run.files.push(FileEdit {
                path: path.to_path_buf(),
                original: before.map(<[u8]>::to_vec),
                written: written.to_vec(),
            }),
        }
    }

    /// How many runs have edits that can be undone.
    pub fn undoable(&self) -> usize {
        self.runs.iter().filter(|r| !r.files.is_empty()).count()
    }

    /// The files the latest run with edits wrote, for showing.
    pub fn last_files(&self) -> Vec<PathBuf> {
        self.runs
            .iter()
            .rev()
            .find(|r| !r.files.is_empty())
            .map(|r| r.files.iter().map(|f| f.path.clone()).collect())
            .unwrap_or_default()
    }

    /// Put the files of the latest run with edits back as they were before
    /// it, except any that changed since it wrote them. `None` when there is
    /// nothing to undo.
    pub fn undo_last(&mut self) -> Option<UndoReport> {
        while self.runs.last().is_some_and(|r| r.files.is_empty()) {
            self.runs.pop();
        }
        let run = self.runs.pop()?;
        let mut report = UndoReport {
            label: run.label,
            ..Default::default()
        };
        for file in run.files.into_iter().rev() {
            let now = std::fs::read(&file.path).ok();
            if now.as_deref() != Some(file.written.as_slice()) {
                report.kept.push(file.path);
                continue;
            }
            let result = match &file.original {
                Some(bytes) => std::fs::write(&file.path, bytes).map(|_| false),
                None => std::fs::remove_file(&file.path).map(|_| true),
            };
            match result {
                Ok(true) => report.removed.push(file.path),
                Ok(false) => report.restored.push(file.path),
                Err(e) => report.failed.push((file.path, e.to_string())),
            }
        }
        Some(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn undo_puts_back_what_the_run_changed_and_removes_what_it_created() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.rs"), dir.path().join("b.rs"));
        write(&a, "old a");
        let mut cp = Checkpoints::new();
        cp.begin_run("add b and change a");
        write(&a, "new a");
        cp.record(&a, Some(b"old a"), b"new a");
        write(&a, "newer a");
        cp.record(&a, Some(b"new a"), b"newer a");
        write(&b, "b");
        cp.record(&b, None, b"b");

        let report = cp.undo_last().unwrap();
        assert_eq!(read(&a), "old a", "back to before the run, not the middle");
        assert!(!b.exists());
        assert_eq!(report.restored, [a]);
        assert_eq!(report.removed, [b]);
        assert!(report.kept.is_empty());
        assert_eq!(
            report.summary(),
            "Undid 2 files from \u{201c}add b and change a\u{201d}."
        );
        assert!(cp.undo_last().is_none());
    }

    #[test]
    fn a_file_you_changed_since_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("main.rs");
        write(&a, "v1");
        let mut cp = Checkpoints::new();
        cp.begin_run("refactor");
        write(&a, "v2");
        cp.record(&a, Some(b"v1"), b"v2");
        write(&a, "v2 plus my edit");

        let report = cp.undo_last().unwrap();
        assert_eq!(read(&a), "v2 plus my edit");
        assert_eq!(report.kept, [a]);
        assert!(
            report.summary().contains("left main.rs, changed since"),
            "{}",
            report.summary()
        );
    }

    #[test]
    fn each_undo_steps_back_one_run_and_runs_without_edits_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        write(&a, "0");
        let mut cp = Checkpoints::new();
        cp.begin_run("one");
        write(&a, "1");
        cp.record(&a, Some(b"0"), b"1");
        cp.begin_run("two");
        write(&a, "2");
        cp.record(&a, Some(b"1"), b"2");
        cp.begin_run("just a question");
        assert_eq!(cp.undoable(), 2);
        assert_eq!(cp.last_files(), std::slice::from_ref(&a));

        assert_eq!(cp.undo_last().unwrap().label, "two");
        assert_eq!(read(&a), "1");
        assert_eq!(cp.undo_last().unwrap().label, "one");
        assert_eq!(read(&a), "0");
        assert_eq!(cp.undoable(), 0);
    }

    #[test]
    fn a_long_request_is_shortened_for_its_label() {
        let mut cp = Checkpoints::new();
        cp.begin_run(&format!("{}\nmore", "x".repeat(80)));
        cp.record(Path::new("/nowhere"), None, b"");
        let label = cp.runs[0].label.clone();
        assert_eq!(label.chars().count(), 48);
        assert!(label.ends_with('…'));
    }
}
