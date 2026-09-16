use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ResourceContents,
};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use rmcp::{
    RoleClient,
    service::{ClientLifecycleMode, ClientServiceExt, RunningService},
};
use serde_json::{Map, Value, json};

use crate::live_labby::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, RunIdentity};

const TOKEN: &str = "codemode-qualification-token";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) timeout_ms: u64,
    pub(crate) max_response_bytes: usize,
    pub(crate) max_response_tokens: usize,
    pub(crate) max_calls_per_run: Option<u64>,
    pub(crate) upstream_request_timeout_ms: Option<u64>,
    pub(crate) upstream_max_in_flight: Option<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_ms: 2_000,
            max_response_bytes: 16_384,
            max_response_tokens: 4_096,
            max_calls_per_run: None,
            upstream_request_timeout_ms: None,
            upstream_max_in_flight: None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Execution {
    pub(crate) structured: Value,
    pub(crate) elapsed: Duration,
    pub(crate) wire_bytes: usize,
    pub(crate) is_error: bool,
}

pub(crate) struct CodeModeQualification {
    guard: Option<LiveLabbyGuard>,
    service: Option<RunningService<RoleClient, ()>>,
    owned_root: Option<tempfile::TempDir>,
    pub(crate) limits: Limits,
}

impl CodeModeQualification {
    pub(crate) async fn start(limits: Limits) -> Result<Self, String> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let fixture = env!("CARGO_BIN_EXE_stdio-mcp-fixture");
        let owned_parent = std::env::temp_dir().join("labby-live-e2e");
        std::fs::create_dir_all(&owned_parent).map_err(|error| error.to_string())?;
        let owned_root = tempfile::Builder::new()
            .prefix("q3-")
            .tempdir_in(owned_parent)
            .map_err(|error| error.to_string())?;
        let ledger = owned_root.path().join("forge-effects.count");
        let config = format!(
            "{}[code_mode]\nenabled = true\ntrace_params = true\ntimeout_ms = {}\nmax_response_bytes = {}\nmax_response_tokens = {}\n{}\n[gateway]\nextra_stdio_commands = [{}]\n\n[[upstream]]\nname = \"forge\"\ntransport = \"stdio\"\ncommand = {}\nargs = [\"--forge\", \"--forge-ledger\", {}]\nproxy_resources = true\n",
            limits
                .upstream_request_timeout_ms
                .map_or_else(String::new, |value| format!(
                    "upstream_request_timeout_ms = {value}\n"
                )),
            limits.timeout_ms,
            limits.max_response_bytes,
            limits.max_response_tokens,
            limits
                .max_calls_per_run
                .map_or_else(String::new, |value| format!(
                    "max_calls_per_run = {value}\n"
                )),
            serde_json::to_string(fixture).map_err(|error| error.to_string())?,
            serde_json::to_string(fixture).map_err(|error| error.to_string())?,
            serde_json::to_string(&ledger).map_err(|error| error.to_string())?,
        );
        let mut builder = LiveLabbyBuilder::new()
            .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
            .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
            .config(config)
            .existing_root(owned_root.path());
        if let Some(max_calls) = limits.max_calls_per_run {
            builder = builder.env("LABBY_CODE_MODE_MAX_CALLS_PER_RUN", max_calls.to_string());
        }
        if let Some(max_in_flight) = limits.upstream_max_in_flight {
            builder = builder.env("LABBY_GW_UPSTREAM_MAX_IN_FLIGHT", max_in_flight.to_string());
        }
        let guard = builder.start().await?;
        let endpoint = format!("{}/mcp", guard.connection().base_url);
        let mut transport = StreamableHttpClientTransportConfig::with_uri(endpoint);
        transport.auth_header = Some(TOKEN.to_string());
        let worker = StreamableHttpClientWorker::new(
            BodyCappedHttpClient::new(reqwest::Client::new(), MAX_RESPONSE_BYTES),
            transport,
        );
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            ().serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "Code Mode MCP initialize timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Self {
            guard: Some(guard),
            service: Some(service),
            owned_root: Some(owned_root),
            limits,
        })
    }

    pub(crate) fn identity(&self) -> RunIdentity {
        self.guard
            .as_ref()
            .expect("active guard")
            .identity()
            .clone()
    }

    pub(crate) async fn discover_mcp_tools(&self) -> Result<Vec<String>, String> {
        let peer = self.service.as_ref().expect("active service").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let mut cursor = None;
        let mut names = Vec::new();
        for _ in 0..8 {
            let params = cursor
                .take()
                .map(|value| PaginatedRequestParams::default().with_cursor(Some(value)));
            let page = tokio::time::timeout_at(deadline, peer.list_tools(params))
                .await
                .map_err(|_| "MCP tools/list timed out".to_owned())?
                .map_err(|error| error.to_string())?;
            names.extend(page.tools.into_iter().map(|tool| tool.name.to_string()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(names);
            }
        }
        Err("MCP tools/list exceeded the eight-page qualification bound".to_owned())
    }

    pub(crate) async fn execute(&self, code: &str) -> Result<Execution, String> {
        let started = Instant::now();
        let request = CallToolRequestParams::new("codemode".to_string()).with_arguments(
            Map::from_iter([("code".to_string(), Value::String(code.to_string()))]),
        );
        let result = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.service
                .as_ref()
                .expect("active service")
                .call_tool(request),
        )
        .await
        .map_err(|_| "Code Mode call timed out".to_string())?
        .map_err(|error| error.to_string())?;
        let text = result_text(&result);
        let wire_bytes = text.len();
        let is_error = result.is_error == Some(true);
        let structured = result.structured_content.clone().unwrap_or_else(|| {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({"text": text}))
        });
        Ok(Execution {
            structured,
            elapsed: started.elapsed(),
            wire_bytes,
            is_error,
        })
    }

    pub(crate) async fn fixture_invocation_count(&self) -> Result<u64, String> {
        let peer = self.service.as_ref().expect("active service").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        for _ in 0..8 {
            let params = cursor
                .take()
                .map(|value| PaginatedRequestParams::default().with_cursor(Some(value)));
            let page = tokio::time::timeout_at(deadline, peer.list_resources(params))
                .await
                .map_err(|_| "resources/list timed out".to_string())?
                .map_err(|error| error.to_string())?;
            for resource in page.resources {
                if !seen.insert(resource.uri.clone()) {
                    return Err(format!("duplicate resource URI: {}", resource.uri));
                }
                if resource.uri.contains("forge-status") {
                    return read_effects(peer, resource.uri, deadline)
                        .await
                        .map(|effects| effects.0);
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        // Regular upstream resources may be omitted from the root discovery
        // snapshot while project routing is being established. The gateway
        // still authorizes and resolves the stable namespaced URI on read.
        read_effects(
            peer,
            "lab://upstream/forge/fixture://forge-status".to_string(),
            deadline,
        )
        .await
        .map(|effects| effects.0)
        .map_err(|error| format!("{error}; discovered URIs: {seen:?}"))
    }

    pub(crate) async fn fixture_settlement_barrier(&self) -> Result<(), String> {
        let peer = self.service.as_ref().expect("active service").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        loop {
            let (_, active) = read_effects(
                peer,
                "lab://upstream/forge/fixture://forge-status".to_string(),
                deadline,
            )
            .await?;
            if active == 0 {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("forge effects did not settle before deadline".into());
            }
            tokio::task::yield_now().await;
        }
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        let mut cleanup = CleanupResult::default();
        if let Some(service) = self.service.take() {
            match tokio::time::timeout(REQUEST_TIMEOUT, service.cancel()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => cleanup
                    .failures
                    .push(format!("MCP client cancellation failed: {error}")),
                Err(_) => cleanup
                    .failures
                    .push("MCP client cancellation timed out".into()),
            }
        }
        if let Some(guard) = self.guard.take() {
            let owned = guard.finish().await;
            cleanup.failures.extend(owned.failures);
        }
        drop(self.owned_root.take());
        cleanup
    }
}

async fn read_effects(
    peer: &rmcp::service::Peer<RoleClient>,
    uri: String,
    deadline: tokio::time::Instant,
) -> Result<(u64, u64), String> {
    let response = tokio::time::timeout_at(
        deadline,
        peer.read_resource(ReadResourceRequestParams::new(uri)),
    )
    .await
    .map_err(|_| "resources/read timed out".to_string())?
    .map_err(|error| error.to_string())?;
    let text = match response.contents.as_slice() {
        [ResourceContents::TextResourceContents { text, .. }] => text,
        _ => return Err("forge status was not one text resource".into()),
    };
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let invocations = value["invocation_count"]
        .as_u64()
        .ok_or("forge status omitted invocation_count")?;
    let active = value["active_count"]
        .as_u64()
        .ok_or("forge status omitted active_count")?;
    Ok((invocations, active))
}

fn result_text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn write_report(
    case_id: &str,
    identity: &RunIdentity,
    limits: Limits,
    execution: &Execution,
    effects_before: u64,
    effects_after: u64,
    cleanup: &CleanupResult,
) -> Result<(), String> {
    let Some(directory) = std::env::var_os("LABBY_Q3_EVIDENCE_DIR") else {
        return Ok(());
    };
    let report = json!({
        "schema_version": 1,
        "lane": "q3_codemode_real_process",
        "case_id": case_id,
        "source": identity,
        "fixture": {"name":"stdio-mcp-fixture", "version":"forge-v1"},
        "seed": identity.seed,
        "transport": "streamable_http_to_labby_to_stdio_fixture",
        "auth_subject": "static-owner",
        "limits": {
            "timeout_ms": limits.timeout_ms,
            "max_response_bytes": limits.max_response_bytes,
            "max_response_tokens": limits.max_response_tokens,
            "max_calls_per_run": limits.max_calls_per_run,
            "upstream_request_timeout_ms": limits.upstream_request_timeout_ms,
            "upstream_max_in_flight": limits.upstream_max_in_flight,
        },
        "observed": {
            "elapsed_ms": execution.elapsed.as_millis(),
            "wire_bytes": execution.wire_bytes,
            "is_error": execution.is_error,
            "effects_before": effects_before,
            "effects_after": effects_after,
            "effect_delta": effects_after.saturating_sub(effects_before),
            "trace": execution.structured,
        },
        "cleanup": {"clean":cleanup.is_clean(), "failures":cleanup.failures},
    });
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{case_id}.json"));
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
}
