use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::FutureExt as _;
use futures::future::BoxFuture;
use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities,
    ClientInfo, ClientRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
    ElicitationCapability, FormElicitationCapability, Implementation, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, Tool,
};
use rmcp::service::{
    ClientLifecycleMode, ClientServiceExt, PeerRequestOptions, RequestContext, RunningService,
};
use rmcp::transport::TokioChildProcess;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::{ClientHandler, ErrorData, RoleClient};
use serde_json::{Map, Value};

use crate::support::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command};

const TEST_TOKEN: &str = "q1-mcp-transport-token";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_PAGES: usize = 8;
const MAX_TOOLS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransportKind {
    Stdio,
    StreamableHttp,
}

pub(crate) struct TransportQualification {
    kind: TransportKind,
    service: Option<RunningService<RoleClient, QualificationClient>>,
    guard: Option<LiveLabbyGuard>,
    owned_root: Option<tempfile::TempDir>,
    ledger: PathBuf,
    upstream_pid_file: PathBuf,
    stdio_pid: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
struct QualificationClient {
    supports_elicitation: bool,
}

impl Default for QualificationClient {
    fn default() -> Self {
        Self {
            supports_elicitation: true,
        }
    }
}

impl ClientHandler for QualificationClient {
    fn get_info(&self) -> ClientInfo {
        let capabilities = if self.supports_elicitation {
            ClientCapabilities::builder()
                .enable_elicitation_with(
                    ElicitationCapability::new().with_form(FormElicitationCapability::new()),
                )
                .build()
        } else {
            ClientCapabilities::default()
        };
        ClientInfo::new(
            capabilities,
            Implementation::new("labby-q1-qualification", "1"),
        )
        .with_protocol_version(ProtocolVersion::V_2026_07_28)
    }

    async fn create_elicitation(
        &self,
        _request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        Ok(ElicitResult::new(ElicitationAction::Decline))
    }
}

impl TransportQualification {
    pub(crate) const fn kind(&self) -> TransportKind {
        self.kind
    }

    pub(crate) fn http_endpoint(&self) -> Option<String> {
        self.guard
            .as_ref()
            .map(|guard| format!("{}/mcp", guard.connection().base_url))
    }

    pub(crate) const fn http_token(&self) -> &'static str {
        TEST_TOKEN
    }

    pub(crate) fn effect_ledger_path(&self) -> &Path {
        &self.ledger
    }

    pub(crate) async fn start(kind: TransportKind, secret_canary: &str) -> Result<Self, String> {
        Self::start_with_elicitation(kind, secret_canary, true).await
    }

    pub(crate) async fn start_without_elicitation(
        kind: TransportKind,
        secret_canary: &str,
    ) -> Result<Self, String> {
        Self::start_with_elicitation(kind, secret_canary, false).await
    }

    async fn start_with_elicitation(
        kind: TransportKind,
        secret_canary: &str,
        supports_elicitation: bool,
    ) -> Result<Self, String> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let owned_parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&owned_parent).map_err(|error| error.to_string())?;
        let owned_root = tempfile::Builder::new()
            .prefix("q1-mcp-")
            .tempdir_in(owned_parent)
            .map_err(|error| error.to_string())?;
        let ledger = owned_root.path().join("forge-effects.count");
        let upstream_pid_file = owned_root.path().join("forge.pid");
        let config = gateway_fixture_config(&ledger, &upstream_pid_file)?;

        let (service, guard, stdio_pid) = match kind {
            TransportKind::Stdio => {
                let labby_home = owned_root.path().join(".labby");
                std::fs::create_dir_all(&labby_home).map_err(|error| error.to_string())?;
                std::fs::create_dir_all(owned_root.path().join("tmp"))
                    .map_err(|error| error.to_string())?;
                std::fs::write(labby_home.join("config.toml"), &config)
                    .map_err(|error| error.to_string())?;
                let mut command = isolated_command(owned_root.path());
                command
                    .env("LABBY_Q1_SECRET_CANARY", secret_canary)
                    .arg("mcp");
                let transport = TokioChildProcess::new(tokio::process::Command::from(command))
                    .map_err(|error| error.to_string())?;
                let pid = transport
                    .id()
                    .ok_or_else(|| "stdio Labby child exposed no PID".to_owned())?;
                let service = connect(transport, "stdio", supports_elicitation).await?;
                (service, None, Some(pid))
            }
            TransportKind::StreamableHttp => {
                let guard = LiveLabbyBuilder::new()
                    .existing_root(owned_root.path())
                    .config(config)
                    .env("LABBY_MCP_HTTP_TOKEN", TEST_TOKEN)
                    .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
                    .env("LABBY_Q1_SECRET_CANARY", secret_canary)
                    .start()
                    .await?;
                let endpoint = format!("{}/mcp", guard.connection().base_url);
                let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
                config.auth_header = Some(TEST_TOKEN.to_owned());
                config.custom_headers.insert(
                    "x-labby-project-id".parse().expect("project header name"),
                    "disposable".parse().expect("project header value"),
                );
                config.custom_headers.insert(
                    "x-labby-team-id".parse().expect("team header name"),
                    LiveLabbyGuard::HARNESS_TEAM_ID
                        .parse()
                        .expect("team header value"),
                );
                let worker = StreamableHttpClientWorker::new(
                    BodyCappedHttpClient::new(reqwest::Client::new(), MAX_RESPONSE_BYTES),
                    config,
                );
                let service = connect(worker, "Streamable HTTP", supports_elicitation).await?;
                (service, Some(guard), None)
            }
        };

        Ok(Self {
            kind,
            service: Some(service),
            guard,
            owned_root: Some(owned_root),
            ledger,
            upstream_pid_file,
            stdio_pid,
        })
    }

    pub(crate) fn assert_exact_server_identity(
        &self,
        name: &str,
        version: &str,
        protocol_version: &str,
    ) -> Result<(), String> {
        let info = self
            .service
            .as_ref()
            .and_then(|service| service.peer().peer_info())
            .ok_or("MCP discovery omitted server peer info")?;
        if info.protocol_version.as_str() != protocol_version {
            return Err(format!(
                "protocol version was {}, expected {protocol_version}",
                info.protocol_version
            ));
        }
        let implementation = info
            .server_info
            .as_ref()
            .ok_or("MCP discovery omitted server implementation")?;
        if implementation.name != name || implementation.version != version {
            return Err(format!(
                "server identity was {} {}, expected {name} {version}",
                implementation.name, implementation.version
            ));
        }
        if info.capabilities.tools.is_none() {
            return Err("MCP discovery did not advertise tools".into());
        }
        Ok(())
    }

    pub(crate) async fn discover_tools(&self) -> Result<BTreeMap<String, Tool>, String> {
        let deadline = tokio::time::Instant::now() + SETTLEMENT_TIMEOUT;
        loop {
            let tools = self.list_tools_once(deadline).await?;
            if tools.contains_key("doctor") && tools.contains_key("forge.delay") {
                wait_for_file(&self.upstream_pid_file, deadline).await?;
                return Ok(tools);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "tool catalog did not converge before deadline: {:?}",
                    tools.keys().collect::<Vec<_>>()
                ));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn list_tools_once(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<BTreeMap<String, Tool>, String> {
        let peer = self.service.as_ref().expect("active service").peer();
        let mut cursor = None;
        let mut tools = BTreeMap::new();
        for _ in 0..MAX_PAGES {
            let params = cursor
                .take()
                .map(|value| PaginatedRequestParams::default().with_cursor(Some(value)));
            let page = tokio::time::timeout_at(deadline, peer.list_tools(params))
                .await
                .map_err(|_| "tools/list exceeded the absolute deadline".to_owned())?
                .map_err(|error| error.to_string())?;
            for tool in page.tools {
                if tools.len() >= MAX_TOOLS {
                    return Err("tools/list exceeded the item bound".into());
                }
                let name = tool.name.to_string();
                if tools.insert(name.clone(), tool).is_some() {
                    return Err(format!("tools/list returned duplicate `{name}`"));
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err("tools/list exceeded the page bound".into())
    }

    pub(crate) async fn call_service(
        &self,
        service: &str,
        action: &str,
        params: Value,
    ) -> Result<CallToolResult, String> {
        self.call_raw(
            service,
            serde_json::json!({"action": action, "params": params}),
        )
        .await
    }

    pub(crate) async fn call_raw(
        &self,
        tool: &str,
        arguments: Value,
    ) -> Result<CallToolResult, String> {
        let arguments = arguments
            .as_object()
            .cloned()
            .ok_or("tools/call arguments must be an object")?;
        let request = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .peer()
                .call_tool(request),
        )
        .await
        .map_err(|_| "tools/call exceeded the request deadline".to_owned())?
        .map_err(|error| error.to_string())
    }

    pub(crate) async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, String> {
        tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .peer()
                .read_resource(ReadResourceRequestParams::new(uri)),
        )
        .await
        .map_err(|_| "resources/read exceeded the request deadline".to_owned())?
        .map_err(|error| error.to_string())
    }

    pub(crate) async fn decline_destructive_service(
        &self,
        service: &str,
        action: &str,
        params: Value,
    ) -> Result<CallToolResult, String> {
        let arguments = serde_json::json!({"action": action, "params": params})
            .as_object()
            .cloned()
            .expect("literal tools/call arguments are an object");
        let request = CallToolRequestParams::new(service.to_owned()).with_arguments(arguments);
        let request = self
            .confirmation_response_params(request, ElicitationAction::Decline, None)
            .await?;
        let response = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .call_tool_once(request),
        )
        .await
        .map_err(|_| "declined destructive call exceeded the request deadline".to_owned())?
        .map_err(|error| error.to_string())?;
        match response {
            CallToolResponse::Complete(result) => Ok(result),
            CallToolResponse::InputRequired(challenge) => Err(format!(
                "declined destructive call returned another input challenge: {challenge:?}"
            )),
            _ => Err("declined destructive call returned an unsupported response variant".into()),
        }
    }

    pub(crate) async fn answer_destructive_upstream(
        &self,
        action: ElicitationAction,
        content: Option<Value>,
    ) -> Result<CallToolResult, String> {
        let params = CallToolRequestParams::new("forge.destructive").with_arguments(Map::new());
        let params = self
            .confirmation_response_params(params, action, content)
            .await?;
        self.complete_once(params).await
    }

    pub(crate) async fn answer_destructive_upstream_malformed(
        &self,
    ) -> Result<CallToolResult, String> {
        let params = CallToolRequestParams::new("forge.destructive").with_arguments(Map::new());
        let mut params = self
            .confirmation_response_params(params, ElicitationAction::Accept, None)
            .await?;
        params.input_responses = Some(BTreeMap::from([(
            "destructive_confirmation".to_owned(),
            serde_json::json!({"action":"accept","content":{"confirm":"not-a-boolean"}}),
        )]));
        self.complete_once(params).await
    }

    pub(crate) async fn abandon_destructive_challenge(&self) -> Result<(), String> {
        let params = CallToolRequestParams::new("forge.destructive").with_arguments(Map::new());
        let response = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .call_tool_once(params),
        )
        .await
        .map_err(|_| "abandoned challenge exceeded the request deadline".to_owned())?
        .map_err(|error| error.to_string())?;
        if matches!(response, CallToolResponse::InputRequired(_)) {
            Ok(())
        } else {
            Err(format!(
                "destructive call did not produce a challenge: {response:?}"
            ))
        }
    }

    pub(crate) async fn call_destructive_without_elicitation(
        &self,
    ) -> Result<CallToolResult, String> {
        self.complete_once(
            CallToolRequestParams::new("forge.destructive").with_arguments(Map::new()),
        )
        .await
    }

    async fn complete_once(&self, params: CallToolRequestParams) -> Result<CallToolResult, String> {
        let response = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .call_tool_once(params),
        )
        .await
        .map_err(|_| "destructive response exceeded the request deadline".to_owned())?
        .map_err(|error| error.to_string())?;
        match response {
            CallToolResponse::Complete(result) => Ok(result),
            CallToolResponse::InputRequired(challenge) => Err(format!(
                "destructive response returned another challenge: {challenge:?}"
            )),
            _ => Err("destructive response returned an unsupported variant".into()),
        }
    }

    pub(crate) async fn cancel_delayed_upstream_after_effect(&mut self) -> Result<(), String> {
        let (before, active) = self.effect_counts()?;
        if active != 0 {
            return Err("effect ledger had an active call before cancellation probe".into());
        }
        let params = CallToolRequestParams::new("forge.delay").with_arguments(Map::new());
        let params = self
            .confirmation_response_params(
                params,
                ElicitationAction::Accept,
                Some(serde_json::json!({"confirm": true})),
            )
            .await?;
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let handle = self
            .service
            .as_ref()
            .expect("active service")
            .peer()
            .send_cancellable_request(request, PeerRequestOptions::with_timeout(REQUEST_TIMEOUT))
            .await
            .map_err(|error| error.to_string())?;

        let admitted = before
            .checked_add(1)
            .ok_or("effect counter exhausted before cancellation probe")?;
        self.wait_for_effect_counts((admitted, 1)).await?;
        handle
            .cancel(Some("q1 cancellation after observed effect".to_owned()))
            .await
            .map_err(|error| error.to_string())?;
        self.wait_for_effect_counts((admitted, 0)).await
    }

    async fn confirmation_response_params(
        &self,
        mut params: CallToolRequestParams,
        action: ElicitationAction,
        content: Option<Value>,
    ) -> Result<CallToolRequestParams, String> {
        let challenge = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .call_tool_once(params.clone()),
        )
        .await
        .map_err(|_| "destructive confirmation challenge exceeded the deadline".to_owned())?
        .map_err(|error| error.to_string())?;
        let CallToolResponse::InputRequired(challenge) = challenge else {
            return Err(format!(
                "delayed upstream did not return a confirmation challenge: {challenge:?}"
            ));
        };
        let requests = challenge
            .input_requests
            .ok_or("confirmation challenge omitted input requests")?;
        if requests.len() != 1 || !requests.contains_key("destructive_confirmation") {
            return Err(format!(
                "confirmation challenge had unexpected inputs: {:?}",
                requests.keys().collect::<Vec<_>>()
            ));
        }
        let request_state = challenge
            .request_state
            .ok_or("confirmation challenge omitted request state")?;
        let mut confirmation = ElicitResult::new(action);
        if let Some(content) = content {
            confirmation = confirmation.with_content(content);
        }
        params.input_responses = Some(BTreeMap::from([(
            "destructive_confirmation".to_owned(),
            serde_json::to_value(confirmation).map_err(|error| error.to_string())?,
        )]));
        params.request_state = Some(request_state);
        Ok(params)
    }

    pub(crate) fn effect_counts(&self) -> Result<(u64, u64), String> {
        let text = std::fs::read_to_string(&self.ledger).map_err(|error| error.to_string())?;
        let mut fields = text.split_ascii_whitespace();
        let invocations = fields
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or("effect ledger omitted invocation count")?;
        let active = fields
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or("effect ledger omitted active count")?;
        if fields.next().is_some() {
            return Err("effect ledger contained trailing fields".into());
        }
        Ok((invocations, active))
    }

    async fn wait_for_effect_counts(&self, expected: (u64, u64)) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + SETTLEMENT_TIMEOUT;
        loop {
            if self.effect_counts().ok() == Some(expected) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "effect ledger did not reach {expected:?}; last observation: {:?}",
                    self.effect_counts()
                ));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        let mut cleanup = CleanupResult::default();
        if let Some(mut service) = self.service.take() {
            match service.close_with_timeout(REQUEST_TIMEOUT).await {
                Ok(Some(_)) => {}
                Ok(None) => cleanup
                    .failures
                    .push("MCP client close exceeded the request deadline".into()),
                Err(error) => cleanup
                    .failures
                    .push(format!("MCP client close join failed: {error}")),
            }
        }
        if let Some(guard) = self.guard.take() {
            let owned = guard.finish().await;
            cleanup.failures.extend(owned.failures);
        }

        for (label, pid) in [
            ("stdio Labby", self.stdio_pid),
            ("stdio upstream", read_pid(&self.upstream_pid_file).ok()),
        ] {
            if let Some(pid) = pid {
                match wait_for_process_exit(pid).await {
                    Ok(true) => {}
                    Ok(false) => cleanup.failures.push(format!(
                        "{label} child {pid} remained observable after {:?} cleanup",
                        self.kind
                    )),
                    Err(error) => cleanup
                        .failures
                        .push(format!("{label} child {pid} liveness failed: {error}")),
                }
            }
        }

        if self.effect_counts().is_ok_and(|(_, active)| active != 0) {
            cleanup
                .failures
                .push("upstream effect remained active after cleanup".into());
        }
        drop(self.owned_root.take());
        cleanup
    }
}

pub(crate) async fn run_failure_safe(
    kind: TransportKind,
    secret_canary: &str,
    operation: for<'a> fn(&'a mut TransportQualification) -> BoxFuture<'a, Result<(), String>>,
) -> Result<(), String> {
    let mut harness = TransportQualification::start(kind, secret_canary).await?;
    let outcome = AssertUnwindSafe(operation(&mut harness))
        .catch_unwind()
        .await;
    let cleanup = harness.finish().await;
    let cleanup_failure = (!cleanup.is_clean())
        .then(|| format!("{kind:?} cleanup was not clean: {:?}", cleanup.failures));

    match (outcome, cleanup_failure) {
        (Ok(Ok(())), None) => Ok(()),
        (Ok(Ok(())), Some(cleanup)) => Err(cleanup),
        (Ok(Err(primary)), None) => Err(primary),
        (Ok(Err(primary)), Some(cleanup)) => Err(format!("{primary}; {cleanup}")),
        (Err(primary), None) => Err(panic_message(primary.as_ref())),
        (Err(primary), Some(cleanup)) => {
            Err(format!("{}; {cleanup}", panic_message(primary.as_ref())))
        }
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "qualification panicked with a non-string payload".to_owned()
    }
}

async fn connect<T, E, A>(
    transport: T,
    label: &str,
    supports_elicitation: bool,
) -> Result<RunningService<RoleClient, QualificationClient>, String>
where
    T: rmcp::transport::IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        QualificationClient {
            supports_elicitation,
        }
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    )
    .await
    .map_err(|_| format!("{label} MCP discovery exceeded the request deadline"))?
    .map_err(|error| error.to_string())
}

fn gateway_fixture_config(ledger: &Path, pid_file: &Path) -> Result<String, String> {
    let fixture = env!("CARGO_BIN_EXE_stdio-mcp-fixture");
    Ok(format!(
        "[code_mode]\nenabled = false\n\n[gateway]\nextra_stdio_commands = [{}]\n\n[[upstream]]\nname = \"forge\"\ntransport = \"stdio\"\ncommand = {}\nargs = [\"--forge\", \"--forge-ledger\", {}, \"--pid-file\", {}]\nproxy_resources = true\n",
        serde_json::to_string(fixture).map_err(|error| error.to_string())?,
        serde_json::to_string(fixture).map_err(|error| error.to_string())?,
        serde_json::to_string(ledger).map_err(|error| error.to_string())?,
        serde_json::to_string(pid_file).map_err(|error| error.to_string())?,
    ))
}

pub(crate) fn assert_service_tool_schema(tool: &Tool) -> Result<(), String> {
    let schema = &tool.input_schema;
    if schema.get("type") != Some(&Value::String("object".to_owned())) {
        return Err("service tool schema type was not `object`".into());
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("service tool schema omitted properties")?;
    if properties.get("action").and_then(|value| value.get("type"))
        != Some(&Value::String("string".to_owned()))
    {
        return Err("service tool action schema was not `string`".into());
    }
    if properties.get("params").and_then(|value| value.get("type"))
        != Some(&Value::String("object".to_owned()))
    {
        return Err("service tool params schema was not `object`".into());
    }
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .ok_or("service tool schema omitted required fields")?;
    if !required.iter().any(|value| value == "action") {
        return Err("service tool schema did not require `action`".into());
    }
    Ok(())
}

pub(crate) fn assert_typed_error(
    result: &CallToolResult,
    expected_kind: &str,
    secret_canary: &str,
) -> Result<(), String> {
    if result.is_error != Some(true) {
        return Err(format!("expected error result, got {result:?}"));
    }
    let text = result_text(result);
    let envelope = result
        .structured_content
        .clone()
        .or_else(|| serde_json::from_str(&text).ok())
        .ok_or_else(|| format!("error result was not a JSON envelope: {text}"))?;
    if envelope["error"]["kind"] != expected_kind {
        return Err(format!(
            "error kind was {:?}, expected {expected_kind}: {envelope}",
            envelope["error"]["kind"]
        ));
    }
    if envelope["error"].get("recovery").is_none() {
        return Err(format!("typed error omitted recovery metadata: {envelope}"));
    }
    if envelope["error"].get("side_effects").is_none() {
        return Err(format!(
            "typed error omitted side-effect metadata: {envelope}"
        ));
    }
    let serialized = serde_json::to_string(&envelope).map_err(|error| error.to_string())?;
    if serialized.contains(secret_canary) || text.contains(secret_canary) {
        return Err("typed error reflected the secret canary".into());
    }
    Ok(())
}

pub(crate) fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn wait_for_file(path: &Path, deadline: tokio::time::Instant) -> Result<(), String> {
    while tokio::time::Instant::now() < deadline {
        if path.is_file() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!(
        "{} was not created before deadline",
        path.display()
    ))
}

fn read_pid(path: &Path) -> Result<u32, String> {
    std::fs::read_to_string(path)
        .map_err(|error| error.to_string())?
        .trim()
        .parse()
        .map_err(|error: std::num::ParseIntError| error.to_string())
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> Result<bool, String> {
    let pid = i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "invalid child PID".to_owned())?;
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => Ok(true),
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> Result<bool, String> {
    use labby_winjob::ProcessLiveness;
    match labby_winjob::pid_liveness(pid).map_err(|error| error.to_string())? {
        ProcessLiveness::Exited | ProcessLiveness::NotFound => Ok(false),
        ProcessLiveness::Alive => Ok(true),
    }
}

async fn wait_for_process_exit(pid: u32) -> Result<bool, String> {
    let deadline = tokio::time::Instant::now() + SETTLEMENT_TIMEOUT;
    loop {
        if !process_is_alive(pid)? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_bounds_are_literal_and_small() {
        assert_eq!(MAX_PAGES, 8);
        assert_eq!(MAX_TOOLS, 64);
        assert_eq!(MAX_RESPONSE_BYTES, 1_048_576);
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(10));
        assert_eq!(SETTLEMENT_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn service_schema_rejects_non_service_shape() {
        let tool: Tool = serde_json::from_value(serde_json::json!({
            "name": "wrong",
            "description": "wrong",
            "inputSchema": {}
        }))
        .expect("literal tool is valid");
        assert_eq!(
            assert_service_tool_schema(&tool).unwrap_err(),
            "service tool schema type was not `object`"
        );
    }

    #[test]
    fn typed_error_requires_contract_metadata_and_redaction() {
        let envelope = serde_json::json!({
            "error": {
                "kind": "invalid_param",
                "recovery": "fix the argument",
                "side_effects": "none"
            }
        });
        let result =
            CallToolResult::error(vec![rmcp::model::ContentBlock::text(envelope.to_string())]);
        assert!(assert_typed_error(&result, "invalid_param", "secret-canary").is_ok());
    }
}
