use crate::error::PhazeError;
use ollama_rs::Ollama;
use ollama_rs::models::create::CreateModelRequest;
use std::path::Path;
use tracing::info;

pub struct OllamaManager {
    ollama: Ollama,
    _base_url: String,
}

impl OllamaManager {
    pub fn new(base_url: &str) -> Result<Self, PhazeError> {
        let ollama = Ollama::try_new(base_url)
            .map_err(|e| PhazeError::Llm(format!("Invalid Ollama URL: {e}")))?;
        Ok(Self {
            ollama,
            _base_url: base_url.to_string(),
        })
    }

    /// Check if Ollama is reachable and responsive
    pub async fn check_health(&self) -> Result<(), PhazeError> {
        self.ollama.list_local_models()
            .await
            .map(|_| ())
            .map_err(|e| PhazeError::Llm(format!("Ollama is not responding at {}. Is it running at that address? Error: {}", self._base_url, e)))
    }

    /// Check if a model exists in the local Ollama instance.
    pub async fn model_exists(&self, model_name: &str) -> Result<bool, PhazeError> {
        let models = self.ollama.list_local_models()
            .await
            .map_err(|e| PhazeError::Llm(format!("Failed to list Ollama models: {e}")))?;
        
        Ok(models.iter().any(|m| m.name == model_name || m.name.starts_with(&format!("{}:", model_name))))
    }

    /// Specifically ensure Phaze-Beast is provisioned with optimized settings
    pub async fn ensure_phaze_beast(&self) -> Result<(), PhazeError> {
        let modelfile = include_str!("../../resources/Modelfile-Phaze-Lite");
        self.ensure_model_from_content("phaze-beast", modelfile).await
    }

    /// Provision a model from strings (useful for bundled resources).
    pub async fn ensure_model_from_content(&self, model_name: &str, modelfile_content: &str) -> Result<(), PhazeError> {
        if self.model_exists(model_name).await? {
            return Ok(());
        }

        info!("Provisioning model from content: {}...", model_name);

        let request = CreateModelRequest::modelfile(model_name.to_string(), modelfile_content.to_string());
        self.ollama.create_model(request)
            .await
            .map_err(|e| PhazeError::Llm(format!("Failed to create model: {e}")))?;
        
        Ok(())
    }

    /// Provision a model from a file path.
    pub async fn ensure_model(&self, model_name: &str, modelfile_path: &Path) -> Result<(), PhazeError> {
        if self.model_exists(model_name).await? {
            return Ok(());
        }

        let modelfile_content = std::fs::read_to_string(modelfile_path)
            .map_err(|e| PhazeError::Llm(format!("Failed to read modelfile: {e}")))?;

        self.ensure_model_from_content(model_name, &modelfile_content).await
    }

    /// Provision all PhazeAI specialized models for the multi-agent system.
    /// These are created from base models with custom system prompts — no training needed.
    pub async fn ensure_all_phaze_models(&self) -> Result<(), PhazeError> {
        // Phaze-Coder: primary code generation
        self.ensure_model_from_content("phaze-coder", MODELFILE_CODER).await?;
        info!("phaze-coder ready");

        // Phaze-Planner: fast planning and analysis
        self.ensure_model_from_content("phaze-planner", MODELFILE_PLANNER).await?;
        info!("phaze-planner ready");

        // Phaze-Reviewer: code review and quality
        self.ensure_model_from_content("phaze-reviewer", MODELFILE_REVIEWER).await?;
        info!("phaze-reviewer ready");

        Ok(())
    }

    /// List all available Ollama models on this machine.
    pub async fn list_models(&self) -> Result<Vec<String>, PhazeError> {
        let models = self.ollama.list_local_models()
            .await
            .map_err(|e| PhazeError::Llm(format!("Failed to list models: {e}")))?;
        Ok(models.iter().map(|m| m.name.clone()).collect())
    }

    /// Check health and provision essential models if missing.
    /// Returns a list of models that were provisioned.
    pub async fn setup_checks(&self) -> Result<Vec<String>, PhazeError> {
        self.check_health().await?;
        
        let mut provisioned = Vec::new();
        
        if !self.model_exists("phaze-beast").await? {
            info!("Provisioning phaze-beast...");
            self.ensure_phaze_beast().await?;
            provisioned.push("phaze-beast".to_string());
        }
        
        if !self.model_exists("phaze-coder").await? {
            info!("Provisioning phaze-coder...");
            self.ensure_model_from_content("phaze-coder", MODELFILE_CODER).await?;
            provisioned.push("phaze-coder".to_string());
        }

        if !self.model_exists("phaze-planner").await? {
            info!("Provisioning phaze-planner...");
            self.ensure_model_from_content("phaze-planner", MODELFILE_PLANNER).await?;
            provisioned.push("phaze-planner".to_string());
        }

        if !self.model_exists("phaze-reviewer").await? {
            info!("Provisioning phaze-reviewer...");
            self.ensure_model_from_content("phaze-reviewer", MODELFILE_REVIEWER).await?;
            provisioned.push("phaze-reviewer".to_string());
        }
        
        Ok(provisioned)
    }
}

// ── Inline Modelfile content ────────────────────────────────────────

const MODELFILE_CODER: &str = r#"FROM qwen2.5-coder:14b
SYSTEM """You are PhazeAI Coder, an elite AI coding assistant.
RULES: Write COMPLETE production code. No placeholders. No TODOs.
Include error handling. Match codebase style. Output diffs with file paths."""
PARAMETER temperature 0.3
PARAMETER top_p 0.9
PARAMETER num_ctx 32768
PARAMETER repeat_penalty 1.1
"#;

const MODELFILE_PLANNER: &str = r#"FROM llama3.2:3b
SYSTEM """You are PhazeAI Planner. Analyze coding requests, produce step-by-step plans.
Output: analysis, numbered steps, files to change, risks. Never write code."""
PARAMETER temperature 0.5
PARAMETER num_ctx 8192
"#;

const MODELFILE_REVIEWER: &str = r#"FROM deepseek-coder-v2:16b
SYSTEM """You are PhazeAI Reviewer. Review code for bugs, security, and quality.
Output: APPROVED, CONCERNS, or REJECTED with specific line references."""
PARAMETER temperature 0.2
PARAMETER num_ctx 16384
"#;
