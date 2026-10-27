use rmcp::model::{CallToolRequestParams, CallToolResponse, ClientCapabilities, Implementation};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde_json::json;

use super::tests::{context, request, server_fixture};
use crate::mcp::server::LabMcpServer;

struct AuthenticatedServer {
    server: LabMcpServer,
    owner: labby_auth::VerifiedIdentity,
}

impl AuthenticatedServer {
    fn authorize(&self, context: &mut RequestContext<RoleServer>) {
        // Test transport authentication: preserve all SDK-decoded request
        // metadata and attach only facts normally supplied by HTTP middleware.
        let authenticated = context_for_auth(context.peer.clone(), &self.owner);
        context.extensions = authenticated.extensions;
    }
}

fn context_for_auth(
    peer: rmcp::service::Peer<RoleServer>,
    owner: &labby_auth::VerifiedIdentity,
) -> RequestContext<RoleServer> {
    context(peer, owner, true)
}

impl ServerHandler for AuthenticatedServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        self.server.get_info()
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        self.authorize(&mut context);
        assert_eq!(
            context.meta.protocol_version(),
            Some(rmcp::model::ProtocolVersion::V_2026_07_28),
            "the wire fixture must negotiate modern request metadata"
        );
        assert!(
            context
                .meta
                .client_capabilities()
                .is_some_and(|caps| caps.supports_tasks())
        );
        self.server.call_tool(request, context).await
    }
    async fn get_task(
        &self,
        request: rmcp::model::GetTaskParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::GetTaskResult, rmcp::ErrorData> {
        self.authorize(&mut context);
        self.server.get_task(request, context).await
    }
    async fn cancel_task(
        &self,
        request: rmcp::model::CancelTaskParams,
        mut context: RequestContext<RoleServer>,
    ) -> Result<(), rmcp::ErrorData> {
        self.authorize(&mut context);
        self.server.cancel_task(request, context).await
    }
}

#[tokio::test]
async fn native_tasks_sdk_transport_roundtrip_queue_get_cancel_and_terminal_result() {
    use rmcp::ServiceExt as _;
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt as _};
    let (_directory, server, store, owner) = server_fixture().await;
    let (server_transport, client_transport) = tokio::io::duplex(16384);
    let mut server = tokio::spawn(async move {
        let running = AuthenticatedServer { server, owner }
            .serve(server_transport)
            .await
            .unwrap();
        running.waiting().await.unwrap();
    });
    let client = rmcp::model::ClientConfig::new(
        ClientCapabilities::builder().enable_tasks().build(),
        Implementation::new("native-task-wire-test", "1"),
    )
    .serve_with_lifecycle(
        client_transport,
        ClientLifecycleMode::Discover {
            preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
        },
    )
    .await
    .unwrap();
    let created = client
        .call_tool_once(request(
            "tasks.create",
            json!({"task_id":"wire-job",
        "idempotency_key":"wire-job-key","owner_kind":"personal","owner_id":"bootstrap-owner",
        "agent_id":"mcp-native-agent","input":"hello"}),
        ))
        .await
        .unwrap();
    assert!(matches!(created, CallToolResponse::Complete(_)));
    let response = client
        .call_tool_once(request("tasks.queue", json!({"task_id":"wire-job"})))
        .await
        .unwrap();
    let CallToolResponse::Task(created) = response else {
        panic!("wire response must be a standard MCP Task, got {response:?}")
    };
    let polled = client
        .peer()
        .get_task(rmcp::model::GetTaskParams::new(&created.task.task_id))
        .await
        .unwrap();
    assert_eq!(polled.task.task.task_id, created.task.task_id);
    // Cancellation races a real executor. Its rejection is valid once the
    // existing durable attempt is terminal; no artificial executor is added.
    let cancellation = client
        .peer()
        .cancel_task(rmcp::model::CancelTaskParams::new(&created.task.task_id))
        .await;
    let mut terminal = None;
    for _ in 0..100 {
        let polled = client
            .peer()
            .get_task(rmcp::model::GetTaskParams::new(&created.task.task_id))
            .await
            .unwrap();
        if polled.task.status().is_terminal() {
            terminal = Some(polled);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let terminal = terminal.expect("real execution or cancellation settles within two seconds");
    assert!(matches!(
        terminal.task.payload,
        rmcp::model::TaskPayload::Cancelled
            | rmcp::model::TaskPayload::Failed { .. }
            | rmcp::model::TaskPayload::Completed { .. }
    ));
    if cancellation.is_ok() {
        assert!(matches!(
            terminal.task.status(),
            rmcp::model::TaskStatus::Cancelled
                | rmcp::model::TaskStatus::Failed
                | rmcp::model::TaskStatus::Completed
        ));
    }
    assert!(
        store
            .get_agent_task("wire-job".into())
            .await
            .unwrap()
            .unwrap()
            .attempt
            <= 1
    );
    client.cancel().await.unwrap();
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut server).await {
        Ok(result) => result.expect("native task wire server exits cleanly"),
        Err(_) => {
            server.abort();
            drop(server.await);
            panic!("native task wire server failed to shut down within five seconds");
        }
    }
}
