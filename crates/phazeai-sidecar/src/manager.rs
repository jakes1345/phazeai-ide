use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::{Child, Command};
use tracing::{info, warn};

/// Manages the Python sidecar process lifecycle.
pub struct SidecarManager {
    python_path: String,
    script_path: PathBuf,
    process: Option<Child>,
}

impl SidecarManager {
    pub fn new(python_path: impl Into<String>, script_path: impl Into<PathBuf>) -> Self {
        Self {
            python_path: python_path.into(),
            script_path: script_path.into(),
            process: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.process.is_some()
    }

    pub async fn start(&mut self) -> Result<(), String> {
        if self.is_running() {
            return Ok(());
        }

        if !self.script_path.exists() {
            return Err(format!(
                "Sidecar script not found: {}",
                self.script_path.display()
            ));
        }

        info!("Starting Python sidecar: {} {}", self.python_path, self.script_path.display());

        let child = Command::new(&self.python_path)
            .arg(&self.script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to start sidecar: {e}"))?;

        self.process = Some(child);
        info!("Sidecar started successfully");
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            info!("Stopping sidecar");
            let _ = process.kill().await;
        }
    }

    pub fn take_process(&mut self) -> Option<Child> {
        self.process.take()
    }

    /// Check if Python is available on the system.
    pub async fn check_python(python_path: &str) -> bool {
        Command::new(python_path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        if let Some(mut process) = self.process.take() {
            warn!("Sidecar process dropped without explicit stop");
            let _ = process.start_kill();
        }
    }
}
