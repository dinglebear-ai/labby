//! Controlled real-process fixture for browser request lifecycle conformance.
//!
//! Observations come from public HTTP/WebSocket boundaries, structured admission
//! diagnostics and read-only audit rows in the owned sandbox. Generation labels
//! identify authenticated fixture sockets; opaque runtime UUIDs are retained
//! only as observed correlation data, never guessed.

use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_tungstenite::{WebSocketStream, tungstenite::client::IntoClientRequest as _};

use crate::live_labby::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, RunIdentity};

type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const EXTENSION_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TAB_ID: i64 = 11;
const DOCUMENT_ID: &str = "conformance-document";
const TOOL_NAME: &str = "conformance.echo";
const CATALOG_REVISION: i64 = 1;
const CATALOG_FINGERPRINT: &str = "conformance-fixture-v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PublicObservation {
    SocketAuthenticated {
        generation_label: String,
    },
    DocumentAcknowledged,
    CallDispatched {
        call_id: String,
    },
    CompletionAcknowledged {
        call_id: String,
    },
    CancellationReceived {
        call_id: String,
    },
    HttpTerminal(HttpTerminal),
    /// A close frame was attempted for this fixture socket. This event alone
    /// does not prove server-side disconnect; require a pending public HTTP
    /// call to terminate with `browser_offline` as the lifecycle barrier.
    SocketCloseRequested {
        generation_label: String,
    },
    CallAdmitted {
        call_id: String,
        generation_id: String,
    },
    AuditObserved(InvocationAudit),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InvocationAudit {
    pub(crate) id: String,
    pub(crate) outcome: String,
    pub(crate) error_kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HttpTerminal {
    Success(Value),
    Error { status: u16, kind: String },
}

pub(crate) struct PendingHttpCall {
    task: Option<JoinHandle<Result<HttpTerminal, String>>>,
    pub(crate) call_id: Option<String>,
}

impl Drop for PendingHttpCall {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DispatchedCall {
    pub(crate) call_id: String,
    pub(crate) generation_label: String,
}

pub(crate) struct AdmissionBarrier {
    connection: Option<rusqlite::Connection>,
}

impl AdmissionBarrier {
    pub(crate) fn release(mut self) -> Result<(), String> {
        let connection = self
            .connection
            .take()
            .ok_or_else(|| "admission barrier was already released".to_string())?;
        connection
            .execute_batch("ROLLBACK")
            .map_err(|error| error.to_string())
    }
}

impl Drop for AdmissionBarrier {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            drop(connection.execute_batch("ROLLBACK"));
        }
    }
}

pub(crate) struct ConformanceFixture {
    guard: LiveLabbyGuard,
    client: reqwest::Client,
    token: String,
    socket: Option<Socket>,
    signing: SigningKey,
    browser_id: String,
    session_id: String,
    catalog_digest: String,
    generation: u64,
    deadline: tokio::time::Instant,
    cleanup_timeout: Duration,
    evidence: Vec<PublicObservation>,
}

impl ConformanceFixture {
    pub(crate) async fn start(deadline: Duration) -> Result<Self, String> {
        let token = uuid::Uuid::new_v4().to_string();
        let guard = LiveLabbyBuilder::new()
            .env("LABBY_MCP_HTTP_TOKEN", &token)
            .env("LABBY_LOG", "labby=info,labby_browser=debug")
            .env("LABBY_LOG_FORMAT", "json")
            .start()
            .await?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(deadline)
            .build()
            .map_err(|error| error.to_string())?;
        let signing = SigningKey::from_bytes(&[73; 32]);
        let (socket, browser_id) = pair_and_authenticate(
            &guard.connection().base_url,
            &client,
            &token,
            &signing,
            deadline,
        )
        .await?;
        let mut fixture = Self {
            guard,
            client,
            token,
            socket: Some(socket),
            signing,
            browser_id,
            session_id: String::new(),
            catalog_digest: String::new(),
            generation: 1,
            deadline: tokio::time::Instant::now() + deadline,
            cleanup_timeout: deadline,
            evidence: vec![PublicObservation::SocketAuthenticated {
                generation_label: "fixture-generation-1".to_string(),
            }],
        };
        fixture.observe_and_enable().await?;
        Ok(fixture)
    }

    pub(crate) fn identity(&self) -> RunIdentity {
        self.guard.identity().clone()
    }

    pub(crate) fn generation_label(&self) -> String {
        format!("fixture-generation-{}", self.generation)
    }

    pub(crate) fn evidence(&self) -> &[PublicObservation] {
        &self.evidence
    }

    /// Capture only allowlisted lifecycle fields from this owned daemon. Raw
    /// request payloads and unrelated log records never enter incident evidence.
    pub(crate) fn incident_input(&self) -> Result<Value, String> {
        use std::io::Read as _;
        let file = std::fs::File::open(self.guard.root().join("stderr.log"))
            .map_err(|_| "cannot open bounded lifecycle log")?;
        let mut bytes = Vec::new();
        file.take(1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read bounded lifecycle log")?;
        if bytes.len() > 1_048_576 {
            return Err("lifecycle log exceeds capture bound".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| "lifecycle log is not UTF-8")?;
        let mut events = Vec::new();
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let fields = record.get("fields").unwrap_or(&record);
            if fields["action"] != "browser.call.lifecycle" {
                continue;
            }
            let mut selected = serde_json::Map::new();
            for key in [
                "action",
                "phase",
                "generation_id",
                "previous_generation_id",
                "call_id",
                "cause",
            ] {
                if let Some(value) = fields.get(key) {
                    selected.insert(key.into(), value.clone());
                }
            }
            events.push(json!({"fields":selected}));
            if events.len() > 256 {
                return Err("lifecycle event count exceeds capture bound".into());
            }
        }
        let generation = events
            .iter()
            .find(|event| event["fields"]["phase"] == "connected")
            .and_then(|event| event["fields"]["generation_id"].as_str())
            .ok_or_else(|| "lifecycle capture omitted initial connection".to_string())?;
        Ok(json!({"schema":1,"generation":generation,"events":events}))
    }

    pub(crate) fn begin_call(&self, arguments: Value) -> PendingHttpCall {
        self.begin_call_with_timeout(arguments, self.remaining())
    }

    pub(crate) fn begin_call_with_timeout(
        &self,
        arguments: Value,
        call_timeout: Duration,
    ) -> PendingHttpCall {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(self.remaining())
            .build()
            .expect("fixed conformance HTTP client configuration");
        let url = format!("{}/v1/browser", self.guard.connection().base_url);
        let token = self.token.clone();
        let params = json!({
            "browser_id": self.browser_id,
            "tab_id": TAB_ID,
            "document_id": DOCUMENT_ID,
            "catalog_revision": CATALOG_REVISION,
            "catalog_digest": self.catalog_digest,
            "tool_name": TOOL_NAME,
            "arguments": arguments,
            "timeout_ms": u64::try_from(call_timeout.as_millis()).unwrap_or(u64::MAX),
        });
        let task = tokio::spawn(async move {
            let response = client
                .post(url)
                .header(reqwest::header::CONNECTION, "close")
                .bearer_auth(token)
                .json(&json!({"action":"browser.call", "params":params}))
                .send()
                .await
                .map_err(|error| error.to_string())?;
            decode_terminal(response).await
        });
        PendingHttpCall {
            task: Some(task),
            call_id: None,
        }
    }

    pub(crate) fn hold_audit_writes(&self) -> Result<AdmissionBarrier, String> {
        let path = self.guard.root().join("labby-home/browser/browser.db");
        let connection = rusqlite::Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .busy_timeout(self.remaining())
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| error.to_string())?;
        Ok(AdmissionBarrier {
            connection: Some(connection),
        })
    }

    pub(crate) async fn wait_admitted(
        &mut self,
        pending: &mut PendingHttpCall,
    ) -> Result<DispatchedCall, String> {
        let log = self.guard.root().join("stderr.log");
        loop {
            use std::io::{Read as _, Seek as _};
            const MAX_LOG_TAIL: u64 = 1_048_576;
            let mut file = std::fs::File::open(&log).map_err(|error| error.to_string())?;
            let length = file.metadata().map_err(|error| error.to_string())?.len();
            file.seek(std::io::SeekFrom::Start(
                length.saturating_sub(MAX_LOG_TAIL),
            ))
            .map_err(|error| error.to_string())?;
            let mut bytes = Vec::new();
            file.take(MAX_LOG_TAIL)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines().rev() {
                let Ok(event) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let fields = event.get("fields").unwrap_or(&event);
                if fields["action"] != "browser.call.lifecycle" || fields["phase"] != "admitted" {
                    continue;
                }
                let call_id = fields["call_id"]
                    .as_str()
                    .ok_or_else(|| "admission event omitted call_id".to_string())?
                    .to_string();
                if self.evidence.iter().any(|event| {
                    matches!(event, PublicObservation::CallAdmitted { call_id: seen, .. } if seen == &call_id)
                }) {
                    continue;
                }
                let generation_id = fields["generation_id"]
                    .as_str()
                    .ok_or_else(|| "admission event omitted generation_id".to_string())?
                    .to_string();
                pending.call_id = Some(call_id.clone());
                self.evidence.push(PublicObservation::CallAdmitted {
                    call_id: call_id.clone(),
                    generation_id,
                });
                return Ok(DispatchedCall {
                    call_id,
                    generation_label: self.generation_label(),
                });
            }
            if self.remaining().is_zero() {
                return Err("browser admission event deadline elapsed".to_string());
            }
            tokio::task::yield_now().await;
        }
    }

    pub(crate) async fn wait_dispatch(
        &mut self,
        pending: &mut PendingHttpCall,
    ) -> Result<DispatchedCall, String> {
        let message = self.receive().await?;
        if message["type"] != "tool_call" {
            return Err(format!(
                "expected tool_call, received message type {:?}",
                message["type"].as_str()
            ));
        }
        let call_id = message["call_id"]
            .as_str()
            .ok_or_else(|| "tool_call omitted call_id".to_string())?
            .to_string();
        pending.call_id = Some(call_id.clone());
        self.evidence.push(PublicObservation::CallDispatched {
            call_id: call_id.clone(),
        });
        Ok(DispatchedCall {
            call_id,
            generation_label: self.generation_label(),
        })
    }

    pub(crate) async fn abort_caller(pending: &mut PendingHttpCall) -> Result<(), String> {
        let task = pending
            .task
            .take()
            .ok_or_else(|| "HTTP caller task was already consumed".to_string())?;
        task.abort();
        match task.await {
            Err(error) if error.is_cancelled() => Ok(()),
            Ok(_) => Err("HTTP call reached a terminal before caller abort".to_string()),
            Err(error) => Err(format!("HTTP caller task failed during abort: {error}")),
        }
    }

    pub(crate) async fn wait_cancel(&mut self, call: &DispatchedCall) -> Result<(), String> {
        let message = self.receive().await?;
        if message["type"] != "tool_cancel" || message["call_id"] != call.call_id {
            return Err(format!(
                "expected matching tool_cancel for {}",
                call.call_id
            ));
        }
        self.evidence.push(PublicObservation::CancellationReceived {
            call_id: call.call_id.clone(),
        });
        Ok(())
    }

    pub(crate) async fn complete_success(
        &mut self,
        call: &DispatchedCall,
        result: Value,
    ) -> Result<(), String> {
        self.send_completion(
            call,
            json!({"type":"tool_result", "call_id":call.call_id, "result":result}),
        )
        .await
    }

    pub(crate) async fn complete_error(
        &mut self,
        call: &DispatchedCall,
        kind: &str,
        message: &str,
    ) -> Result<(), String> {
        self.send_completion(
            call,
            json!({"type":"tool_error", "call_id":call.call_id, "kind":kind, "message":message}),
        )
        .await
    }

    /// Send a completion after cancellation and wait only for the protocol ack.
    /// The public adapter does not expose whether this completion matched.
    pub(crate) async fn send_late_completion(
        &mut self,
        call: &DispatchedCall,
        result: Value,
    ) -> Result<(), String> {
        self.send_completion(
            call,
            json!({"type":"tool_result", "call_id":call.call_id, "result":result}),
        )
        .await
    }

    pub(crate) async fn wait_http_terminal(
        &mut self,
        mut pending: PendingHttpCall,
    ) -> Result<HttpTerminal, String> {
        let task = pending
            .task
            .take()
            .ok_or_else(|| "HTTP caller task was already consumed".to_string())?;
        let terminal = tokio::time::timeout(self.remaining(), task)
            .await
            .map_err(|_| "HTTP terminal deadline elapsed".to_string())?
            .map_err(|error| format!("HTTP call task failed: {error}"))??;
        self.evidence
            .push(PublicObservation::HttpTerminal(terminal.clone()));
        Ok(terminal)
    }

    pub(crate) async fn wait_audit_count(
        &mut self,
        expected: usize,
    ) -> Result<Vec<InvocationAudit>, String> {
        loop {
            let audits = self.audit_snapshot()?;
            if audits.len() == expected && audits.iter().all(|audit| audit.outcome != "started") {
                for audit in &audits {
                    if !self.evidence.iter().any(
                        |event| matches!(event, PublicObservation::AuditObserved(old) if old == audit),
                    ) {
                        self.evidence
                            .push(PublicObservation::AuditObserved(audit.clone()));
                    }
                }
                return Ok(audits);
            }
            if self.remaining().is_zero() {
                return Err(format!(
                    "audit terminal deadline elapsed: expected {expected}, observed {}",
                    audits.len()
                ));
            }
            tokio::task::yield_now().await;
        }
    }

    pub(crate) fn audit_snapshot(&self) -> Result<Vec<InvocationAudit>, String> {
        let path = self.guard.root().join("labby-home/browser/browser.db");
        let connection = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| error.to_string())?;
        bounded_audit_snapshot(&connection, &self.browser_id)
    }

    pub(crate) async fn disconnect(&mut self) -> Result<(), String> {
        let label = self.generation_label();
        if let Some(mut socket) = self.socket.take() {
            drop(tokio::time::timeout(self.remaining(), socket.close(None)).await);
        }
        self.evidence.push(PublicObservation::SocketCloseRequested {
            generation_label: label,
        });
        Ok(())
    }

    pub(crate) async fn invalidate_document(
        &mut self,
        call: &DispatchedCall,
    ) -> Result<(), String> {
        self.send(json!({
            "type":"document_closed", "tab_id":TAB_ID, "document_id":DOCUMENT_ID
        }))
        .await?;
        let first = self.receive().await?;
        let second = self.receive().await?;
        let messages = [first, second];
        if !messages
            .iter()
            .any(|message| message["type"] == "tool_cancel" && message["call_id"] == call.call_id)
        {
            return Err("document invalidation omitted matching tool_cancel".into());
        }
        if !messages.iter().any(|message| {
            message["type"] == "acknowledged" && message["received"] == "document_closed"
        }) {
            return Err("document invalidation omitted acknowledgement".into());
        }
        self.evidence.push(PublicObservation::CancellationReceived {
            call_id: call.call_id.clone(),
        });
        Ok(())
    }

    pub(crate) async fn replace_connection(&mut self) -> Result<String, String> {
        let (socket, browser_id) = authenticate_existing(
            &self.guard.connection().base_url,
            &self.signing,
            &self.browser_id,
            self.remaining(),
        )
        .await?;
        if browser_id != self.browser_id {
            return Err("replacement authenticated a different browser".to_string());
        }
        self.socket = Some(socket);
        self.generation += 1;
        let label = self.generation_label();
        self.evidence.push(PublicObservation::SocketAuthenticated {
            generation_label: label.clone(),
        });
        Ok(label)
    }

    pub(crate) async fn replace_connection_with_pending(
        &mut self,
        call: &DispatchedCall,
    ) -> Result<String, String> {
        let mut replaced = self
            .socket
            .take()
            .ok_or_else(|| "browser socket is disconnected".to_string())?;
        let (socket, browser_id) = authenticate_existing(
            &self.guard.connection().base_url,
            &self.signing,
            &self.browser_id,
            self.remaining(),
        )
        .await?;
        if browser_id != self.browser_id {
            return Err("replacement authenticated a different browser".to_string());
        }
        let cancellation = receive(&mut replaced, self.remaining()).await?;
        if cancellation["type"] != "tool_cancel" || cancellation["call_id"] != call.call_id {
            return Err(format!(
                "replacement did not cancel pending call {}",
                call.call_id
            ));
        }
        self.evidence.push(PublicObservation::CancellationReceived {
            call_id: call.call_id.clone(),
        });
        self.socket = Some(socket);
        self.generation += 1;
        let label = self.generation_label();
        self.evidence.push(PublicObservation::SocketAuthenticated {
            generation_label: label.clone(),
        });
        Ok(label)
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        self.guard.finish_with_deadline(self.cleanup_timeout).await
    }

    async fn observe_and_enable(&mut self) -> Result<(), String> {
        self.send(json!({
            "type":"observe", "tab_id":TAB_ID, "document_id":DOCUMENT_ID,
            "origin":"https://conformance.example", "sanitized_path":"/fixture",
            "page_title":"Conformance fixture", "catalog_revision":CATALOG_REVISION,
            "catalog_fingerprint":CATALOG_FINGERPRINT,
            "tools":[{"name":TOOL_NAME,"title":"Conformance echo",
                "origin":"https://conformance.example","description":"Controlled fixture tool",
                "input_schema":{"type":"object"},"annotations":{}}]
        }))
        .await?;
        self.expect_ack("observe").await?;
        self.evidence.push(PublicObservation::DocumentAcknowledged);
        let sessions = self.action("browser.sessions", json!({})).await?;
        let session_id = sessions["sessions"][0]["id"]
            .as_str()
            .ok_or_else(|| "browser.sessions returned no session".to_string())?
            .to_string();
        let session = self
            .action("browser.session.get", json!({"session_id":session_id}))
            .await?;
        self.session_id = session["id"]
            .as_str()
            .ok_or_else(|| "session omitted id".to_string())?
            .to_string();
        self.catalog_digest = session["catalog_digest"]
            .as_str()
            .ok_or_else(|| "session omitted catalog_digest".to_string())?
            .to_string();
        self.action(
            "browser.session.enable",
            json!({
                "session_id":self.session_id,"enabled":true,"catalog_digest":self.catalog_digest
            }),
        )
        .await?;
        Ok(())
    }

    async fn send_completion(&mut self, call: &DispatchedCall, value: Value) -> Result<(), String> {
        self.send(value).await?;
        self.expect_ack("tool_completion").await?;
        self.evidence
            .push(PublicObservation::CompletionAcknowledged {
                call_id: call.call_id.clone(),
            });
        Ok(())
    }

    async fn action(&self, action: &str, params: Value) -> Result<Value, String> {
        let response = self
            .client
            .post(format!("{}/v1/browser", self.guard.connection().base_url))
            .bearer_auth(&self.token)
            .json(&json!({"action":action,"params":params}))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let value: Value = response.json().await.map_err(|error| error.to_string())?;
        if status.is_success() {
            Ok(value)
        } else {
            Err(format!("{action} failed with {status}: {value}"))
        }
    }

    async fn send(&mut self, mut value: Value) -> Result<(), String> {
        value["version"] = json!(1);
        value["request_id"] = json!(uuid::Uuid::new_v4().to_string());
        let remaining = self.remaining();
        let socket = self
            .socket
            .as_mut()
            .ok_or_else(|| "browser socket is disconnected".to_string())?;
        tokio::time::timeout(
            remaining,
            socket.send(tokio_tungstenite::tungstenite::Message::Text(
                value.to_string().into(),
            )),
        )
        .await
        .map_err(|_| "browser send deadline elapsed".to_string())?
        .map_err(|error| error.to_string())
    }

    async fn receive(&mut self) -> Result<Value, String> {
        let remaining = self.remaining();
        receive(
            self.socket
                .as_mut()
                .ok_or_else(|| "browser socket is disconnected".to_string())?,
            remaining,
        )
        .await
    }

    async fn expect_ack(&mut self, received: &str) -> Result<(), String> {
        let message = self.receive().await?;
        if message["type"] == "acknowledged" && message["received"] == received {
            Ok(())
        } else {
            Err(format!("expected {received} acknowledgement"))
        }
    }

    fn remaining(&self) -> Duration {
        self.deadline
            .saturating_duration_since(tokio::time::Instant::now())
    }
}

async fn pair_and_authenticate(
    base: &str,
    client: &reqwest::Client,
    token: &str,
    signing: &SigningKey,
    deadline: Duration,
) -> Result<(Socket, String), String> {
    let mut socket = connect(base, deadline).await?;
    send_raw(&mut socket, json!({"type":"pairing_request","display_name":"Conformance fixture","extension_id":EXTENSION_ID,"public_key":URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes())}), deadline).await?;
    let pending = receive(&mut socket, deadline).await?;
    let pairing_id = pending["pairing_id"]
        .as_str()
        .ok_or_else(|| "pairing response omitted id".to_string())?;
    let pairing_fingerprint = pending["pairing_fingerprint"]
        .as_str()
        .ok_or_else(|| "pairing response omitted fingerprint".to_string())?;
    let response = client
        .post(format!("{base}/v1/browser"))
        .bearer_auth(token)
        .json(&json!({"action":"browser.pairing.approve","params":{"pairing_id":pairing_id,"pairing_fingerprint":pairing_fingerprint}}))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("pairing approval failed: {}", response.status()));
    }
    let approved: Value = response.json().await.map_err(|error| error.to_string())?;
    let browser_id = approved["id"]
        .as_str()
        .ok_or_else(|| "approval omitted browser id".to_string())?
        .to_string();
    drop(socket);
    let (socket, authenticated_id) =
        authenticate_existing(base, signing, &browser_id, deadline).await?;
    if authenticated_id != browser_id {
        return Err("authenticated browser id changed".to_string());
    }
    Ok((socket, browser_id))
}

async fn authenticate_existing(
    base: &str,
    signing: &SigningKey,
    browser_id: &str,
    deadline: Duration,
) -> Result<(Socket, String), String> {
    let mut socket = connect(base, deadline).await?;
    send_raw(
        &mut socket,
        json!({"type":"auth_challenge","browser_id":browser_id}),
        deadline,
    )
    .await?;
    let nonce = receive(&mut socket, deadline).await?;
    let signature = signing.sign(
        &URL_SAFE_NO_PAD
            .decode(
                nonce["nonce"]
                    .as_str()
                    .ok_or_else(|| "challenge omitted nonce".to_string())?,
            )
            .map_err(|error| error.to_string())?,
    );
    send_raw(&mut socket, json!({"type":"auth_response","challenge_id":nonce["challenge_id"],"signature":URL_SAFE_NO_PAD.encode(signature.to_bytes())}), deadline).await?;
    let authenticated = receive(&mut socket, deadline).await?;
    let id = authenticated["browser_id"]
        .as_str()
        .ok_or_else(|| format!("authentication failed: {authenticated}"))?
        .to_string();
    Ok((socket, id))
}

async fn connect(base: &str, deadline: Duration) -> Result<Socket, String> {
    let url = format!("{}/browser/socket", base.replacen("http://", "ws://", 1));
    let mut request = url
        .into_client_request()
        .map_err(|error| error.to_string())?;
    request.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{EXTENSION_ID}")
            .parse()
            .map_err(|error| format!("invalid extension origin: {error}"))?,
    );
    tokio::time::timeout(deadline, tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| "browser connect deadline elapsed".to_string())?
        .map(|(socket, _)| socket)
        .map_err(|error| error.to_string())
}

async fn send_raw(socket: &mut Socket, mut value: Value, deadline: Duration) -> Result<(), String> {
    value["version"] = json!(1);
    value["request_id"] = json!(uuid::Uuid::new_v4().to_string());
    tokio::time::timeout(
        deadline,
        socket.send(tokio_tungstenite::tungstenite::Message::Text(
            value.to_string().into(),
        )),
    )
    .await
    .map_err(|_| "browser send deadline elapsed".to_string())?
    .map_err(|error| error.to_string())
}

async fn receive(socket: &mut Socket, deadline: Duration) -> Result<Value, String> {
    let message = tokio::time::timeout(deadline, socket.next())
        .await
        .map_err(|_| "browser receive deadline elapsed".to_string())?
        .ok_or_else(|| "browser socket closed".to_string())?
        .map_err(|error| error.to_string())?;
    serde_json::from_str(message.to_text().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}

async fn decode_terminal(response: reqwest::Response) -> Result<HttpTerminal, String> {
    let status = response.status();
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    if status.is_success() {
        Ok(HttpTerminal::Success(body))
    } else {
        Ok(HttpTerminal::Error {
            status: status.as_u16(),
            kind: body["kind"].as_str().unwrap_or("unknown").to_string(),
        })
    }
}

fn bounded_audit_snapshot(
    connection: &rusqlite::Connection,
    browser_id: &str,
) -> Result<Vec<InvocationAudit>, String> {
    let mut statement = connection
            .prepare(
                "SELECT substr(id,1,129),substr(outcome,1,65),substr(error_kind,1,129) FROM invocation_audits WHERE browser_id=?1 AND tool_name=?2 ORDER BY created_at,id LIMIT 101",
            )
            .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(rusqlite::params![browser_id, TOOL_NAME], |row| {
            Ok(InvocationAudit {
                id: row.get(0)?,
                outcome: row.get(1)?,
                error_kind: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?;
    let rows = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    if rows.len() > 100 {
        return Err("conformance audit observation exceeds 100-row bound".into());
    }
    if rows.iter().any(|audit| {
        audit.id.len() > 128
            || audit.outcome.len() > 64
            || audit
                .error_kind
                .as_ref()
                .is_some_and(|kind| kind.len() > 128)
    }) {
        return Err("conformance audit field exceeds observation bound".into());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conformance_audit_observation_rejects_oversized_rows_and_fields() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE invocation_audits(id TEXT, outcome TEXT, error_kind TEXT, browser_id TEXT, tool_name TEXT, created_at INTEGER)").unwrap();
        let insert = |id: &str, outcome: &str, kind: Option<&str>| {
            connection
                .execute(
                    "INSERT INTO invocation_audits VALUES (?1,?2,?3,'browser',?4,0)",
                    rusqlite::params![id, outcome, kind, TOOL_NAME],
                )
                .unwrap();
        };
        insert("call", "succeeded", None);
        assert_eq!(
            bounded_audit_snapshot(&connection, "browser").unwrap(),
            vec![InvocationAudit {
                id: "call".into(),
                outcome: "succeeded".into(),
                error_kind: None
            }]
        );
        for (id, outcome, kind) in [
            ("x".repeat(129), "succeeded".into(), None),
            ("call".into(), "x".repeat(65), None),
            ("call".into(), "failed".into(), Some("x".repeat(129))),
        ] {
            connection
                .execute("DELETE FROM invocation_audits", [])
                .unwrap();
            insert(&id, &outcome, kind.as_deref());
            assert_eq!(
                bounded_audit_snapshot(&connection, "browser").unwrap_err(),
                "conformance audit field exceeds observation bound"
            );
        }
        connection
            .execute("DELETE FROM invocation_audits", [])
            .unwrap();
        for _ in 0..101 {
            insert("call", "succeeded", None);
        }
        assert_eq!(
            bounded_audit_snapshot(&connection, "browser").unwrap_err(),
            "conformance audit observation exceeds 100-row bound"
        );
        assert!(
            bounded_audit_snapshot(&connection, "unrelated-browser")
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn conformance_http_caller_abort_delivers_cancel_and_late_completion_only_acks() {
        let mut fixture = ConformanceFixture::start(Duration::from_secs(20))
            .await
            .expect("start controlled conformance fixture");
        let mut pending = fixture.begin_call(json!({"value":"controlled"}));
        let dispatched = fixture
            .wait_dispatch(&mut pending)
            .await
            .expect("public tool_call dispatch barrier");

        ConformanceFixture::abort_caller(&mut pending)
            .await
            .expect("abort caller after dispatch");
        fixture
            .wait_cancel(&dispatched)
            .await
            .expect("matching public tool_cancel barrier");
        let before_late = fixture
            .wait_audit_count(1)
            .await
            .expect("post-dispatch cancellation audit terminal");
        assert_eq!(before_late[0].outcome, "abandoned");
        assert_eq!(
            before_late[0].error_kind.as_deref(),
            Some("caller_cancelled")
        );
        fixture
            .send_late_completion(&dispatched, json!({"late":true}))
            .await
            .expect("late completion receives only the public generic acknowledgement");
        assert_eq!(fixture.audit_snapshot().unwrap(), before_late);

        assert!(fixture.evidence().iter().any(|event| matches!(
            event,
            PublicObservation::CancellationReceived { call_id } if call_id == &dispatched.call_id
        )));
        let cleanup = fixture.finish().await;
        assert!(
            cleanup.is_clean(),
            "fixture cleanup: {:?}",
            cleanup.failures
        );
    }

    #[tokio::test]
    async fn conformance_pre_dispatch_cancellation_has_durable_immutable_terminal() {
        let mut fixture = ConformanceFixture::start(Duration::from_secs(20))
            .await
            .expect("start controlled conformance fixture");
        let barrier = fixture
            .hold_audit_writes()
            .expect("hold durable audit before admission");
        let mut pending = fixture.begin_call(json!({"value":"never-dispatched"}));
        let admitted = fixture
            .wait_admitted(&mut pending)
            .await
            .expect("structured admission barrier");
        ConformanceFixture::abort_caller(&mut pending)
            .await
            .expect("cancel admitted HTTP caller");
        fixture
            .wait_cancel(&admitted)
            .await
            .expect("cancel is the first socket event, before any tool_call");
        barrier.release().expect("release durable audit barrier");

        let before_late = fixture
            .wait_audit_count(1)
            .await
            .expect("cancelled audit terminal");
        assert_eq!(before_late[0].outcome, "abandoned");
        assert_eq!(
            before_late[0].error_kind.as_deref(),
            Some("caller_cancelled")
        );
        fixture
            .send_late_completion(&admitted, json!({"late":true}))
            .await
            .expect("late completion receives generic acknowledgement");
        assert_eq!(fixture.audit_snapshot().unwrap(), before_late);

        let cleanup = fixture.finish().await;
        assert!(
            cleanup.is_clean(),
            "fixture cleanup: {:?}",
            cleanup.failures
        );
    }

    #[tokio::test]
    async fn conformance_replacement_generation_owns_only_new_calls() {
        let mut fixture = ConformanceFixture::start(Duration::from_secs(20))
            .await
            .expect("start controlled conformance fixture");
        let mut old_pending = fixture.begin_call(json!({"generation":1}));
        let old_call = fixture
            .wait_dispatch(&mut old_pending)
            .await
            .expect("old generation dispatch");
        assert_eq!(old_call.generation_label, "fixture-generation-1");
        assert_eq!(
            fixture
                .replace_connection_with_pending(&old_call)
                .await
                .expect("authenticate replacement generation"),
            "fixture-generation-2"
        );
        assert!(matches!(
            fixture.wait_http_terminal(old_pending).await.unwrap(),
            HttpTerminal::Error { kind, .. } if kind == "browser_offline"
        ));

        let mut new_pending = fixture.begin_call(json!({"generation":2}));
        let new_call = fixture
            .wait_dispatch(&mut new_pending)
            .await
            .expect("replacement generation dispatch");
        assert_eq!(new_call.generation_label, "fixture-generation-2");
        fixture
            .complete_success(&new_call, json!({"accepted":true}))
            .await
            .expect("replacement generation completion");
        assert_eq!(
            fixture.wait_http_terminal(new_pending).await.unwrap(),
            HttpTerminal::Success(json!({"accepted":true}))
        );
        let audits = fixture.wait_audit_count(2).await.unwrap();
        assert!(audits.iter().any(|audit| audit.outcome == "failed"
            && audit.error_kind.as_deref() == Some("browser_offline")));
        assert!(audits.iter().any(|audit| audit.outcome == "succeeded"));

        let cleanup = fixture.finish().await;
        assert!(
            cleanup.is_clean(),
            "fixture cleanup: {:?}",
            cleanup.failures
        );
    }
}
