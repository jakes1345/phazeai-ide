use super::*;
use crate::llm::{LlmClient, LlmResponse, Message, ProviderConfig, ProviderRegistry, StreamEvent};
use crate::tools::{
    BashTool, CopyPathTool, EditTool, GlobTool, GrepTool, ListFilesTool, ReadFileTool, Tool,
    ToolDefinition, WriteFileTool,
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
