//! Running llama.cpp's `llama-server` as a child process, and the
//! process-wide handle ([`LOCAL_AI`]) that the `BuiltIn` provider, the IDE and
//! the CLI share.
//!
//! The server listens only on 127.0.0.1, on a free port picked at start. It
//! speaks the OpenAI chat API, so chat goes through the existing
//! `OpenAIClient`.
//!
//! **Stopping.** `llama-server` is a single process (it starts no helpers), so
//! terminating the child — `TerminateProcess` on Windows, `SIGKILL` elsewhere —
//! frees everything; no job object / process-group kill is needed. The IDE and
//! CLI call [`LocalAi::stop`] on exit, and [`LlamaServer`]'s `Drop` kills the
//! child too. Because statics are never dropped and a crash skips both, a
//! pid file records `<server pid> <owner pid>`; the next start kills a leftover
//! server whose owner is gone (checked by name, so a recycled pid is never
//! touched).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::catalog::{self, CatalogEntry};
use super::download::{is_downloaded, model_path};
use super::install::{find_server_binary, manual_install_hint, FoundServer};
use super::LocalAiSettings;

/// How long to wait for the model to load before giving up.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);
/// Lines of server stderr kept for error messages.
const STDERR_TAIL: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStatus {
    Stopped,
    Starting { model_id: String },
    Running { port: u16, model_id: String },
    Failed(String),
}

impl ServerStatus {
    pub fn describe(&self) -> String {
        let name = |id: &str| {
            catalog::find(id)
                .map(|e| e.display_name.to_string())
                .unwrap_or_else(|| id.to_string())
        };
        match self {
            Self::Stopped => "Stopped".into(),
            Self::Starting { model_id } => format!("Starting — loading {}…", name(model_id)),
            Self::Running { port, model_id } => {
                format!("Running {} on 127.0.0.1:{port}", name(model_id))
            }
            Self::Failed(e) => format!("Error: {e}"),
        }
    }
}

/// Everything needed to launch one server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub binary: PathBuf,
    pub model_path: PathBuf,
    pub model_id: String,
    pub ctx_size: u32,
}

/// Command-line for `llama-server`: model, loopback host, port, context size,
/// the model's own chat template (`--jinja`, needed for tool calls) and an
/// alias so `/v1/models` reports our catalog id.
pub fn build_args(spec: &LaunchSpec, port: u16) -> Vec<OsString> {
    vec![
        "-m".into(),
        spec.model_path.clone().into_os_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string().into(),
        "-c".into(),
        spec.ctx_size.to_string().into(),
        "--jinja".into(),
        "--alias".into(),
        spec.model_id.clone().into(),
    ]
}

/// A free TCP port on 127.0.0.1 (bound, read, released).
pub fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// A running `llama-server` child.
pub struct LlamaServer {
    child: Child,
    port: u16,
    model_id: String,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    pid_file: Option<PathBuf>,
}

impl LlamaServer {
    /// Start the process (does not wait for it to be ready).
    pub fn spawn(spec: &LaunchSpec, port: u16, pid_file: Option<PathBuf>) -> std::io::Result<Self> {
        let mut cmd = crate::process::std_command(&spec.binary);
        cmd.args(build_args(spec, port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // Shared libraries of PhazeAI-installed builds sit next to the binary.
        if let Some(dir) = spec.binary.parent() {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn()?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            std::thread::Builder::new()
                .name("llama-server-stderr".into())
                .spawn(move || {
                    for line in std::io::BufReader::new(stderr).lines() {
                        let Ok(line) = line else { break };
                        let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                        if t.len() == STDERR_TAIL {
                            t.pop_front();
                        }
                        t.push_back(line);
                    }
                })?;
        }
        if let Some(pf) = &pid_file {
            if let Some(dir) = pf.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(pf, format!("{} {}", child.id(), std::process::id()));
        }
        Ok(Self {
            child,
            port,
            model_id: spec.model_id.clone(),
            stderr_tail,
            pid_file,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// The last lines the server wrote to stderr.
    pub fn stderr_tail(&self, n: usize) -> String {
        let t = self.stderr_tail.lock().unwrap_or_else(|e| e.into_inner());
        let skip = t.len().saturating_sub(n);
        t.iter().skip(skip).cloned().collect::<Vec<_>>().join("\n")
    }

    /// `Some(description)` if the process has exited.
    pub fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(e) => Some(e.to_string()),
        }
    }

    /// Poll `GET /health` until it answers 200 (`{"status":"ok"}`), the process
    /// dies, or `timeout` passes. Errors include the server's last stderr lines.
    pub async fn wait_healthy(&mut self, timeout: Duration) -> Result<(), String> {
        self.wait_healthy_while(timeout, || true).await
    }

    /// [`Self::wait_healthy`], giving up early once `keep_waiting` returns false.
    pub async fn wait_healthy_while(
        &mut self,
        timeout: Duration,
        keep_waiting: impl Fn() -> bool,
    ) -> Result<(), String> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| e.to_string())?;
        let url = format!("{}/health", self.base_url());
        let deadline = Instant::now() + timeout;
        loop {
            if !keep_waiting() {
                return Err("start was cancelled".into());
            }
            if let Some(status) = self.exited() {
                // Give the stderr reader a moment to drain the last lines.
                tokio::time::sleep(Duration::from_millis(100)).await;
                return Err(format!(
                    "llama-server exited ({status}) before it was ready.\n{}",
                    self.stderr_tail(12)
                ));
            }
            if let Ok(resp) = client.get(&url).send().await {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "llama-server did not become ready within {}s.\n{}",
                    timeout.as_secs(),
                    self.stderr_tail(12)
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Kill the process and wait for it to exit.
    pub fn stop(&mut self) {
        if self.exited().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(pf) = self.pid_file.take() {
            let _ = std::fs::remove_file(pf);
        }
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Kill a `llama-server` left behind by a PhazeAI process that crashed.
/// Only acts when the recorded owner is gone and the recorded pid is still a
/// process named `llama-server`.
pub fn reap_orphan(pid_file: &Path) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return;
    };
    let mut parts = text.split_whitespace().map(|s| s.parse::<u32>());
    let (Some(Ok(server)), Some(Ok(owner))) = (parts.next(), parts.next()) else {
        let _ = std::fs::remove_file(pid_file);
        return;
    };
    let mut sys = System::new();
    let pids = [Pid::from_u32(server), Pid::from_u32(owner)];
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        true,
        ProcessRefreshKind::nothing(),
    );
    let owner_alive = owner != std::process::id() && sys.process(pids[1]).is_some();
    if owner_alive {
        return; // another PhazeAI instance owns it
    }
    if let Some(p) = sys.process(pids[0]) {
        if p.name().to_string_lossy().contains("llama-server") {
            p.kill();
        }
    }
    let _ = std::fs::remove_file(pid_file);
}

/// Resolve which model and binary to run from `[local_ai]` settings.
pub fn resolve_launch(
    settings: &LocalAiSettings,
    models_dir: &Path,
    install_root: &Path,
    path_env: Option<&std::ffi::OsStr>,
) -> Result<LaunchSpec, String> {
    let entry = selected_model(settings, models_dir)?;
    if !is_downloaded(entry, models_dir) {
        return Err(format!(
            "{} isn't downloaded yet. Download it in Settings → Local AI.",
            entry.display_name
        ));
    }
    let found: Option<FoundServer> =
        find_server_binary(settings.server_path.as_deref(), path_env, install_root)
            .map_err(|e| e.to_string())?;
    let Some(found) = found else {
        return Err(format!(
            "llama-server (llama.cpp) isn't installed. Click \"Install llama.cpp\" in Settings → Local AI. {}",
            manual_install_hint(std::env::consts::OS)
        ));
    };
    Ok(LaunchSpec {
        binary: found.path,
        model_path: model_path(entry, models_dir),
        model_id: entry.id.to_string(),
        ctx_size: settings.ctx_size.unwrap_or(entry.ctx_size),
    })
}

/// The model `[local_ai] model` names, or — when unset — the largest catalog
/// model already on disk.
pub fn selected_model(
    settings: &LocalAiSettings,
    models_dir: &Path,
) -> Result<&'static CatalogEntry, String> {
    match settings.model.as_deref().filter(|m| !m.is_empty()) {
        Some(id) => catalog::find(id).ok_or_else(|| {
            format!("Unknown built-in model \"{id}\". Pick one in Settings → Local AI.")
        }),
        None => catalog::CATALOG
            .iter()
            .rev()
            .find(|e| is_downloaded(e, models_dir))
            .ok_or_else(|| {
                "No built-in model downloaded yet. Open Settings → Local AI and click Download."
                    .to_string()
            }),
    }
}

/// Why the built-in provider can't answer right now, or `None` when it can.
pub fn readiness_problem_in(
    settings: &LocalAiSettings,
    models_dir: &Path,
    status: &ServerStatus,
) -> Option<String> {
    let entry = match selected_model(settings, models_dir) {
        Ok(e) => e,
        Err(e) => return Some(e),
    };
    if !is_downloaded(entry, models_dir) {
        return Some(format!(
            "{} isn't downloaded yet. Download it in Settings → Local AI.",
            entry.display_name
        ));
    }
    match status {
        ServerStatus::Running { model_id, .. } if model_id == entry.id => None,
        ServerStatus::Running { .. } => Some(format!(
            "PhazeAI Local is running a different model; restart it in Settings → Local AI to load {}.",
            entry.display_name
        )),
        ServerStatus::Starting { .. } => Some(format!(
            "PhazeAI Local is starting (loading {}).",
            entry.display_name
        )),
        ServerStatus::Failed(e) => Some(format!("PhazeAI Local failed to start: {e}")),
        ServerStatus::Stopped => Some(
            "PhazeAI Local isn't running. Start it in Settings → Local AI.".to_string(),
        ),
    }
}

/// [`readiness_problem_in`] for the real models folder and the live server.
pub fn readiness_problem(settings: &LocalAiSettings) -> Option<String> {
    readiness_problem_in(settings, &super::models_dir(), &LOCAL_AI.status())
}

// ── process-wide handle ─────────────────────────────────────────────────────

struct Inner {
    status: ServerStatus,
    server: Option<LlamaServer>,
    /// Bumped by every start/stop so a slow start that was superseded throws
    /// its server away instead of installing it.
    generation: u64,
}

/// The one built-in server of this process.
pub struct LocalAi {
    inner: Mutex<Inner>,
}

pub static LOCAL_AI: LazyLock<LocalAi> = LazyLock::new(|| LocalAi {
    inner: Mutex::new(Inner {
        status: ServerStatus::Stopped,
        server: None,
        generation: 0,
    }),
});

/// This process's pid file. One per owner, so an IDE and a CLI running at the
/// same time never overwrite each other's record.
fn pid_file() -> PathBuf {
    super::data_root().join(format!("llama-server-{}.pid", std::process::id()))
}

/// Run [`reap_orphan`] on every `llama-server-*.pid` in `dir`.
pub fn reap_orphans(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(|e| e.ok()) {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("llama-server") && name.ends_with(".pid") {
            reap_orphan(&e.path());
        }
    }
}

impl LocalAi {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Current status; notices a server that died since the last call.
    pub fn status(&self) -> ServerStatus {
        let mut g = self.lock();
        if let Some(server) = g.server.as_mut() {
            if let Some(exit) = server.exited() {
                let tail = server.stderr_tail(8);
                g.server = None;
                g.status = ServerStatus::Failed(format!("llama-server exited ({exit}).\n{tail}"));
            }
        }
        g.status.clone()
    }

    /// `http://127.0.0.1:<port>` while a server is ready.
    pub fn base_url(&self) -> Option<String> {
        match self.status() {
            ServerStatus::Running { port, .. } => Some(format!("http://127.0.0.1:{port}")),
            _ => None,
        }
    }

    /// Model id of the ready server.
    pub fn running_model(&self) -> Option<String> {
        match self.status() {
            ServerStatus::Running { model_id, .. } => Some(model_id),
            _ => None,
        }
    }

    /// Start (or restart) the server for `spec` and wait until it is ready.
    pub async fn start_spec(&self, spec: LaunchSpec) -> Result<String, String> {
        let generation = {
            let mut g = self.lock();
            g.generation += 1;
            if let Some(mut old) = g.server.take() {
                old.stop();
            }
            g.status = ServerStatus::Starting {
                model_id: spec.model_id.clone(),
            };
            g.generation
        };
        let fail = |msg: String| {
            let mut g = self.lock();
            if g.generation == generation {
                g.status = ServerStatus::Failed(msg.clone());
            }
            Err(msg)
        };
        let pf = pid_file();
        reap_orphans(&super::data_root());
        let port = match free_port() {
            Ok(p) => p,
            Err(e) => return fail(format!("no free port on 127.0.0.1: {e}")),
        };
        let mut server = match LlamaServer::spawn(&spec, port, Some(pf)) {
            Ok(s) => s,
            Err(e) => return fail(format!("could not run {}: {e}", spec.binary.display())),
        };
        let still_wanted = || self.lock().generation == generation;
        if let Err(e) = server
            .wait_healthy_while(HEALTH_TIMEOUT, still_wanted)
            .await
        {
            server.stop();
            return fail(e);
        }
        let mut g = self.lock();
        if g.generation != generation {
            // Stopped or restarted while loading.
            drop(g);
            server.stop();
            return Err("start was cancelled".into());
        }
        let url = server.base_url();
        g.status = ServerStatus::Running {
            port,
            model_id: spec.model_id.clone(),
        };
        g.server = Some(server);
        Ok(url)
    }

    /// Resolve the launch from settings and start; failures land in the status.
    pub async fn start(&self, settings: &LocalAiSettings) -> Result<String, String> {
        let path = std::env::var_os("PATH");
        match resolve_launch(
            settings,
            &super::models_dir(),
            &super::llama_cpp_dir(),
            path.as_deref(),
        ) {
            Ok(spec) => self.start_spec(spec).await,
            Err(e) => {
                let mut g = self.lock();
                g.generation += 1;
                g.status = ServerStatus::Failed(e.clone());
                Err(e)
            }
        }
    }

    /// Return the base URL of a server running the selected model, starting it
    /// if needed.
    pub async fn ensure_running(&self, settings: &LocalAiSettings) -> Result<String, String> {
        if let ServerStatus::Running { port, model_id } = self.status() {
            let wanted = selected_model(settings, &super::models_dir()).ok();
            if wanted.is_none_or(|e| e.id == model_id) {
                return Ok(format!("http://127.0.0.1:{port}"));
            }
        }
        self.start(settings).await
    }

    /// Start on a background thread (never blocks the caller).
    pub fn start_in_background(&'static self, settings: LocalAiSettings) {
        std::thread::spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => {
                    if let Err(e) = rt.block_on(self.start(&settings)) {
                        tracing::warn!(target: "phazeai::local_ai", "built-in server failed to start: {e}");
                    }
                }
                Err(e) => {
                    let mut g = self.lock();
                    g.status = ServerStatus::Failed(format!("could not start a runtime: {e}"));
                }
            }
        });
    }

    /// Stop the server (also cancels a start in progress).
    pub fn stop(&self) {
        let server = {
            let mut g = self.lock();
            g.generation += 1;
            g.status = ServerStatus::Stopped;
            g.server.take()
        };
        if let Some(mut s) = server {
            s.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(binary: PathBuf) -> LaunchSpec {
        LaunchSpec {
            binary,
            model_path: PathBuf::from("/models/m.gguf"),
            model_id: "qwen2.5-coder-1.5b-instruct-q4_k_m".into(),
            ctx_size: 8192,
        }
    }

    #[test]
    fn args_are_minimal_and_loopback_only() {
        let args: Vec<String> = build_args(&spec("llama-server".into()), 4321)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-m",
                "/models/m.gguf",
                "--host",
                "127.0.0.1",
                "--port",
                "4321",
                "-c",
                "8192",
                "--jinja",
                "--alias",
                "qwen2.5-coder-1.5b-instruct-q4_k_m"
            ]
        );
    }

    #[test]
    fn free_port_is_bindable() {
        let p = free_port().unwrap();
        assert!(p > 0);
        std::net::TcpListener::bind(("127.0.0.1", p)).unwrap();
    }

    fn fake_download(entry: &CatalogEntry, dir: &Path) {
        // A sparse file of the exact size counts as downloaded.
        let f = std::fs::File::create(model_path(entry, dir)).unwrap();
        f.set_len(entry.size).unwrap();
    }

    #[test]
    fn readiness_messages() {
        let dir = tempfile::tempdir().unwrap();
        let small = &catalog::CATALOG[0];
        let mid = &catalog::CATALOG[1];
        let mut s = LocalAiSettings::default();

        let msg = readiness_problem_in(&s, dir.path(), &ServerStatus::Stopped).unwrap();
        assert!(msg.contains("No built-in model downloaded"), "{msg}");

        s.model = Some("nope".into());
        let msg = readiness_problem_in(&s, dir.path(), &ServerStatus::Stopped).unwrap();
        assert!(msg.contains("Unknown built-in model"), "{msg}");

        s.model = Some(small.id.into());
        let msg = readiness_problem_in(&s, dir.path(), &ServerStatus::Stopped).unwrap();
        assert!(msg.contains("isn't downloaded yet"), "{msg}");

        fake_download(small, dir.path());
        let msg = readiness_problem_in(&s, dir.path(), &ServerStatus::Stopped).unwrap();
        assert!(msg.contains("isn't running"), "{msg}");
        let starting = ServerStatus::Starting {
            model_id: small.id.into(),
        };
        let msg = readiness_problem_in(&s, dir.path(), &starting).unwrap();
        assert!(msg.contains("starting"), "{msg}");
        let failed = ServerStatus::Failed("boom".into());
        let msg = readiness_problem_in(&s, dir.path(), &failed).unwrap();
        assert!(msg.contains("failed to start: boom"), "{msg}");
        let running = |id: &str| ServerStatus::Running {
            port: 1,
            model_id: id.into(),
        };
        assert_eq!(
            readiness_problem_in(&s, dir.path(), &running(small.id)),
            None
        );
        let msg = readiness_problem_in(&s, dir.path(), &running(mid.id)).unwrap();
        assert!(msg.contains("different model"), "{msg}");

        // Unset model falls back to the largest downloaded one.
        s.model = None;
        assert_eq!(
            readiness_problem_in(&s, dir.path(), &running(small.id)),
            None
        );
    }

    #[test]
    fn resolve_launch_requires_model_and_binary() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let root = dir.path().join("llama.cpp");
        let entry = &catalog::CATALOG[0];
        let mut s = LocalAiSettings {
            model: Some(entry.id.into()),
            ..Default::default()
        };
        let err = resolve_launch(&s, &models, &root, None).unwrap_err();
        assert!(err.contains("isn't downloaded"), "{err}");
        fake_download(entry, &models);
        let err = resolve_launch(&s, &models, &root, None).unwrap_err();
        assert!(err.contains("isn't installed"), "{err}");

        let bin = dir.path().join(super::super::install::SERVER_EXE);
        std::fs::write(&bin, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        s.server_path = Some(bin.clone());
        s.ctx_size = Some(4096);
        let spec = resolve_launch(&s, &models, &root, None).unwrap();
        assert_eq!(spec.binary, bin);
        assert_eq!(spec.ctx_size, 4096);
        assert_eq!(spec.model_path, model_path(entry, &models));
    }

    /// A process that exits immediately is reported with its stderr.
    #[cfg(unix)]
    #[tokio::test]
    async fn early_exit_reports_stderr() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("llama-server");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'error: failed to load model' >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = LlamaServer::spawn(&spec(script), free_port().unwrap(), None).unwrap();
        let err = s.wait_healthy(Duration::from_secs(10)).await.unwrap_err();
        assert!(err.contains("exited"), "{err}");
        assert!(err.contains("failed to load model"), "{err}");
    }

    /// A process that never serves /health times out with a clear message,
    /// and stop() kills it.
    #[cfg(unix)]
    #[tokio::test]
    async fn health_timeout_then_stop_kills() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("llama-server");
        std::fs::write(&script, "#!/bin/sh\necho loading >&2\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let pf = dir.path().join("pid");
        let mut s =
            LlamaServer::spawn(&spec(script), free_port().unwrap(), Some(pf.clone())).unwrap();
        assert!(pf.exists());
        let err = s
            .wait_healthy(Duration::from_millis(800))
            .await
            .unwrap_err();
        assert!(err.contains("did not become ready"), "{err}");
        assert!(err.contains("loading"), "{err}");
        s.stop();
        assert!(s.exited().is_some());
        assert!(!pf.exists());
    }

    /// A server answering /health proves the readiness wait. The stand-in is
    /// this test binary run again (see `fake_llama_server_process`) behind a
    /// shell wrapper, so no other program has to be installed.
    #[cfg(unix)]
    #[tokio::test]
    async fn health_ok_is_detected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("llama-server");
        let exe = std::env::current_exe().unwrap();
        let exe = exe
            .to_str()
            .expect("the test binary path is UTF-8")
            .replace('\'', r"'\''");
        // llama-server is started with `--port N`; hand N on to the helper.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 while [ $# -gt 0 ]; do\n\
                 [ \"$1\" = --port ] && PHAZEAI_FAKE_LLAMA_PORT=$2\n\
                 shift\n\
                 done\n\
                 export PHAZEAI_FAKE_LLAMA_PORT\n\
                 exec '{exe}' --ignored --nocapture fake_llama_server_process\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = LlamaServer::spawn(&spec(script), free_port().unwrap(), None).unwrap();
        s.wait_healthy(Duration::from_secs(20)).await.unwrap();
        assert!(s.exited().is_none());
        drop(s); // Drop kills it.
    }

    /// The process behind `health_ok_is_detected`, not a test: it serves
    /// `GET /health` on `PHAZEAI_FAKE_LLAMA_PORT` until it is killed, and does
    /// nothing when that variable is unset (`cargo test -- --ignored`).
    #[cfg(unix)]
    #[test]
    #[ignore = "helper process of health_ok_is_detected"]
    fn fake_llama_server_process() {
        use std::io::{Read, Write};
        let Some(port) = std::env::var("PHAZEAI_FAKE_LLAMA_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
        else {
            return;
        };
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        // A test run killed before it could clean up must not leave this behind.
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut conn, _)) => {
                    conn.set_nonblocking(false).unwrap();
                    conn.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    let mut request = Vec::new();
                    let mut buf = [0u8; 512];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match conn.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let (status, body) = if request.starts_with(b"GET /health ") {
                        ("200 OK", r#"{"status":"ok"}"#)
                    } else {
                        ("404 Not Found", "")
                    };
                    let _ = write!(
                        conn,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }

    #[test]
    fn orphan_reaper_ignores_garbage_and_live_owners() {
        let dir = tempfile::tempdir().unwrap();
        let pf = dir.path().join("pid");
        std::fs::write(&pf, "garbage").unwrap();
        reap_orphan(&pf);
        assert!(!pf.exists());
        // Owner = this test process, server = this process too: not a
        // llama-server, so nothing is killed (we are still alive afterwards).
        std::fs::write(&pf, format!("{} 999999999", std::process::id())).unwrap();
        reap_orphan(&pf);
        assert!(!pf.exists());
    }

    #[cfg(unix)]
    #[test]
    fn reaper_keeps_records_of_live_owners() {
        let dir = tempfile::tempdir().unwrap();
        // Owner is this (live) test process's parent, so the record must stay.
        let parent = std::os::unix::process::parent_id();
        let live = dir.path().join("llama-server-1.pid");
        std::fs::write(&live, format!("999999998 {parent}")).unwrap();
        let dead = dir.path().join("llama-server-2.pid");
        std::fs::write(&dead, "999999997 999999996").unwrap();
        let other = dir.path().join("unrelated.pid");
        std::fs::write(&other, "1 1").unwrap();
        reap_orphans(dir.path());
        assert!(live.exists());
        assert!(!dead.exists());
        assert!(other.exists());
    }
}
