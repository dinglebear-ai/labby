//! Detect and dispatch to an already-running `labby serve` daemon's HTTP
//! surface.
//!
//! One-shot `labby gateway <subcommand>` invocations build their own
//! throwaway `GatewayManager` from `config.toml` and exit -- they never talk
//! to an already-running `labby serve` daemon. That's fine for read-only
//! commands, but for mutations (`add`, `update`, `remove`, `reload`, ...) it
//! means the change is durably written to disk but invisible to the live
//! daemon (and therefore to the WebUI/MCP clients it's actually serving)
//! until the service is restarted or sent `SIGUSR1`. The WebUI never hits
//! this gap because it's served *by* the live daemon and shares its
//! `GatewayManager` instance directly.
//!
//! This module closes that gap: CLI commands try the live daemon's HTTP API
//! first (matching what the WebUI does) and only fall back to local
//! dispatch when no daemon is reachable -- which keeps bootstrap workflows
//! (`labby setup --provision`, `labby doctor`, the very first `gateway add`
//! before `labby serve` has ever run) working standalone.
//!
//! Detection isn't loopback-only: it tries, in order, the local bind
//! address (fast path when the CLI runs on the same host/container as the
//! daemon), then the gateway's own configured public URLs
//! (`LAB_MCP_GATEWAY_URL`, `LAB_PUBLIC_URL` -- resolved the same way
//! `LabConfig::public_urls()` already does everywhere else). That means
//! `labby gateway add` reaches the real daemon whether it's run inside the
//! `labby` container or from any other machine that shares
//! `~/.labby/.env` (for `LAB_MCP_HTTP_TOKEN`).

use std::time::Duration;

use serde_json::Value;

use crate::config::LabConfig;
use crate::dispatch::error::ToolError;

/// Timeout for the initial reachability probe. This runs on every CLI
/// invocation, so an unreachable host must fail over to local dispatch
/// quickly rather than hang the command.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
// Deliberately no blanket request timeout on the client: some actions block
// server-side by design (e.g. `gateway.oauth.wait` with a caller-supplied
// `--wait-timeout-secs`, which can legitimately run past two minutes). Only
// the reachability probe gets an explicit short timeout below.

/// A reachable, already-running `labby serve` daemon.
pub(crate) struct LiveGateway {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

/// Candidate base URLs to try, in priority order: the local bind address
/// `labby serve` itself would resolve (identical env-var → config → default
/// order as `cli/serve.rs`: `LAB_MCP_HTTP_HOST`/`LAB_MCP_HTTP_PORT`, then
/// `config.mcp.host`/`.port`, then `127.0.0.1:8765`), followed by the
/// gateway's own configured public URLs. The local candidate is tried first
/// because it's a fast same-host round trip when the CLI happens to be
/// co-located with the daemon; the public URLs are what let the CLI reach
/// the daemon from anywhere else.
fn candidate_base_urls(config: &LabConfig) -> Vec<String> {
    candidate_base_urls_from(
        std::env::var("LAB_MCP_HTTP_HOST").ok(),
        std::env::var("LAB_MCP_HTTP_PORT").ok(),
        config,
    )
}

/// Pure resolution logic, split out from `candidate_base_urls` so it's
/// testable without mutating process-global env vars (which would race with
/// other tests in the same binary).
fn candidate_base_urls_from(
    host_env: Option<String>,
    port_env: Option<String>,
    config: &LabConfig,
) -> Vec<String> {
    let host = host_env
        .or_else(|| config.mcp.host.clone())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port_env
        .and_then(|value| value.parse::<u16>().ok())
        .or(config.mcp.port)
        .unwrap_or(8765);

    let mut candidates = vec![format!("http://{host}:{port}")];
    let public = config.public_urls();
    for url in [public.mcp_gateway, public.app].into_iter().flatten() {
        let trimmed = url.trim_end_matches('/').to_string();
        if !trimmed.is_empty() && !candidates.contains(&trimmed) {
            candidates.push(trimmed);
        }
    }
    candidates
}

/// Probe candidate base URLs in order and return a client for the first
/// reachable one.
///
/// Returns `None` if every candidate fails (daemon not running anywhere
/// reachable, network error, non-2xx `/health` on all of them) -- callers
/// must fall back to local dispatch. A live daemon is a nice-to-have
/// consistency guarantee here, not a hard requirement, so standalone CLI use
/// keeps working.
pub(crate) async fn detect(config: &LabConfig) -> Option<LiveGateway> {
    let client = reqwest::Client::builder().build().ok()?;
    let token = std::env::var("LAB_MCP_HTTP_TOKEN").ok();

    for base_url in candidate_base_urls(config) {
        let Ok(health) = client
            .get(format!("{base_url}/health"))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
        else {
            continue;
        };
        if health.status().is_success() {
            return Some(LiveGateway {
                base_url,
                token,
                client,
            });
        }
    }
    None
}

impl LiveGateway {
    /// Dispatch `action`/`params` through the daemon's generic gateway
    /// action route (`POST /v1/gateway`) -- the same `{action, params}`
    /// shape MCP and the CLI's own local dispatch already use, so this
    /// needs no per-action endpoint mapping.
    pub(crate) async fn dispatch_action(
        &self,
        action: &str,
        params: Value,
    ) -> Result<Value, ToolError> {
        let mut request = self
            .client
            .post(format!("{}/v1/gateway", self.base_url))
            .json(&serde_json::json!({ "action": action, "params": params }));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.map_err(|e| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("request to live gateway daemon failed: {e}"),
        })?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);

        if status.is_success() {
            return Ok(body);
        }

        let sdk_kind = body
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("internal_error")
            .to_string();
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("live gateway daemon returned HTTP {status}"));
        Err(ToolError::Sdk { sdk_kind, message })
    }

    /// Execute a Code Mode snippet against the live daemon's actual `codemode`
    /// MCP tool over its already-warm upstream connection pool, instead of
    /// the CLI's own throwaway (lazily-seeded, cold) connections.
    ///
    /// `gateway.add`/`update`/etc. all had one generic `{action, params}`
    /// route to reuse; Code Mode execution doesn't -- it's an MCP tool call,
    /// not a gateway action -- so this speaks the MCP streamable-HTTP
    /// protocol directly, the same way `labby-gateway`'s own upstream pool
    /// connects to any other MCP server (see `pool/connect.rs`).
    pub(crate) async fn call_codemode_tool(&self, code: &str) -> anyhow::Result<Value> {
        use rmcp::ServiceExt;
        use rmcp::model::CallToolRequestParams;
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
        };

        let mut transport_config =
            StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", self.base_url));
        transport_config.auth_header = self.token.clone();
        let worker = StreamableHttpClientWorker::new(self.client.clone(), transport_config);
        let service = ().serve(worker).await?;
        let peer = service.peer().clone();

        let mut arguments = serde_json::Map::new();
        arguments.insert("code".to_string(), Value::String(code.to_string()));
        let result = peer
            .call_tool(CallToolRequestParams::new("codemode").with_arguments(arguments))
            .await?;
        service.cancel().await.ok();

        if let Some(structured) = result.structured_content {
            return Ok(structured);
        }
        let text = result
            .content
            .iter()
            .find_map(|block| block.as_text().map(|t| t.text.clone()))
            .unwrap_or_default();
        Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // See google.rs::GoogleProvider::new for why this call is needed under
    // "rustls-no-provider" -- idempotent, safe to call repeatedly.
    fn ensure_tls_provider() {
        drop(rustls::crypto::ring::default_provider().install_default());
    }

    fn test_gateway(base_url: String, token: Option<String>) -> LiveGateway {
        ensure_tls_provider();
        LiveGateway {
            base_url,
            token,
            client: reqwest::Client::new(),
        }
    }

    #[test]
    fn local_candidate_prefers_env_over_config_over_default() {
        let mut config = LabConfig::default();
        config.mcp.host = Some("configured.example".to_string());
        config.mcp.port = Some(1234);

        assert_eq!(
            candidate_base_urls_from(None, None, &LabConfig::default()),
            vec!["http://127.0.0.1:8765".to_string()]
        );
        assert_eq!(
            candidate_base_urls_from(None, None, &config),
            vec!["http://configured.example:1234".to_string()]
        );
        assert_eq!(
            candidate_base_urls_from(
                Some("env.example".to_string()),
                Some("9999".to_string()),
                &config
            ),
            vec!["http://env.example:9999".to_string()]
        );
    }

    #[test]
    fn candidates_fall_through_to_configured_public_urls() {
        let mut config = LabConfig::default();
        config.public_urls = Some(crate::config::PublicUrlsConfig {
            app: Some("https://labby.example.com/".to_string()),
            mcp_gateway: Some("https://mcp.example.com".to_string()),
        });

        // Local bind address first (fast path), then the dedicated gateway
        // URL, then the general app URL -- and a trailing slash is trimmed
        // so it composes cleanly with `/health` and `/v1/gateway`.
        assert_eq!(
            candidate_base_urls_from(None, None, &config),
            vec![
                "http://127.0.0.1:8765".to_string(),
                "https://mcp.example.com".to_string(),
                "https://labby.example.com".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn dispatch_action_returns_success_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(header("authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })),
            )
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), Some("test-token".to_string()));
        let result = gateway
            .dispatch_action("gateway.list", serde_json::json!({}))
            .await
            .expect("dispatch should succeed");
        assert_eq!(result, serde_json::json!({ "ok": true }));
    }

    #[tokio::test]
    async fn dispatch_action_maps_error_envelope_to_tool_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "kind": "missing_param",
                "message": "upstream is required",
            })))
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), None);
        let error = gateway
            .dispatch_action("gateway.add", serde_json::json!({}))
            .await
            .expect_err("dispatch should fail");
        assert_eq!(error.kind(), "missing_param");
        assert_eq!(error.user_message(), "upstream is required");
    }

    #[tokio::test]
    async fn detect_returns_none_when_unreachable() {
        // Port 0 never accepts a connection, so this exercises the "not
        // running" fallback path without depending on anything actually
        // listening (or not) on a fixed port.
        ensure_tls_provider();
        let mut config = LabConfig::default();
        config.mcp.host = Some("127.0.0.1".to_string());
        config.mcp.port = Some(0);

        assert!(detect(&config).await.is_none());
    }

    #[tokio::test]
    async fn detect_returns_some_when_health_check_succeeds() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let url = url::Url::parse(&server.uri()).expect("wiremock uri parses");
        let mut config = LabConfig::default();
        config.mcp.host = Some(url.host_str().expect("wiremock host").to_string());
        config.mcp.port = url.port();

        assert!(detect(&config).await.is_some());
    }

    #[tokio::test]
    async fn detect_falls_through_to_a_public_url_when_local_is_unreachable() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        // Local bind address (port 0) never accepts a connection; only the
        // configured public URL (standing in for the wiremock server) is
        // actually reachable, matching a CLI invocation on a different
        // machine than the daemon.
        let mut config = LabConfig::default();
        config.mcp.host = Some("127.0.0.1".to_string());
        config.mcp.port = Some(0);
        config.public_urls = Some(crate::config::PublicUrlsConfig {
            app: Some(server.uri()),
            mcp_gateway: None,
        });

        let live = detect(&config)
            .await
            .expect("should fall through to public url");
        assert_eq!(live.base_url, server.uri());
    }
}
