// Real caller-bound MCP regressions for Access error-contract preservation.

use super::*;
use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput, CreateTeamInput};
use crate::mcp::permanent_tools::ToolProjectionMode;
use crate::mcp::route_scope::McpRouteScope;
use serde_json::json;
use std::time::Duration;

const TEAM: &str = "bootstrap-initial-team";

struct Fixture {
    running: rmcp::service::RunningService<rmcp::RoleServer, LabMcpServer>,
    _client: tokio::io::DuplexStream,
    store: AccessStore,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(bound: bool) -> Self {
        let directory = crate::access::test_support::secure_tempdir();
        let runtime = Arc::new(AccessRuntime::initialize(directory.path().join("access.db")).await);
        runtime
            .bootstrap_owner(
                BootstrapOwnerInput::new(primary_static_bearer_identity(), "Local", "Default")
                    .unwrap(),
            )
            .await
            .unwrap();
        let store = runtime.store().await.unwrap();
        let mut registry = crate::registry::build_default_registry();
        registry.set_tool_projection_mode(ToolProjectionMode::Both);
        let mut server = test_server(
            registry,
            None,
            McpRouteScope::Root,
            crate::mcp::logging::LoggingLevel::Emergency,
        );
        server.transport_label = "http";
        server.installation_id = bound.then(|| Arc::from("local-installation"));
        server.access_runtime = runtime;
        let (transport, client) = tokio::io::duplex(256 * 1024);
        let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        Self {
            running,
            _client: client,
            store,
            _directory: directory,
        }
    }

    async fn invoke(&self, action: &str, params: Value) -> rmcp::model::CallToolResult {
        self.invoke_named("access", json!({"action":action,"params":params}))
            .await
    }

    async fn invoke_named(&self, name: &str, params: Value) -> rmcp::model::CallToolResult {
        let response = Box::pin(
            self.running.service().call_tool(
                CallToolRequestParams::new(name.to_owned())
                    .with_arguments(params.as_object().unwrap().clone()),
                durable_scoped_context(self.running.peer().clone(), &["lab:admin"]),
            ),
        )
        .await
        .unwrap();
        let CallToolResponse::Complete(result) = response else {
            panic!("Access actions must complete directly")
        };
        result
    }
}

fn envelope(result: &rmcp::model::CallToolResult) -> Value {
    let text: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(result.structured_content.as_ref(), Some(&text));
    text
}

fn error(result: &rmcp::model::CallToolResult, action: &str, kind: &str, risk: &str) -> Value {
    assert_eq!(result.is_error, Some(true));
    let envelope = envelope(result);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["service"], "access");
    assert_eq!(envelope["action"], action);
    let error = &envelope["error"];
    assert_eq!(error["kind"], kind, "{envelope}");
    assert_eq!(error["contract_version"], 1, "{envelope}");
    assert_eq!(error["side_effects"], risk, "{envelope}");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    assert!(error["origin"].is_string());
    assert!(
        error["recovery"]["guidance"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    error.clone()
}

async fn revision_conflict(action: &str) {
    let fixture = Fixture::new(true).await;
    let first = fixture.invoke(action, json!({"team_id":TEAM})).await;
    assert_ne!(first.is_error, Some(true), "{first:?}");
    let revision = envelope(&first)["data"]["global_revision"].clone();
    fixture
        .store
        .create_team(
            CreateTeamInput::new(
                primary_static_bearer_identity(),
                "revision-change",
                "Changed",
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let result = fixture
        .invoke(action, json!({"team_id":TEAM,"snapshot_revision":revision}))
        .await;
    let error = error(&result, action, "conflict", "none_expected");
    assert_eq!(error["existing_id"], "access_inventory");
    assert_eq!(error["recovery"]["action"], "rediscover");
    assert_eq!(error["recovery"]["same_arguments"], "never");
    assert_eq!(
        error["recovery"]["guidance"],
        "Restart this inventory at offset zero."
    );
}

async fn admission_failure(action: &str) {
    let fixture = Fixture::new(true).await;
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::sync_channel(0);
    let store = fixture.store.clone();
    let holder = tokio::spawn(async move {
        crate::access::test_support::with_connection(&store, move |_| {
            entered.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    });
    started.await.unwrap();
    let result = fixture.invoke(action, json!({"team_id":TEAM})).await;
    release.send(()).unwrap();
    holder.await.unwrap();
    let error = error(&result, action, "queue_saturated", "none_expected");
    assert_eq!(error["origin"], "budget");
    assert!(error["recovery"]["action"].is_string());
    let next = fixture.invoke(action, json!({"team_id":TEAM})).await;
    assert_ne!(next.is_error, Some(true), "{next:?}");
}

async fn execution_timeout(action: &str) {
    let fixture = Fixture::new(true).await;
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::sync_channel(0);
    crate::access::test_support::with_connection(&fixture.store, move |connection| {
        let mut entered = Some(entered);
        connection
            .authorizer(Some(move |event: rusqlite::hooks::AuthContext<'_>| {
                if matches!(
                    event.action,
                    rusqlite::hooks::AuthAction::Read {
                        table_name: "principal_links",
                        ..
                    }
                ) && let Some(entered) = entered.take()
                {
                    entered.send(()).unwrap();
                    blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                rusqlite::hooks::Authorization::Allow
            }))
            .map_err(crate::access::test_support::sqlite_error)
    })
    .await
    .unwrap();
    let controller = async move {
        started.await.unwrap();
        // Cross the actual 1500 ms MCP inventory deadline after SQL entered.
        // The worker remains admitted until this gate releases and cleanup completes.
        let expired =
            tokio::time::timeout(Duration::from_millis(1500), std::future::pending::<()>()).await;
        assert!(expired.is_err());
        release.send(()).unwrap();
    };
    let (result, ()) = tokio::join!(fixture.invoke(action, json!({"team_id":TEAM})), controller);
    crate::access::test_support::with_connection(&fixture.store, |connection| {
        connection
            .authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            )
            .map_err(crate::access::test_support::sqlite_error)
    })
    .await
    .unwrap();
    let error = error(&result, action, "timeout", "none_expected");
    assert_eq!(error["origin"], "upstream_transport");
    assert!(error["recovery"]["action"].is_string());
    let next = fixture.invoke(action, json!({"team_id":TEAM})).await;
    assert_ne!(next.is_error, Some(true), "{next:?}");
}

async fn overflow(action: &str, limit: u32) {
    let fixture = Fixture::new(true).await;
    crate::access::test_support::with_connection(&fixture.store,|connection| {
        // Historical values remain legal SQLite rows beyond modern input limits.
        let id=format!("A{}","\"".repeat(31_000));
        connection.execute("INSERT INTO principals(principal_id,organization_id,kind,status,created_at,updated_at) VALUES(?1,'bootstrap-local','user','active',1,1)",[&id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO team_memberships VALUES('large-membership','bootstrap-local','bootstrap-initial-team',?1,'member','active',1,'bootstrap-owner',1,1,NULL)",[&id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO platform_administrators VALUES(?1,'revoked',1,'bootstrap-owner',1,1,1)",[&id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO team_invitations VALUES(?1,'bootstrap-local','bootstrap-initial-team','member',?2,'bootstrap-owner',1,'pending',NULL,1,4102444800,NULL,NULL,1)",rusqlite::params![[77u8;32].as_slice(),id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO projects VALUES(?1,'bootstrap-local','Large','active',1,1,1)",[&id]).map_err(crate::access::test_support::sqlite_error)?;
        connection.execute("INSERT INTO team_project_assignments VALUES('large-assignment','bootstrap-local','bootstrap-initial-team',?1,'viewer','revoked',1,'bootstrap-owner',1,1,1)",[&id]).map_err(crate::access::test_support::sqlite_error)?;
        Ok(())
    }).await.unwrap();
    let result = fixture
        .invoke(action, json!({"team_id":TEAM,"limit":limit}))
        .await;
    let error = error(&result, action, "response_too_large", "none_expected");
    assert_eq!(error["origin"], "budget");
    assert_eq!(error["budget_terminal"], limit == 1);
    assert_eq!(error["recovery"]["same_arguments"], "never");
    assert_eq!(
        error["recovery"]["action"],
        if limit == 1 {
            "inspect_and_escalate"
        } else {
            "reduce_work"
        }
    );
    assert!(result.content[0].as_text().unwrap().text.len() < 2048);
}

macro_rules! inventory_cases {
    ($revision:ident,$admission:ident,$timeout:ident,$page:ident,$single:ident,$action:literal) => {
        #[tokio::test]
        async fn $revision() {
            revision_conflict($action).await;
        }
        #[tokio::test]
        async fn $admission() {
            admission_failure($action).await;
        }
        #[tokio::test]
        async fn $timeout() {
            execution_timeout($action).await;
        }
        #[tokio::test]
        async fn $page() {
            overflow($action, 50).await;
        }
        #[tokio::test]
        async fn $single() {
            overflow($action, 1).await;
        }
    };
}
inventory_cases!(
    members_revision,
    members_admission,
    members_timeout,
    members_page,
    members_single,
    "access.team.member.list"
);
inventory_cases!(
    assignments_revision,
    assignments_admission,
    assignments_timeout,
    assignments_page,
    assignments_single,
    "access.team_project.list"
);
inventory_cases!(
    invitations_revision,
    invitations_admission,
    invitations_timeout,
    invitations_page,
    invitations_single,
    "access.team_invitation.list"
);
inventory_cases!(
    administrators_revision,
    administrators_admission,
    administrators_timeout,
    administrators_page,
    administrators_single,
    "access.platform_admin.list"
);

#[tokio::test]
async fn unbound_administrator_inventory_has_no_effects() {
    let fixture = Fixture::new(false).await;
    let result = fixture
        .invoke("access.platform_admin.list", json!({}))
        .await;
    let error = error(
        &result,
        "access.platform_admin.list",
        "service_unavailable",
        "none_expected",
    );
    assert_eq!(error["message"], "access inventory is unavailable");
}

#[tokio::test]
async fn unbound_platform_write_retains_conservative_risk() {
    let fixture = Fixture::new(false).await;
    let result = fixture
        .invoke(
            "access.platform_admin.grant",
            json!({"principal_id":"bootstrap-owner"}),
        )
        .await;
    let error = error(
        &result,
        "access.platform_admin.grant",
        "service_unavailable",
        "possible",
    );
    assert_eq!(
        error["message"],
        "installation-bound access administration is unavailable"
    );
}

#[tokio::test]
async fn committed_invitation_expiry_retains_possible_risk_and_repeat_has_no_effects() {
    let fixture = Fixture::new(true).await;
    fixture
        .store
        .create_team_invitation(
            crate::access::CreateTeamInvitationInput::new(
                primary_static_bearer_identity(),
                TEAM,
                "bootstrap-owner",
                crate::access::TeamRole::Member,
                [88; 32],
                3600,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    // Historical expiry timestamps exercise the normal committed-expiry operation.
    fixture
        .store
        .execute_test_statement(
            "UPDATE team_invitations SET created_at=0,expires_at=1,updated_at=0",
        )
        .await
        .unwrap();
    let before = store_effects(&fixture.store).await;
    let params = json!({"token":hex::encode([88;32])});
    let first = fixture
        .invoke("access.team_invitation.accept", params.clone())
        .await;
    let committed = store_effects(&fixture.store).await;
    assert_eq!(committed[1], before[1] + 1);
    assert_eq!(committed[2], before[2] + 1);
    assert_eq!(committed[3], before[3] + 1);
    let first = error(
        &first,
        "access.team_invitation.accept",
        "forbidden",
        "possible",
    );
    assert_eq!(first["origin"], "policy");
    assert_eq!(first["recovery"]["action"], "do_not_retry");
    assert_eq!(first["recovery"]["same_arguments"], "discouraged");
    assert_eq!(
        first["recovery"]["guidance"],
        "Refresh authorized invitation or Team metadata, or obtain a new invitation."
    );
    let repeat = fixture
        .invoke("access.team_invitation.accept", params)
        .await;
    let repeat = error(
        &repeat,
        "access.team_invitation.accept",
        "forbidden",
        "none_expected",
    );
    assert_eq!(repeat["recovery"]["same_arguments"], "never");
    assert_eq!(store_effects(&fixture.store).await, committed);
}

#[tokio::test]
async fn caller_bound_access_atomic_names_are_unavailable_without_store_effects() {
    let fixture = Fixture::new(true).await;
    let before = store_effects(&fixture.store).await;
    let registry = &fixture.running.service().registry;
    assert_eq!(
        registry.dispatch_capability("access"),
        Some(crate::registry::DispatchCapability::CallerBound)
    );
    for action in [
        "access.team.member.list",
        "access.team_project.list",
        "access.team_invitation.list",
        "access.platform_admin.list",
        "access.team_invitation.accept",
        "access.platform_admin.grant",
    ] {
        let name = format!("access.{action}");
        assert!(registry.resolve_atomic_action(&name).is_none());
        let result=fixture.invoke_named(&name,json!({"team_id":TEAM,"token":hex::encode([88;32]),"principal_id":"bootstrap-owner"})).await;
        assert_eq!(result.is_error, Some(true));
        let envelope = envelope(&result);
        assert_eq!(envelope["error"]["kind"], "internal_error", "{envelope}");
        assert_eq!(
            envelope["error"]["message"],
            format!("service `{name}` has no dispatcher wired")
        );
    }
    assert_eq!(store_effects(&fixture.store).await, before);
}

async fn store_effects(store: &AccessStore) -> Vec<i64> {
    crate::access::test_support::with_connection(store,|connection| {
        connection.query_row("SELECT total_changes(),global_revision,(SELECT count(*) FROM access_audit),(SELECT count(*) FROM authority_projection_outbox),(SELECT count(*) FROM team_invitations),(SELECT authority_epoch FROM platform_administrators WHERE principal_id='bootstrap-owner') FROM access_metadata WHERE singleton=1",[],|row| {
            Ok(vec![row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?])
        }).map_err(crate::access::test_support::sqlite_error)
    }).await.unwrap()
}
