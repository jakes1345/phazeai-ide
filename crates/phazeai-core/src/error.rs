use thiserror::Error;

#[derive(Error, Debug)]
pub enum PhazeError {
    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Tool error: {tool}: {message}")]
    Tool { tool: String, message: String },

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Agent exceeded maximum iterations ({0})")]
    MaxIterations(usize),

    #[error("Sidecar error: {0}")]
    Sidecar(String),

    #[error("{0}")]
    Other(String),

    #[error("Agent execution cancelled")]
    Cancelled,
}

impl PhazeError {
    pub fn tool(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Tool {
            tool: tool.into(),
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PhazeError>;
