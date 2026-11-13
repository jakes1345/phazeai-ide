//! `phazeai models`: find, download and pick the model PhazeAI Local runs.
//!
//! Any GGUF model on Hugging Face works, not just the built-in catalog:
//! `phazeai models pull bartowski/Qwen2.5-Coder-14B-Instruct-GGUF` picks the
//! best quantization this machine's RAM can run, downloads and verifies it,
//! and makes it the model PhazeAI Local serves.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use phazeai_core::config::{LlmProvider, Settings};
use phazeai_core::local_ai::{
    self, catalog, download,
    hub::{self, Hub, HubFile, HubListing},
    server, Fit, HardwareInfo,
};

#[derive(Args, Debug)]
pub struct ModelsArgs {
    #[command(subcommand)]
    pub action: ModelsAction,
}

#[derive(Subcommand, Debug)]
pub enum ModelsAction {
    /// Search Hugging Face for GGUF models, most downloaded first.
    Search {
        /// Words to search for, e.g. "qwen coder".
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// List a repository's quantizations, with whether each fits this machine.
    Files {
        /// A Hugging Face repository, e.g. bartowski/Qwen2.5-Coder-7B-Instruct-GGUF.
        repo: String,
    },
    /// Download a model and make it the one PhazeAI Local runs. Without
    /// --quant or --file, the best quantization that fits this machine.
    Pull {
        repo: String,
        /// The quantization to download, e.g. Q4_K_M.
        #[arg(long, conflicts_with = "file")]
        quant: Option<String>,
        /// The exact file in the repository (the first shard of a split model).
        #[arg(long)]
        file: Option<String>,
        /// Download even if it won't fit in this machine's RAM.
        #[arg(long)]
        force: bool,
        /// Download only; keep the current model selected.
        #[arg(long)]
        no_use: bool,
    },
    /// The models on disk, catalog and Hugging Face, and which one is selected.
    List,
    /// Make a downloaded model (catalog id or hf:... id) the one PhazeAI Local runs.
    Use { id: String },
    /// Delete a downloaded Hugging Face model.
    Rm { id: String },
}

pub async fn run(args: ModelsArgs) -> Result<()> {
    let models_dir = local_ai::models_dir();
    match args.action {
        ModelsAction::Search { query, limit } => {
            let hub = Hub::from_env()?;
            let found = hub::search(&hub, &query.join(" "), limit as usize).await?;
            if found.is_empty() {
                println!(
                    "No GGUF models on Hugging Face match \"{}\".",
                    query.join(" ")
                );
                return Ok(());
            }
            for r in found {
                println!("{:<60} {:>10} downloads", r.id, r.downloads);
            }
            println!("\nSee a model's sizes: phazeai models files <repo>");
        }
        ModelsAction::Files { repo } => {
            let hub = Hub::from_env()?;
            let listing = hub::listing(&hub, &repo).await?;
            let hw = HardwareInfo::probe(&models_dir);
            print!("{}", files_table(&listing, &hw));
        }
        ModelsAction::Pull {
            repo,
            quant,
            file,
            force,
            no_use,
        } => {
            let hub = Hub::from_env()?;
            let listing = hub::listing(&hub, &repo).await?;
            let hw = HardwareInfo::probe(&models_dir);
            let pick = pick_file(&listing, &hw, quant.as_deref(), file.as_deref())?;
            let ctx = hub::default_ctx(listing.context_length);
            let fit = hub::fit_file(&listing, pick, &hw, ctx);
            if fit.fit == Fit::TooBig && !force {
                bail!(
                    "{} is too big for this machine: {} Pick a smaller one (phazeai models files {repo}) or pass --force.",
                    pick.path,
                    fit.reason
                );
            }
            eprintln!(
                "Downloading {} ({}) from {repo} at {}: {}",
                pick.path,
                local_ai::human_bytes(pick.size()),
                &listing.revision[..listing.revision.len().min(12)],
                fit.reason
            );
            let cancel = Arc::new(AtomicBool::new(false));
            let stop = cancel.clone();
            let ctrl_c = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    stop.store(true, Ordering::SeqCst);
                }
            });
            let mut shown = u64::MAX;
            let result = hub::pull(&hub, &listing, pick, &models_dir, cancel, |p| {
                let pct = (p.fraction() * 100.0) as u64;
                if pct != shown {
                    shown = pct;
                    eprint!(
                        "\r  {pct:>3}%  {} of {}",
                        local_ai::human_bytes(p.downloaded),
                        local_ai::human_bytes(p.total)
                    );
                    let _ = std::io::stderr().flush();
                }
            })
            .await;
            ctrl_c.abort();
            eprintln!();
            let model = match result {
                Ok(m) => m,
                Err(local_ai::LocalAiError::Cancelled) => {
                    bail!("Stopped. Run the same command again to resume.")
                }
                Err(e) => return Err(e.into()),
            };
            println!(
                "{} downloaded and verified (SHA-256). It runs with {} tokens of context.",
                model.display_name, model.ctx_size
            );
            println!("Id: {}", model.id);
            if !no_use {
                select(&model.id)?;
            }
        }
        ModelsAction::List => {
            let settings = Settings::load();
            print!("{}", list_text(&settings, &models_dir));
        }
        ModelsAction::Use { id } => {
            let id = id.trim();
            if hub::parse_id(id).is_some() {
                if hub::find_installed(&models_dir, id).is_none() {
                    bail!("{id} isn't downloaded. See what is: phazeai models list");
                }
            } else {
                let entry = catalog::find(id).with_context(|| {
                    format!("No model \"{id}\". See what's downloaded: phazeai models list")
                })?;
                if !download::is_downloaded(entry, &models_dir) {
                    bail!(
                        "{} isn't downloaded. Download it in Settings → Local AI.",
                        entry.display_name
                    );
                }
            }
            select(id)?;
        }
        ModelsAction::Rm { id } => {
            let id = id.trim();
            let Some(model) = hub::find_installed(&models_dir, id) else {
                bail!(
                    "{id} isn't a downloaded Hugging Face model. Catalog models are removed in Settings → Local AI."
                );
            };
            hub::remove(&models_dir, &model)?;
            let mut settings = Settings::load();
            if settings.local_ai.model.as_deref() == Some(id) {
                settings.local_ai.model = None;
                settings.save()?;
            }
            println!(
                "Deleted {} ({} freed).",
                model.display_name,
                local_ai::human_bytes(model.size())
            );
        }
    }
    Ok(())
}

/// Make `id` the model PhazeAI Local runs, and say how to use it.
fn select(id: &str) -> Result<()> {
    let mut settings = Settings::load();
    settings.local_ai.model = Some(id.to_string());
    settings.save()?;
    if settings.llm.provider == LlmProvider::BuiltIn {
        println!("PhazeAI Local now runs it.");
    } else {
        println!("Selected for PhazeAI Local. Use it with: phazeai --provider builtin");
    }
    Ok(())
}

/// The file to pull: the one named, the quantization named, or the best fit.
fn pick_file<'a>(
    listing: &'a HubListing,
    hw: &HardwareInfo,
    quant: Option<&str>,
    file: Option<&str>,
) -> Result<&'a HubFile> {
    if listing.files.is_empty() {
        bail!("{} has no GGUF model files.", listing.repo);
    }
    let names = || {
        listing
            .files
            .iter()
            .map(|f| f.quant.clone().unwrap_or_else(|| f.path.clone()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(file) = file {
        return listing
            .files
            .iter()
            .find(|f| f.path == file || f.shards.iter().any(|s| s.path == file))
            .with_context(|| {
                format!(
                    "No GGUF file {file} in {}. It has: {}",
                    listing.repo,
                    names()
                )
            });
    }
    if let Some(q) = quant {
        let matches: Vec<&HubFile> = listing
            .files
            .iter()
            .filter(|f| {
                f.quant
                    .as_deref()
                    .is_some_and(|fq| fq.eq_ignore_ascii_case(q))
            })
            .collect();
        return match matches.as_slice() {
            [one] => Ok(one),
            [] => bail!("{} has no {q} file. It has: {}", listing.repo, names()),
            many => bail!(
                "{} has several {q} files; pick one with --file: {}",
                listing.repo,
                many.iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }
    hub::choose(listing, hw).with_context(|| {
        format!(
            "Every file in {} is too big for this machine ({} of RAM). Try a smaller model.",
            listing.repo,
            local_ai::hardware::gib(hw.total_ram)
        )
    })
}

/// A table of a repository's files: size, RAM while running, fit, and the
/// one `pull` would pick.
pub fn files_table(listing: &HubListing, hw: &HardwareInfo) -> String {
    let ctx = hub::default_ctx(listing.context_length);
    let pick = hub::choose(listing, hw).map(|f| f.path.clone());
    let mut out = format!(
        "{} at {} ({} tokens of context; this machine: {})\n",
        listing.repo,
        &listing.revision[..listing.revision.len().min(12)],
        ctx,
        hw.summary()
    );
    if let Some(l) = &listing.license {
        out.push_str(&format!("License: {l}\n"));
    }
    if listing.files.is_empty() {
        out.push_str("No GGUF model files.\n");
        return out;
    }
    out.push_str(&format!(
        "\n  {:<10} {:>10} {:>12}  {:<8} {}\n",
        "QUANT", "SIZE", "RAM", "FIT", "FILE"
    ));
    for f in &listing.files {
        let fit = hub::fit_file(listing, f, hw, ctx);
        let mark = if pick.as_deref() == Some(f.path.as_str()) {
            "*"
        } else {
            " "
        };
        let parts = if f.shards.len() > 1 {
            format!(" ({} parts)", f.shards.len())
        } else {
            String::new()
        };
        out.push_str(&format!(
            "{mark} {:<10} {:>10} {:>12}  {:<8} {}{parts}\n",
            f.quant.as_deref().unwrap_or("?"),
            local_ai::human_bytes(f.size()),
            local_ai::hardware::gib(fit.need),
            fit.fit.label(),
            f.path
        ));
    }
    match pick {
        Some(_) => out.push_str(&format!(
            "\n* is what `phazeai models pull {}` downloads here.\n",
            listing.repo
        )),
        None => out.push_str("\nNone of these fit this machine.\n"),
    }
    out
}

/// The downloaded models, with the selected one marked.
pub fn list_text(settings: &Settings, models_dir: &std::path::Path) -> String {
    let selected = server::selected_model(&settings.local_ai, models_dir)
        .ok()
        .map(|c| c.id);
    let mark = |id: &str| {
        if selected.as_deref() == Some(id) {
            "*"
        } else {
            " "
        }
    };
    let mut out = String::new();
    for e in catalog::CATALOG
        .iter()
        .filter(|e| download::is_downloaded(e, models_dir))
    {
        out.push_str(&format!(
            "{} {:<52} {:>10}  {}\n",
            mark(e.id),
            e.id,
            local_ai::human_bytes(e.size),
            e.display_name
        ));
    }
    for m in hub::installed(models_dir) {
        out.push_str(&format!(
            "{} {:<52} {:>10}  {}\n",
            mark(&m.id),
            m.id,
            local_ai::human_bytes(m.size()),
            m.display_name
        ));
    }
    if out.is_empty() {
        return "No models downloaded. Get one with: phazeai models pull <repo>  (find one: phazeai models search <words>)\n".into();
    }
    if selected.is_some() {
        out.push_str("\n* runs in PhazeAI Local.\n");
    }
    out
}
