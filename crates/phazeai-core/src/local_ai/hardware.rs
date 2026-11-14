//! What the machine can run: RAM, CPU cores, free disk space, OS and arch,
//! and the GPUs llama.cpp can use ([`list_devices`]).
//!
//! Backed by `sysinfo`, which reads the native APIs on Windows
//! (`GlobalMemoryStatusEx`, `GetDiskFreeSpaceExW`), macOS (`host_statistics64`,
//! `statfs`) and Linux (`/proc/meminfo`, `statvfs`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A snapshot of the machine's resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareInfo {
    /// Installed RAM in bytes.
    pub total_ram: u64,
    /// RAM the OS reports as available for new allocations right now, in bytes.
    pub available_ram: u64,
    /// Physical cores, when the OS reports them (llama.cpp's thread sweet spot).
    pub physical_cores: Option<usize>,
    /// Logical CPUs (hardware threads).
    pub logical_cores: usize,
    /// Free bytes on the volume that holds the models folder, if it could be found.
    pub free_disk: Option<u64>,
    /// `std::env::consts::OS`: "windows", "macos", "linux", ...
    pub os: String,
    /// `std::env::consts::ARCH`: "x86_64", "aarch64", ...
    pub arch: String,
    /// GPUs the installed `llama-server` can use (empty when none, or before
    /// one is installed). Filled by [`HardwareInfo::with_gpus`].
    #[serde(default)]
    pub gpus: Vec<GpuDevice>,
}

/// A GPU as `llama-server --list-devices` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuDevice {
    /// llama.cpp's device name: `Vulkan0`, `Metal`, `CUDA0`.
    pub name: String,
    /// The driver's description: `AMD Radeon Graphics (RADV GFX1200)`.
    pub description: String,
    /// Video memory in bytes.
    pub vram_total: u64,
    /// Video memory free when listed, in bytes.
    pub vram_free: u64,
}

impl HardwareInfo {
    /// Probe the current machine. `models_dir` need not exist yet: free space is
    /// measured on the volume of its nearest existing ancestor.
    pub fn probe(models_dir: &Path) -> Self {
        use sysinfo::{MemoryRefreshKind, RefreshKind, System};
        let sys = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        Self {
            total_ram: sys.total_memory(),
            available_ram: sys.available_memory(),
            physical_cores: System::physical_core_count(),
            logical_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            free_disk: free_disk_space(models_dir),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            gpus: Vec::new(),
        }
    }

    /// The same snapshot with the GPUs `server` lists (see [`list_devices`]);
    /// unchanged when listing fails, so a broken server never hides RAM data.
    pub fn with_gpus(mut self, server: &Path) -> Self {
        if let Ok(gpus) = list_devices(server) {
            self.gpus = gpus;
        }
        self
    }

    /// The GPU with the most video memory: the one a model is sized for.
    pub fn best_gpu(&self) -> Option<&GpuDevice> {
        self.gpus.iter().max_by_key(|g| g.vram_total)
    }

    /// One line for the UI, e.g. `16.0 GiB RAM (9.2 GiB free) · 8 cores · 120.4 GB free disk · windows/x86_64`.
    pub fn summary(&self) -> String {
        let cores = match self.physical_cores {
            Some(p) if p != self.logical_cores => {
                format!("{p} cores / {} threads", self.logical_cores)
            }
            Some(p) => format!("{p} cores"),
            None => format!("{} threads", self.logical_cores),
        };
        let disk = self
            .free_disk
            .map(|d| format!("{} free disk", super::human_bytes(d)))
            .unwrap_or_else(|| "free disk unknown".into());
        let gpu = match self.best_gpu() {
            Some(g) => format!(" · GPU: {} ({})", g.description, gib(g.vram_total)),
            None => String::new(),
        };
        format!(
            "{} RAM ({} free) · {cores} · {disk} · {}/{}{gpu}",
            gib(self.total_ram),
            gib(self.available_ram),
            self.os,
            self.arch
        )
    }
}

/// How long `llama-server --list-devices` may take before it is killed.
const LIST_DEVICES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The GPUs `server` (a `llama-server` binary) can use, from
/// `llama-server --list-devices`. Empty for a CPU-only build or a machine with
/// no usable GPU. Blocks for up to 20 s (the first Vulkan run may compile
/// shaders); call it off the UI thread.
pub fn list_devices(server: &Path) -> Result<Vec<GpuDevice>, String> {
    use std::io::Read;
    let mut cmd = crate::process::std_command(server);
    cmd.arg("--list-devices")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    // PhazeAI-installed builds load their backend libraries from beside the binary.
    if let Some(dir) = server.parent() {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    // Read on a thread so a chatty child can't fill the pipe and stall.
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });
    let deadline = std::time::Instant::now() + LIST_DEVICES_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("llama-server --list-devices timed out".into());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("llama-server --list-devices exited with {status}"));
    }
    Ok(parse_devices(&out))
}

/// Parse `llama-server --list-devices` output (llama.cpp's
/// `common_print_available_devices`):
///
/// ```text
/// Available devices:
///   Vulkan0: AMD Radeon Graphics (RADV GFX1200) (16304 MiB, 15800 MiB free)
/// ```
///
/// The description may contain parentheses itself, so the memory figures are
/// read from the last `(`. `(none)`, unparsable lines and software
/// rasterizers (`llvmpipe`, `lavapipe`, `SwiftShader`, which report system RAM
/// as video memory) give nothing.
pub fn parse_devices(out: &str) -> Vec<GpuDevice> {
    const SOFTWARE: [&str; 3] = ["llvmpipe", "lavapipe", "swiftshader"];
    const MIB: u64 = 1024 * 1024;
    out.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (name, rest) = line.split_once(": ")?;
            if name.is_empty() || name.contains(' ') {
                return None;
            }
            let open = rest.rfind(" (")?;
            let (description, mem) = (&rest[..open], &rest[open + 2..]);
            let mem = mem.strip_suffix(" MiB free)")?;
            let (total, free) = mem.split_once(" MiB, ")?;
            let lower = description.to_ascii_lowercase();
            if SOFTWARE.iter().any(|s| lower.contains(s)) {
                return None;
            }
            Some(GpuDevice {
                name: name.to_string(),
                description: description.trim().to_string(),
                vram_total: total.trim().parse::<u64>().ok()? * MIB,
                vram_free: free.trim().parse::<u64>().ok()? * MIB,
            })
        })
        .collect()
}

/// `16.0 GiB` — RAM is shown in binary units because that is what OSes report.
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB as f64)
}

pub const GIB: u64 = 1024 * 1024 * 1024;

/// Free bytes on the volume containing `path` (or its nearest existing ancestor).
///
/// On Unix this asks about that one path (`statvfs`), never about other
/// mounts: listing every disk queries each mount point, and one slow or hung
/// mount (a sleeping USB drive, an unreachable network share) blocks the
/// caller indefinitely. The download runs this before its first request, so
/// such a mount left it at "Connecting…" forever, with no error.
pub fn free_disk_space(path: &Path) -> Option<u64> {
    let target = existing_ancestor(path)?;
    #[cfg(unix)]
    {
        statvfs_available(&target)
    }
    #[cfg(not(unix))]
    {
        free_disk_space_listed(&target)
    }
}

/// Bytes an unprivileged user may still write on the volume holding `path`
/// (`f_bavail` blocks of `f_frsize` bytes, as `df` reports "Avail").
#[cfg(unix)]
fn statvfs_available(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` a
    // writable statvfs that the call fills in; it is read only on success.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)] // the field types differ between platforms
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

/// The disk-listing lookup, for platforms without `statvfs`.
#[cfg(not(unix))]
fn free_disk_space_listed(target: &Path) -> Option<u64> {
    let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let target = strip_verbatim(&target);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| target.starts_with(strip_verbatim(d.mount_point())))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

fn existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut cur = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    loop {
        if cur.exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// `canonicalize` on Windows returns `\\?\C:\...`, while disk mount points are
/// plain `C:\`. Drop the verbatim prefix so prefix matching works.
#[cfg_attr(unix, allow(dead_code))] // used by the disk listing, which Unix no longer needs
fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_list_devices_output() {
        let out = "Available devices:\n  \
                   Vulkan0: AMD Radeon Graphics (RADV GFX1200) (16304 MiB, 15800 MiB free)\n  \
                   Vulkan1: llvmpipe (LLVM 20.1.2, 256 bits) (32016 MiB, 32016 MiB free)\n";
        let gpus = super::parse_devices(out);
        // llvmpipe is a software rasterizer, not a GPU.
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "Vulkan0");
        assert_eq!(gpus[0].description, "AMD Radeon Graphics (RADV GFX1200)");
        assert_eq!(gpus[0].vram_total, 16304 * 1024 * 1024);
        assert_eq!(gpus[0].vram_free, 15800 * 1024 * 1024);

        assert!(super::parse_devices("Available devices:\n  (none)\n").is_empty());
        assert!(super::parse_devices("").is_empty());
        assert!(super::parse_devices("  Metal: Apple M2 (no memory figures)\n").is_empty());
    }

    #[test]
    fn best_gpu_and_summary() {
        let mut hw = super::HardwareInfo::probe(std::path::Path::new("."));
        assert!(hw.best_gpu().is_none());
        assert!(!hw.summary().contains("GPU"));
        hw.gpus = super::parse_devices(
            "  Vulkan0: Small (2048 MiB, 2000 MiB free)\n  Vulkan1: Big (16304 MiB, 15800 MiB free)\n",
        );
        assert_eq!(hw.best_gpu().unwrap().description, "Big");
        assert!(
            hw.summary().ends_with(" · GPU: Big (15.9 GiB)"),
            "{}",
            hw.summary()
        );
    }

    /// A stand-in `llama-server` that prints a device list.
    #[cfg(unix)]
    #[test]
    fn list_devices_runs_the_binary() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("llama-server");
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1\" = --list-devices ] || exit 3\n\
             echo 'Available devices:'\necho '  Vulkan0: Test GPU (8192 MiB, 8000 MiB free)'\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let gpus = super::list_devices(&bin).unwrap();
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vram_total, 8192 * 1024 * 1024);

        std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(super::list_devices(&bin).is_err());
        // with_gpus keeps the snapshot when listing fails.
        let hw = super::HardwareInfo::probe(tmp.path()).with_gpus(&bin);
        assert!(hw.gpus.is_empty());
    }

    use super::*;

    #[test]
    fn probe_reports_plausible_values() {
        let dir = tempfile::tempdir().unwrap();
        let hw = HardwareInfo::probe(&dir.path().join("not/yet/created"));
        assert!(hw.total_ram > 256 * 1024 * 1024, "{hw:?}");
        assert!(hw.available_ram <= hw.total_ram, "{hw:?}");
        assert!(hw.logical_cores >= 1);
        if let Some(p) = hw.physical_cores {
            assert!(p >= 1 && p <= hw.logical_cores, "{hw:?}");
        }
        // The temp dir is on a real volume, so its free space must be known.
        assert!(hw.free_disk.is_some_and(|d| d > 0), "{hw:?}");
        assert_eq!(hw.os, std::env::consts::OS);
        assert!(hw.summary().contains("GiB RAM"));
    }

    /// The free space is the volume's own figure, as `df` reports it, for
    /// the folder asked about (and for a folder that does not exist yet).
    #[cfg(target_os = "linux")]
    #[test]
    fn free_space_matches_df_for_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        let out = std::process::Command::new("df")
            .args(["-B1", "--output=avail"])
            .arg(dir.path())
            .output()
            .expect("run df");
        let df: u64 = String::from_utf8_lossy(&out.stdout)
            .lines()
            .nth(1)
            .and_then(|l| l.trim().parse().ok())
            .expect("df prints Avail in bytes");
        for path in [dir.path().to_path_buf(), dir.path().join("models/not/yet")] {
            let ours = free_disk_space(&path).expect("free space of a real folder");
            // Other processes may write in between; allow 256 MiB of drift.
            assert!(
                ours.abs_diff(df) < 256 << 20,
                "{path:?}: ours {ours}, df {df}"
            );
        }
    }

    #[test]
    fn existing_ancestor_walks_up() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b/c");
        assert_eq!(existing_ancestor(&deep).unwrap(), dir.path());
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            strip_verbatim(Path::new("/home/me")),
            PathBuf::from("/home/me")
        );
    }
}
