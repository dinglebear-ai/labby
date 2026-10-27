use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ClientCapabilities, Implementation, NumberOrString,
    ProtocolVersion, RequestMetaObject,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde_json::{Value, json};

use crate::dispatch::agents::test_support::{agent_context, agent_params, fixture};
use crate::mcp::server::LabMcpServer;

pub(super) async fn server_fixture() -> (
    tempfile::TempDir,
    LabMcpServer,
    crate::access::AccessStore,
    labby_auth::VerifiedIdentity,
) {
    let (directory, store, owner) = fixture().await;
    crate::dispatch::agents::dispatch(
        agent_context(&store, &owner),
        "agents.create",
        agent_params("mcp-native-agent"),
    )
    .await
    .unwrap();
    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        access_runtime: Arc::new(
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
        ),
        installation_id: None,
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        #[cfg(feature = "gateway")]
        gateway_manager: None,
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Default::default(),
        #[cfg(feature = "gateway")]
        client_registry: Default::default(),
        transport_label: "http",
        logging_level: Arc::new(std::sync::atomic::AtomicU8::new(
            crate::mcp::logging::logging_level_rank(crate::mcp::logging::LoggingLevel::Info),
        )),
        route_scope: Default::default(),
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    (directory, server, store, owner)
}

pub(super) fn context(
    peer: rmcp::service::Peer<RoleServer>,
    identity: &labby_auth::VerifiedIdentity,
    tasks: bool,
) -> RequestContext<RoleServer> {
    let mut context = RequestContext::new(NumberOrString::Number(1), peer);
    context.extensions.insert(identity.clone());
    let (mut parts, _) = axum::http::Request::new(()).into_parts();
    parts
        .extensions
        .insert(labby_auth::auth_context::AuthContext {
            sub: "owner-subject".into(),
            issuer: "https://accounts.google.com".into(),
            scopes: vec!["lab".into()],
            actor_key: None,
            email: None,
            via_session: false,
            csrf_token: None,
        });
    context.extensions.insert(parts);
    context.meta = RequestMetaObject::with_client_context(
        ProtocolVersion::V_2026_07_28,
        Implementation::new("native-task-test", "1"),
        if tasks {
            ClientCapabilities::builder().enable_tasks().build()
        } else {
            ClientCapabilities::default()
        },
    );
    context
}

pub(super) fn request(action: &str, params: Value) -> CallToolRequestParams {
    CallToolRequestParams::new("tasks").with_arguments(
        json!({"action":action,"params":params})
            .as_object()
            .unwrap()
            .clone(),
    )
}

async fn create(server: &LabMcpServer, context: RequestContext<RoleServer>, id: &str) {
    let result = server
        .call_tool(
            request(
                "tasks.create",
                json!({
                    "task_id":id,"idempotency_key":format!("{id}-key"),
                    "owner_kind":"personal","owner_id":"bootstrap-owner",
                    "agent_id":"mcp-native-agent","input":"hello"
                }),
            ),
            context,
        )
        .await
        .unwrap();
    let CallToolResponse::Complete(result) = result else {
        panic!("create stays an ordinary operation")
    };
    assert_ne!(result.is_error, Some(true), "{result:?}");
}

#[tokio::test]
async fn native_tasks_queue_returns_standard_task_from_the_mcp_entrypoint() {
    let (_directory, server, store, owner) = server_fixture().await;
    let (transport, _client) = tokio::io::duplex(4096);
    let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    create(
        running.service(),
        context(running.peer().clone(), &owner, true),
        "native-one",
    )
    .await;
    let response = running
        .service()
        .call_tool(
            request("tasks.queue", json!({"task_id":"native-one"})),
            context(running.peer().clone(), &owner, true),
        )
        .await
        .unwrap();
    let CallToolResponse::Task(task) = response else {
        panic!("negotiated native queue must return a standard MCP Task, got {response:?}")
    };
    assert!(task.task.task_id.starts_with("labby:task:"));
    assert!(task.task.ttl_ms.is_some());
    assert!(task.task.poll_interval_ms.is_some());
    let polled = running
        .service()
        .get_task(
            rmcp::model::GetTaskParams::new(&task.task.task_id),
            context(running.peer().clone(), &owner, true),
        )
        .await
        .unwrap();
    assert_eq!(polled.task.task.task_id, task.task.task_id);
    assert!(
        store
            .get_agent_task("native-one".into())
            .await
            .unwrap()
            .unwrap()
            .attempt
            <= 1
    );
    running.cancel().await.unwrap();
}

#[tokio::test]
async fn native_tasks_capability_is_available_without_an_upstream_pool() {
    let (_directory, server, _, _) = server_fixture().await;
    assert!(server.get_info().capabilities.supports_tasks());
}

#[tokio::test]
async fn native_tasks_non_opt_in_queue_stays_ordinary_and_never_replays() {
    let (_directory, server, store, owner) = server_fixture().await;
    let (transport, _client) = tokio::io::duplex(4096);
    let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    create(
        running.service(),
        context(running.peer().clone(), &owner, false),
        "ordinary",
    )
    .await;
    let response = running
        .service()
        .call_tool(
            request("tasks.queue", json!({"task_id":"ordinary"})),
            context(running.peer().clone(), &owner, false),
        )
        .await
        .unwrap();
    assert!(matches!(response, CallToolResponse::Complete(_)));
    for _ in 0..100 {
        if store
            .get_agent_task("ordinary".into())
            .await
            .unwrap()
            .unwrap()
            .state
            .terminal()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let before = store
        .get_agent_task("ordinary".into())
        .await
        .unwrap()
        .unwrap();
    assert!(before.state.terminal(), "bounded executor settlement");
    let duplicate = running
        .service()
        .call_tool(
            request("tasks.queue", json!({"task_id":"ordinary"})),
            context(running.peer().clone(), &owner, true),
        )
        .await
        .unwrap();
    let CallToolResponse::Complete(duplicate) = duplicate else {
        panic!("terminal queue replay must be refused")
    };
    assert_eq!(duplicate.is_error, Some(true));
    let after = store
        .get_agent_task("ordinary".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.attempt, before.attempt);
    assert_eq!(after.state, before.state);
    running.cancel().await.unwrap();
}

#[tokio::test]
async fn native_tasks_pending_cancellation_is_durable_and_owner_scoped() {
    let (_directory, server, store, owner) = server_fixture().await;
    let (transport, _client) = tokio::io::duplex(4096);
    let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    create(
        running.service(),
        context(running.peer().clone(), &owner, true),
        "pending",
    )
    .await;
    // The durable queue transition has committed, but its worker has not yet
    // acquired an attempt. This is also the restart/recovery handoff state.
    let seed = running
        .service()
        .native_task_seed(
            &json!({"task_id":"pending"}),
            &context(running.peer().clone(), &owner, true),
        )
        .await
        .unwrap();
    let now = i64::try_from(labby_runtime::agent_runtime::system_now_millis()).unwrap();
    store
        .transition_agent_task(
            "pending".into(),
            labby_primitives::task::TaskState::Created,
            labby_primitives::task::TaskState::Queued,
            owner.safe_fingerprint(),
            0,
            now,
        )
        .await
        .unwrap();
    let stranger = crate::dispatch::agents::test_support::browser("stranger-subject");
    for operation in ["get", "cancel"] {
        let ctx = context(running.peer().clone(), &stranger, true);
        let error = if operation == "get" {
            running
                .service()
                .get_task(rmcp::model::GetTaskParams::new(&seed.task_id), ctx)
                .await
                .map(drop)
        } else {
            running
                .service()
                .cancel_task(rmcp::model::CancelTaskParams::new(&seed.task_id), ctx)
                .await
        }
        .unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
    assert_eq!(
        store
            .get_agent_task("pending".into())
            .await
            .unwrap()
            .unwrap()
            .state,
        labby_primitives::task::TaskState::Queued
    );
    running
        .service()
        .cancel_task(
            rmcp::model::CancelTaskParams::new(&seed.task_id),
            context(running.peer().clone(), &owner, true),
        )
        .await
        .unwrap();
    let cancelled = running
        .service()
        .get_task(
            rmcp::model::GetTaskParams::new(&seed.task_id),
            context(running.peer().clone(), &owner, true),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.task.status(), rmcp::model::TaskStatus::Cancelled);
    let row = store
        .get_agent_task("pending".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, labby_primitives::task::TaskState::Cancelled);
    assert_eq!(
        row.attempt, 0,
        "cancellation before worker acquisition keeps the same protocol job"
    );
    running.cancel().await.unwrap();
}

#[tokio::test]
async fn native_tasks_route_and_legacy_requests_cannot_use_a_root_handle() {
    let (_directory, mut server, store, owner) = server_fixture().await;
    let summary = crate::dispatch::tasks::dispatch(crate::dispatch::tasks::TaskDispatchContext {
        store:store.clone(),identity:owner.clone(),ceiling:crate::access::AuthorityCeiling::trusted_local(),
    }, "tasks.create", json!({"task_id":"routed","idempotency_key":"routed-key",
        "owner_kind":"personal","owner_id":"bootstrap-owner","agent_id":"mcp-native-agent","input":"hello"})).await.unwrap();
    assert_eq!(summary["state"], "created");
    let summary = crate::dispatch::tasks::dispatch(
        crate::dispatch::tasks::TaskDispatchContext {
            store,
            identity: owner.clone(),
            ceiling: crate::access::AuthorityCeiling::trusted_local(),
        },
        "tasks.get",
        json!({"task_id":"routed"}),
    )
    .await
    .unwrap();
    let root_handle = server.native_task_metadata(&summary).unwrap();
    server.route_scope = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "narrow",
        ["upstream"],
        ["tasks"],
        false,
    );
    let (transport, _client) = tokio::io::duplex(4096);
    let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    for tasks in [true, false] {
        let error = running
            .service()
            .get_task(
                rmcp::model::GetTaskParams::new(&root_handle.task_id),
                context(running.peer().clone(), &owner, tasks),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
    running.cancel().await.unwrap();
}
