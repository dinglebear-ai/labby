//! Explicit live qualification. Never creates a guest in ordinary test runs.
use super::{
    HelperArtifact,
    manager::{Manager, NativeOwner},
};
use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};

#[tokio::test]
#[ignore = "requires pinned Tailcat assets, Chromium, Microsandbox hardware and a live relay"]
async fn browser_to_native_project_to_network_disabled_vm() {
    let assets = PathBuf::from(
        std::env::var("LABBY_TAILCAT_ACCEPTANCE_BUILD")
            .expect("set absolute pinned build directory"),
    );
    assert!(assets.is_absolute());
    let node =
        std::env::var("LABBY_TAILCAT_ACCEPTANCE_NODE").expect("set explicit Node executable");
    let playwright = std::env::var("LABBY_TAILCAT_ACCEPTANCE_PLAYWRIGHT")
        .expect("set explicit Playwright module");
    let msb = std::env::var("LABBY_TAILCAT_ACCEPTANCE_MSB")
        .expect("set explicit compatible Microsandbox executable");
    let firmware = std::env::var("LABBY_TAILCAT_ACCEPTANCE_FIRMWARE")
        .expect("set matching Microsandbox firmware");
    assert!(PathBuf::from(&msb).is_absolute());
    assert!(PathBuf::from(&firmware).is_absolute());
    let version = std::process::Command::new(&msb)
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        "msb 0.7.6"
    );
    let allowed = [
        "sandbox_create",
        "sandbox_exec",
        "sandbox_stop",
        "sandbox_remove",
        "sandbox_list",
        "sandbox_inspect",
    ];
    let host_paths = tempfile::Builder::new()
        .prefix("tailcat-empty-")
        .tempdir_in("/private/tmp")
        .unwrap();
    let upstream: crate::config::UpstreamConfig = serde_json::from_value(serde_json::json!({
        "name":"msb", "command":node, "args":[PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/labby-microsandbox/server.mjs")],
        "env":{
            "MICROSANDBOX_MCP_HOST_PATH_POLICY":"allowlist",
            "MICROSANDBOX_MCP_HOST_PATHS":host_paths.path(),
            "MICROSANDBOX_MCP_ENABLE_DANGEROUS":"0",
            "MSB_PATH":msb, "MSB_LIBKRUNFW_PATH":firmware, "MSB_BACKEND":"local"
        },
        "expose_tools":allowed, "proxy_resources":false, "proxy_prompts":false
    }))
    .unwrap();
    let (directory, runtime, adapter, _, gateway) =
        super::testing::fixture_with_upstream(Some(upstream)).await;
    tokio::time::timeout(
        Duration::from_secs(90),
        gateway.reload_with_origin(None, None),
    )
    .await
    .unwrap()
    .unwrap();
    let key = labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap();
    let route: crate::config::ProtectedMcpRouteConfig = toml::from_str(
        r#"
name = "sandbox"
public_host = "labby.example"
public_path = "/sandbox"
scopes = ["lab"]
[target]
kind = "gateway_subset"
project_id = "bootstrap-default"
loadout = "sandbox"
"#,
    )
    .unwrap();
    let mut state = crate::api::AppState::new();
    state.config = Arc::new(crate::config::LabConfig {
        protected_mcp_routes: vec![route],
        loadouts: vec![crate::config::GatewayLoadoutConfig {
            name: "sandbox".into(),
            upstreams: vec!["msb".into()],
            expose_resources: false,
            expose_prompts: false,
            expose_skills: false,
            ..Default::default()
        }],
        ..Default::default()
    });
    let gateway = Arc::new(gateway);
    state.gateway_manager = Some(gateway.clone());
    state.installation_id = Some("machine".into());
    state.access_runtime = runtime.clone();
    state.access_credential_adapter = Some(adapter.clone());
    state.oauth_state = Some(Arc::new(
        labby_auth::state::AuthState::new(labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://labby.example").unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth.pem"),
            google: labby_auth::config::GoogleConfig {
                client_id: "fixture".into(),
                client_secret: "fixture".into(),
                ..Default::default()
            },
            token_encryption_key: Some(key.clone()),
            ..Default::default()
        })
        .await
        .unwrap(),
    ));
    let routers = crate::cli::serve::tailcat_test_projection(&state).unwrap();
    state = state.with_protected_mcp_routers(routers);
    let executable = assets.join("tailcat-bridge");
    let manager = Arc::new(
        Manager::new(
            NativeOwner {
                oauth_enabled: true,
                runtime,
                adapter,
                gateway: gateway.clone(),
                installation: "machine".into(),
                key: Arc::new(key),
                projection: Arc::new(move |authority, route| {
                    crate::api::tailcat::restricted_router(state.clone(), route, authority)
                }),
            },
            HelperArtifact {
                expected_sha256: Sha256::digest(std::fs::read(&executable).unwrap()).into(),
                executable,
                state_dir: directory.path().to_owned(),
                derp_map_url: "https://tailcat.dev/derpmap.json".into(),
            },
        )
        .unwrap(),
    );
    let socket = directory.path().join("control.sock");
    let controller = crate::api::tailcat::ControlListener::start(socket.clone(), manager.clone())
        .await
        .unwrap();
    let credential = directory.path().join("source-credential");
    crate::installation::secure_file::publish_private_artifact(
        &credential,
        format!(
            "lby_pc_v1_source_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
        )
        .as_bytes(),
    )
    .unwrap();
    let name = format!("labby-tailcat-{}", uuid::Uuid::new_v4().simple());
    // Persist only public recovery identity before any browser can create a VM.
    let recovery = assets.join("vm-acceptance-resource.json");
    std::fs::write(&recovery, serde_json::json!({"name":name,"owner":"lab-et7en","host":"local","networkDisabled":true,"hostMounts":false}).to_string()).unwrap();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/tailcat-bridge/integration/native-vm-browser.mjs");
    use process_wrap::tokio::{CommandWrap, ProcessGroup};
    use tokio::io::AsyncReadExt as _;
    let mut command = CommandWrap::with_new(&node, |cmd| {
        cmd.arg(fixture)
            .arg(&assets)
            .arg(&socket)
            .arg(&credential)
            .arg(directory.path())
            .arg(&playwright)
            .arg(&name)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
    });
    command.wrap(ProcessGroup::leader());
    let mut child = command.spawn().unwrap();
    let guard = labby_gateway::upstream::process_guard::ProcessGroupGuard::arm(child.id().unwrap());
    let mut stdout = child.stdout().take().unwrap().take(16 * 1024 + 1);
    let mut stderr = child.stderr().take().unwrap();
    let output = tokio::time::timeout(Duration::from_secs(210), async {
        let mut bytes = Vec::new();
        let reading = async { stdout.read_to_end(&mut bytes).await };
        let draining = async {
            let mut scratch = [0; 4096];
            while stderr.read(&mut scratch).await? != 0 {}
            Ok::<_, std::io::Error>(())
        };
        let (status, read, drained) = tokio::join!(child.wait(), reading, draining);
        read?;
        drained?;
        Ok::<_, std::io::Error>(std::process::Output {
            status: status?,
            stdout: bytes,
            stderr: Vec::new(),
        })
    })
    .await;
    drop(guard); // Covers Chromium descendants on success, timeout and failure.
    let label = format!("labby-tailcat-run={name}");
    let owned = tokio::process::Command::new(&msb)
        .args(["list", "--quiet", "--label", &label])
        .output()
        .await
        .unwrap();
    assert!(
        owned.status.success(),
        "ownership inventory unavailable; see recovery identity"
    );
    let owned_names = String::from_utf8(owned.stdout).unwrap();
    assert!(
        owned_names.lines().all(|entry| entry == name),
        "unexpected ownership label; no cleanup attempted"
    );
    if owned_names.lines().any(|entry| entry == name) {
        let cleanup = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new(&msb)
                .args(["remove", "--force", "--quiet", &name])
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            cleanup.status.success(),
            "owned VM compensation failed; see recovery identity"
        );
    }
    for (id, _) in manager.status().unwrap() {
        manager.stop(&id).await.unwrap();
    }
    drop(controller);
    drop(manager);
    // Clear only this fixture's durable config, then use the existing pool
    // reconciliation owner to close its upstream and child process group.
    std::fs::write(
        directory.path().join("gateway.toml"),
        toml::to_string(&labby_runtime::gateway_config::GatewayConfig::default()).unwrap(),
    )
    .unwrap();
    gateway.reload_with_origin(None, None).await.unwrap();
    let listed = tokio::process::Command::new(&msb)
        .args(["list", "--quiet"])
        .output()
        .await
        .unwrap();
    assert!(
        listed.status.success(),
        "cleanup verification inventory unavailable"
    );
    assert!(
        !String::from_utf8(listed.stdout)
            .unwrap()
            .lines()
            .any(|entry| entry == name),
        "owned VM remains; see recovery identity"
    );
    if let Ok(Ok(output)) = &output
        && output.stdout.len() <= 16 * 1024
        && serde_json::from_slice::<serde_json::Value>(&output.stdout).is_ok()
    {
        let _evidence_write =
            std::fs::write(assets.join("vm-acceptance-evidence.json"), &output.stdout);
    }
    let output = output
        .expect("browser acceptance deadline")
        .expect("browser driver launch");
    assert!(output.stdout.len() <= 16 * 1024, "browser evidence budget");
    assert!(
        output.status.success(),
        "browser/native/VM acceptance failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let evidence: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(evidence["ok"], true);
    std::fs::write(assets.join("vm-acceptance-evidence.json"), &output.stdout).unwrap();
}
