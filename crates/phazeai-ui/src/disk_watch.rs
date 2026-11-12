//! Notice when a file open in a tab changes on disk.
//!
//! The agent's edits, rename, the formatter that runs on save, `git checkout`
//! and other programs all write files behind the editor's back. Without this,
//! a tab kept showing the old text, and its next save wrote that old text
//! back over the change.
//!
//! A background thread polls the open files (a stat each, about once a
//! second; a slow or hung drive blocks that thread, never the UI) and sends
//! the new text of any file whose modification time or size moved. The
//! editor then decides with [`on_disk_change`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

/// What to do with a tab whose file changed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskChange {
    /// The tab already shows this text (our own save, say).
    Same,
    /// No unsaved edits: show the new text.
    Reload,
    /// Unsaved edits: keep them, and say the file changed underneath.
    Conflict,
}

/// Decide what a change on disk means for a tab showing `tab_text`.
pub fn on_disk_change(tab_text: &str, disk_text: &str, unsaved: bool) -> DiskChange {
    if tab_text == disk_text {
        DiskChange::Same
    } else if unsaved {
        DiskChange::Conflict
    } else {
        DiskChange::Reload
    }
}

/// Modification time and size: a change in either means the file changed.
type Stamp = (Option<SystemTime>, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok(), meta.len()))
}

/// The files to watch. The watcher stops once every clone of this is gone.
pub type Watched = Arc<Mutex<Vec<PathBuf>>>;

/// Start polling the files in `watched` every `every`. For each one whose
/// stamp moves after it was first seen, the new text is sent on `tx` (a file
/// that is gone or not UTF-8 text is skipped). The thread ends when `watched`
/// is dropped or `tx`'s receiver is.
pub fn spawn(watched: &Watched, every: Duration, tx: SyncSender<(PathBuf, String)>) {
    let watched: Weak<Mutex<Vec<PathBuf>>> = Arc::downgrade(watched);
    std::thread::Builder::new()
        .name("disk-watch".into())
        .spawn(move || {
            let mut seen: HashMap<PathBuf, Stamp> = HashMap::new();
            loop {
                let Some(list) = watched.upgrade() else {
                    return;
                };
                let paths = list.lock().map(|l| l.clone()).unwrap_or_default();
                drop(list);
                seen.retain(|p, _| paths.contains(p));
                for path in paths {
                    let Some(now) = stamp(&path) else {
                        continue;
                    };
                    match seen.insert(path.clone(), now) {
                        Some(before) if before != now => {
                            if let Ok(text) = std::fs::read_to_string(&path) {
                                if tx.send((path, text)).is_err() {
                                    return;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(every);
            }
        })
        .expect("spawn the disk watcher thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_tab_without_unsaved_edits_is_reloaded() {
        assert_eq!(on_disk_change("a", "a", true), DiskChange::Same);
        assert_eq!(on_disk_change("a", "b", false), DiskChange::Reload);
        assert_eq!(on_disk_change("a", "b", true), DiskChange::Conflict);
    }

    #[test]
    fn a_file_changed_by_another_program_is_reported_with_its_new_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn old() {}\n").unwrap();
        let watched: Watched = Arc::new(Mutex::new(vec![file.clone()]));
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        spawn(&watched, Duration::from_millis(20), tx);

        // Nothing is reported for the file as it was when first seen.
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());

        // A different size is a change even within one mtime tick.
        std::fs::write(&file, "fn renamed_by_the_agent() {}\n").unwrap();
        let (path, text) = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the change is reported");
        assert_eq!(path, file);
        assert_eq!(text, "fn renamed_by_the_agent() {}\n");

        // Once the list is gone the thread stops: the channel closes.
        drop(watched);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                _ if std::time::Instant::now() > deadline => {
                    panic!("the watcher kept running after its list was dropped")
                }
                _ => {}
            }
        }
    }
}
