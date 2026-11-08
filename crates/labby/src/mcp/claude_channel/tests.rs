use super::*;
use crate::dispatch::codemode_notices::{NoticeProducer, PublishNotice};
use crate::mcp::server::LabMcpServer;
use rmcp::ServiceExt as _;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};

async fn fixture(
    directory: &std::path::Path,
    store: NoticeStore,
    enabled: bool,
) -> (
    Arc<ChannelSession>,
    BufReader<DuplexStream>,
    tokio::task::JoinHandle<()>,
) {
    let channel = ChannelSession::new(store.clone());
    let mut runtime = crate::mcp::runtime::McpRouteRuntime::with_notification_store(store);
    if enabled {
        runtime.claude_channel = Some(channel.clone());
    }
    let gateway = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            directory.join("gateway.toml"),
            Default::default(),
        ),
    );
    let server = LabMcpServer {
        registry: Arc::new(crate::registry::build_default_registry()),
        installation_id: None,
        access_runtime: Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
        file_stash_runtime: Arc::new(crate::file_stash::FileStashRuntime::blocked()),
        gateway_manager: Some(gateway),
        peers: Default::default(),
        code_mode_app_state: Default::default(),
        last_listed_tool_contract: Default::default(),
        route_runtime: Arc::new(runtime),
        client_registry: Default::default(),
        transport_label: "stdio",
        logging_level: Default::default(),
        route_scope: super::super::route_scope::McpRouteScope::Root,
        relay_session_id: super::super::server::next_relay_session_id(),
        code_mode_widget_callbacks_enabled_for_test: false,
    };
    let (server_pipe, client_pipe) = tokio::io::duplex(65_536);
    let channel_copy = channel.clone();
    let task = tokio::spawn(async move {
        let running = server.serve(server_pipe).await.unwrap();
        if enabled {
            channel_copy.wait(running).await.unwrap();
        } else {
            running.waiting().await.unwrap();
        }
    });
    (channel, BufReader::new(client_pipe), task)
}

async fn write(pipe: &mut BufReader<DuplexStream>, value: Value) {
    let mut line = value.to_string();
    line.push('\n');
    pipe.get_mut().write_all(line.as_bytes()).await.unwrap();
}

async fn read(pipe: &mut BufReader<DuplexStream>) -> Value {
    let mut line = String::new();
    let n = pipe.read_line(&mut line).await.unwrap();
    assert_ne!(n, 0, "unexpected MCP transport close");
    serde_json::from_str(&line).unwrap()
}

async fn initialize(pipe: &mut BufReader<DuplexStream>) -> Value {
    write(pipe, json!({
        "jsonrpc":"2.0","method":"initialize","id":1,
        "params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}
    })).await;
    tokio::time::timeout(Duration::from_secs(3), read(pipe))
        .await
        .unwrap()
}

async fn initialized(pipe: &mut BufReader<DuplexStream>) {
    write(
        pipe,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
}

#[tokio::test]
async fn opted_in_stdio_pushes_notice_only_to_its_own_connection() {
    let directory = tempfile::tempdir().unwrap();
    let first_store = NoticeStore::open_at(directory.path()).unwrap();
    let second_store = NoticeStore::open_at(directory.path()).unwrap();
    let producer_store = NoticeStore::open_at(directory.path()).unwrap();
    let (channel, mut first, task1) = fixture(directory.path(), first_store, true).await;
    let (_, mut second, task2) = fixture(directory.path(), second_store, true).await;
    write(
        &mut first,
        json!({
            "jsonrpc":"2.0", "id":0, "method":"server/discover", "params":{"_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientInfo":{"name":"claude-preview-probe","version":"1"},
                "io.modelcontextprotocol/clientCapabilities":{},
            }}
        }),
    )
    .await;
    let probe = tokio::time::timeout(Duration::from_secs(3), read(&mut first))
        .await
        .unwrap();
    assert_eq!(probe["error"]["code"], -32022, "{probe}");
    let info = initialize(&mut first).await;
    assert_eq!(
        info["result"]["capabilities"]["experimental"]["claude/channel"],
        json!({}),
        "{info}"
    );
    assert!(
        info["result"]["capabilities"]["experimental"]
            .get("claude/channel/permission")
            .is_none()
    );
    assert!(info["result"]["capabilities"].get("events").is_none());
    assert_eq!(
        initialize(&mut second).await["result"]["capabilities"]["experimental"]["claude/channel"],
        json!({})
    );
    initialized(&mut second).await;

    // A separate durable handle publishes; the stdio worker must see it
    // without an in-process wakeup or any tools/call.
    let inbox = producer_store
        .register(channel.recipient.clone())
        .await
        .unwrap();
    let receipt = producer_store
        .publish(
            NoticeProducer {
                actor: "fixture".into(),
                admin: true,
            },
            PublishNotice {
                inbox_id: inbox.id,
                source: "ci".into(),
                level: NoticeLevel::Warning,
                message: "Build finished.".into(),
                dedupe_key: "build-1".into(),
                ttl_seconds: 3_600,
            },
        )
        .await
        .unwrap();
    let premature = tokio::time::timeout(Duration::from_millis(100), read(&mut first)).await;
    assert!(
        premature.is_err(),
        "channel may not push before initialization"
    );
    initialized(&mut first).await;
    let notice = tokio::time::timeout(Duration::from_secs(3), read(&mut first))
        .await
        .unwrap();
    assert_eq!(notice["method"], "notifications/claude/channel");
    assert_eq!(notice["params"]["content"], "Build finished.");
    let meta = notice["params"]["meta"].as_object().unwrap();
    assert_eq!(meta.len(), 5);
    assert_eq!(meta["notice_id"], receipt.id);
    assert_eq!(meta["level"], "warning");
    assert_eq!(meta["source"], "ci");
    assert_eq!(meta["delivery_attempt"], "1");
    assert!(
        meta["expires_at_unix_ms"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .is_ok()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(150), read(&mut second))
            .await
            .is_err()
    );

    let (_, ack) = producer_store
        .controls(
            channel.recipient.clone(),
            false,
            Some(vec![receipt.id.clone()]),
        )
        .await
        .unwrap();
    assert_eq!(ack, Some(vec![receipt.id]));
    drop(first);
    drop(second);
    tokio::time::timeout(Duration::from_secs(3), task1)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), task2)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn default_stdio_does_not_advertise_channel() {
    let directory = tempfile::tempdir().unwrap();
    let (_, mut pipe, task) =
        fixture(directory.path(), NoticeStore::memory().unwrap(), false).await;
    let info = initialize(&mut pipe).await;
    assert!(
        info["result"]["capabilities"]["experimental"]
            .get("claude/channel")
            .is_none()
    );
    initialized(&mut pipe).await;
    drop(pipe);
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
}
