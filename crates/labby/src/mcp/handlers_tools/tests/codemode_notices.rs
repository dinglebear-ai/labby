//! Native MCP / actual JavaScript runner qualification for agent notices.
use super::{code_mode_manager, completion_test_registry, test_server};
use crate::mcp::{logging::LoggingLevel, route_scope::McpRouteScope};
use crate::notifications::codemode::{
    NoticeConsumer, NoticeLevel, NoticeProducer, NoticeRecipient, PublishNotice,
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ProtocolVersion},
    service::{ClientLifecycleMode, ClientServiceExt},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
fn message(inbox: String, key: &str) -> PublishNotice {
    PublishNotice {
        inbox_id: inbox,
        source: "native-test".into(),
        level: NoticeLevel::Info,
        message: "Indexing finished.".into(),
        dedupe_key: key.into(),
        ttl_seconds: 3600,
    }
}
fn producer() -> NoticeProducer {
    NoticeProducer {
        actor: "native-test-admin".into(),
        admin: true,
    }
}
#[tokio::test]
async fn notice_production_denied_execution_never_drains_inbox() {
    let mut server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        McpRouteScope::Root,
        LoggingLevel::Emergency,
    );
    server.route_runtime = Arc::new(
        crate::mcp::runtime::McpRouteRuntime::with_notification_store(
            crate::notifications::codemode::NoticeStore::memory().unwrap(),
        ),
    );
    let route = Arc::clone(&server.route_runtime);
    let who = NoticeRecipient {
        actor: Some("actor-alice".into()),
        route: "root".into(),
        consumer: NoticeConsumer::Client {
            id: "client".into(),
            conversation: None,
        },
    };
    let inbox = route
        .code_mode_notifications
        .register(who.clone())
        .await
        .unwrap();
    route
        .code_mode_notifications
        .publish(producer(), message(inbox.id, "event"))
        .await
        .unwrap();
    let (transport, _client) = tokio::io::duplex(65_536);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let mut context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );
    let (mut parts, _) = axum::http::Request::new(()).into_parts();
    parts
        .extensions
        .insert(labby_auth::auth_context::AuthContext {
            sub: "alice".into(),
            actor_key: Some(Arc::from("actor-alice")),
            scopes: vec!["lab:read".into()],
            issuer: "https://example.test".into(),
            via_session: false,
            csrf_token: None,
            email: None,
        });
    parts
        .extensions
        .insert(labby_auth::auth_context::AuthorizedClientId(Arc::from(
            "client",
        )));
    context.extensions.insert(parts);
    let args = json!({"code":"async () => 42","notification_inbox":true})
        .as_object()
        .unwrap()
        .clone();
    let denied = running
        .service()
        .call_tool_codemode_impl("codemode", &args, &context)
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    assert!(
        !serde_json::to_string(&denied)
            .unwrap()
            .contains("Indexing finished.")
    );
    assert!(
        route
            .code_mode_notifications
            .deliver(who, |v| Some(v.notifications.len()))
            .await
            .unwrap()
            .is_some()
    );
    running.cancel().await.unwrap();
}
#[tokio::test]
#[ignore = "requires LABBY_CODE_MODE_RUNNER_EXE built from this worktree and isolated LABBY_HOME"]
async fn notice_production_native_mcp_real_runner() {
    assert!(std::env::var_os("LABBY_CODE_MODE_RUNNER_EXE").is_some());
    let mut server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        McpRouteScope::Root,
        LoggingLevel::Emergency,
    );
    server.transport_label = "stdio";
    server.relay_session_id = crate::mcp::server::next_relay_session_id();
    server.route_runtime = Arc::new(
        crate::mcp::runtime::McpRouteRuntime::with_notification_store(
            crate::notifications::codemode::NoticeStore::memory().unwrap(),
        ),
    );
    let route = Arc::clone(&server.route_runtime);
    let (server_transport, client_transport) = tokio::io::duplex(262_144);
    let task = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
    });
    let client = tokio::time::timeout(
        Duration::from_secs(10),
        ().serve_with_lifecycle(
            client_transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let request = |args: Value| {
        CallToolRequestParams::new("codemode").with_arguments(args.as_object().unwrap().clone())
    };
    let registered = tokio::time::timeout(
        Duration::from_secs(30),
        client.call_tool(request(
            json!({"code":"async () => 0","notification_inbox":true}),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(registered.is_error, Some(true), "{registered:?}");
    let inbox = registered.structured_content.as_ref().unwrap()["notification_inbox"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let published = route
        .code_mode_notifications
        .publish(producer(), message(inbox.clone(), "event-1"))
        .await
        .unwrap();
    let first = tokio::time::timeout(
        Duration::from_secs(30),
        client.call_tool(request(json!({"code":"async () => ({ok:true,value:42})"}))),
    )
    .await
    .unwrap()
    .unwrap();
    let second = tokio::time::timeout(
        Duration::from_secs(30),
        client.call_tool(request(
            json!({"code":"async () => 43","ack_notifications":[published.id]}),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    route
        .code_mode_notifications
        .publish(producer(), message(inbox, "event-2"))
        .await
        .unwrap();
    let failed = tokio::time::timeout(
        Duration::from_secs(30),
        client.call_tool(request(
            json!({"code":"async () => {throw new Error('expected native failure');}"}),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    client.cancel().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text: Value = serde_json::from_str(&first.content[0].as_text().unwrap().text).unwrap();
    let structured = first.structured_content.as_ref().unwrap();
    assert_ne!(first.is_error, Some(true), "{first:?}");
    assert_eq!(structured["result"], json!({"ok":true,"value":42}));
    assert_eq!(text["notifications"], structured["notifications"]);
    assert_eq!(
        structured["notifications"][0]["message"],
        "Indexing finished."
    );
    assert_eq!(
        second.structured_content.as_ref().unwrap()["acknowledged_notifications"],
        json!([published.id])
    );
    assert!(
        second
            .structured_content
            .as_ref()
            .unwrap()
            .get("notifications")
            .is_none()
    );
    assert_eq!(failed.is_error, Some(true));
    assert!(
        failed
            .structured_content
            .as_ref()
            .unwrap()
            .get("notifications")
            .is_some()
    );
    println!(
        "NATIVE_NOTICE_EVIDENCE={}",
        json!({"registered":registered,"first":first,"acknowledged":second,"error":failed})
    );
}
