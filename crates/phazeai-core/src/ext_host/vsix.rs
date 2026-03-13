use super::ExtensionManager;
use crate::ext_host::js::JsExtension;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Debug)]
pub struct VSCodePackageJson {
    pub name: String,
    pub main: Option<String>,
    #[serde(default)]
    pub contributes: VSCodeContributes,
}

#[derive(Deserialize, Debug, Default)]
pub struct VSCodeContributes {
    #[serde(default)]
    pub commands: Vec<VSCodeCommand>,
}

#[derive(Deserialize, Debug)]
pub struct VSCodeCommand {
    pub command: String,
    pub title: String,
}

pub struct VsixLoader;

impl VsixLoader {
    /// Loads a VSCode extension from an unpacked directory
    pub async fn load_from_dir(dir: &Path, manager: &ExtensionManager) -> Result<(), String> {
        let pkg_path = dir.join("package.json");
        if !pkg_path.exists() {
            return Err("No package.json found".into());
        }

        let pkg_str = fs::read_to_string(&pkg_path)
            .map_err(|e| format!("Failed to read package.json: {}", e))?;

        let pkg: VSCodePackageJson =
            serde_json::from_str(&pkg_str).map_err(|e| format!("Invalid package.json: {}", e))?;

        let main_file = pkg.main.unwrap_or_else(|| "index.js".to_string());
        let main_path = dir.join(main_file);

        if !main_path.exists() {
            return Err(format!("Main file {} not found", main_path.display()));
        }

        let js_code = fs::read_to_string(&main_path)
            .map_err(|e| format!("Failed to read main file: {}", e))?;

        // Extract commands and register them
        let mut js_wrapper = String::new();

        // Add CommonJS requires shim (since we are injecting directly into V8)
        js_wrapper.push_str(
            r#"
            const module = { exports: {} };
            const exports = module.exports;
            function require(mod) {
                if (mod === 'vscode') return globalThis.vscode;
                return {};
            }
        "#,
        );

        js_wrapper.push_str(&js_code);

        // Add activation call
        js_wrapper.push_str(
            r#"
            if (typeof module.exports.activate === 'function') {
                module.exports.activate({
                    subscriptions: [],
                    extensionPath: ""
                });
            }
        "#,
        );

        let commands: Vec<String> = pkg
            .contributes
            .commands
            .into_iter()
            .map(|c| c.command)
            .collect();

        let ext = JsExtension::new(&pkg.name, commands, js_wrapper)?;
        manager.load_extension(Box::new(ext)).await?;

        Ok(())
    }

    /// Extracts a .vsix file to a temp directory and loads it
    pub async fn load_vsix(vsix_path: &Path, manager: &ExtensionManager) -> Result<(), String> {
        let file = fs::File::open(vsix_path).map_err(|e| format!("Failed to open vsix: {}", e))?;

        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("Invalid zip archive: {}", e))?;

        // Use persistent extension dir so JS runtime can still reference sibling files at runtime
        let ext_store = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("phazeai")
            .join("extensions")
            .join(format!("ext-{}", uuid::Uuid::new_v4()));
        let temp_dir = ext_store;
        fs::create_dir_all(&temp_dir)
            .map_err(|e| format!("Failed to create extension dir: {}", e))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            let outpath = match file.enclosed_name() {
                Some(path) => temp_dir.join(path),
                None => continue,
            };

            if file.name().ends_with('/') {
                fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        fs::create_dir_all(p).map_err(|e| e.to_string())?;
                    }
                }
                let mut outfile = fs::File::create(&outpath).map_err(|e| e.to_string())?;
                std::io::copy(&mut file, &mut outfile).map_err(|e| e.to_string())?;
            }
        }

        // VSCode extensions in vsix are typically inside an "extension" subdirectory
        let ext_dir = temp_dir.join("extension");
        let target_dir = if ext_dir.exists() {
            ext_dir
        } else {
            temp_dir.clone()
        };

        // No cleanup — extension dir is persistent so JS runtime can reference sibling files
        Self::load_from_dir(&target_dir, manager).await
    }
}
