#![cfg(feature = "gateway")]
#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use std::{path::Path, process::Stdio, time::Duration};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

use live_labby::{LiveLabbyBuilder, isolated_command};

const TOKEN: &str = "fixture-agent-message-bearer";
const FIRST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DEADLINE: Duration = Duration::from_secs(5);

fn reply(value: &Value) -> &Value {
    assert!(
        value["error"].is_null() && value["result"]["isError"] != true,
        "{value}"
    );
    assert!(value["result"]["structuredContent"].is_object(), "{value}");
    &value["result"]["structuredContent"]
}

struct Http {
    client: reqwest::Client,
    base: String,
}

impl Http {
    fn new(base: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(DEADLINE)
                .build()
                .unwrap(),
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    async fn rpc(&self, key: Option<&str>, method: &str, mut params: Value) -> Value {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"labby-message-fixture","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{},
        });
        let mut request = self
            .client
            .post(format!("{}/mcp", self.base))
            .bearer_auth(TOKEN)
            .header("accept", "application/json, text/event-stream")
            .header("mcp-method", method)
            .header("mcp-protocol-version", "2026-07-28");
        if let Some(key) = key {
            request = request.header("labby-session-key", key);
        }
        if method == "tools/call" {
            request = request.header("mcp-name", "messages");
        }
        let result = request
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!(result.status(), reqwest::StatusCode::OK);
        result.json().await.unwrap()
    }

    async fn messages(&self, key: &str, action: &str, params: Value) -> Value {
        let mut reply = Value::Null;
        for _ in 0..4 {
            reply = self
                .rpc(Some(key), "tools/call", arguments(action, params.clone()))
                .await;
            if reply["error"]["data"]["kind"] != "rate_limited" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        reply
    }
}

fn arguments(action: &str, params: Value) -> Value {
    json!({"name":"messages","arguments":{"action":action,"params":params}})
}

struct Local {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

impl Local {
    async fn start(root: &Path, server: Option<&str>) -> Self {
        let mut command = isolated_command(root);
        command
            .args(["mcp", "--agent-messages"])
            .current_dir(root)
            .env("LABBY_AUTH_MODE", "bearer");
        if let Some(server) = server {
            command
                .arg("--claude-channel")
                .env("LABBY_HOME", root.join("labby-home"))
                .env("LABBY_SERVER_URL", server)
                .env("LABBY_MCP_HTTP_TOKEN", TOKEN);
        } else {
            command.env("LABBY_MCP_FORCE_STANDALONE", "1");
        }
        let mut command = Command::from(command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut local = Self {
            child,
            input,
            output,
            next_id: 1,
        };
        let result = local
            .rpc(
                "initialize",
                json!({
                    "protocolVersion":"2025-11-25",
                    "clientInfo":{"name":"labby-channel-fixture","version":"1"},
                    "capabilities":{}
                }),
            )
            .await;
        let advertised = &result["result"]["capabilities"]["experimental"]["claude/channel"];
        if server.is_some() {
            assert_eq!(advertised, &json!({}), "{result}");
        } else {
            assert!(
                advertised.is_null() && result["error"].is_null(),
                "{result}"
            );
        }
        local
            .write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        local
    }

    async fn write(&mut self, value: Value) {
        let mut line = value.to_string();
        line.push('\n');
        timeout(DEADLINE, self.input.write_all(line.as_bytes()))
            .await
            .unwrap()
            .unwrap();
    }

    async fn read(&mut self) -> Value {
        let mut line = String::new();
        let count = self.output.read_line(&mut line).await.unwrap();
        assert!(count > 0, "unexpected stdio closure");
        serde_json::from_str(&line).unwrap()
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        timeout(DEADLINE, async {
            for _ in 0..8 {
                let value = self.read().await;
                if value["id"] == id {
                    return value;
                }
                assert_eq!(value["method"], "notifications/claude/channel");
            }
            panic!("missing stdio RPC result");
        })
        .await
        .unwrap()
    }

    async fn messages(&mut self, action: &str, params: Value) -> Value {
        let mut reply = Value::Null;
        for _ in 0..4 {
            reply = self
                .rpc("tools/call", arguments(action, params.clone()))
                .await;
            if reply["error"]["data"]["kind"] != "rate_limited" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        reply
    }

    async fn finish(mut self) {
        self.input.shutdown().await.unwrap();
        drop(self.input);
        let status = timeout(DEADLINE, self.child.wait()).await.unwrap().unwrap();
        assert!(status.success(), "stdio child exited with {status}");
    }
}

#[tokio::test]
async fn session_messages_live_http_and_stdio() {
    let mut daemon = LiveLabbyBuilder::new()
        .arg("--agent-messages")
        .arg("--mcp-events")
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .start()
        .await
        .unwrap();
    let http = Http::new(&daemon.connection().base_url);
    let root = daemon.root();
    let mut claude = Local::start(root, Some(&http.base)).await;
    let mut sibling = Local::start(root, Some(&http.base)).await;

    let a = http
        .messages(FIRST, "register", json!({"label":"http-a"}))
        .await;
    let b = http
        .messages(SECOND, "register", json!({"label":"http-b"}))
        .await;
    let c = claude
        .messages("register", json!({"label":"stdio-c"}))
        .await;
    let d = sibling
        .messages("register", json!({"label":"stdio-d"}))
        .await;
    let a = &reply(&a)["session_id"];
    let b = &reply(&b)["session_id"];
    let c = &reply(&c)["session_id"];
    let d = &reply(&d)["session_id"];

    let text = "Build output includes diagnostic details. ".repeat(250);
    let direct = http
        .messages(
            FIRST,
            "send",
            json!({
                "to":{"kind":"session","session_id":c},
                "body":text, "summary":"Build result ready", "idempotency_key":"direct-fixture"
            }),
        )
        .await;
    let id = &reply(&direct)["message_id"];
    assert_eq!(reply(&direct)["recipient_ids"], json!([c]));
    let pushed = timeout(DEADLINE, claude.read()).await.unwrap();
    assert_eq!(pushed["method"], "notifications/claude/channel");
    assert_eq!(&pushed["params"]["meta"]["message_id"], id);
    assert_eq!(pushed["params"]["content"], "Build result ready");
    assert!(!pushed.to_string().contains(&text));
    assert!(
        timeout(Duration::from_millis(150), sibling.read())
            .await
            .is_err()
    );

    let denied = http
        .messages(SECOND, "read", json!({"message_id":id}))
        .await;
    assert_eq!(denied["error"]["data"]["kind"], "not_found");
    let body = claude.messages("read", json!({"message_id":id})).await;
    assert_eq!(reply(&body)["body"], text);
    assert_eq!(&reply(&body)["sender_session_id"], a);
    let ack = claude.messages("ack", json!({"message_ids":[id]})).await;
    assert_eq!(reply(&ack)["acknowledged"], json!([id]));

    let broadcast_args = json!({
        "to":{"kind":"all_active"}, "body":"Deployment finished", "idempotency_key":"broadcast-fixture"
    });
    let broadcast = http.messages(FIRST, "send", broadcast_args.clone()).await;
    let ids = reply(&broadcast)["recipient_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 4);
    for recipient in [a, b, c, d] {
        assert!(ids.contains(recipient), "{ids:?}");
    }
    let broadcast_id = &reply(&broadcast)["message_id"];
    let broadcast_frame = timeout(DEADLINE, claude.read()).await.unwrap();
    assert_eq!(
        &broadcast_frame["params"]["meta"]["message_id"],
        broadcast_id
    );
    let denied = http
        .rpc(
            None,
            "tools/call",
            arguments("read", json!({"message_id":broadcast_id})),
        )
        .await;
    assert!(!denied["error"].is_null());

    let callback = http
        .rpc(
            Some(SECOND),
            "events/subscribe",
            json!({
                "name":"labby.notification.created", "arguments":{"session_messages":true},
                "delivery":{"mode":"webhook","url":"https://127.0.0.1:1/events",
                    "secret":"whsec_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE="}
            }),
        )
        .await;
    assert_eq!(callback["error"]["code"], -32015, "{callback}");
    assert_eq!(callback["error"]["data"]["reason"], "destination_blocked");

    sibling.finish().await;
    daemon.restart().await.unwrap();
    let old_bridge = claude.messages("sessions", json!({})).await;
    assert_eq!(
        old_bridge["error"]["data"]["kind"], "forbidden",
        "{old_bridge}"
    );

    // Make the still-unacknowledged typed reference due again in this disposable
    // fixture. The revoked bridge must skip it while delivering a legacy notice.
    let inbox: String = {
        let db = rusqlite::Connection::open(
            daemon
                .root()
                .join("labby-home/agent-notifications/inbox.sqlite3"),
        )
        .unwrap();
        db.busy_timeout(Duration::from_secs(1)).unwrap();
        let inbox: String = db
            .query_row(
                "SELECT inbox_id FROM agent_message_sessions WHERE id=? AND closed=1",
                [c.as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            db.execute(
                "UPDATE agent_notices SET next_attempt=0 WHERE inbox_id=?1 AND message_id=?2",
                rusqlite::params![inbox, broadcast_id.as_str().unwrap()],
            )
            .unwrap(),
            1
        );
        inbox
    };
    let ordinary: Value = http
        .client
        .post(format!("{}/v1/notifications/agent", http.base))
        .bearer_auth(TOKEN)
        .json(&json!({
            "inbox_id":inbox,"source":"fixture","level":"info","message":"Legacy notice",
            "dedupe_key":"ordinary-after-restart","ttl_seconds":60,
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let frame = timeout(DEADLINE, claude.read()).await.unwrap();
    assert_eq!(
        frame["params"]["meta"]["notice_id"], ordinary["id"],
        "{frame}"
    );
    assert!(frame["params"]["meta"].get("message_id").is_none());
    assert!(
        timeout(Duration::from_millis(150), claude.read())
            .await
            .is_err()
    );
    claude.finish().await;
    let recovered = http
        .messages(FIRST, "register", json!({"label":"http-a"}))
        .await;
    assert_eq!(&reply(&recovered)["session_id"], a);
    let retry = http.messages(FIRST, "send", broadcast_args).await;
    assert_eq!(&reply(&retry)["message_id"], broadcast_id);
    assert_eq!(reply(&retry)["recipient_ids"].as_array().unwrap(), ids);
    assert_eq!(reply(&retry)["duplicate"], true);
    let available = http
        .messages(SECOND, "read", json!({"message_id":broadcast_id}))
        .await;
    assert_eq!(reply(&available)["body"], "Deployment finished");
    let directory = http.messages(FIRST, "sessions", json!({})).await;
    let entries = reply(&directory)["sessions"].as_array().unwrap();
    for disconnected in [c, d] {
        let entry = entries
            .iter()
            .find(|s| &s["session_id"] == disconnected)
            .unwrap();
        assert_eq!(entry["active"], false);
    }
    let cleanup = daemon.finish().await;
    assert!(cleanup.is_clean(), "{cleanup:?}");
}

#[tokio::test]
async fn standalone_stdio_can_register_and_receive_its_broadcast() {
    let root = tempfile::tempdir().unwrap();
    let mut local = Local::start(root.path(), None).await;
    let registration = local
        .messages("register", json!({"label":"standalone"}))
        .await;
    let sent = local
        .messages(
            "send",
            json!({
                "to":{"kind":"all_active"}, "body":"Standalone body", "idempotency_key":"standalone"
            }),
        )
        .await;
    assert_eq!(
        reply(&sent)["recipient_ids"],
        json!([reply(&registration)["session_id"]])
    );
    let message_id = &reply(&sent)["message_id"];
    // A notification may precede the send response on stdio. The cross-session
    // test observes the unsolicited frame on its own independent pipe.
    let body = local
        .messages("read", json!({"message_id":message_id}))
        .await;
    assert_eq!(reply(&body)["body"], "Standalone body");
    local.finish().await;
}

#[tokio::test]
async fn default_tcp_keeps_message_calls_and_selector_disabled() {
    let daemon = LiveLabbyBuilder::new()
        .arg("--mcp-events")
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .start()
        .await
        .unwrap();
    let http = Http::new(&daemon.connection().base_url);
    let hidden = http
        .messages(FIRST, "register", json!({"label":"disabled"}))
        .await;
    assert_eq!(hidden["error"]["code"], -32602, "{hidden}");
    let listed = http.rpc(Some(FIRST), "events/list", json!({})).await;
    let event = &listed["result"]["events"][0];
    assert_eq!(event["name"], "labby.notification.created");
    assert!(
        event["inputSchema"]["properties"]
            .get("session_messages")
            .is_none()
    );
    assert!(
        event["payloadSchema"]["properties"]
            .get("message_id")
            .is_none()
    );
    let denied = http
        .rpc(
            Some(FIRST),
            "events/subscribe",
            json!({
                "name":"labby.notification.created","arguments":{"session_messages":true},
                "delivery":{"mode":"webhook","url":"https://127.0.0.1:1/must-not-contact",
                    "secret":"whsec_YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE="}
            }),
        )
        .await;
    assert_eq!(denied["error"]["data"]["kind"], "forbidden", "{denied}");
    let cleanup = daemon.finish().await;
    assert!(cleanup.is_clean(), "{cleanup:?}");
}
