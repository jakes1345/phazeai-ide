//! User-visible error reporting for places that have no toast signal handy.

use std::path::Path;

/// Show a modal error dialog without blocking the UI thread.
pub fn error_dialog(title: &str, body: String) {
    let title = title.to_string();
    std::thread::spawn(move || {
        let _ = rfd::MessageDialog::new()
            .set_title(&title)
            .set_description(&body)
            .set_level(rfd::MessageLevel::Error)
            .show();
    });
}

/// "Couldn't save foo.rs: Permission denied (os error 13)"
pub fn save_failure_message(path: &Path, err: &std::io::Error) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    format!(
        "Couldn't save {name}: {err}\n\nYour changes are still in the editor (the tab stays marked as modified)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_file_and_reason() {
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Permission denied");
        let msg = save_failure_message(Path::new("/a/b/main.rs"), &err);
        assert!(msg.contains("main.rs") && msg.contains("Permission denied"));
        assert!(
            !msg.contains("/a/b"),
            "should show the file name, not the full path"
        );
    }
}
