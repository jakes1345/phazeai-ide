//! What the machine can run: RAM, CPU cores, free disk space, OS and arch.
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
        }
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
        format!(
            "{} RAM ({} free) · {cores} · {disk} · {}/{}",
            gib(self.total_ram),
            gib(self.available_ram),
            self.os,
            self.arch
        )
    }
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
