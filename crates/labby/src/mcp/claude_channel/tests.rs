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
    messages_enabled: bool,
) -> (
    Arc<ChannelSession>,
    BufReader<DuplexStream>,
    tokio::task::JoinHandle<()>,
) {
    let channel = ChannelSession::new(store.clone());
    let mut runtime = crate::mcp::runtime::McpRouteRuntime::with_notification_store(store.clone());
    if enabled {
        runtime.claude_channel = Some(channel.clone());
    }
    let messages = messages_enabled.then(|| {
        let key = crate::dispatch::agent_messages::proof::MessageKey::from_test_bytes([8; 32]);
        LocalMessageSession::standalone(store, &key, &"1".repeat(64), channel.recipient.clone())
            .unwrap()
    });
    runtime.local_messages = messages.clone();
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
        if let Some(messages) = messages {
            messages
                .wait(running, enabled.then_some(channel_copy.as_ref()))
                .await
                .unwrap();
        } else if enabled {
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
    let (channel, mut first, task1) = fixture(directory.path(), first_store, true, false).await;
    let (_, mut second, task2) = fixture(directory.path(), second_store, true, false).await;
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
    let (_, mut pipe, task) = fixture(
        directory.path(),
        NoticeStore::memory().unwrap(),
        false,
        false,
    )
    .await;
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

async fn message_call(
    pipe: &mut BufReader<DuplexStream>,
    id: u64,
    action: &str,
    params: Value,
) -> Value {
    for attempt in 0..4 {
        let request_id = id * 10 + attempt;
        write(
            pipe,
            json!({
                "jsonrpc":"2.0", "id":request_id, "method":"tools/call",
                "params":{"name":"messages","arguments":{"action":action,"params":params}}
            }),
        )
        .await;
        let result = tokio::time::timeout(Duration::from_secs(3), read(pipe))
            .await
            .unwrap();
        assert_eq!(result["id"], request_id, "{result}");
        if result["error"]["message"] == "message store busy" && attempt < 3 {
            tokio::time::sleep(Duration::from_millis(25 * (attempt + 1))).await;
            continue;
        }
        assert!(
            result["result"]["structuredContent"].is_object(),
            "{result}"
        );
        return result["result"]["structuredContent"].clone();
    }
    unreachable!("the final attempt returns or reports its failure")
}

#[tokio::test]
async fn claude_channel_delivers_reference_to_its_peer() {
    let directory = tempfile::tempdir().unwrap();
    let store = || NoticeStore::open_at(directory.path()).unwrap();
    let (_, mut sender, task1) = fixture(directory.path(), store(), true, true).await;
    let (_, mut target, task2) = fixture(directory.path(), store(), true, true).await;
    initialize(&mut sender).await;
    initialize(&mut target).await;
    initialized(&mut sender).await;
    initialized(&mut target).await;
    let sender_info = message_call(&mut sender, 2, "register", json!({"label":"sender"})).await;
    let target_info = message_call(&mut target, 2, "register", json!({"label":"target"})).await;
    let text = "Text stays behind the authenticated read. ".repeat(100);
    let receipt = message_call(
        &mut sender,
        3,
        "send",
        json!({
            "to":{"kind":"session","session_id":target_info["session_id"]},
            "body":text, "summary":"Please review the result", "idempotency_key":"reference"
        }),
    )
    .await;
    let frame = tokio::time::timeout(Duration::from_secs(3), read(&mut target))
        .await
        .unwrap();
    assert_eq!(frame["method"], "notifications/claude/channel");
    assert_eq!(frame["params"]["meta"]["message_id"], receipt["message_id"]);
    assert_eq!(frame["params"]["content"], "Please review the result");
    assert!(!frame.to_string().contains(&text));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), read(&mut sender))
            .await
            .is_err()
    );
    let full = message_call(
        &mut target,
        3,
        "read",
        json!({"message_id":receipt["message_id"]}),
    )
    .await;
    assert_eq!(full["body"], text);
    assert_eq!(full["sender_session_id"], sender_info["session_id"]);
    let ack = message_call(
        &mut target,
        4,
        "ack",
        json!({"message_ids":[receipt["message_id"]]}),
    )
    .await;
    assert_eq!(ack["acknowledged"], json!([receipt["message_id"]]));
    drop(sender);
    drop(target);
    tokio::time::timeout(Duration::from_secs(3), task1)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), task2)
        .await
        .unwrap()
        .unwrap();
}
