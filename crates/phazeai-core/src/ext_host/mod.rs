pub mod js;
pub mod vsix;
pub mod wasm;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// A trait that the UI layer implements to provide real IDE functionality to extensions
pub trait IdeDelegate: Send + Sync {
    fn log(&self, msg: &str);
    fn show_message(&self, msg: &str);
    fn get_active_text(&self) -> String;
}

/// A default dummy implementation for testing
pub struct DummyDelegate;
impl IdeDelegate for DummyDelegate {
    fn log(&self, msg: &str) {
        tracing::info!("[IDE] {}", msg);
    }
    fn show_message(&self, msg: &str) {
        tracing::info!("[IDE Message] {}", msg);
    }
    fn get_active_text(&self) -> String {
        "No active document".to_string()
    }
}

/// The context passed to extensions. 
#[derive(Clone)]
pub struct IdeContext {
    pub delegate: Arc<dyn IdeDelegate>,
}

impl IdeContext {
    pub fn new(delegate: Arc<dyn IdeDelegate>) -> Self {
        Self { delegate }
    }

    pub fn log(&self, msg: &str) {
        self.delegate.log(msg);
    }
}

impl Default for IdeContext {
    fn default() -> Self {
        Self::new(Arc::new(DummyDelegate))
    }
}

/// A unified trait for both WASM and JS extensions.
#[async_trait::async_trait]
pub trait Extension: Send + Sync {
    /// Unique identifier for this extension
    fn id(&self) -> &str;
    
    /// Get the commands this extension contributes
    fn commands(&self) -> Vec<String>;
    
    /// Called when the extension is activated
    async fn activate(&mut self, context: IdeContext) -> Result<(), String>;
    
    /// Execute a registered command
    async fn execute_command(&mut self, command: &str, args: Value) -> Result<Value, String>;
    
    /// Called when the extension is deactivated
    async fn deactivate(&mut self) -> Result<(), String>;
}

/// Manages multiple loaded extensions
pub struct ExtensionManager {
    extensions: Mutex<Vec<Box<dyn Extension>>>,
    command_registry: Mutex<HashMap<String, usize>>,
    context: IdeContext,
}

impl ExtensionManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            extensions: Mutex::new(Vec::new()),
            command_registry: Mutex::new(HashMap::new()),
            context: IdeContext::default(),
        })
    }

    pub fn with_delegate(delegate: Arc<dyn IdeDelegate>) -> Arc<Self> {
        Arc::new(Self {
            extensions: Mutex::new(Vec::new()),
            command_registry: Mutex::new(HashMap::new()),
            context: IdeContext::new(delegate),
        })
    }

    pub async fn load_extension(&self, mut ext: Box<dyn Extension>) -> Result<(), String> {
        ext.activate(self.context.clone()).await?;
        
        let mut exts = self.extensions.lock().await;
        let mut registry = self.command_registry.lock().await;
        
        let idx = exts.len();
        for cmd in ext.commands() {
            registry.insert(cmd, idx);
        }
        
        exts.push(ext);
        Ok(())
    }

    pub async fn execute_command(&self, command: &str, args: Value) -> Result<Value, String> {
        let registry = self.command_registry.lock().await;
        let ext_idx = registry.get(command).copied();
        drop(registry); // Release registry lock before taking extension lock

        if let Some(idx) = ext_idx {
            let mut exts = self.extensions.lock().await;
            if let Some(ext) = exts.get_mut(idx) {
                return ext.execute_command(command, args).await;
            }
        }
        Err(format!("Command {} not handled by any extension", command))
    }

    pub async fn get_extensions(&self) -> Vec<String> {
        let exts = self.extensions.lock().await;
        exts.iter().map(|e| e.id().to_string()).collect()
    }
}
