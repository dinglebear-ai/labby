//! Native MCP adaptation regressions: shared team credentials never replace a caller.
use super::*;
use crate::mcp::route_scope::McpRouteScope;
use labby_gateway::codemode_journal::StepJournalStore;
use serde_json::json;
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

const TEAM: &str = "bootstrap-initial-team";

fn isolated_case(name: &str, flag: &str) -> Option<std::path::PathBuf> {
    if let Ok(home) = std::env::var(flag) {
        return Some(home.into());
    }
    let home = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env(flag, home.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            drop(child.kill());
            let output = child.wait_with_output().unwrap();
            panic!(
                "native snippet test deadline: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    None
}

fn team_route(binding_generation: Option<u64>) -> McpRouteScope {
    let route = ProtectedMcpRouteConfig {
        name: "snippet-team-route".into(),
        enabled: true,
        public_host: "mcp.example.test".into(),
        public_path: "/team".into(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".into(),
        scopes: vec![],
        health_path: None,
        target: Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                loadout: Some(format!("team:{TEAM}:snippets")),
                ..Default::default()
            },
        )),
    };
    let loadout = GatewayLoadoutConfig {
        name: format!("team:{TEAM}:snippets"),
        upstreams: binding_generation
            .map(|_| vec!["shared".into()])
            .unwrap_or_default(),
        services: vec!["snippets".into()],
        credential_bindings: binding_generation
            .map(|generation| {
                vec![labby_runtime::gateway_config::GatewayCredentialBindingRef {
                    upstream_name: "shared".into(),
                    binding_id: "snippet-binding".into(),
                    generation,
                }]
            })
            .unwrap_or_default(),
        ..Default::default()
    };
    let scope = McpRouteScope::from_protected_route(&route, &[loadout])
        .unwrap()
        .unwrap();
    assert_eq!(
        scope.team_credential_subject().as_deref(),
        Some(format!("team:{TEAM}").as_str())
    );
    scope
}

async fn manager(
    home: &Path,
    upstreams: Vec<crate::config::UpstreamConfig>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let journal = Arc::new(
        StepJournalStore::open(home.join("journal.db"))
            .await
            .unwrap(),
    );
    let manager = crate::dispatch::gateway::config_store::test_gateway_manager(
        home.join("config.toml"),
        crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
    )
    .with_step_journal(journal)
    .with_code_mode_runner_spawn(test_labby_runner_spawn());
    let config = crate::config::LabConfig {
        upstream: upstreams,
        code_mode: crate::config::CodeModeConfig {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    manager
        .seed_config_unchecked_for_tests(config.to_gateway_config())
        .await;
    Arc::new(manager)
}

fn save_source(home: &Path) {
    crate::dispatch::snippets::store::create_user_snippet(home, "native-authority",
        "---\nname: native-authority\ndescription: Native authority regression\ntools: []\n---\n```js\nasync () => { await writeArtifact('proof.txt', 'owner-only evidence', {contentType:'text/plain'}); return {ok:true}; }\n```", None, false).unwrap();
}

async fn native_call(
    server: &LabMcpServer,
    peer: rmcp::service::Peer<rmcp::RoleServer>,
    subject: &str,
    action: &str,
    params: Value,
) -> rmcp::model::CallToolResult {
    let mut context = scoped_context(peer.clone(), &["lab:admin"]);
    if action == "snippets.replay" {
        context.meta = request_context_with_elicitation(peer).meta;
    }
    let parts = context
        .extensions
        .get_mut::<axum::http::request::Parts>()
        .unwrap();
    let auth = parts
        .extensions
        .get_mut::<labby_auth::auth_context::AuthContext>()
        .unwrap();
    auth.sub = subject.into();
    assert!(
        auth.actor_key.is_none(),
        "exercise supported actor-key-unavailable mode"
    );
    let arguments = json!({"action":action,"params":params});
    eprintln!("native regression request: {arguments}");
    let mut request = CallToolRequestParams::new("snippets")
        .with_arguments(arguments.as_object().unwrap().clone());
    let response = Box::pin(server.call_tool(request.clone(), context.clone()))
        .await
        .unwrap();
    if action == "snippets.replay" {
        assert!(
            matches!(&response, CallToolResponse::InputRequired(_)),
            "replay must obtain a real bound confirmation challenge: {response:?}"
        );
    }
    let response = if let CallToolResponse::InputRequired(challenge) = response {
        assert_eq!(action, "snippets.replay", "only replay is destructive here");
        request.request_state = Some(
            challenge
                .request_state
                .expect("server-owned confirmation state"),
        );
        request.input_responses = Some(BTreeMap::from([(
            "destructive_confirmation".into(),
            json!({"action":"accept","content":{"confirm":true}}),
        )]));
        Box::pin(server.call_tool(request, context)).await.unwrap()
    } else {
        response
    };
    let CallToolResponse::Complete(result) = response else {
        panic!("confirmed native request must complete");
    };
    result
}

fn data(result: &rmcp::model::CallToolResult) -> Value {
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    result.structured_content.as_ref().unwrap()["data"].clone()
}

fn denied(result: &rmcp::model::CallToolResult, kind: &str) {
    assert_eq!(result.is_error, Some(true), "{result:?}");
    assert!(
        serde_json::to_string(result).unwrap().contains(kind),
        "{result:?}"
    );
}

#[test]
fn native_snippet_team_receipts_isolate_initiating_principals() {
    let Some(home) = isolated_case(
        "mcp::handlers_tools::tests::snippet_authority::native_snippet_team_receipts_isolate_initiating_principals",
        "LABBY_NATIVE_OWNER_TEST_CHILD",
    ) else {
        return;
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        save_source(&home);
        let server = test_server(crate::registry::build_default_registry(), Some(manager(&home, vec![]).await), team_route(None), crate::mcp::logging::LoggingLevel::Emergency);
        let (transport, _client) = tokio::io::duplex(256 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
        let run = data(&native_call(running.service(), running.peer().clone(), "alice", "snippets.exec", json!({"name":"native-authority"})).await);
        assert_eq!(run["receipt_status"], "persisted");
        let id = run["execution_id"].as_str().unwrap();
        let own = data(&native_call(running.service(), running.peer().clone(), "alice", "snippets.history", json!({"name":"native-authority"})).await);
        assert_eq!(own["receipts"].as_array().unwrap().len(), 1);
        let other = data(&native_call(running.service(), running.peer().clone(), "bob", "snippets.history", json!({"name":"native-authority"})).await);
        assert!(other["receipts"].as_array().unwrap().is_empty(), "a team credential subject cannot own both users' receipts: {other}");
        denied(&native_call(running.service(), running.peer().clone(), "bob", "snippets.receipt", json!({"execution_id":id})).await, "unknown_execution");
        denied(&native_call(running.service(), running.peer().clone(), "bob", "snippets.artifact", json!({"execution_id":id,"path":"proof.txt"})).await, "unknown_execution");
        let artifact = data(&native_call(running.service(), running.peer().clone(), "alice", "snippets.artifact", json!({"execution_id":id,"path":"proof.txt"})).await);
        assert_eq!(artifact["bytes"], 19);
    });
}

#[test]
fn native_snippets_revalidate_team_binding_after_rotation_and_revocation() {
    let Some(home) = isolated_case(
        "mcp::handlers_tools::tests::snippet_authority::native_snippets_revalidate_team_binding_after_rotation_and_revocation",
        "LABBY_NATIVE_BINDING_TEST_CHILD",
    ) else {
        return;
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        save_source(&home);
        let (_directory, access) = uninitialized_test_access_runtime().await;
        access.bootstrap_owner(crate::access::BootstrapOwnerInput::new(primary_static_bearer_identity(), "Local", "Default").unwrap()).await.unwrap();
        let store = access.store().await.unwrap();
        let binding = || crate::access::PutTeamCredentialBinding {
            binding_id: "snippet-binding".into(), team_id: TEAM.into(), upstream_name: "shared".into(),
            custodian_principal_id: "bootstrap-owner".into(), rotated_at_millis: 1,
        };
        let first = store.put_team_gateway_credential_binding(binding()).await.unwrap();
        let mut server = test_server(crate::registry::build_default_registry(), Some(manager(&home, vec![fixture_oauth_upstream_config("shared")]).await), team_route(Some(first.generation)), crate::mcp::logging::LoggingLevel::Emergency);
        server.access_runtime = access;
        let (transport, _client) = tokio::io::duplex(256 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(server, transport, None);
        assert!(running.service().route_team_credentials_current().await);
        let preview = native_call(running.service(), running.peer().clone(), "alice", "snippets.preview", json!({"name":"native-authority","params":{}})).await;
        assert_eq!(data(&preview)["can_execute"], true, "current binding positive control");
        let rotated = store.put_team_gateway_credential_binding(binding()).await.unwrap();
        assert_ne!(rotated.generation, first.generation);
        assert!(!running.service().route_team_credentials_current().await);
        for (action, params) in [
            ("snippets.preview", json!({"name":"native-authority","params":{}})),
            ("snippets.exec", json!({"name":"native-authority","params":{}})),
            ("snippets.test", json!({"name":"native-authority","params":{},"live":true})),
            ("snippets.replay", json!({"execution_id":"snippet_missing","params":{},"expected_preview_fingerprint":"x","acknowledged_drift":[]})),
        ] {
            denied(&native_call(running.service(), running.peer().clone(), "alice", action, params).await, "forbidden");
        }
        store.revoke_team_gateway_credential_binding(TEAM.into(), "shared".into(), 2).await.unwrap();
        assert!(!running.service().route_team_credentials_current().await);
        denied(&native_call(running.service(), running.peer().clone(), "alice", "snippets.exec", json!({"name":"native-authority"})).await, "forbidden");
        let rows: i64 = rusqlite::Connection::open(home.join("journal.db")).unwrap().query_row("SELECT COUNT(*) FROM snippet_receipts", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 0, "denied native requests must not reach execution or retain started receipts");
    });
}
