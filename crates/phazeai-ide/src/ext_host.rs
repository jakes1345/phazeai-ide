use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{error, info, trace};

pub struct ExtHostManager {
    msg_tx: mpsc::UnboundedSender<Value>,
}

impl ExtHostManager {
    pub fn new(ide_tx: mpsc::UnboundedSender<crate::app::IdeEvent>) -> Self {
        let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Value>();

        tokio::spawn(async move {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

            let mut child = match Command::new("node")
                .arg(cwd.join("ext-host/src/main.js"))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    error!(
                        "Failed to start Node extension host. Is Node installed? Error: {}",
                        e
                    );
                    return;
                }
            };

            info!("Node Extension Host started!");

            let stdout = child.stdout.take().expect("Failed to open stdout");
            let mut stdin = child.stdin.take().expect("Failed to open stdin");

            // Reader task
            let reader_ide_tx = ide_tx.clone();
            let reader_handle = tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    trace!("ExtHost Output: {}", line);
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        let _ = reader_ide_tx.send(crate::app::IdeEvent::ExtHostMessage(value));
                    }
                }
            });

            // Writer task
            let writer_handle = tokio::spawn(async move {
                while let Some(msg) = msg_rx.recv().await {
                    let mut line = msg.to_string();
                    line.push('\n');
                    if let Err(e) = stdin.write_all(line.as_bytes()).await {
                        error!("Failed to write to ExtHost: {}", e);
                        break;
                    }
                }
            });

            let _ = tokio::try_join!(
                async { reader_handle.await.map_err(|e| anyhow::anyhow!(e)) },
                async { writer_handle.await.map_err(|e| anyhow::anyhow!(e)) }
            );

            let _ = child.wait().await;
            info!("Extension host process exited.");
        });

        Self { msg_tx }
    }

    pub fn send(&self, msg: Value) {
        let _ = self.msg_tx.send(msg);
    }
}
