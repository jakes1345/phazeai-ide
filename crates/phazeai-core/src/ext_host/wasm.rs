use super::{Extension, IdeContext};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;
use wasmtime::*;

pub struct WasmExtension {
    id: String,
    commands: Vec<String>,
    #[allow(dead_code)]
    engine: Engine,
    store: Arc<Mutex<Store<IdeContext>>>,
    instance: Instance,
}

impl WasmExtension {
    pub fn new(id: impl Into<String>, commands: Vec<String>, wasm_bytes: &[u8]) -> Result<Self, String> {
        let engine = Engine::default();
        let mut store = Store::new(&engine, IdeContext::default());

        let module = Module::from_binary(&engine, wasm_bytes)
            .map_err(|e| format!("Invalid WASM: {}", e))?;

        let mut linker = Linker::new(&engine);
        
        // Define host function: phazeai.log(ptr, len)
        linker.func_wrap("phazeai", "log", |mut caller: Caller<'_, IdeContext>, ptr: u32, len: u32| {
            let mem = match caller.get_export("memory") {
                Some(Extern::Memory(mem)) => mem,
                _ => return,
            };
            
            let data = mem.data(&caller)
                .get(ptr as usize..(ptr + len) as usize)
                .unwrap_or(&[]);
                
            let msg = std::str::from_utf8(data).unwrap_or("invalid utf8").to_string();
                
            let ctx = caller.data();
            ctx.log(&msg);
        }).map_err(|e| format!("Linker error: {}", e))?;

        let instance = linker.instantiate(&mut store, &module)
            .map_err(|e| format!("Instantiation error: {}", e))?;

        Ok(Self {
            id: id.into(),
            commands,
            engine,
            store: Arc::new(Mutex::new(store)),
            instance,
        })
    }
}

#[async_trait::async_trait]
impl Extension for WasmExtension {
    fn id(&self) -> &str {
        &self.id
    }

    fn commands(&self) -> Vec<String> {
        self.commands.clone()
    }

    async fn activate(&mut self, context: IdeContext) -> Result<(), String> {
        let mut store = self.store.lock().await;
        *store.data_mut() = context;
        
        // Call the `activate` export if it exists
        if let Some(activate_fn) = self.instance.get_typed_func::<(), ()>(&mut *store, "activate").ok() {
            activate_fn.call(&mut *store, ()).map_err(|e| format!("WASM activate error: {}", e))?;
        }
        Ok(())
    }

    async fn execute_command(&mut self, _command: &str, _args: Value) -> Result<Value, String> {
        let mut store = self.store.lock().await;
        // In a real implementation we'd pass strings via memory.
        if let Some(cmd_fn) = self.instance.get_typed_func::<(), ()>(&mut *store, "execute_command").ok() {
            cmd_fn.call(&mut *store, ()).map_err(|e| format!("WASM command error: {}", e))?;
            return Ok(Value::Null);
        }
        Err("Command not exported".to_string())
    }

    async fn deactivate(&mut self) -> Result<(), String> {
        Ok(())
    }
}
