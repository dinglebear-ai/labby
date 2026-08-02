//! Detect and connect to an already-running `labby serve` daemon.
//!
//! `labby` has three surfaces that can each run as their own process: the CLI
//! (one-shot commands), the MCP stdio transport, and the HTTP daemon. Only
//! the HTTP daemon is meant to be the canonical, long-running gateway --
//! everything else should be a thin client to it whenever one is reachable,
//! rather than spinning up its own independent `GatewayManager` with its own
//! config view, upstream connections, and OAuth state. The WebUI never hits
//! this problem because it's served *by* the live daemon and shares its
//! manager directly; every other surface has to detect the daemon for
//! itself, which is what this module does.
//!
//! Detection isn't loopback-only: it tries, in order, the local bind address
//! (fast path when co-located with the daemon), then the gateway's own
//! configured public URLs (`LABBY_MCP_GATEWAY_URL`, `LABBY_PUBLIC_URL` --
//! resolved the same way `LabConfig::public_urls()` already does everywhere
//! else). That means a thin client reaches the real daemon whether it runs
//! inside the same container/host as `labby serve` or from any other machine
//! that shares `~/.labby/.env` (for `LABBY_MCP_HTTP_TOKEN`).

use std::time::Duration;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::Value;

use crate::config::LabConfig;
use crate::dispatch::error::ToolError;

/// Timeout for the initial reachability probe. This runs on every thin-client
/// startup, so an unreachable host must fail over quickly rather than hang.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);
// Deliberately no blanket request timeout on the client: some actions block
// server-side by design (e.g. `gateway.oauth.wait` with a caller-supplied
// `--wait-timeout-secs`, which can legitimately run past two minutes). Only
// the reachability probe gets an explicit short timeout below.

/// A reachable, already-running `labby serve` daemon.
#[derive(Clone)]
pub struct LiveGateway {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

/// Candidate base URLs to try, in priority order: the local bind address
/// `labby serve` itself would resolve (identical env-var → config → default
/// order as `cli/serve.rs`: `LABBY_MCP_HTTP_HOST`/`LABBY_MCP_HTTP_PORT`, then
/// `config.mcp.host`/`.port`, then `127.0.0.1:8765`), followed by the
/// gateway's own configured public URLs. The local candidate is tried first
/// because it's a fast same-host round trip when co-located with the daemon;
/// the public URLs are what let a thin client reach the daemon from anywhere
/// else.
fn candidate_base_urls(config: &LabConfig) -> Vec<String> {
    candidate_base_urls_from(
        std::env::var("LABBY_MCP_HTTP_HOST").ok(),
        std::env::var("LABBY_MCP_HTTP_PORT").ok(),
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
/// must fall back to running standalone. A live daemon is a nice-to-have
/// consistency guarantee here, not a hard requirement, so standalone use
/// (bootstrap, `labby doctor`, the very first `gateway add`) keeps working.
pub async fn detect(config: &LabConfig) -> Option<LiveGateway> {
    let client = reqwest::Client::builder().build().ok()?;
    let token = std::env::var("LABBY_MCP_HTTP_TOKEN").ok();

    for base_url in candidate_base_urls(config) {
        let Ok(health) = client
            .get(format!("{base_url}/health"))
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
        else {
            continue;
        };
        if health.status().is_success()
            && is_labby_gateway_daemon(&client, &base_url, token.as_deref()).await
        {
            return Some(LiveGateway {
                base_url,
                token,
                client,
            });
        }
    }
    None
}

async fn is_labby_gateway_daemon(
    client: &reqwest::Client,
    base_url: &str,
    token: Option<&str>,
) -> bool {
    if token.is_none() && labby_discovery_identifies_daemon(client, base_url).await {
        return true;
    }

    let mut request = client
        .get(format!("{base_url}/v1/gateway/actions"))
        .timeout(PROBE_TIMEOUT);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let Ok(response) = request.send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(actions) = response.json::<Value>().await else {
        return false;
    };
    actions_include_gateway_reload(&actions)
}

async fn labby_discovery_identifies_daemon(client: &reqwest::Client, base_url: &str) -> bool {
    let Ok(response) = client
        .get(format!("{base_url}/.well-known/labby.json"))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(discovery) = response.json::<Value>().await else {
        return false;
    };
    discovery
        .get("paletteCatalogUrl")
        .and_then(Value::as_str)
        .is_some()
        && discovery
            .get("paletteExecuteUrl")
            .and_then(Value::as_str)
            .is_some()
}

fn actions_include_gateway_reload(actions: &Value) -> bool {
    actions.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "gateway.reload")
        })
    })
}

impl LiveGateway {
    pub async fn verify_resource_lease_actions(&self) -> Result<(), ToolError> {
        const REQUIRED: [&str; 3] = [
            "gateway.oauth.resource_lease.create",
            "gateway.oauth.resource_lease.renew",
            "gateway.oauth.resource_lease.release",
        ];
        for action in REQUIRED {
            if !self.supports_action(action).await? {
                return Err(ToolError::Sdk {
                    sdk_kind: "proxy_auth_unavailable".to_string(),
                    message: format!(
                        "live Labby daemon does not support required action `{action}`"
                    ),
                });
            }
        }
        Ok(())
    }

    pub async fn verify_oauth_issuer(
        &self,
        issuer: &url::Url,
    ) -> Result<labby_auth::jwt::JwksDocument, ToolError> {
        let stable_issuer = issuer.as_str().trim_end_matches('/');
        let metadata_url = format!("{stable_issuer}/.well-known/oauth-authorization-server");
        let response = self
            .client
            .get(&metadata_url)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!("OAuth authorization-server metadata is unreachable: {error}"),
            })?;
        if !response.status().is_success() {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!(
                    "OAuth authorization-server metadata returned HTTP {}",
                    response.status()
                ),
            });
        }
        let metadata: Value = response.json().await.map_err(|error| ToolError::Sdk {
            sdk_kind: "proxy_auth_unavailable".to_string(),
            message: format!("OAuth authorization-server metadata is invalid: {error}"),
        })?;
        if metadata.get("issuer").and_then(Value::as_str) != Some(stable_issuer) {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message:
                    "OAuth metadata issuer does not exactly match the configured stable issuer"
                        .to_string(),
            });
        }
        let jwks_uri = metadata
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: "OAuth metadata does not advertise a JWKS URI".to_string(),
            })?;
        let response = self
            .client
            .get(jwks_uri)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!("OAuth JWKS is unreachable: {error}"),
            })?;
        if !response.status().is_success() {
            return Err(ToolError::Sdk {
                sdk_kind: "proxy_auth_unavailable".to_string(),
                message: format!("OAuth JWKS returned HTTP {}", response.status()),
            });
        }
        response.json().await.map_err(|error| ToolError::Sdk {
            sdk_kind: "proxy_auth_unavailable".to_string(),
            message: format!("OAuth JWKS is invalid: {error}"),
        })
    }

    pub async fn supports_action(&self, action: &str) -> Result<bool, ToolError> {
        let mut request = self
            .client
            .get(format!("{}/v1/gateway/actions", self.base_url));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(live_gateway_network_error)?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(tool_error_from_response(status, &body));
        }
        Ok(body.as_array().is_some_and(|actions| {
            actions
                .iter()
                .any(|candidate| candidate.get("name").and_then(Value::as_str) == Some(action))
        }))
    }

    pub async fn create_resource_lease(
        &self,
        resource: &str,
        scopes: Vec<String>,
        ttl: Duration,
        owner: &str,
    ) -> Result<labby_auth::resource_registry::ResourceLease, ToolError> {
        let value = self
            .dispatch_action(
                "gateway.oauth.resource_lease.create",
                serde_json::json!({
                    "resource": resource,
                    "scopes": scopes,
                    "ttl_secs": ttl.as_secs(),
                    "owner": owner,
                }),
            )
            .await?;
        serde_json::from_value(value).map_err(typed_response_error)
    }

    pub async fn renew_resource_lease(
        &self,
        id: &str,
        ttl: Duration,
    ) -> Result<labby_auth::resource_registry::ResourceLease, ToolError> {
        let value = self
            .dispatch_action(
                "gateway.oauth.resource_lease.renew",
                serde_json::json!({"id": id, "ttl_secs": ttl.as_secs()}),
            )
            .await?;
        serde_json::from_value(value).map_err(typed_response_error)
    }

    pub async fn release_resource_lease(
        &self,
        id: &str,
    ) -> Result<labby_gateway::gateway::types::ResourceLeaseReleaseView, ToolError> {
        let value = self
            .dispatch_action(
                "gateway.oauth.resource_lease.release",
                serde_json::json!({"id": id}),
            )
            .await?;
        serde_json::from_value(value).map_err(typed_response_error)
    }

    /// Dispatch `action`/`params` through the daemon's generic gateway
    /// action route (`POST /v1/gateway`) -- the same `{action, params}`
    /// shape MCP and the CLI's own local dispatch already use, so this
    /// needs no per-action endpoint mapping.
    pub async fn dispatch_action(&self, action: &str, params: Value) -> Result<Value, ToolError> {
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
    /// MCP tool over its already-warm upstream connection pool, instead of a
    /// throwaway caller's own cold connections.
    ///
    /// The generic `{action, params}` route above doesn't apply here -- Code
    /// Mode execution is an MCP tool call, not a gateway action -- so this
    /// speaks the MCP streamable-HTTP protocol directly via a short-lived
    /// connection, the same way `labby-gateway`'s own upstream pool connects
    /// to any other MCP server (see `pool/connect.rs`).
    pub async fn call_codemode_tool(&self, code: &str) -> anyhow::Result<Value> {
        use rmcp::model::CallToolRequestParams;

        let service = self.connect_service(()).await?;
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

    /// Open a long-lived MCP streamable-HTTP connection to the daemon's
    /// `/mcp` endpoint and return the running client service. Callers own the
    /// resulting `Peer<RoleClient>` for as long as they need it (e.g. the
    /// stdio bridge holds one for its entire process lifetime, versus
    /// `call_codemode_tool` above which opens one per call).
    ///
    /// Generic over the `ClientHandler` so callers that need the daemon's
    /// server->client requests (elicitation/sampling/roots) answered --
    /// rather than declined, which is what the unit handler `()` does --
    /// can pass one that forwards them somewhere (see
    /// `crate::mcp::bridge::BridgeClientHandler`).
    pub async fn connect_service<H: rmcp::ClientHandler>(
        &self,
        handler: H,
    ) -> anyhow::Result<RunningService<RoleClient, H>> {
        use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
        };

        let mut transport_config =
            StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", self.base_url));
        transport_config.auth_header = self.token.clone();
        let worker = StreamableHttpClientWorker::new(self.client.clone(), transport_config);
        Ok(handler
            .serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            )
            .await?)
    }
}

fn live_gateway_network_error(error: reqwest::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "network_error".to_string(),
        message: format!("request to live gateway daemon failed: {error}"),
    }
}

fn typed_response_error(error: serde_json::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("invalid typed response from live gateway daemon: {error}"),
    }
}

fn tool_error_from_response(status: reqwest::StatusCode, body: &Value) -> ToolError {
    ToolError::Sdk {
        sdk_kind: body
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("internal_error")
            .to_string(),
        message: body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("live gateway daemon returned HTTP {status}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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
    async fn detect_returns_some_when_health_check_and_gateway_actions_succeed() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "name": "gateway.reload" }])),
            )
            .mount(&server)
            .await;

        let url = url::Url::parse(&server.uri()).expect("wiremock uri parses");
        let mut config = LabConfig::default();
        config.mcp.host = Some(url.host_str().expect("wiremock host").to_string());
        config.mcp.port = url.port();

        assert!(detect(&config).await.is_some());
    }

    #[tokio::test]
    async fn detect_accepts_labby_discovery_when_no_static_token_is_configured() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "apiBaseUrl": server.uri(),
                "paletteCatalogUrl": format!("{}/v1/palette/catalog", server.uri()),
                "paletteExecuteUrl": format!("{}/v1/palette/execute", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(is_labby_gateway_daemon(&client, &server.uri(), None).await);
    }

    #[tokio::test]
    async fn detect_rejects_discovery_when_static_token_fails_gateway_actions_probe() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/labby.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "apiBaseUrl": server.uri(),
                "paletteCatalogUrl": format!("{}/v1/palette/catalog", server.uri()),
                "paletteExecuteUrl": format!("{}/v1/palette/execute", server.uri()),
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .and(header("authorization", "Bearer wrong-token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        assert!(!is_labby_gateway_daemon(&client, &server.uri(), Some("wrong-token")).await);
    }

    #[tokio::test]
    async fn detect_ignores_healthy_non_labby_server() {
        ensure_tls_provider();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": "not_found"
            })))
            .mount(&server)
            .await;

        let url = url::Url::parse(&server.uri()).expect("wiremock uri parses");
        let mut config = LabConfig::default();
        config.mcp.host = Some(url.host_str().expect("wiremock host").to_string());
        config.mcp.port = url.port();

        assert!(detect(&config).await.is_none());
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
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "name": "gateway.reload" }])),
            )
            .mount(&server)
            .await;

        // Local bind address (port 0) never accepts a connection; only the
        // configured public URL (standing in for the wiremock server) is
        // actually reachable, matching a caller running on a different
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

    #[tokio::test]
    async fn typed_resource_lease_methods_use_generic_gateway_actions() {
        let server = MockServer::start().await;
        let lease = serde_json::json!({
            "id": "opaque-lease-id",
            "resource": "https://proxy.example:53147/mcp",
            "scopes": ["mcp:read", "mcp:write"],
            "expires_at_unix": 4_000_000_000_u64
        });
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.oauth.resource_lease.create",
                "params": {
                    "resource": "https://proxy.example:53147/mcp",
                    "scopes": ["mcp:read", "mcp:write"],
                    "ttl_secs": 120,
                    "owner": "proxy-test"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&lease))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.oauth.resource_lease.renew",
                "params": {"id": "opaque-lease-id", "ttl_secs": 240}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&lease))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_json(json!({
                "action": "gateway.oauth.resource_lease.release",
                "params": {"id": "opaque-lease-id"}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released": true})))
            .mount(&server)
            .await;

        let gateway = test_gateway(server.uri(), None);
        let created = gateway
            .create_resource_lease(
                "https://proxy.example:53147/mcp",
                vec!["mcp:read".to_string(), "mcp:write".to_string()],
                Duration::from_mins(2),
                "proxy-test",
            )
            .await
            .unwrap();
        assert_eq!(created.id, "opaque-lease-id");
        gateway
            .renew_resource_lease(&created.id, Duration::from_mins(4))
            .await
            .unwrap();
        gateway.release_resource_lease(&created.id).await.unwrap();
    }

    #[tokio::test]
    async fn resource_lease_action_support_detection_reads_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"name": "gateway.reload"},
                {"name": "gateway.oauth.resource_lease.create"}
            ])))
            .mount(&server)
            .await;
        let gateway = test_gateway(server.uri(), None);
        assert!(
            gateway
                .supports_action("gateway.oauth.resource_lease.create")
                .await
                .unwrap()
        );
        assert!(
            !gateway
                .supports_action("gateway.oauth.resource_lease.release")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn oauth_proxy_prerequisites_require_all_lease_actions() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/gateway/actions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"name": "gateway.oauth.resource_lease.create"},
                {"name": "gateway.oauth.resource_lease.renew"}
            ])))
            .mount(&server)
            .await;
        let error = test_gateway(server.uri(), None)
            .verify_resource_lease_actions()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("release"));
    }

    #[tokio::test]
    async fn oauth_proxy_prerequisites_verify_exact_issuer_metadata_and_jwks() {
        let server = MockServer::start().await;
        let issuer = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "issuer": issuer,
                "jwks_uri": format!("{}/jwks", server.uri())
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys": []})))
            .mount(&server)
            .await;

        let jwks = test_gateway(server.uri(), None)
            .verify_oauth_issuer(&url::Url::parse(&server.uri()).unwrap())
            .await
            .unwrap();
        assert!(jwks.keys.is_empty());
    }

    #[tokio::test]
    async fn oauth_proxy_prerequisites_reject_unreachable_metadata() {
        let server = MockServer::start().await;
        let error = test_gateway(server.uri(), None)
            .verify_oauth_issuer(&url::Url::parse(&server.uri()).unwrap())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("metadata"));
    }

    #[tokio::test]
    async fn oauth_lease_guard_renews_and_releases_without_exposing_id() {
        use crate::proxy::oauth::{OAuthLeaseGuard, OAuthLeaseTiming};

        let server = MockServer::start().await;
        let lease = json!({
            "id": "lease-secret-id",
            "resource": "https://proxy.example:53147/mcp",
            "scopes": ["mcp:read"],
            "expires_at_unix": 4_000_000_000_u64
        });
        for (action, response) in [
            ("gateway.oauth.resource_lease.create", lease.clone()),
            ("gateway.oauth.resource_lease.renew", lease.clone()),
            (
                "gateway.oauth.resource_lease.release",
                json!({"released": true}),
            ),
        ] {
            Mock::given(method("POST"))
                .and(path("/v1/gateway"))
                .and(wiremock::matchers::body_partial_json(
                    json!({"action": action}),
                ))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .mount(&server)
                .await;
        }
        let mut guard = OAuthLeaseGuard::create(
            test_gateway(server.uri(), None),
            "https://proxy.example:53147/mcp",
            vec!["mcp:read".to_string()],
            "owner-fingerprint",
            OAuthLeaseTiming {
                ttl: Duration::from_millis(90),
                renew_interval: Duration::from_millis(20),
                jitter_max: Duration::ZERO,
            },
        )
        .await
        .unwrap();
        assert!(!format!("{guard:?}").contains("lease-secret-id"));
        tokio::time::sleep(Duration::from_millis(35)).await;
        guard.release().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let bodies = requests
            .iter()
            .filter_map(|request| serde_json::from_slice::<Value>(&request.body).ok())
            .collect::<Vec<_>>();
        assert!(
            bodies
                .iter()
                .any(|body| body["action"] == "gateway.oauth.resource_lease.renew")
        );
        assert!(
            bodies
                .iter()
                .any(|body| body["action"] == "gateway.oauth.resource_lease.release")
        );
    }

    #[tokio::test]
    async fn oauth_lease_guard_propagates_renewal_failure_and_still_releases() {
        use crate::proxy::oauth::{OAuthLeaseGuard, OAuthLeaseTiming};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.create"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "lease-secret-id",
                "resource": "https://proxy.example:53147/mcp",
                "scopes": ["mcp:read"],
                "expires_at_unix": 4_000_000_000_u64
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.renew"
            })))
            .respond_with(ResponseTemplate::new(503).set_body_json(json!({
                "kind": "daemon_unavailable", "message": "renew failed"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(wiremock::matchers::body_partial_json(json!({
                "action": "gateway.oauth.resource_lease.release"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"released": true})))
            .mount(&server)
            .await;

        let mut guard = OAuthLeaseGuard::create(
            test_gateway(server.uri(), None),
            "https://proxy.example:53147/mcp",
            vec!["mcp:read".to_string()],
            "owner-fingerprint",
            OAuthLeaseTiming {
                ttl: Duration::from_millis(90),
                renew_interval: Duration::from_millis(10),
                jitter_max: Duration::ZERO,
            },
        )
        .await
        .unwrap();
        let error = guard.wait_for_failure().await.unwrap_err();
        assert!(error.to_string().contains("renewal failed"));
        guard.release().await.unwrap();
    }
}
