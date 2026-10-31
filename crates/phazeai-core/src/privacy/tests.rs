use super::*;
use crate::llm::{LlmClient, LlmResponse, Message, ProviderConfig, ProviderRegistry, StreamEvent};
use crate::tools::{
    BashTool, CopyPathTool, CreateDirectoryTool, DeletePathTool, EditTool, GlobTool, GrepTool,
    ListFilesTool, MovePathTool, ReadFileTool, Tool, ToolDefinition, WriteFileTool,
};
use std::sync::Mutex;

/// Tests that change the process-wide active project run one at a time.
static ACTIVE_LOCK: Mutex<()> = Mutex::new(());

struct ActiveProject {
    _privacy: std::sync::MutexGuard<'static, ()>,
    _sandbox: std::sync::MutexGuard<'static, ()>,
    prev_sandbox: Option<PathBuf>,
}

impl Drop for ActiveProject {
    fn drop(&mut self) {
        clear_active_project();
        crate::tools::sandbox::set_workspace_root(self.prev_sandbox.take());
    }
}

/// Make `root` the active project (and sandbox root) for the guard's lifetime.
fn activate(root: &Path) -> ActiveProject {
    let privacy = ACTIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sandbox = crate::tools::sandbox::tests::SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev_sandbox = crate::tools::sandbox::workspace_root();
    crate::tools::sandbox::set_workspace_root(Some(root.to_path_buf()));
    set_active_project(root);
    ActiveProject {
        _privacy: privacy,
        _sandbox: sandbox,
        prev_sandbox,
    }
}

fn project(policy: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    if let Some(text) = policy {
        std::fs::create_dir_all(root.join(".phazeai")).unwrap();
        std::fs::write(root.join(POLICY_FILE), text).unwrap();
    }
    (dir, root)
}

fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
    p
}

// ── glob semantics ────────────────────────────────────────────────────────────

#[test]
fn builtin_list_matches_secrets_at_any_depth() {
    let (_d, root) = project(None);
    let p = PrivacyPolicy::load(&root).unwrap();
    for denied in [
        ".env",
        "config/.env.local",
        ".env.production",
        "certs/server.pem",
        "a/b/tls.key",
        "keystore.jks",
        ".ssh/id_rsa",
        "home/.ssh/config",
        "deploy/id_ed25519.pub",
        "ID_RSA",
        ".aws/credentials",
        ".gnupg/pubring.kbx",
        ".npmrc",
        "pkg/.pypirc",
        ".git-credentials",
        "credentials.json",
        "gcp/credentials-prod.json",
        "my-service-account-key.json",
        "secrets.yaml",
        ".docker/config.json",
        "infra/terraform.tfstate",
        "infra/terraform.tfstate.backup",
    ] {
        let d = p.check(Path::new(denied));
        assert!(d.is_some(), "{denied} should be denied");
        assert_eq!(d.unwrap().source, DenySource::BuiltIn, "{denied}");
    }
    for allowed in [
        ".env.example",
        "sub/.env.sample",
        ".env.template",
        "src/main.rs",
        "README.md",
        "docker/config.json",
        "keys.rs",
        "environment.ts",
    ] {
        assert!(
            !p.is_denied(Path::new(allowed)),
            "{allowed} should be allowed"
        );
    }
}

#[test]
fn env_template_exemption_does_not_override_other_builtins() {
    let (_d, root) = project(None);
    let p = PrivacyPolicy::load(&root).unwrap();
    assert!(p.is_denied(Path::new(".ssh/.env.example")));
}

#[test]
fn project_patterns_follow_gitignore_anchoring() {
    let (_d, root) = project(Some(
        r#"deny = ["secrets/**", "*.sqlite", "/top.txt", "data/", "build", "docs/internal/*.md"]"#,
    ));
    let p = PrivacyPolicy::load(&root).unwrap();
    assert!(p.file_exists());
    let denied = |s: &str| p.is_denied(Path::new(s));
    // Anchored by the slash: only at the root.
    assert!(denied("secrets/a.txt"));
    assert!(denied("secrets/deep/b.txt"));
    assert!(!denied("sub/secrets/a.txt"));
    assert!(denied("top.txt"));
    assert!(!denied("sub/top.txt"));
    assert!(denied("docs/internal/plan.md"));
    assert!(!denied("docs/internal/sub/plan.md"), "* must not cross /");
    // No slash: any depth.
    assert!(denied("db.sqlite"));
    assert!(denied("x/y/db.sqlite"));
    // Directory patterns deny everything inside.
    assert!(denied("build/out.o"));
    assert!(denied("a/build/out.o"));
    assert!(denied("data/rows.csv"));
    assert!(!denied("database.csv"));
    // Absolute paths inside the root work the same.
    assert!(p.is_denied(&root.join("x/db.sqlite")));
    assert!(!p.is_denied(&root.join("src/lib.rs")));
    let d = p.check(Path::new("x/db.sqlite")).unwrap();
    assert_eq!(d.source, DenySource::Project);
    assert_eq!(d.pattern, "*.sqlite");
    assert!(d.reason().contains(".phazeai/privacy.toml"));
}

#[test]
fn outside_root_only_builtins_apply() {
    let (_d, root) = project(Some(r#"deny = ["*.sqlite"]"#));
    let (_o, other) = project(None);
    let p = PrivacyPolicy::load(&root).unwrap();
    assert!(!p.is_denied(&other.join("db.sqlite")));
    assert!(p.is_denied(&other.join(".env")));
    assert!(p.is_denied(Path::new("/home/someone/.ssh/id_rsa")));
    assert!(!p.is_denied(Path::new("/usr/share/doc/readme.txt")));
    // `..` can't be used to dodge the root-relative match.
    assert!(p.is_denied(&root.join("sub/../db.sqlite")));
}

#[test]
fn ancestors_of_the_root_do_not_trigger_builtins() {
    // A project living under a directory named like a secret is still usable.
    let dir = tempfile::tempdir().unwrap();
    let root = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("secrets.d")
        .join("app");
    std::fs::create_dir_all(&root).unwrap();
    let p = PrivacyPolicy::load(&root).unwrap();
    assert!(!p.is_denied(&root.join("src/main.rs")));
}

#[cfg(unix)]
#[test]
fn symlink_to_denied_file_is_denied() {
    let (_d, root) = project(None);
    write(&root, ".env", "TOKEN=1");
    std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).unwrap();
    let p = PrivacyPolicy::load(&root).unwrap();
    assert!(p.is_denied(&root.join("notes.txt")));
}

#[cfg(unix)]
#[tokio::test]
async fn directory_tree_check_follows_symlinks() {
    use std::os::unix::fs::symlink;
    let (_d, root) = project(Some(r#"deny = ["secrets/**"]"#));
    write(&root, "secrets/token.txt", "tok");
    write(&root, ".env", "A=1");
    let p = PrivacyPolicy::load(&root).unwrap();
    // `secrets/**` denies the contents, not the directory entry itself, so a
    // walk that doesn't descend into the link never sees a denied path.
    assert!(!p.is_denied(&root.join("secrets")));

    // A directory holding a symlink to the denied directory.
    write(&root, "pub1/readme.md", "ok");
    symlink(root.join("secrets"), root.join("pub1/link")).unwrap();
    let (hit, d) = first_denied_under(&p, &root.join("pub1")).expect("linked dir must be found");
    assert!(hit.starts_with(root.join("pub1/link")), "{}", hit.display());
    assert_eq!(d.pattern, "secrets/**");

    // A symlinked file.
    write(&root, "pub2/readme.md", "ok");
    symlink(root.join(".env"), root.join("pub2/notes.txt")).unwrap();
    let (hit, _) = first_denied_under(&p, &root.join("pub2")).unwrap();
    assert_eq!(hit, root.join("pub2/notes.txt"));

    // A link loop terminates and, with nothing denied, passes.
    write(&root, "pub3/readme.md", "ok");
    symlink(root.join("pub3"), root.join("pub3/loop")).unwrap();
    symlink(&root, root.join("pub3/up")).unwrap(); // loop through the root (contains .env → denied)
    assert!(first_denied_under(&p, &root.join("pub3")).is_some());
    std::fs::remove_file(root.join("pub3/up")).unwrap();
    assert!(first_denied_under(&p, &root.join("pub3")).is_none());

    // Through the tool: the copy is refused and nothing is written.
    let _g = activate(&root);
    let err = CopyPathTool
        .execute(serde_json::json!({ "source": s(&root.join("pub1")), "destination": s(&root.join("out")) }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("REFUSED") && err.contains("pub1/link"),
        "{err}"
    );
    assert!(!root.join("out").exists());
}

// ── parsing ───────────────────────────────────────────────────────────────────

#[test]
fn missing_file_means_defaults() {
    let (_d, root) = project(None);
    let p = PrivacyPolicy::load(&root).unwrap();
    assert!(!p.file_exists());
    assert!(!p.local_only());
    assert!(p.project_patterns().is_empty());
}

#[test]
fn parse_errors_are_reported() {
    let root = Path::new("/nonexistent-root");
    let cases = [
        ("deny = []\nlocal_onyl = true", "local_onyl"),
        ("deny = [\"[abc\"]", "[abc"),
        ("deny = [\"!keep.txt\"]", "not supported"),
        ("deny = [\"\"]", "empty"),
        ("deny = \"*.db\"", "deny"),
        ("local_only = \"yes\"", "local_only"),
        ("deny = [", "privacy.toml"),
    ];
    for (text, needle) in cases {
        let err = PrivacyPolicy::parse(root, text).expect_err(text);
        assert!(err.contains(needle), "{text:?} → {err}");
        assert!(err.contains("privacy.toml"), "{err}");
    }
    let ok = PrivacyPolicy::parse(root, "# comment only\n").unwrap();
    assert!(ok.file_exists() && !ok.local_only());
}

// ── active policy ─────────────────────────────────────────────────────────────

#[test]
fn active_policy_reloads_and_fails_closed() {
    let (_d, root) = project(Some("deny = [\"*.sqlite\"]\nlocal_only = false\n"));
    let _g = activate(&root);
    let p = active();
    assert!(p.is_denied(Path::new("a.sqlite")));
    assert!(!p.local_only());
    assert_eq!(active_root().as_deref(), Some(root.as_path()));

    // A broken edit keeps the last good deny list and forces local_only.
    std::fs::write(root.join(POLICY_FILE), "deny = [\"*.sqlite\"]\nbogus = 1\n").unwrap();
    let p = active();
    assert!(p.load_error().unwrap().contains("bogus"));
    assert!(p.local_only(), "invalid file must fail closed");
    assert!(p.is_denied(Path::new("a.sqlite")), "last good list kept");
    let err =
        ensure_provider_allowed(&ProviderId::Claude, "https://api.anthropic.com").unwrap_err();
    assert!(err.contains("couldn't be loaded"), "{err}");

    // Fixing it is picked up on the next call.
    std::fs::write(root.join(POLICY_FILE), "deny = [\"*.csv\"]\n").unwrap();
    let p = active();
    assert!(p.load_error().is_none());
    assert!(!p.local_only());
    assert!(p.is_denied(Path::new("rows.csv")));
    assert!(!p.is_denied(Path::new("a.sqlite")));

    // Deleting it falls back to defaults.
    std::fs::remove_file(root.join(POLICY_FILE)).unwrap();
    let p = active();
    assert!(!p.file_exists());
    assert!(!p.is_denied(Path::new("rows.csv")));
    assert!(p.is_denied(Path::new(".env")));
}

#[test]
fn write_local_only_preserves_comments() {
    let (_d, root) = project(Some(
        "# my notes\ndeny = [\"*.db\"] # keep db out\n\nlocal_only = false # flip me\n# trailer\n",
    ));
    write_local_only(&root, true).unwrap();
    let text = std::fs::read_to_string(root.join(POLICY_FILE)).unwrap();
    assert_eq!(
        text,
        "# my notes\ndeny = [\"*.db\"] # keep db out\n\nlocal_only = true # flip me\n# trailer\n"
    );
    assert!(PrivacyPolicy::load(&root).unwrap().local_only());

    // Missing key → appended; missing file → template created.
    let (_d2, root2) = project(Some("deny = []\n"));
    write_local_only(&root2, true).unwrap();
    assert!(PrivacyPolicy::load(&root2).unwrap().local_only());
    let (_d3, root3) = project(None);
    write_local_only(&root3, true).unwrap();
    let text3 = std::fs::read_to_string(root3.join(POLICY_FILE)).unwrap();
    assert!(text3.starts_with("# PhazeAI privacy policy"));
    assert!(PrivacyPolicy::load(&root3).unwrap().local_only());

    // A broken file is left alone.
    let (_d4, root4) = project(Some("nope = 1\n"));
    assert!(write_local_only(&root4, true).is_err());
    assert_eq!(
        std::fs::read_to_string(root4.join(POLICY_FILE)).unwrap(),
        "nope = 1\n"
    );
}

#[test]
fn policy_writes_are_atomic() {
    // A large policy, so a non-atomic write would leave a visible window of
    // empty / partial content.
    let patterns: Vec<String> = (0..800).map(|i| format!("\"private-{i:03}/**\"")).collect();
    let body = format!(
        "# header comment\ndeny = [\n  {}\n]\nlocal_only = false # keep\n",
        patterns.join(",\n  ")
    );
    let (_d, root) = project(Some(&body));
    let path = root.join(POLICY_FILE);
    let off = body.clone();
    let on = body.replace("local_only = false # keep", "local_only = true # keep");
    assert!(on.len() > 10_000);

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (path, stop, off, on) = (path.clone(), stop.clone(), off.clone(), on.clone());
        std::thread::spawn(move || {
            let mut reads = 0usize;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        assert!(
                            text == off || text == on,
                            "reader saw a partial file ({} bytes)",
                            text.len()
                        );
                        reads += 1;
                    }
                    // Only a rename-less platform could miss the file for an instant.
                    Err(e) => {
                        if cfg!(unix) || e.kind() != std::io::ErrorKind::NotFound {
                            panic!("read failed: {e}");
                        }
                    }
                }
            }
            reads
        })
    };
    for i in 0..40 {
        write_local_only(&root, i % 2 == 0).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(&text, if i % 2 == 0 { &on } else { &off });
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let reads = reader.join().expect("reader thread must not panic");
    assert!(reads > 0);

    // The temp file is renamed over the original, never left behind, and the
    // comments survive (the file is rewritten whole, from the edited text).
    let leftovers: Vec<_> = std::fs::read_dir(root.join(".phazeai"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "privacy.toml")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .starts_with("# header comment\n"));

    // ensure_policy_file goes through the same path.
    let (_d2, root2) = project(None);
    ensure_policy_file(&root2).unwrap();
    assert_eq!(
        std::fs::read_to_string(root2.join(POLICY_FILE)).unwrap(),
        TEMPLATE
    );
    assert_eq!(
        std::fs::read_dir(root2.join(".phazeai")).unwrap().count(),
        1
    );
}

#[test]
fn template_parses_to_defaults() {
    let p = PrivacyPolicy::parse(Path::new("/x"), TEMPLATE).unwrap();
    assert!(!p.local_only());
    assert!(p.project_patterns().is_empty());
}

// ── local_only ────────────────────────────────────────────────────────────────

#[test]
fn loopback_detection() {
    for h in [
        "localhost",
        "LOCALHOST",
        "127.0.0.1",
        "127.8.9.10",
        "::1",
        "[::1]",
        "::ffff:127.0.0.1",
    ] {
        assert!(is_loopback_host(h), "{h}");
    }
    for h in [
        "api.openai.com",
        "192.168.1.5",
        "10.0.0.1",
        "0.0.0.0",
        "localhost.evil.com",
        "::",
    ] {
        assert!(!is_loopback_host(h), "{h}");
    }
    assert_eq!(
        url_host("http://localhost:11434").as_deref(),
        Some("localhost")
    );
    assert_eq!(url_host("localhost:1234/v1").as_deref(), Some("localhost"));
    assert_eq!(url_host("http://[::1]:8080/v1").as_deref(), Some("[::1]"));

    assert!(is_local_endpoint(&ProviderId::Ollama, ""));
    assert!(is_local_endpoint(
        &ProviderId::Ollama,
        "http://localhost:11434"
    ));
    assert!(is_local_endpoint(
        &ProviderId::LmStudio,
        "http://127.0.0.1:1234/v1"
    ));
    assert!(is_local_endpoint(
        &ProviderId::Custom("mine".into()),
        "http://127.0.0.1:8080/v1"
    ));
    // A model server on another machine is not on-device.
    assert!(!is_local_endpoint(
        &ProviderId::Ollama,
        "http://gpu-box:11434"
    ));
    assert!(!is_local_endpoint(
        &ProviderId::Claude,
        "https://api.anthropic.com"
    ));
    assert!(!is_local_endpoint(&ProviderId::Claude, ""));
    assert!(!is_local_endpoint(
        &ProviderId::OpenAI,
        "https://api.openai.com/v1"
    ));
}

fn config(id: ProviderId, base_url: &str) -> ProviderConfig {
    ProviderConfig {
        id,
        enabled: true,
        api_key_env: "PHAZEAI_PRIVACY_TEST_KEY_THAT_IS_NOT_SET".into(),
        base_url: base_url.into(),
        default_model: "m".into(),
    }
}

#[tokio::test]
async fn local_only_blocks_cloud_clients() {
    let (_d, root) = project(Some("local_only = true\n"));
    let _g = activate(&root);
    let registry = ProviderRegistry::new();

    let err = match registry.build_client_for(
        &config(ProviderId::Claude, "https://api.anthropic.com"),
        "claude",
    ) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("cloud client must be refused"),
    };
    assert!(err.contains("local-only"), "{err}");
    assert!(err.contains("Settings › Privacy"), "{err}");
    let err = match registry.build_client_for(
        &config(ProviderId::Groq, "https://api.groq.com/openai/v1"),
        "llama",
    ) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("cloud client must be refused"),
    };
    assert!(err.contains("Groq"), "{err}");
    assert!(audit::recent(10).iter().any(|e| matches!(
        &e.kind,
        audit::AuditKind::BlockedProvider { provider, .. } if provider == "Groq"
    )));

    // Local endpoints are fine.
    assert!(registry
        .build_client_for(
            &config(ProviderId::Ollama, "http://localhost:11434"),
            "llama3"
        )
        .is_ok());
    assert!(registry
        .build_client_for(&config(ProviderId::LmStudio, "http://127.0.0.1:9/v1"), "x")
        .is_ok());
}

#[tokio::test]
async fn local_only_applies_to_clients_built_before_the_switch() {
    let (_d, root) = project(None);
    let _g = activate(&root);
    let registry = ProviderRegistry::new();
    // An LM Studio server on another machine: needs no key, but isn't on-device.
    let cfg = config(ProviderId::LmStudio, "http://lan-box.invalid:1234/v1");
    let client = match registry.build_client_for(&cfg, "qwen") {
        Ok(c) => c,
        Err(e) => panic!("build failed: {e}"),
    };
    write_local_only(&root, true).unwrap();
    let err = match client.chat(&[Message::user("hi")], &[]).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("request must be refused before any network call"),
    };
    assert!(err.contains("local-only"), "{err}");
    let err = client
        .chat_stream(&[Message::user("hi")], &[])
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("local-only"), "{err}");
}

// ── audit ─────────────────────────────────────────────────────────────────────

/// Inner client standing in for a provider, so the guard can be exercised
/// without a network. The guard and audit log are the code under test.
struct EchoClient;

#[async_trait::async_trait]
impl LlmClient for EchoClient {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse, crate::error::PhazeError> {
        Ok(LlmResponse {
            message: Message::assistant("ok"),
            usage: None,
        })
    }
    async fn chat_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
    ) -> Result<futures::channel::mpsc::UnboundedReceiver<StreamEvent>, crate::error::PhazeError>
    {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let _ = tx.unbounded_send(StreamEvent::Done);
        Ok(rx)
    }
}

#[tokio::test]
async fn audit_records_requests_with_attached_files() {
    let (_d, root) = project(None);
    let _g = activate(&root);
    let model = format!("audit-model-{}", std::process::id());
    let client = GuardedClient::new(
        Box::new(EchoClient),
        ProviderId::Custom("echo".into()),
        model.clone(),
        "http://127.0.0.1:9",
    );
    let msgs = [Message::system("sys"), Message::user("hello world")];
    audit::scope(vec![root.join("src/a.rs")], async {
        client.chat(&msgs, &[]).await.unwrap();
        audit::note_file(&root.join("src/b.rs"));
        audit::note_file(&root.join("src/b.rs"));
        client.chat_stream(&msgs, &[]).await.unwrap();
        client.chat(&msgs, &[]).await.unwrap();
    })
    .await;

    let mine: Vec<_> = audit::recent(audit::CAPACITY)
        .into_iter()
        .filter_map(|e| match e.kind {
            audit::AuditKind::Request {
                model: m,
                chars,
                files,
                ..
            } if m == model => Some((chars, files)),
            _ => None,
        })
        .collect();
    assert_eq!(mine.len(), 3);
    // Newest first.
    assert_eq!(mine[2], (14, vec!["src/a.rs".to_string()]));
    assert_eq!(mine[1], (14, vec!["src/b.rs".to_string()]));
    assert_eq!(mine[0], (14, Vec::<String>::new()));
}

#[test]
fn audit_ring_is_bounded() {
    // Holds the lock so it can't evict entries another test is asserting on.
    let _g = ACTIVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for i in 0..(audit::CAPACITY + 25) {
        audit::record_blocked_file(&format!("f{i}"), "r", "test");
    }
    assert_eq!(audit::recent(usize::MAX).len(), audit::CAPACITY);
    let newest = &audit::recent(1)[0];
    assert!(newest.is_blocked());
    assert!(newest.summary().contains("BLOCKED"));
}

// ── content helpers ───────────────────────────────────────────────────────────

#[test]
fn diff_sections_for_denied_files_are_removed() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    let p = PrivacyPolicy::load(&root).unwrap();
    let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-a\n+b\n\
diff --git a/.env b/.env\n--- a/.env\n+++ b/.env\n@@ -1 +1 @@\n-K=1\n+K=2\n\
diff --git a/private/notes.md b/private/notes.md\nnew file mode 100644\n--- /dev/null\n+++ b/private/notes.md\n@@ -0,0 +1 @@\n+hidden\n\
diff --git a/old.txt b/public.txt\nrename from old.txt\nrename to public.txt\n";
    let (out, omitted) = filter_unified_diff(diff, &p);
    assert!(out.contains("src/main.rs"));
    assert!(out.contains("public.txt"));
    assert!(!out.contains("K=2"));
    assert!(!out.contains("hidden"));
    assert_eq!(
        omitted,
        vec![".env".to_string(), "private/notes.md".to_string()]
    );
}

#[test]
fn c_unquote_decodes_git_escapes() {
    let ok = |q: &str| c_unquote_prefix(q).map(|(s, rest)| (s, rest.to_string()));
    assert_eq!(
        ok(r#"caf\303\251.txt" tail"#),
        Some(("café.txt".to_string(), " tail".to_string()))
    );
    assert_eq!(
        ok(r#"a\"b\\c\td\ne\0f""#),
        Some(("a\"b\\c\td\ne\0f".to_string(), String::new()))
    );
    assert_eq!(ok(r#"\101\61x""#), Some(("A1x".to_string(), String::new())));
    assert_eq!(ok("unterminated"), None);
    assert_eq!(ok(r#"bad\q""#), None);
    assert_eq!(ok(r#"\777""#), None, "octal > 255");
    assert_eq!(ok(r#"\303""#), None, "not UTF-8");
}

#[test]
fn quoted_diff_paths_are_decoded_and_unparsable_headers_dropped() {
    let (_d, root) = project(Some(r#"deny = ["café.txt", "*.secret", "private/**"]"#));
    let p = PrivacyPolicy::load(&root).unwrap();
    // The exact forms git emits with core.quotePath=true (non-ASCII as octal)
    // and =false (quotes, backslashes, tabs still escaped).
    let diff = "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\n\
                index 1..2 100644\n\
                --- \"a/caf\\303\\251.txt\"\n\
                +++ \"b/caf\\303\\251.txt\"\n\
                @@ -1 +1 @@\n-old\n+secret-in-cafe\n\
                diff --git \"a/my\\tfile.secret\" \"b/my\\tfile.secret\"\n\
                new file mode 100644\n--- /dev/null\n+++ \"b/my\\tfile.secret\"\n\
                @@ -0,0 +1 @@\n+secret-with-tab\n\
                diff --git a/old.txt \"b/private/q\\\"uote.txt\"\n\
                rename from old.txt\nrename to \"private/q\\\"uote.txt\"\n\
                diff --git a/ok.txt b/ok.txt\n--- a/ok.txt\n+++ b/ok.txt\n\
                @@ -1,2 +1 @@\n--- a/.env\n-x\n+public-change\n\
                diff --git a/sp ace.txt b/sp ace.txt\n--- a/sp ace.txt\n+++ b/sp ace.txt\n\
                @@ -1 +1 @@\n-a\n+spaced-change\n\
                diff --git ???\n@@ -1 +1 @@\n+behind-broken-header\n\
                diff --git \"a/bad\\q\" \"b/bad\\q\"\n@@ -1 +1 @@\n+behind-bad-escape\n";
    let (out, omitted) = filter_unified_diff(diff, &p);
    assert!(out.contains("public-change"), "{out}");
    assert!(
        out.contains("--- a/.env"),
        "a removed line that looks like a header is content, not a path: {out}"
    );
    assert!(out.contains("spaced-change"), "{out}");
    for hidden in [
        "secret-in-cafe",
        "secret-with-tab",
        "q\\\"uote",
        "behind-broken-header",
        "behind-bad-escape",
    ] {
        assert!(!out.contains(hidden), "{hidden} leaked: {out}");
    }
    assert_eq!(
        omitted,
        vec![
            "café.txt".to_string(),
            "my\tfile.secret".to_string(),
            "private/q\"uote.txt".to_string(),
            "<unparsed diff header: diff --git ???>".to_string(),
            "<unparsed diff header: diff --git \"a/bad\\q\" \"b/bad\\q\">".to_string(),
        ]
    );
    // A combined diff (merge conflict) is a section of its own.
    let cc = "diff --cc .env\nindex 1,2..3\n--- a/.env\n+++ b/.env\n@@@ -1,1 -1,1 +1,1 @@@\n+K=9\n\
              diff --git a/ok.txt b/ok.txt\n--- a/ok.txt\n+++ b/ok.txt\n@@ -1 +1 @@\n+fine\n";
    let (out, omitted) = filter_unified_diff(cc, &p);
    assert!(out.contains("fine") && !out.contains("K=9"), "{out}");
    assert_eq!(omitted, vec![".env".to_string()]);
}

#[test]
fn status_lines_for_denied_files_are_removed() {
    let (_d, root) = project(Some(r#"deny = ["private/**", "*.secret"]"#));
    let p = PrivacyPolicy::load(&root).unwrap();
    let status = " M src/main.rs\n\
                  ?? \"caf\\303\\251.txt\"\n\
                  ?? sp ace.txt\n\
                  R  old.txt -> private/new.txt\n\
                  R  \"old 2.txt\" -> \"private/new\\t2.txt\"\n\
                  A  \"tab\\there.secret\"\n\
                  \x20M .env\n\
                  ?? \"bad\\q\"\n\
                  garbage\n";
    let (kept, omitted) = filter_status_porcelain(status, &root, &p);
    assert_eq!(
        kept,
        vec![
            " M src/main.rs".to_string(),
            "?? \"caf\\303\\251.txt\"".to_string(),
            "?? sp ace.txt".to_string(),
        ]
    );
    assert_eq!(
        omitted,
        vec![
            "private/new.txt".to_string(),
            "private/new\t2.txt".to_string(),
            "tab\there.secret".to_string(),
            ".env".to_string(),
            "<unparsed status line: ?? \"bad\\q\">".to_string(),
            "<unparsed status line: garbage>".to_string(),
        ]
    );
}

#[test]
fn bash_words_naming_denied_files_are_found() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    write(&root, "certs/site.pem", "x");
    let p = PrivacyPolicy::load(&root).unwrap();
    let hit = |cmd: &str| command_denial(&p, cmd, &root).map(|(w, _)| w);
    assert_eq!(hit("cat .env").as_deref(), Some(".env"));
    assert_eq!(
        hit("cat 'private/a b.txt' | head").as_deref(),
        Some("private/a b.txt")
    );
    assert_eq!(
        hit("tool --config=.env.local").as_deref(),
        Some(".env.local")
    );
    assert!(hit("cat ~/.ssh/id_rsa").is_some());
    assert!(hit("cat certs/s*").unwrap().contains("site.pem"));
    assert!(hit("cat certs/*.pem").is_some());
    assert!(hit("ls certs/*.txt").is_none());
    assert!(hit("cargo test && cat src/main.rs > out.txt").is_none());
    assert!(hit("cat .env.example").is_none());
}

#[test]
fn bash_words_naming_the_policy_file_are_protected() {
    let (_d, root) = project(Some("deny = []\n"));
    let p = PrivacyPolicy::load(&root).unwrap();
    let hit = |cmd: &str| command_denial(&p, cmd, &root);
    for cmd in [
        "echo 'deny = []' > .phazeai/privacy.toml",
        "cat .phazeai/privacy.toml",
        "sed -i s/true/false/ ./.phazeai/../.phazeai/privacy.toml",
        "rm -rf .phazeai",
        "mv .phazeai .phazeai.bak",
        "cat .phazeai/*",
        "vim .phazeai/priv*",
    ] {
        let (word, d) = hit(cmd).unwrap_or_else(|| panic!("{cmd} must be refused"));
        assert_eq!(d.source, DenySource::Protected, "{cmd}");
        assert_eq!(d.reason(), PROTECTED_REASON);
        assert!(word.contains(".phazeai"), "{cmd} → {word}");
        let text = refusal(&word, &d.reason());
        assert!(text.contains("edited by you"), "{text}");
    }
    // Absolute path and a different cwd.
    let abs = root.join(POLICY_FILE);
    assert!(hit(&format!("cat {}", abs.display())).is_some());
    assert!(command_denial(&p, "cat privacy.toml", &root.join(".phazeai")).is_some());
    // Other files under .phazeai/ are ordinary files.
    assert!(hit("cat .phazeai/designs/home.html").is_none());
    assert!(hit("ls .phazeai/skills").is_none());
    assert!(
        hit("cat privacy.toml").is_none(),
        "a privacy.toml elsewhere isn't the policy"
    );
    // Without a project root nothing is protected (there is no policy file).
    assert!(command_denial(
        &PrivacyPolicy::defaults(),
        "rm .phazeai/privacy.toml",
        &root
    )
    .is_none());
}

// ── tool enforcement ──────────────────────────────────────────────────────────

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[tokio::test]
async fn read_file_refuses_denied_paths() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    let env = write(&root, ".env", "API_KEY=supersecret");
    let notes = write(&root, "private/notes.md", "confidential");
    let ok = write(&root, "src/main.rs", "fn main() {}");
    let _g = activate(&root);

    let err = ReadFileTool
        .execute(serde_json::json!({ "path": s(&env) }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("blocked by the built-in secrets list"),
        "{err}"
    );
    assert!(!err.contains("supersecret"));

    let err = ReadFileTool
        .execute(serde_json::json!({ "path": s(&notes) }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("blocked by .phazeai/privacy.toml"), "{err}");

    let out = ReadFileTool
        .execute(serde_json::json!({ "path": s(&ok) }))
        .await
        .unwrap();
    assert!(out["content"].as_str().unwrap().contains("fn main"));
    assert!(audit::recent(audit::CAPACITY).iter().any(|e| matches!(
        &e.kind,
        audit::AuditKind::BlockedFile { path, via, .. } if path == "private/notes.md" && via == "read_file"
    )));
}

#[tokio::test]
async fn grep_glob_and_list_skip_denied_files() {
    let (_d, root) = project(Some(r#"deny = ["*.sqlite"]"#));
    write(&root, ".env", "TOKEN=needle-in-secret");
    write(&root, "data/app.sqlite", "needle-in-db");
    write(&root, "src/lib.rs", "// needle-in-code");
    let _g = activate(&root);

    let out = GrepTool
        .execute(serde_json::json!({ "pattern": "needle", "path": s(&root) }))
        .await
        .unwrap();
    let text = out.to_string();
    assert!(text.contains("needle-in-code"));
    assert!(!text.contains("needle-in-secret"), "{text}");
    assert!(!text.contains("needle-in-db"), "{text}");
    assert_eq!(out["count"], 1);
    assert_eq!(out["privacy_skipped"], 2);

    // Grepping a denied file directly is refused.
    let err = GrepTool
        .execute(serde_json::json!({ "pattern": "needle", "path": s(&root.join(".env")) }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("REFUSED"), "{err}");

    let out = GlobTool
        .execute(serde_json::json!({ "pattern": "**/*", "path": s(&root) }))
        .await
        .unwrap()
        .to_string();
    assert!(out.contains("lib.rs"));
    assert!(
        !out.contains(".env") && !out.contains("app.sqlite"),
        "{out}"
    );

    let out = ListFilesTool
        .execute(serde_json::json!({ "path": s(&root), "recursive": true }))
        .await
        .unwrap()
        .to_string();
    assert!(out.contains("lib.rs"));
    assert!(
        !out.contains(".env") && !out.contains("app.sqlite"),
        "{out}"
    );
}

#[tokio::test]
async fn write_and_edit_refuse_denied_paths() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    let env = write(&root, ".env", "A=1");
    let _g = activate(&root);

    let target = root.join("private/new.txt");
    let err = WriteFileTool
        .execute(serde_json::json!({ "path": s(&target), "content": "x" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("REFUSED"), "{err}");
    assert!(!target.exists());

    let err = EditTool
        .execute(serde_json::json!({ "path": s(&env), "old_text": "A=1", "new_text": "A=2" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("REFUSED"), "{err}");
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "A=1");

    // Copying a directory that contains a secret is refused too.
    write(&root, "conf/.env", "B=2");
    let err = CopyPathTool
        .execute(serde_json::json!({ "source": s(&root.join("conf")), "destination": s(&root.join("pub")) }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("contains conf/.env"), "{err}");
    assert!(!root.join("pub").exists());
}

#[tokio::test]
async fn policy_file_is_protected_from_mutating_tools() {
    let policy_text = "deny = [\"private/**\"]\nlocal_only = true\n";
    let (_d, root) = project(Some(policy_text));
    let policy_path = root.join(POLICY_FILE);
    write(&root, "src/main.rs", "");
    write(&root, "other.toml", "deny = []\n");
    write(&root, "evil/privacy.toml", "deny = []\n");
    write(&root, "evil2/.phazeai/privacy.toml", "deny = []\n");
    let _g = activate(&root);
    assert!(active().local_only());

    let refused = |err: String, tool: &str| {
        assert!(err.contains("REFUSED"), "{tool}: {err}");
        assert!(err.contains("privacy policy"), "{tool}: {err}");
        assert!(err.contains("edited by you"), "{tool}: {err}");
    };
    let unchanged = || {
        assert_eq!(std::fs::read_to_string(&policy_path).unwrap(), policy_text);
        assert!(active().local_only(), "the policy must still be in force");
    };

    // write_file, also through `..` and (on Unix) a symlink.
    let mut targets = vec![
        s(&policy_path),
        s(&root.join("src/../.phazeai/privacy.toml")),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&policy_path, root.join("p.toml")).unwrap();
        targets.push(s(&root.join("p.toml")));
    }
    for target in &targets {
        let err = WriteFileTool
            .execute(
                serde_json::json!({ "path": target, "content": "deny = []\nlocal_only = false\n" }),
            )
            .await
            .unwrap_err()
            .to_string();
        refused(err, "write_file");
        unchanged();
    }

    let err = EditTool
        .execute(
            serde_json::json!({ "path": s(&policy_path), "old_text": "true", "new_text": "false" }),
        )
        .await
        .unwrap_err()
        .to_string();
    refused(err, "edit_file");
    unchanged();

    for target in [s(&policy_path), s(&root.join(".phazeai")), s(&root)] {
        let err = DeletePathTool
            .execute(serde_json::json!({ "path": target }))
            .await
            .unwrap_err()
            .to_string();
        refused(err, "delete_path");
        unchanged();
    }

    for (src, dst) in [
        (s(&policy_path), s(&root.join("moved.toml"))),
        (s(&root.join(".phazeai")), s(&root.join("hidden"))),
        (s(&root.join("other.toml")), s(&policy_path)),
        (s(&root.join("evil")), s(&root.join(".phazeai"))),
        (s(&root.join("evil2")), s(&root)),
    ] {
        let err = MovePathTool
            .execute(serde_json::json!({ "source": src, "destination": dst }))
            .await
            .unwrap_err()
            .to_string();
        refused(err, "move_path");
        unchanged();
        assert!(root.join("other.toml").exists() && !root.join("moved.toml").exists());
    }

    for (src, dst) in [
        (s(&root.join("other.toml")), s(&policy_path)),
        (s(&root.join("evil")), s(&root.join(".phazeai"))),
        (s(&root.join("evil2")), s(&root)),
    ] {
        let err = CopyPathTool
            .execute(serde_json::json!({ "source": src, "destination": dst }))
            .await
            .unwrap_err()
            .to_string();
        refused(err, "copy_path");
        unchanged();
    }

    // Reading and copying the policy elsewhere are fine; so are other files in .phazeai/.
    let out = ReadFileTool
        .execute(serde_json::json!({ "path": s(&policy_path) }))
        .await
        .unwrap();
    assert!(out["content"].as_str().unwrap().contains("private/**"));
    CopyPathTool
        .execute(serde_json::json!({ "source": s(&policy_path), "destination": s(&root.join("backup.toml")) }))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("backup.toml")).unwrap(),
        policy_text
    );
    WriteFileTool
        .execute(serde_json::json!({ "path": s(&root.join(".phazeai/designs/home.html")), "content": "<p>" }))
        .await
        .unwrap();
    CopyPathTool
        .execute(serde_json::json!({ "source": s(&root.join("evil")), "destination": s(&root.join("elsewhere")) }))
        .await
        .unwrap();
    assert!(audit::recent(audit::CAPACITY).iter().any(|e| matches!(
        &e.kind,
        audit::AuditKind::BlockedFile { reason, via, .. } if reason == PROTECTED_REASON && via == "write_file"
    )));
}

#[tokio::test]
async fn policy_path_is_protected_before_the_file_exists() {
    let (_d, root) = project(None);
    let _g = activate(&root);
    let target = s(&root.join(POLICY_FILE));
    let err = CreateDirectoryTool
        .execute(serde_json::json!({ "path": target }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("REFUSED") && err.contains("edited by you"),
        "{err}"
    );
    let err = WriteFileTool
        .execute(serde_json::json!({ "path": target, "content": "deny = []\n" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("REFUSED"), "{err}");
    assert!(!root.join(POLICY_FILE).exists());
    assert!(!active().file_exists());
    // The directory itself may be created (skills, designs live there).
    CreateDirectoryTool
        .execute(serde_json::json!({ "path": s(&root.join(".phazeai/skills")) }))
        .await
        .unwrap();
}

#[tokio::test]
async fn bash_refuses_commands_naming_the_policy_file() {
    let (_d, root) = project(Some("deny = []\nlocal_only = true\n"));
    let _g = activate(&root);
    let bash = BashTool::new(root.clone());
    for cmd in [
        "echo 'local_only = false' > .phazeai/privacy.toml",
        "rm -rf .phazeai",
        "cat .phazeai/privacy.toml",
    ] {
        let err = bash
            .execute(serde_json::json!({ "command": cmd }))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("REFUSED") && err.contains("edited by you"),
            "{cmd}: {err}"
        );
    }
    assert!(active().local_only());
    let out = bash
        .execute(serde_json::json!({ "command": "echo .phazeai/designs" }))
        .await
        .unwrap();
    assert!(out["stdout"].as_str().unwrap().contains(".phazeai/designs"));
}

#[tokio::test]
async fn bash_refuses_commands_naming_denied_files() {
    let (_d, root) = project(None);
    write(&root, ".env", "SECRET=1");
    let _g = activate(&root);
    let bash = BashTool::new(root.clone());
    let err = bash
        .execute(serde_json::json!({ "command": "cat .env" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("REFUSED") && err.contains(".env"), "{err}");
    let out = bash
        .execute(serde_json::json!({ "command": "echo fine" }))
        .await
        .unwrap();
    assert!(out["stdout"].as_str().unwrap().contains("fine"));
}

// ── MCP tool calls ──────────────────────────────────────────────────────────

use super::mcp::{check_call, McpChecked, McpDenial, McpRefusal, McpServerPaths, Role};

/// A server whose relative paths resolve against `bases` and whose `~` is `home`.
fn server(bases: Vec<PathBuf>, home: Option<PathBuf>) -> McpServerPaths {
    McpServerPaths {
        bases,
        home,
        tree_dirs: false,
    }
}

fn mcp_denials(
    policy: &PrivacyPolicy,
    tool: &str,
    read_only: bool,
    args: serde_json::Value,
    srv: &McpServerPaths,
) -> Vec<McpDenial> {
    match check_call(policy, tool, read_only, &args, srv) {
        Err(McpRefusal::Denied(d)) => d,
        other => panic!("expected a denial, got {other:?}"),
    }
}

fn mcp_passes(
    policy: &PrivacyPolicy,
    tool: &str,
    read_only: bool,
    args: serde_json::Value,
    srv: &McpServerPaths,
) -> McpChecked {
    match check_call(policy, tool, read_only, &args, srv) {
        Ok(c) => c,
        other => panic!("expected the call to pass, got {other:?}"),
    }
}

#[test]
fn mcp_path_args_take_every_string_but_free_text_and_urls() {
    use super::mcp::path_args;
    let args = serde_json::json!({
        "path": "a.txt",
        "paths": ["b.txt", "c.txt"],
        "files": [{ "path": "nested.txt" }],
        "source": "s",
        "destination": "d",
        "command": "cat x",
        "args": ["--flag", "y"],
        "content": "free prose",
        "newText": "/abs/in/free",
    });
    let pargs = path_args(&args).unwrap();
    let role = |k: &str, raw: &str| {
        pargs
            .iter()
            .find(|p| p.key == k && p.raw == raw)
            .map(|p| p.role)
    };
    assert_eq!(role("path", "a.txt"), Some(Role::Plain));
    assert_eq!(role("paths", "b.txt"), Some(Role::Plain));
    assert_eq!(role("paths", "c.txt"), Some(Role::Plain));
    // An array element inherits its array's key, and nested objects recurse.
    assert_eq!(role("path", "nested.txt"), Some(Role::Plain));
    assert_eq!(role("source", "s"), Some(Role::Source));
    assert_eq!(role("destination", "d"), Some(Role::Destination));
    assert_eq!(role("command", "cat x"), Some(Role::Command));
    assert_eq!(role("args", "--flag"), Some(Role::Command));
    assert_eq!(role("content", "free prose"), Some(Role::FreeText));
    assert_eq!(role("newText", "/abs/in/free"), Some(Role::FreeText));

    // More than MAX_STRINGS strings → refused rather than checked in part.
    let many: Vec<serde_json::Value> = (0..10_001)
        .map(|i| serde_json::json!(format!("f{i}")))
        .collect();
    assert!(path_args(&serde_json::json!({ "paths": many })).is_err());
}

#[test]
fn mcp_call_naming_a_denied_path_is_refused() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    write(&root, ".env", "API_KEY=supersecret");
    write(&root, "private/plan.md", "confidential");
    write(&root, "notes.txt", "ok");
    let home = tempfile::tempdir().unwrap();
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], Some(home.path().to_path_buf()));

    let one = |args: serde_json::Value| mcp_denials(&p, "read_text_file", true, args, &srv);

    // Relative, absolute, and `..`-laundered spellings of a built-in secret.
    let d = one(serde_json::json!({ "path": ".env" }));
    assert_eq!(d[0].denial.source, DenySource::BuiltIn);
    assert_eq!(d[0].denial.pattern, ".env");
    assert!(!one(serde_json::json!({ "path": s(&root.join(".env")) })).is_empty());
    assert!(!one(serde_json::json!({ "path": "src/../.env" })).is_empty());

    // A file:// URI.
    let uri = format!("file://{}", root.join(".env").display());
    assert!(!one(serde_json::json!({ "path": uri })).is_empty());

    // A project-denied path under an unusual key, and inside an array.
    let d = one(serde_json::json!({ "input": s(&root.join("private/plan.md")) }));
    assert_eq!(d[0].denial.source, DenySource::Project);
    assert_eq!(d[0].denial.pattern, "private/**");
    assert!(!one(serde_json::json!({ "paths": ["notes.txt", "private/plan.md"] })).is_empty());

    // `~` expands to the server's home, where the built-in list still applies.
    assert!(!one(serde_json::json!({ "path": "~/.ssh/id_rsa" })).is_empty());
}

#[cfg(unix)]
#[test]
fn mcp_symlink_to_a_denied_file_is_refused() {
    let (_d, root) = project(None);
    write(&root, ".env", "A=1");
    std::os::unix::fs::symlink(root.join(".env"), root.join("notes-link.txt")).unwrap();
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    let d = mcp_denials(
        &p,
        "read_text_file",
        true,
        serde_json::json!({ "path": "notes-link.txt" }),
        &srv,
    );
    assert_eq!(d[0].denial.source, DenySource::BuiltIn);
}

#[test]
fn mcp_relative_paths_are_checked_against_every_base() {
    // The server would resolve `config/...` against its allowed directory
    // `<root>/src`, not the project root. Checking only the root would miss it.
    let (_d, root) = project(Some(r#"deny = ["src/config/**"]"#));
    write(&root, "src/config/secret.yaml", "k: v");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.join("src")], None);
    let d = mcp_denials(
        &p,
        "read_text_file",
        true,
        serde_json::json!({ "path": "config/secret.yaml" }),
        &srv,
    );
    assert_eq!(d[0].denial.pattern, "src/config/**");
}

#[test]
fn mcp_directory_whose_contents_are_denied_is_refused() {
    let (_d, root) = project(Some(r#"deny = ["secrets/**"]"#));
    write(&root, "secrets/token.txt", "tok");
    write(&root, "src/main.rs", "fn main() {}");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);

    // `secrets/**` denies the contents, so listing the directory is refused…
    let d = mcp_denials(
        &p,
        "list_directory",
        true,
        serde_json::json!({ "path": "secrets" }),
        &srv,
    );
    assert!(
        d[0].shown.contains("everything inside it"),
        "{}",
        d[0].shown
    );
    // …but the project root and an ordinary directory are fine.
    mcp_passes(
        &p,
        "list_directory",
        true,
        serde_json::json!({ "path": "." }),
        &srv,
    );
    mcp_passes(
        &p,
        "list_directory",
        true,
        serde_json::json!({ "path": "src" }),
        &srv,
    );
}

#[test]
fn mcp_source_directory_holding_a_denied_file_is_refused() {
    // `mixed` is not wholesale-denied (only the .pem inside it is), so it is
    // refused by the whole-tree check, not the directory-contents check.
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    write(&root, "mixed/ok.txt", "ok");
    write(&root, "mixed/key.pem", "secret"); // built-in
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);

    // A move/copy source directory is checked as a whole tree.
    let d = mcp_denials(
        &p,
        "move_file",
        false,
        serde_json::json!({ "source": "mixed", "destination": "pub" }),
        &srv,
    );
    assert!(
        d[0].shown.contains("contains mixed/key.pem"),
        "{}",
        d[0].shown
    );

    // A plain directory argument is not checked as a tree, unless the server
    // opts into tree_dirs.
    mcp_passes(
        &p,
        "list_directory",
        true,
        serde_json::json!({ "path": "mixed" }),
        &srv,
    );
    let tree_srv = McpServerPaths {
        tree_dirs: true,
        ..server(vec![root.clone()], None)
    };
    let d = mcp_denials(
        &p,
        "list_directory",
        true,
        serde_json::json!({ "path": "mixed" }),
        &tree_srv,
    );
    assert!(
        d[0].shown.contains("contains mixed/key.pem"),
        "{}",
        d[0].shown
    );
}

#[test]
fn mcp_settings_dir_is_protected_from_tools_not_marked_read_only() {
    let (_d, root) = project(Some("deny = []\n"));
    write(&root, "tmp/privacy.toml", "deny = []\n");
    write(&root, "evil/.phazeai/privacy.toml", "deny = []\n");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);

    // A mutating tool may not name the policy file, the .phazeai directory, or
    // anything else inside it (the MCP server list lives there too).
    for path in [".phazeai/privacy.toml", ".phazeai", ".phazeai/mcp.json"] {
        let d = mcp_denials(
            &p,
            "write_file",
            false,
            serde_json::json!({ "path": path }),
            &srv,
        );
        assert_eq!(d[0].denial.source, DenySource::Protected, "{path}");
    }

    // Moving the project root (which contains the policy) is protected.
    let d = mcp_denials(
        &p,
        "move_file",
        false,
        serde_json::json!({ "source": ".", "destination": "backup" }),
        &srv,
    );
    assert!(d[0].shown.contains("privacy.toml"), "{}", d[0].shown);

    // Copying a tree onto the root so its privacy.toml lands in place.
    let d = mcp_denials(
        &p,
        "copy_file",
        false,
        serde_json::json!({ "source": "evil", "destination": "." }),
        &srv,
    );
    assert!(d[0].shown.contains("would replace"), "{}", d[0].shown);

    // A read-only tool may still read the policy.
    mcp_passes(
        &p,
        "read_text_file",
        true,
        serde_json::json!({ "path": ".phazeai/privacy.toml" }),
        &srv,
    );
}

#[test]
fn mcp_command_arguments_are_checked_word_by_word() {
    let (_d, root) = project(None);
    write(&root, ".env", "A=1");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    assert_eq!(
        mcp_denials(
            &p,
            "run",
            false,
            serde_json::json!({ "command": "cat .env" }),
            &srv
        )[0]
        .shown,
        ".env"
    );
    assert_eq!(
        mcp_denials(
            &p,
            "run",
            false,
            serde_json::json!({ "args": ["--config=.env.local"] }),
            &srv
        )[0]
        .shown,
        ".env.local"
    );
}

#[cfg(unix)]
#[test]
fn mcp_unicode_spellings_of_denied_names_are_refused() {
    let (_d, root) = project(Some("deny = [\"Client Files/\", \"priv\u{e9}/\"]\n"));
    write(&root, "Client Files/contract.txt", "confidential");
    write(&root, "priv\u{e9}/plan.txt", "confidential"); // NFC é
    write(&root, "notes.txt", "ok");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    let one = |path: &str| {
        mcp_denials(
            &p,
            "write_file",
            false,
            serde_json::json!({ "path": path }),
            &srv,
        )
    };

    // No-break space where the directory has a normal space.
    assert!(!one("Client\u{a0}Files/contract.txt").is_empty());
    // NFD spelling of an NFC directory name.
    assert!(!one("prive\u{301}/plan.txt").is_empty());
    // A new file under the no-break-space spelling of a denied directory.
    let d = one("Client\u{a0}Files/new.txt");
    assert!(
        d[0].shown.contains("the server would open"),
        "{}",
        d[0].shown
    );

    // A new ASCII name with no denied equivalent still passes.
    mcp_passes(
        &p,
        "write_file",
        false,
        serde_json::json!({ "path": "newnotes.txt" }),
        &srv,
    );
}

#[test]
fn mcp_free_text_remote_urls_and_allowed_paths_pass_untouched() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    write(&root, "private/plan.md", "x");
    write(&root, ".env.example", "A=");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    let ok = |args: serde_json::Value| mcp_passes(&p, "do", false, args, &srv);

    // Free text that merely mentions a denied relative path is not a path.
    ok(serde_json::json!({ "content": "see private/plan.md for the key" }));
    ok(serde_json::json!({ "title": "Rotate the .env secret" }));
    // A remote URL is skipped.
    ok(serde_json::json!({ "url": "https://example.com/private/plan.md" }));
    // Non-path arguments a GitHub-style server takes.
    ok(serde_json::json!({ "owner": "x", "repo": "y", "ref": "main" }));
    // An allowed template that the built-in list exempts.
    ok(serde_json::json!({ "path": ".env.example" }));
}

#[test]
fn mcp_successful_reads_are_noted_for_the_audit_log() {
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    write(&root, "notes.txt", "ok");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    let checked = mcp_passes(
        &p,
        "read_text_file",
        true,
        serde_json::json!({ "path": "notes.txt" }),
        &srv,
    );
    assert_eq!(checked.files, vec![root.join("notes.txt")]);
    // A free-text or command argument is never noted as a read file.
    let checked = mcp_passes(
        &p,
        "do",
        false,
        serde_json::json!({ "content": "notes.txt", "command": "ls notes.txt" }),
        &srv,
    );
    assert!(checked.files.is_empty());
}

#[test]
fn mcp_text_results_lose_denied_paths_and_diff_sections() {
    use super::mcp::filter_text;
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    let p = PrivacyPolicy::load(&root).unwrap();
    let bases = vec![root.clone()];

    // A path listing (search_files / list_allowed_directories shape).
    let listing = format!(
        "{}\n{}\n{}\n",
        root.join(".env").display(),
        root.join("notes.txt").display(),
        root.join("private/plan.md").display()
    );
    let (out, hidden) = filter_text(&p, &bases, None, &listing);
    assert!(out.contains("notes.txt"));
    assert!(!out.contains(".env") && !out.contains("plan.md"), "{out}");
    assert_eq!(hidden, 2);

    // A diff loses only the denied file's section, keeping its preamble.
    let diff = "On branch main\n\
                diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-a\n+b\n\
                diff --git a/.env b/.env\n--- a/.env\n+++ b/.env\n@@ -1 +1 @@\n-K=1\n+K=2\n";
    let (out, hidden) = filter_text(&p, &bases, None, diff);
    assert!(out.contains("On branch main") && out.contains("src/main.rs"));
    assert!(!out.contains("K=2"), "{out}");
    assert_eq!(hidden, 1);

    // Plain text that happens to contain a `--- .env` line but no `diff --git`
    // is never parsed as a diff.
    let prose = "readme\n--- .env\nnot a diff\n";
    let (out, hidden) = filter_text(&p, &bases, None, prose);
    assert_eq!(out, prose);
    assert_eq!(hidden, 0);
}

#[test]
fn mcp_filesystem_listings_hide_denied_entries() {
    use super::mcp::filter_fs_listing;
    let (_d, root) = project(Some(r#"deny = ["private/**"]"#));
    write(&root, ".env", "A=1");
    write(&root, ".env.example", "A=");
    write(&root, "notes.txt", "ok");
    write(&root, "private/plan.md", "x");
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    let args = serde_json::json!({ "path": s(&root) });

    // list_directory: the denied entries are dropped, the allowed ones kept.
    let text = "[FILE] .env\n[FILE] .env.example\n[FILE] notes.txt\n[DIR] private\n[DIR] src";
    let (out, hidden) = filter_fs_listing(&p, &srv, "list_directory", &args, text)
        .unwrap()
        .unwrap();
    assert!(out.contains("[FILE] .env.example") && out.contains("[FILE] notes.txt"));
    assert!(out.contains("[DIR] private")); // the directory entry itself isn't denied
    assert!(!out.contains("[FILE] .env\n") && !out.lines().any(|l| l == "[FILE] .env"));
    assert_eq!(hidden, 1);

    // list_directory_with_sizes: entry columns can't be split on whitespace,
    // so denied names come from the directory itself.
    let sized = "[FILE] .env                                 20 B\n\
                 [FILE] notes.txt                            14 B\n\
                 [DIR] private                       \n\
                 \n\
                 Total: 2 files, 1 directories\n\
                 Combined size: 34 B";
    let (out, hidden) = filter_fs_listing(&p, &srv, "list_directory_with_sizes", &args, sized)
        .unwrap()
        .unwrap();
    assert!(out.contains("notes.txt") && out.contains("Total: 2 files"));
    assert!(!out.contains(".env"), "{out}");
    assert_eq!(hidden, 1);

    // directory_tree: denied nodes are pruned, nesting preserved.
    let tree = "[\n  {\n    \"name\": \".env\",\n    \"type\": \"file\"\n  },\n  \
                {\n    \"name\": \"notes.txt\",\n    \"type\": \"file\"\n  },\n  \
                {\n    \"name\": \"private\",\n    \"type\": \"directory\",\n    \"children\": [\n      \
                {\n        \"name\": \"plan.md\",\n        \"type\": \"file\"\n      }\n    ]\n  }\n]";
    let (out, hidden) = filter_fs_listing(&p, &srv, "directory_tree", &args, tree)
        .unwrap()
        .unwrap();
    assert!(out.contains("notes.txt"));
    assert!(!out.contains(".env") && !out.contains("plan.md"), "{out}");
    assert_eq!(hidden, 2); // .env and private/plan.md

    // A non-listing tool is not handled here.
    assert!(filter_fs_listing(&p, &srv, "read_text_file", &args, "x").is_none());
}

#[test]
fn mcp_filesystem_listing_in_an_unknown_format_is_refused() {
    use super::mcp::filter_fs_listing;
    let (_d, root) = project(None);
    let p = PrivacyPolicy::load(&root).unwrap();
    let srv = server(vec![root.clone()], None);
    let args = serde_json::json!({ "path": s(&root) });

    // A line that is neither an entry nor a known summary line.
    let bad = "[FILE] ok.txt\nsomething unexpected";
    assert!(filter_fs_listing(&p, &srv, "list_directory", &args, bad)
        .unwrap()
        .is_err());
    // Not valid JSON for a tree.
    assert!(
        filter_fs_listing(&p, &srv, "directory_tree", &args, "not json")
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn mcp_bridge_refuses_before_contacting_the_server() {
    use crate::tools::mcp_bridge::McpToolBridge;
    let (_d, root) = project(Some("deny = []\n"));
    write(&root, ".env", "API_KEY=supersecret");
    let _g = activate(&root);

    // An empty manager: there is no server called "fs". The privacy check must
    // refuse the call before the bridge ever tries to reach one.
    let manager = Arc::new(Mutex::new(crate::mcp::McpManager::new()));
    let bridge = McpToolBridge::new(
        "fs".to_string(),
        "read_text_file".to_string(),
        "read a file".to_string(),
        serde_json::json!({ "type": "object" }),
        manager,
        None,
        true, // read-only, yet a denied path is still refused
        Arc::new(server(vec![root.clone()], None)),
        false,
    );
    let err = bridge
        .execute(serde_json::json!({ "path": ".env" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("REFUSED") && err.contains("mcp__fs__read_text_file"),
        "{err}"
    );
    assert!(
        !err.contains("No MCP server connected"),
        "reached the server: {err}"
    );
    assert!(!err.contains("supersecret"), "leaked the file: {err}");
    assert!(audit::recent(audit::CAPACITY).iter().any(|e| matches!(
        &e.kind,
        audit::AuditKind::BlockedFile { path, via, .. }
            if path == ".env" && via == "mcp__fs__read_text_file"
    )));
}
