//! The curated list of models PhazeAI can download and run itself.
//!
//! Every entry is pinned to a Hugging Face commit, with the exact file size and
//! SHA-256 taken from the Hugging Face API (`/api/models/<repo>/tree/<rev>`,
//! field `lfs.oid`) and the license taken from the model card. Downloads that do
//! not match are rejected, so a changed upstream file can never be run silently.
//!
//! Only models under permissive licenses (Apache-2.0 / MIT) are listed.
//! Deliberately absent: Qwen2.5-Coder-3B (Qwen Research License, non-commercial),
//! Google/Gemma models (maintainer's call) and "uncensored"/abliterated variants.

use super::hardware::{HardwareInfo, GIB};
use serde::Serialize;

/// llama.cpp scratch buffers on top of weights + KV cache (measured ~0.3 GB for
/// 7B at 16k context; rounded up so estimates err on the safe side).
pub const COMPUTE_OVERHEAD: u64 = 512 * 1024 * 1024;
/// RAM left for the OS, the IDE itself and language servers when deciding that
/// a model *fits comfortably*. Each entry's `min_ram` is at least
/// `runtime_bytes + OS_IDE_RESERVE` (enforced by a test).
pub const OS_IDE_RESERVE: u64 = 4 * GIB;
/// Below `runtime_bytes + MIN_SYSTEM_RESERVE` of *installed* RAM the model can't
/// run without swapping the whole machine, so it is "too big".
pub const MIN_SYSTEM_RESERVE: u64 = 2 * GIB;
/// Largest extra free disk demanded beyond the file itself.
pub const DISK_MARGIN: u64 = 512 * 1024 * 1024;

/// Free space to keep beyond a download of `size` bytes: a tenth of the file,
/// between 1 MiB and [`DISK_MARGIN`], so the disk is never filled to the brim.
pub fn disk_margin(size: u64) -> u64 {
    (size / 10).clamp(1024 * 1024, DISK_MARGIN)
}

/// Context size used when no model is selected yet; every catalog entry
/// currently uses it too.
pub const DEFAULT_CTX_SIZE: u32 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogEntry {
    /// Stable id stored in config (`[local_ai] model`) and sent as the model name.
    pub id: &'static str,
    pub display_name: &'static str,
    /// What it's good at, for the UI.
    pub good_at: &'static str,
    /// Hugging Face repo.
    pub repo: &'static str,
    /// Commit the size and hash were taken from; the download URL pins it.
    pub revision: &'static str,
    /// GGUF file name inside the repo.
    pub file: &'static str,
    /// Exact file size in bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: &'static str,
    /// SPDX license id from the model card.
    pub license: &'static str,
    /// Installed RAM at which this model is recommended (its tier).
    pub min_ram: u64,
    /// Context window PhazeAI runs it with unless `[local_ai] ctx_size` overrides.
    pub ctx_size: u32,
    /// f16 KV-cache bytes per token of context:
    /// `2 (K,V) × attention layers × kv_heads × head_dim × 2 bytes`, from the
    /// model's config.json (hybrid models: only the full-attention layers).
    pub kv_bytes_per_token: u64,
    /// Runtime requirement worth telling the user about, if any.
    pub runtime_note: Option<&'static str>,
}

impl CatalogEntry {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.file
        )
    }

    /// Estimated resident memory while serving with `ctx` tokens of context.
    pub fn runtime_bytes(&self, ctx: u32) -> u64 {
        self.size + self.kv_bytes_per_token * ctx as u64 + COMPUTE_OVERHEAD
    }
}

/// Ordered from weakest to strongest (and smallest to largest download);
/// `recommend` picks the last entry whose RAM tier the machine meets.
pub static CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "qwen2.5-coder-1.5b-instruct-q4_k_m",
        display_name: "Qwen2.5-Coder 1.5B Instruct",
        good_at: "Coding model, 1.5B parameters (dense). Fastest option and fits 8 GB \
                  machines. Fine for questions and small edits; in PhazeAI's tests it \
                  wrote tool calls as plain text, so agent actions are unreliable.",
        repo: "Qwen/Qwen2.5-Coder-1.5B-Instruct-GGUF",
        revision: "f86cb2c1fa58255f8052cc32aeede1b7482d4361",
        file: "qwen2.5-coder-1.5b-instruct-q4_k_m.gguf",
        size: 1_117_320_768,
        sha256: "cc324af070c2ecbfd324a30884d2f951a7ff756aba85cb811a6ec436933bb046",
        license: "Apache-2.0",
        min_ram: 6 * GIB,
        ctx_size: 16_384,
        // 28 layers × 2 KV heads × 128 head_dim
        kv_bytes_per_token: 2 * 28 * 2 * 128 * 2,
        runtime_note: None,
    },
    CatalogEntry {
        id: "qwen2.5-coder-7b-instruct-q4_k_m",
        display_name: "Qwen2.5-Coder 7B Instruct",
        good_at: "Coding model, 7B parameters (dense). Moderate speed on CPU. Answers \
                  directly without a thinking phase; loads in any llama.cpp from 2024 on.",
        repo: "Qwen/Qwen2.5-Coder-7B-Instruct-GGUF",
        revision: "13fb94bfda8c8cf22497dc57b78f391a9acb426a",
        file: "qwen2.5-coder-7b-instruct-q4_k_m.gguf",
        size: 4_683_073_536,
        sha256: "509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c",
        license: "Apache-2.0",
        min_ram: 14 * GIB,
        ctx_size: 16_384,
        // 28 layers × 4 KV heads × 128 head_dim
        kv_bytes_per_token: 2 * 28 * 4 * 128 * 2,
        runtime_note: None,
    },
    CatalogEntry {
        id: "qwen3.5-9b-q4_k_m",
        display_name: "Qwen3.5 9B",
        good_at: "General model with coding, 9B parameters (dense, hybrid attention), \
                  2026. A little slower than the 7B, and it thinks before answering \
                  (Qwen's default), so replies start later.",
        repo: "lmstudio-community/Qwen3.5-9B-GGUF",
        revision: "1379f25c6b505a3fc737bd7818cb09389cf807c1",
        file: "Qwen3.5-9B-Q4_K_M.gguf",
        size: 5_627_044_256,
        sha256: "cd76ec205963b3b33350093e6904d9de16c4e666fd104e1f632d25c7f15f2a13",
        license: "Apache-2.0",
        min_ram: 14 * GIB,
        ctx_size: 16_384,
        // Hybrid: 8 of 32 layers use full attention (4 KV heads × 256 head_dim);
        // the 24 linear-attention layers keep a fixed ~2 MiB state each, inside
        // COMPUTE_OVERHEAD.
        kv_bytes_per_token: 2 * 8 * 4 * 256 * 2,
        runtime_note: Some(QWEN35_NOTE),
    },
    CatalogEntry {
        id: "gpt-oss-20b-mxfp4",
        display_name: "gpt-oss 20B (OpenAI)",
        good_at: "OpenAI open-weight model, 21B parameters, mixture-of-experts with \
                  about 3.6B active per token, so it generates roughly as fast as a 4B \
                  dense model. Needs about 13 GB of RAM for weights and cache.",
        repo: "ggml-org/gpt-oss-20b-GGUF",
        revision: "ef9b12f2ff56c69cf32153a02784e7a3c88bf524",
        file: "gpt-oss-20b-MXFP4.gguf",
        size: 12_109_566_624,
        sha256: "27cd6c432c7672cb812a92f611cf3ba7bbc35928262bb1e1253ff4ee6ae35901",
        license: "Apache-2.0",
        min_ram: 22 * GIB,
        ctx_size: 16_384,
        // 24 layers × 8 KV heads × 64 head_dim. Half of them are 128-token
        // sliding-window layers that llama.cpp caches far less of; counting all
        // 24 keeps the estimate on the safe side.
        kv_bytes_per_token: 2 * 24 * 8 * 64 * 2,
        runtime_note: Some(
            "Uses the harmony chat template (llama-server --jinja, which PhazeAI passes) \
             and needs a llama.cpp release from August 2025 or later.",
        ),
    },
    CatalogEntry {
        id: "qwen3.5-35b-a3b-q4_k_m",
        display_name: "Qwen3.5 35B-A3B",
        good_at: "General model with coding, 35B parameters, mixture-of-experts with \
                  about 3B active per token: generates at small-model speed but needs \
                  about 21 GB of RAM for its weights. Thinks before answering.",
        repo: "lmstudio-community/Qwen3.5-35B-A3B-GGUF",
        revision: "529ae8d8705097e646043ff81181c6564b6a8449",
        file: "Qwen3.5-35B-A3B-Q4_K_M.gguf",
        size: 21_169_116_992,
        sha256: "f25d609171b8f80950a60f38696597f74025de4070682d2fb1eeffe306ca7d5d",
        license: "Apache-2.0",
        min_ram: 28 * GIB,
        ctx_size: 16_384,
        // Hybrid: 10 of 40 layers use full attention (2 KV heads × 256 head_dim).
        kv_bytes_per_token: 2 * 10 * 2 * 256 * 2,
        runtime_note: Some(QWEN35_NOTE),
    },
];

const QWEN35_NOTE: &str = "Needs a recent llama.cpp with Qwen3.5 support (2026); older \
     packaged llama-server builds fail to load it. PhazeAI's own install fetches the \
     latest release.";

pub fn find(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// How well an entry suits a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Fit {
    /// Installed RAM meets the entry's tier and enough RAM is free right now.
    Fits,
    /// It can run, but the machine is below the tier or currently short on free
    /// RAM: expect slowness or swapping until other programs are closed.
    Tight,
    /// Installed RAM can't hold it plus a minimal OS reserve.
    TooBig,
}

impl Fit {
    pub fn label(self) -> &'static str {
        match self {
            Fit::Fits => "Fits",
            Fit::Tight => "Tight",
            Fit::TooBig => "Too big",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FitReport {
    pub fit: Fit,
    /// Estimated RAM while serving, in bytes.
    pub need: u64,
    /// Whether there is room on disk for the file (None: unknown free space).
    pub disk_ok: Option<bool>,
    /// Plain-language explanation for the UI.
    pub reason: String,
}

/// Fit verdict for `entry` on `hw` at context `ctx`.
///
/// Rule: *Too big* when installed RAM < need + 2 GiB; *Fits* when installed RAM
/// ≥ the entry's tier (`min_ram` ≥ need + 4 GiB for OS + IDE) **and** the RAM
/// available right now ≥ need; otherwise *Tight*.
pub fn fit(entry: &CatalogEntry, hw: &HardwareInfo, ctx: u32) -> FitReport {
    use super::hardware::gib;
    let need = entry.runtime_bytes(ctx);
    let disk_ok = hw
        .free_disk
        .map(|d| d >= entry.size + disk_margin(entry.size));
    let (fit, reason) = if hw.total_ram < need + MIN_SYSTEM_RESERVE {
        (
            Fit::TooBig,
            format!(
                "Needs about {} of RAM; this machine has {}.",
                gib(need + MIN_SYSTEM_RESERVE),
                gib(hw.total_ram)
            ),
        )
    } else if hw.total_ram >= entry.min_ram && hw.available_ram >= need {
        (
            Fit::Fits,
            format!("Uses about {} of RAM while running.", gib(need)),
        )
    } else if hw.total_ram < entry.min_ram {
        (
            Fit::Tight,
            format!(
                "Recommended for {}+ of RAM; this machine has {}. It will run, but leaves little room for other apps.",
                gib(entry.min_ram),
                gib(hw.total_ram)
            ),
        )
    } else {
        (
            Fit::Tight,
            format!(
                "Needs about {} free; {} is free right now. Close some programs first.",
                gib(need),
                gib(hw.available_ram)
            ),
        )
    };
    FitReport {
        fit,
        need,
        disk_ok,
        reason,
    }
}

/// The best model for this machine: the strongest entry (latest in
/// [`CATALOG`]) whose RAM tier the machine meets — installed RAM ≥ `min_ram`,
/// which already includes 4 GiB of headroom for the OS + IDE. Current free RAM is deliberately
/// ignored — it changes minute to minute — and shown via [`fit`] instead. If
/// the machine is below every tier, the smallest entry that is not too big;
/// `None` if even that can't run.
pub fn recommend(hw: &HardwareInfo) -> Option<&'static CatalogEntry> {
    CATALOG
        .iter()
        .rev()
        .find(|e| hw.total_ram >= e.min_ram)
        .or_else(|| {
            CATALOG
                .iter()
                .find(|e| fit(e, hw, e.ctx_size).fit != Fit::TooBig)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(total_gib: f64, avail_gib: f64) -> HardwareInfo {
        HardwareInfo {
            total_ram: (total_gib * GIB as f64) as u64,
            available_ram: (avail_gib * GIB as f64) as u64,
            physical_cores: Some(4),
            logical_cores: 8,
            free_disk: Some(100 * GIB),
            os: "linux".into(),
            arch: "x86_64".into(),
        }
    }

    #[test]
    fn catalog_entries_are_complete_and_pinned() {
        let mut ids = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(ids.insert(e.id), "duplicate id {}", e.id);
            assert_eq!(e.sha256.len(), 64, "{}", e.id);
            assert!(e
                .sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            assert_eq!(
                e.revision.len(),
                40,
                "{}: revision must be a full commit",
                e.id
            );
            assert!(e.size > 100_000_000, "{}", e.id);
            assert!(e.file.ends_with(".gguf"));
            assert!(matches!(e.license, "Apache-2.0" | "MIT"), "{}", e.license);
            assert!(e.ctx_size >= 4096);
            assert!(e.url().starts_with("https://huggingface.co/") && e.url().contains(e.revision));
            // The tier leaves room for the OS + IDE on top of the model.
            assert!(
                e.min_ram >= e.runtime_bytes(e.ctx_size) + OS_IDE_RESERVE,
                "{}: min_ram too low",
                e.id
            );
        }
    }

    #[test]
    fn catalog_is_sorted_smallest_first() {
        assert!(CATALOG
            .windows(2)
            .all(|w| w[0].size < w[1].size && w[0].min_ram <= w[1].min_ram));
    }

    #[test]
    fn excluded_models_are_absent() {
        for e in CATALOG {
            let s = format!("{} {} {}", e.repo, e.file, e.id).to_lowercase();
            assert!(!s.contains("gemma") && !s.contains("google"), "{s}");
            assert!(!s.contains("coder-3b"), "{s}");
            for bad in ["uncensored", "heretic", "abliterated"] {
                assert!(!s.contains(bad), "{s}");
            }
        }
    }

    #[test]
    fn verified_entries_are_pinned_exactly() {
        let expect = [
            (
                "qwen2.5-coder-1.5b-instruct-q4_k_m",
                1_117_320_768u64,
                "cc324af0",
            ),
            (
                "qwen2.5-coder-7b-instruct-q4_k_m",
                4_683_073_536,
                "509287f7",
            ),
            ("qwen3.5-9b-q4_k_m", 5_627_044_256, "cd76ec20"),
            ("gpt-oss-20b-mxfp4", 12_109_566_624, "27cd6c43"),
            ("qwen3.5-35b-a3b-q4_k_m", 21_169_116_992, "f25d6091"),
        ];
        assert_eq!(CATALOG.len(), expect.len());
        for (id, size, sha_prefix) in expect {
            let e = find(id).unwrap_or_else(|| panic!("{id} missing"));
            assert_eq!(e.size, size, "{id}");
            assert!(e.sha256.starts_with(sha_prefix), "{id}");
        }
        // Newer architectures say what runtime they need.
        for id in [
            "qwen3.5-9b-q4_k_m",
            "gpt-oss-20b-mxfp4",
            "qwen3.5-35b-a3b-q4_k_m",
        ] {
            assert!(find(id).unwrap().runtime_note.is_some(), "{id}");
        }
    }

    #[test]
    fn recommend_picks_the_strongest_with_headroom() {
        let id = |h: &HardwareInfo| recommend(h).map(|e| e.id);
        // 8 GB laptops report ~7.6 GiB; 12 GB is below the 16 GB tier.
        assert_eq!(
            id(&hw(7.6, 3.0)),
            Some("qwen2.5-coder-1.5b-instruct-q4_k_m")
        );
        assert_eq!(
            id(&hw(11.7, 8.0)),
            Some("qwen2.5-coder-1.5b-instruct-q4_k_m")
        );
        // 16 GB machines report ~15.6 GiB: the 9B is the strongest that fits.
        assert_eq!(id(&hw(15.6, 9.0)), Some("qwen3.5-9b-q4_k_m"));
        // Free RAM does not change the tier choice.
        assert_eq!(id(&hw(15.6, 1.0)), Some("qwen3.5-9b-q4_k_m"));
        // 24 GB (~23.4 GiB reported).
        assert_eq!(id(&hw(23.4, 15.0)), Some("gpt-oss-20b-mxfp4"));
        // 32 GB and up.
        assert_eq!(id(&hw(31.8, 20.0)), Some("qwen3.5-35b-a3b-q4_k_m"));
        assert_eq!(id(&hw(128.0, 100.0)), Some("qwen3.5-35b-a3b-q4_k_m"));
        // 4 GB: below every tier but the 1.5B still runs (needs ~2 GiB + 2 GiB).
        assert_eq!(
            id(&hw(4.0, 2.5)),
            Some("qwen2.5-coder-1.5b-instruct-q4_k_m")
        );
        // 2 GB: nothing runs.
        assert_eq!(id(&hw(2.0, 1.0)), None);
    }

    #[test]
    fn fit_verdicts() {
        let small = find("qwen2.5-coder-1.5b-instruct-q4_k_m").unwrap();
        let mid = find("qwen2.5-coder-7b-instruct-q4_k_m").unwrap();
        let q9 = find("qwen3.5-9b-q4_k_m").unwrap();
        let oss = find("gpt-oss-20b-mxfp4").unwrap();
        let moe = find("qwen3.5-35b-a3b-q4_k_m").unwrap();
        let h16 = hw(15.6, 9.0);
        for e in [small, mid, q9] {
            assert_eq!(fit(e, &h16, e.ctx_size).fit, Fit::Fits, "{}", e.id);
        }
        // gpt-oss needs ~12.5 GiB: runs on 16 GB but below its 24 GB tier → Tight.
        let r = fit(oss, &h16, oss.ctx_size);
        assert_eq!(r.fit, Fit::Tight);
        assert!(r.reason.contains("Recommended for"), "{}", r.reason);
        assert_eq!(fit(oss, &hw(23.4, 16.0), oss.ctx_size).fit, Fit::Fits);
        // The 35B MoE (~20.5 GiB) can't run on 16 GB at all.
        let r = fit(moe, &h16, moe.ctx_size);
        assert_eq!(r.fit, Fit::TooBig);
        assert!(r.reason.contains("Needs about"));
        assert_eq!(fit(moe, &hw(23.4, 20.0), moe.ctx_size).fit, Fit::Tight);
        // Enough installed RAM but little free → Tight with a "close programs" hint.
        let busy = fit(mid, &hw(15.6, 2.0), mid.ctx_size);
        assert_eq!(busy.fit, Fit::Tight);
        assert!(busy.reason.contains("free right now"), "{}", busy.reason);
    }

    #[test]
    fn hybrid_and_moe_ram_estimates() {
        let need = |id| find(id).unwrap();
        let gib_need = |e: &CatalogEntry| e.runtime_bytes(e.ctx_size) as f64 / GIB as f64;
        assert!((6.0..6.5).contains(&gib_need(need("qwen3.5-9b-q4_k_m"))));
        assert!((12.0..13.0).contains(&gib_need(need("gpt-oss-20b-mxfp4"))));
        assert!((20.0..21.0).contains(&gib_need(need("qwen3.5-35b-a3b-q4_k_m"))));
    }

    #[test]
    fn disk_check_uses_file_size_plus_margin() {
        let mut h = hw(16.0, 10.0);
        let mid = &CATALOG[1];
        h.free_disk = Some(mid.size);
        assert_eq!(fit(mid, &h, mid.ctx_size).disk_ok, Some(false));
        h.free_disk = Some(mid.size + disk_margin(mid.size));
        assert_eq!(fit(mid, &h, mid.ctx_size).disk_ok, Some(true));
        h.free_disk = None;
        assert_eq!(fit(mid, &h, mid.ctx_size).disk_ok, None);
    }

    #[test]
    fn disk_margin_scales_with_size() {
        assert_eq!(disk_margin(1000), 1024 * 1024);
        assert_eq!(disk_margin(1_117_320_768), 111_732_076);
        assert_eq!(disk_margin(21_169_116_992), DISK_MARGIN);
    }

    #[test]
    fn bigger_context_needs_more_ram() {
        let mid = &CATALOG[1];
        assert!(mid.runtime_bytes(32_768) > mid.runtime_bytes(16_384));
        assert_eq!(
            mid.runtime_bytes(16_384) - mid.runtime_bytes(0),
            57_344 * 16_384
        );
    }
}
