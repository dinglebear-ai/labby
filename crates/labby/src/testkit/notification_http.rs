//! Loopback HTTP qualification using the actual auth middleware and JS runner.
use crate::api::AppState;
use crate::cli::serve::build_http_router;
use crate::config::{LabConfig, McpPreferences};
use crate::dispatch::codemode_notices::NoticeStore;
use crate::mcp::peers::PeerNotifier;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

type Server = (
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
);
async fn start(
    manager: Arc<crate::dispatch::gateway::manager::GatewayManager>,
    token: &str,
) -> Server {
    let mut state = AppState::new().with_gateway_manager(manager);
    state.agent_notifications = NoticeStore::open_installation()
        .await
        .expect("private durable test inbox");
    let app = build_http_router(
        state,
        Some(token.to_owned()),
        None,
        &McpPreferences::default(),
        &[],
        PeerNotifier::default(),
        true,
        false,
    )
    .expect("production HTTP router");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("task-owned loopback listener");
    let address = listener.local_addr().unwrap();
    let (stop, done) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = done.await;
            })
            .await
    });
    (format!("http://{address}"), stop, task)
}
async fn stop(server: Server) {
    let (_, signal, mut task) = server;
    let _ = signal.send(());
    match tokio::time::timeout(Duration::from_secs(5), &mut task).await {
        Ok(joined) => joined
            .expect("HTTP server task joined")
            .expect("HTTP server shut down"),
        Err(_) => {
            task.abort();
            drop(task.await);
            panic!("test-owned HTTP server failed to shut down within five seconds");
        }
    }
}
async fn request(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    body: Value,
    mcp: bool,
) -> Result<(u16, Value), String> {
    let mut builder = client.post(url).header("host", "localhost").json(&body);
    if let Some(token) = token {
        builder = builder.bearer_auth(token);
    }
    if mcp {
        builder = builder
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "tools/call")
            .header("mcp-name", "codemode");
    }
    let response = builder.send().await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("HTTP response exceeded test evidence budget".into());
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("HTTP {status}: invalid response JSON: {e}"))?;
    Ok((status, value))
}
fn call(id: u64, arguments: Value, session: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"codemode","arguments":arguments,"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"notice-http-qualification","version":"1.0"},
        "io.modelcontextprotocol/clientCapabilities":{},"openai/session":session}}})
}
fn require(condition: bool, message: &str, value: &Value) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(format!("{message}: {value}"))
    }
}

#[tokio::test]
#[ignore = "requires this revision's LABBY_CODE_MODE_RUNNER_EXE and an isolated LABBY_HOME"]
async fn notice_production_real_http_auth_delivery_ack_and_restart() {
    assert!(std::env::var_os("LABBY_CODE_MODE_RUNNER_EXE").is_some());
    assert!(std::env::var_os("LABBY_HOME").is_some());
    let directory = tempfile::tempdir().unwrap();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            directory.path().join("gateway.toml"),
            crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled: true,
                    mcp_ui_enabled: false,
                    ..Default::default()
                },
                ..Default::default()
            }
            .to_gateway_config(),
        )
        .await;
    // Synthetic, per-test bearer authority; never log or persist the credential.
    let token = ulid::Ulid::new().to_string();
    let session = ulid::Ulid::new().to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(35))
        .build()
        .unwrap();
    let first_server = start(Arc::clone(&manager), &token).await;
    let base = &first_server.0;
    let first: Result<(Value, String), String> = async {
        let (status, denied) = request(&client, &format!("{base}/mcp"), None, call(1,json!({"code":"async () => 1","notification_inbox":true}),&session),true).await?;
        require(status == 401,"unauthenticated MCP request must be refused",&denied)?;
        let (status, registration) = request(&client,&format!("{base}/mcp"),Some(&token),call(2,json!({"code":"async () => Object.fromEntries([['ready', true]])","notification_inbox":true}),&session),true).await?;
        require(status == 200 && registration["result"]["isError"] != true,"register inbox through real bearer middleware",&registration)?;
        let address = registration["result"]["structuredContent"]["notification_inbox"]["id"].as_str().ok_or_else(||format!("missing registered inbox: {registration}"))?.to_owned();
        let message = json!({"inbox_id":address,"source":"http-qualification","level":"info","message":"HTTP test event completed.","dedupe_key":"http-event","ttl_seconds":3600});
        let (status, denied) = request(&client,&format!("{base}/v1/notifications/agent"),None,message.clone(),false).await?;
        require(status == 401,"unauthenticated producer must be refused",&denied)?;
        let (status, published) = request(&client,&format!("{base}/v1/notifications/agent"),Some(&token),message.clone(),false).await?;
        require(status == 200,"authorized HTTP publication",&published)?;
        let notice = published["id"].as_str().ok_or("publish receipt missing ID")?.to_owned();
        let (_, duplicate) = request(&client,&format!("{base}/v1/notifications/agent"),Some(&token),message,false).await?;
        require(duplicate["id"] == notice && duplicate["duplicate"] == true,"HTTP idempotency",&duplicate)?;
        let (_, foreign) = request(&client,&format!("{base}/mcp"),Some(&token),call(3,json!({"code":"async () => 7","ack_notifications":[notice]}),"other-test-conversation"),true).await?;
        require(foreign["result"]["structuredContent"]["notifications"].is_null() && foreign["result"]["structuredContent"]["acknowledged_notifications"]==json!([]),"conversation isolation and foreign ACK refusal",&foreign)?;
        let (_, delivered) = request(&client,&format!("{base}/mcp"),Some(&token),call(4,json!({"code":"async () => Object.fromEntries([['value', 42]])"}),&session),true).await?;
        let trace = &delivered["result"]["structuredContent"];
        require(trace["result"]==json!({"value":42}) && trace["notifications"][0]["id"]==notice,"result preservation and delivery",&delivered)?;
        let text: Value = serde_json::from_str(delivered["result"]["content"][0]["text"].as_str().ok_or("text block missing")?).map_err(|e|e.to_string())?;
        require(text["notifications"]==trace["notifications"],"text/structured delivery parity",&text)?;
        let (_, ack) = request(&client,&format!("{base}/mcp"),Some(&token),call(5,json!({"code":"async () => 43","ack_notifications":[notice]}),&session),true).await?;
        require(ack["result"]["structuredContent"]["acknowledged_notifications"]==json!([notice]) && ack["result"]["structuredContent"]["notifications"].is_null(),"ACK and no repeat",&ack)?;
        let (_, pending) = request(&client,&format!("{base}/v1/notifications/agent"),Some(&token),json!({"inbox_id":address,"source":"http-qualification","level":"warning","message":"Survives server recreation.","dedupe_key":"restart-event","ttl_seconds":3600}),false).await?;
        let restart_id = pending["id"].as_str().ok_or("restart publication failed")?.to_owned();
        Ok((json!({"registration":registration,"publication":published,"delivery":delivered,"acknowledgment":ack}),restart_id))
    }.await;
    stop(first_server).await;
    let (first, restart_id) = first.expect("first HTTP phase");
    let restarted = start(manager, &token).await;
    let result: Result<Value, String> = async {
        let (_, response) = request(
            &client,
            &format!("{}/mcp", restarted.0),
            Some(&token),
            call(
                6,
                json!({"code":"async () => { throw new Error('expected HTTP error'); }"}),
                &session,
            ),
            true,
        )
        .await?;
        require(
            response["result"]["isError"] == true
                && response["result"]["structuredContent"]["notifications"][0]["id"] == restart_id,
            "durable reopen and error preservation",
            &response,
        )?;
        Ok(response)
    }
    .await;
    stop(restarted).await;
    let restarted = result.expect("restarted HTTP phase");
    println!(
        "NOTICE_HTTP_EVIDENCE={}",
        json!({"first":first,"restarted":restarted})
    );
}
