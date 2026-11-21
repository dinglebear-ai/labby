#![cfg(feature = "gateway")]
#![cfg(unix)]
#![allow(clippy::panic, dead_code)]

#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/lib.rs"]
mod support;

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use live_identity::{LiveIdentity, policy};
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::process::Command;

const OUTPUT_LIMIT: usize = 24 * 1024;

async fn read_capped(mut input: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut chunk = [0_u8; 4096];
    loop {
        let read = input
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > limit {
            return Err(format!("browser output exceeded {limit} byte cap"));
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn write_private_json(path: &Path, value: &serde_json::Value) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
    serde_json::to_writer(&mut file, value).expect("serialize private browser fixture");
    file.write_all(b"\n")
        .expect("finish private browser fixture");
}

fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/web")
        .canonicalize()
        .expect("Gateway Admin directory")
}

#[tokio::test]
async fn rust_supervisor_owns_live_backend_session_browser_and_cleanup() {
    supervise_live_browser(None).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn guided_add_server_adapters_complete_real_browser_canaries() {
    if std::env::var_os("LABBY_LIVE_BROWSER_RUN").is_none() {
        return;
    }
    let fixtures = GuidedAdapterFixtures::new();
    supervise_live_browser(Some(&fixtures)).await;
}

struct GuidedAdapterFixtures {
    // The fixture root must outlive the daemon and its stdio children.
    root: tempfile::TempDir,
    manifest: PathBuf,
    spec: PathBuf,
    labby: PathBuf,
    openapi: PathBuf,
}

#[cfg(target_os = "linux")]
impl GuidedAdapterFixtures {
    fn new() -> Self {
        // The CLI manifest guard rejects group/other-writable ancestors. Use
        // the owned checkout, not /tmp, for these non-secret source fixtures.
        let root = tempfile::Builder::new()
            .prefix(".guided-adapter-fixture-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("private adapter fixture directory in the checkout");
        let manifest = root.path().join("cli.toml");
        fs::write(
            &manifest,
            r#"
[[tools]]
name = "fixture_greeting"
description = "Return a deterministic fixture greeting."
executable = "/usr/bin/printf"
args = ["%s"]
read_only = true

[tools.choices]
world = ["Labby CLI browser canary"]
"#,
        )
        .expect("write owned CLI manifest");
        let spec = root.path().join("openapi.json");
        fs::write(
            &spec,
            serde_json::to_vec(&json!({
                "openapi": "3.1.0",
                "info": { "title": "Labby Browser Canary", "version": "1.0.0" },
                "paths": {
                    "/items/{itemId}": {
                        "get": {
                            "operationId": "getFixtureItem",
                            "parameters": [{
                                "name": "itemId", "in": "path", "required": true,
                                "schema": { "type": "string" }
                            }],
                            "responses": { "200": { "description": "OK" } }
                        }
                    },
                    "/unselected": {
                        "get": {
                            "operationId": "notSelected",
                            "responses": { "200": { "description": "OK" } }
                        }
                    }
                }
            }))
            .expect("serialize OpenAPI fixture"),
        )
        .expect("write owned OpenAPI spec");
        let cargo_labby = PathBuf::from(env!("CARGO_BIN_EXE_labby"));
        let labby = std::env::var_os("LABBY_E2E_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| cargo_labby.clone())
            .canonicalize()
            .expect("actual fixture Labby executable");
        let openapi = std::env::var_os("LABBY_E2E_OPENAPI_MCP_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| cargo_labby.with_file_name("labby-openapi-mcp"))
            .canonicalize()
            .expect("build the standalone adapter: cargo build -p labby-openapi --features standalone-mcp --bin labby-openapi-mcp --locked");
        Self {
            root,
            manifest,
            spec,
            labby,
            openapi,
        }
    }
}

async fn supervise_live_browser(adapters: Option<&GuidedAdapterFixtures>) {
    if std::env::var_os("LABBY_LIVE_BROWSER_RUN").is_none() {
        return;
    }
    let app_dir = app_dir();
    let assets = std::env::var_os("LABBY_LIVE_BROWSER_ASSETS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| app_dir.join("out"))
        .canonicalize()
        .expect("prebuilt Gateway Admin assets");
    assert!(
        assets.join("index.html").is_file(),
        "prebuilt index.html missing"
    );

    let mut config = format!(
        "{}\n[[loadouts]]\nname = \"team:{}:production\"\nupstreams = []\nservices = [\"gateway\"]\n\n[web]\nassets_dir = {}\n",
        policy(&["lab:read", "lab", "lab:admin"]).replacen(
            "services = [\"gateway\"]",
            "services = [\"gateway\", \"snippets\"]\nexpose_code_mode = false",
            1,
        ) + "\n[code_mode]\nenabled = true\n",
        support::LiveLabbyGuard::HARNESS_TEAM_ID,
        serde_json::to_string(&assets.display().to_string()).unwrap()
    );
    // Credential verification fences the Loadout policy fingerprint. Install
    // Snippets before issuing the browser credential, rather than expanding
    // its bound policy after bootstrap and invalidating the credential.
    let scopes = &["lab:read", "lab", "lab:admin"];
    let mut identity = if let Some(adapters) = adapters {
        config.push_str(&format!(
            "\n[gateway]\nextra_stdio_commands = {}\n",
            serde_json::to_string(&[&adapters.labby, &adapters.openapi])
                .expect("serialize exact adapter executable allowlist"),
        ));
        let root = adapters.root.path().to_str().expect("UTF-8 fixture root");
        LiveIdentity::bootstrap_with_policy_scopes_and_env(
            "guided-adapters-browser@labby.invalid",
            &config,
            scopes,
            &[
                ("LABBY_CLI_MCP_MANIFEST_ROOT", root),
                ("LABBY_OPENAPI_SPEC_ROOT", root),
                (
                    "LABBY_OPENAPI_MCP_BIN",
                    adapters.openapi.to_str().expect("UTF-8 adapter path"),
                ),
            ],
        )
        .await
    } else {
        LiveIdentity::bootstrap_with_policy_and_scopes(
            "live-browser@labby.invalid",
            &config,
            scopes,
        )
        .await
    }
    .expect("public identity bootstrap with final browser policy");
    identity
        .create_session()
        .await
        .expect("real browser session");
    #[cfg(target_os = "linux")]
    if adapters.is_none() {
        identity
            .provision_stash_recipient("browser-stash-recipient", "Browser Stash Recipient")
            .await
            .expect("provision Stash recipient through live identity fixture");
    }
    let session = identity.session.as_ref().expect("session materialized");
    let (cookie_name, cookie_value) = session.cookie.split_once('=').expect("cookie pair");
    // Chromium grants the Secure-cookie loopback exception to localhost,
    // not a raw loopback IP. The daemon remains bound to 127.0.0.1.
    let browser_base = identity.base().replace("127.0.0.1", "localhost");
    let secure_cookie_url = browser_base.replacen("http://", "https://", 1);
    let fixture_root = identity.root().join("browser-supervisor");
    let evidence_dir = fixture_root.join("evidence");
    fs::create_dir_all(&evidence_dir).expect("browser evidence directory");
    let storage_state = fixture_root.join("storage-state.json");
    let csrf_state = fixture_root.join("csrf-state.json");
    let scan_secrets = fixture_root.join("scan-secrets.txt");
    let restart_request = fixture_root.join("restart.request");
    let restart_complete = fixture_root.join("restart.complete");
    write_private_json(
        &storage_state,
        &json!({
            "cookies": [{
                "name": cookie_name,
                "value": cookie_value,
                "url": secure_cookie_url,
                "httpOnly": true,
                "secure": true,
                "sameSite": "Lax",
                "expires": session.expires_at
            }],
            "origins": []
        }),
    );
    write_private_json(&csrf_state, &json!({ "csrf_token": session.csrf }));
    let mut scan_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&scan_secrets)
        .expect("scan-only secrets");
    for secret in identity.exact_secret_canaries() {
        writeln!(scan_file, "{secret}").expect("write scan-only secret");
    }
    let descriptor = fixture_root.join("descriptor.json");
    write_private_json(
        &descriptor,
        &json!({
            "version": 1,
            "run_id": format!("browser-{}", ulid::Ulid::new()),
            "base_url": browser_base,
            "run_root": fixture_root,
            "storage_state_path": storage_state,
            "csrf_state_path": csrf_state,
            "evidence_dir": evidence_dir,
            "scan_secrets_path": scan_secrets,
            "restart_request_path": restart_request,
            "restart_complete_path": restart_complete,
            "stash_supported": cfg!(target_os = "linux"),
            "recipient_principal_id": "browser-stash-recipient",
            "nightly": std::env::var("LABBY_LIVE_BROWSER_NIGHTLY").as_deref() == Ok("true")
        }),
    );
    fs::create_dir_all(identity.root().join("browser-home")).expect("browser HOME");
    fs::create_dir_all(identity.root().join("tmp")).expect("browser TMPDIR");

    let node = PathBuf::from(std::env::var_os("LABBY_NODE_BIN").expect("absolute LABBY_NODE_BIN"));
    let browser_cache = PathBuf::from(
        std::env::var_os("PLAYWRIGHT_BROWSERS_PATH").expect("absolute PLAYWRIGHT_BROWSERS_PATH"),
    );
    assert!(node.is_absolute(), "LABBY_NODE_BIN must be absolute");
    assert!(
        browser_cache.is_absolute(),
        "PLAYWRIGHT_BROWSERS_PATH must be absolute"
    );
    let minimal_path = format!(
        "{}:/usr/bin:/bin:/usr/sbin:/sbin",
        node.parent().expect("node parent").display()
    );
    let browser_stdout = fixture_root.join("browser.stdout.log");
    let browser_stderr = fixture_root.join("browser.stderr.log");
    let browser_progress = fixture_root.join("browser.progress.log");
    let mut command = Command::new(&node);
    command
        .args([
            "--test",
            "--test-concurrency=1",
            "--experimental-strip-types",
            if adapters.is_some() {
                "lib/browser/live-adapters.browser.test.ts"
            } else {
                "lib/browser/live-backend.browser.test.ts"
            },
        ])
        .current_dir(&app_dir)
        .env_clear()
        .env("PATH", minimal_path)
        .env("HOME", identity.root().join("browser-home"))
        .env("TMPDIR", identity.root().join("tmp"))
        .env("PLAYWRIGHT_BROWSERS_PATH", browser_cache)
        .env("LABBY_LIVE_BROWSER_DESCRIPTOR", &descriptor)
        .env("LABBY_LIVE_BROWSER_PROGRESS", &browser_progress)
        .env(
            "LABBY_LIVE_BROWSER_NIGHTLY",
            std::env::var_os("LABBY_LIVE_BROWSER_NIGHTLY").unwrap_or_default(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(adapters) = adapters {
        command
            .env("LABBY_LIVE_BROWSER_ADAPTER_MANIFEST", &adapters.manifest)
            .env("LABBY_LIVE_BROWSER_ADAPTER_SPEC", &adapters.spec);
    }
    for key in ["LABBY_E2E_HELPER_REGISTRY", "LABBY_E2E_GROUP_TOKEN"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let mut child = command.spawn().expect("spawn browser");
    let stdout_task = tokio::spawn(read_capped(
        child.stdout.take().expect("stdout"),
        OUTPUT_LIMIT,
    ));
    let stderr_task = tokio::spawn(read_capped(
        child.stderr.take().expect("stderr"),
        OUTPUT_LIMIT,
    ));
    let status = tokio::time::timeout(Duration::from_secs(100), async {
        loop {
            if restart_request.exists() && !restart_complete.exists() {
                identity.restart().await.expect("browser-requested restart");
                fs::write(&restart_complete, b"complete\n").expect("publish restart completion");
            }
            if let Some(status) = child.try_wait().expect("poll browser") {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if status.is_err() {
        drop(child.kill().await);
    }
    let stdout = stdout_task.await.expect("stdout reader");
    let stderr = stderr_task.await.expect("stderr reader");
    let progress = fs::read(&browser_progress).unwrap_or_default();
    let progress_text = String::from_utf8_lossy(&progress);
    let canaries_ran = adapters.is_none()
        || ["guided-cli-completed", "guided-openapi-completed"]
            .iter()
            .all(|marker| progress_text.lines().filter(|line| line == marker).count() == 1);
    let browser_failure = match (status, stdout, stderr, progress.len() <= OUTPUT_LIMIT) {
        (Ok(status), Ok(_), Ok(_), true) if status.success() && canaries_ran => {
            if adapters.is_some() {
                eprint!("{progress_text}");
            }
            None
        }
        (status, stdout, stderr, progress_ok) => Some(format!(
            "Playwright failed: status={status:?}; stdout={}; stderr={}; progress={progress_text}; progress_ok={progress_ok}; canaries_ran={canaries_ran}",
            String::from_utf8_lossy(&stdout.unwrap_or_default()),
            String::from_utf8_lossy(&stderr.unwrap_or_default()),
        )),
    };
    fs::write(&browser_stdout, browser_failure.as_deref().unwrap_or(""))
        .expect("bounded stdout evidence");
    fs::write(&browser_stderr, browser_failure.as_deref().unwrap_or(""))
        .expect("bounded stderr evidence");
    for secret_fixture in [&descriptor, &storage_state, &csrf_state, &scan_secrets] {
        fs::remove_file(secret_fixture).expect("remove scan-only browser fixture");
    }
    let cleanup = identity.cleanup().await;
    assert!(
        cleanup.is_ok(),
        "browser supervisor cleanup failed: {cleanup:?}"
    );
    assert!(
        browser_failure.is_none(),
        "{}",
        browser_failure.unwrap_or_default()
    );
}

#[tokio::test]
async fn browser_output_reader_enforces_cap_while_reading() {
    assert_eq!(read_capped(&b"bounded"[..], 7).await.unwrap(), b"bounded");
    assert!(read_capped(&b"too-large"[..], 4).await.is_err());
}
