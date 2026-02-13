use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::mpsc;

/// Watches a directory for file system changes and sends events through a channel.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    /// Start watching a directory. Returns a receiver for file change events.
    pub fn watch(
        path: &Path,
    ) -> Result<(Self, mpsc::UnboundedReceiver<FileChangeEvent>), crate::error::PhazeError> {
        let (tx, rx) = mpsc::unbounded_channel();

        let event_tx = tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Ok(event) = result {
                    let kind = match &event.kind {
                        notify::EventKind::Create(_) => FileChangeKind::Created,
                        notify::EventKind::Modify(_) => FileChangeKind::Modified,
                        notify::EventKind::Remove(_) => FileChangeKind::Removed,
                        _ => return,
                    };

                    for path in event.paths {
                        let _ = event_tx.send(FileChangeEvent {
                            path,
                            kind: kind.clone(),
                        });
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| crate::error::PhazeError::Other(format!("Failed to create watcher: {e}")))?;

        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|e| crate::error::PhazeError::Other(format!("Failed to watch path: {e}")))?;

        Ok((Self { _watcher: watcher }, rx))
    }
}

#[derive(Debug, Clone)]
pub struct FileChangeEvent {
    pub path: std::path::PathBuf,
    pub kind: FileChangeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FileChangeKind {
    Created,
    Modified,
    Removed,
}
