use std::collections::BTreeSet;
use std::time::Duration;

use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use rmcp::model::{CallToolRequestParams, CallToolResult, PaginatedRequestParams};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};

use crate::support::{CleanupResult, LiveLabbyBuilder, LiveLabbyGuard};

pub(crate) const MAX_PAGES: usize = 8;
pub(crate) const MAX_TOOLS: usize = 64;
pub(crate) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_CONCURRENCY: usize = 4;
pub(crate) const MAX_OUTSTANDING: usize = 8;
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_TOKEN: &str = "live-mcp-action-matrix-token";
const _: () = assert!(MAX_CONCURRENCY > 0 && MAX_CONCURRENCY <= 4);
const _: () = assert!(MAX_OUTSTANDING >= MAX_CONCURRENCY && MAX_OUTSTANDING <= 8);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdentityTuple {
    pub(crate) issuer: String,
    pub(crate) subject: String,
    pub(crate) project: String,
    pub(crate) loadout: String,
    pub(crate) route: String,
    pub(crate) scopes: Vec<String>,
}
impl IdentityTuple {
    pub(crate) fn local_admin() -> Self {
        Self {
            issuer: "labby-static-bearer".into(),
            subject: "local-action-matrix".into(),
            project: "disposable".into(),
            loadout: "root".into(),
            route: "/mcp".into(),
            scopes: vec!["lab:read".into(), "lab:admin".into()],
        }
    }

    pub(crate) fn from_public(identity: &crate::live_identity::PublicIdentity) -> Self {
        Self {
            issuer: identity.issuer.clone(),
            subject: identity.subject.clone(),
            project: identity.project_id.clone(),
            loadout: identity.loadout_id.clone(),
            route: identity.route_id.clone(),
            scopes: identity.scopes.clone(),
        }
    }

    pub(crate) fn fingerprint(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let material = format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            self.issuer,
            self.subject,
            self.project,
            self.loadout,
            self.route,
            self.scopes.join(",")
        );
        hex::encode(Sha256::digest(material.as_bytes()))
    }
}

pub(crate) struct BuiltinMcpRunner {
    guard: Option<LiveLabbyGuard>,
    service: Option<RunningService<rmcp::RoleClient, ()>>,
    identity: IdentityTuple,
    concurrency: tokio::sync::Semaphore,
    outstanding: tokio::sync::Semaphore,
}

fn capped_http_client() -> BodyCappedHttpClient {
    // Each integration-test binary is a separate process and this constructor
    // is also exercised directly by unit tests before a runner is started.
    // Install the same provider used by the product before reqwest constructs
    // its rustls client so parallel nextest execution cannot observe an
    // uninitialized process-global provider.
    drop(rustls::crypto::ring::default_provider().install_default());
    BodyCappedHttpClient::new(reqwest::Client::new(), MAX_RESPONSE_BYTES)
}

impl BuiltinMcpRunner {
    pub(crate) async fn start() -> Result<Self, String> {
        Self::start_with_config(None).await
    }

    pub(crate) async fn start_code_mode() -> Result<Self, String> {
        Self::start_with_config(Some("[code_mode]\nenabled = true\n")).await
    }

    async fn start_with_config(config_text: Option<&str>) -> Result<Self, String> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let mut builder = LiveLabbyBuilder::new().env("LABBY_MCP_HTTP_TOKEN", TEST_TOKEN);
        if let Some(config_text) = config_text {
            builder = builder.config(config_text);
        }
        let guard = builder.start().await?;
        let endpoint = format!("{}/mcp", guard.connection().base_url);
        let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
        config.auth_header = Some(TEST_TOKEN.to_string());
        let worker = StreamableHttpClientWorker::new(capped_http_client(), config);
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            ().serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "MCP initialize timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Self {
            guard: Some(guard),
            service: Some(service),
            identity: IdentityTuple::local_admin(),
            concurrency: tokio::sync::Semaphore::new(MAX_CONCURRENCY),
            outstanding: tokio::sync::Semaphore::new(MAX_OUTSTANDING),
        })
    }

    pub(crate) async fn connect_project(
        base: &str,
        credential: &str,
        identity: IdentityTuple,
    ) -> Result<Self, String> {
        let local = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
        let local_host = local.host_str().ok_or("project MCP base has no host")?;
        let local_port = local
            .port_or_known_default()
            .ok_or("project MCP base has no port")?;
        let local_address = format!("{local_host}:{local_port}")
            .parse()
            .map_err(|error: std::net::AddrParseError| error.to_string())?;
        // Keep the public virtual-host authority while connecting the disposable
        // listener port through reqwest's resolver.
        let endpoint = format!("http://mcp.example.test:{local_port}/operator");
        let http_client = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .resolve("mcp.example.test", local_address)
            .build()
            .map_err(|error| error.to_string())?;
        let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
        config.auth_header = Some(credential.to_string());
        let worker = StreamableHttpClientWorker::new(
            BodyCappedHttpClient::new(http_client, MAX_RESPONSE_BYTES),
            config,
        );
        let service = tokio::time::timeout(
            REQUEST_TIMEOUT,
            ().serve_with_lifecycle(
                worker,
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                },
            ),
        )
        .await
        .map_err(|_| "project MCP initialize timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Self {
            guard: None,
            service: Some(service),
            identity,
            concurrency: tokio::sync::Semaphore::new(MAX_CONCURRENCY),
            outstanding: tokio::sync::Semaphore::new(MAX_OUTSTANDING),
        })
    }

    pub(crate) fn identity_fingerprint(&self) -> String {
        self.identity.fingerprint()
    }

    pub(crate) async fn list_tool_names(&self) -> Result<BTreeSet<String>, String> {
        let peer = self.service.as_ref().expect("runner active").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let mut cursor = None;
        let mut tools = BTreeSet::new();
        for _ in 0..MAX_PAGES {
            let params = cursor
                .take()
                .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let page = tokio::time::timeout_at(deadline, peer.list_tools(params))
                .await
                .map_err(|_| "tools/list timed out".to_string())?
                .map_err(|error| error.to_string())?;
            for tool in page.tools {
                if !tools.insert(tool.name.into_owned()) {
                    return Err("tools/list returned a duplicate tool".into());
                }
                if tools.len() > MAX_TOOLS {
                    return Err("tools/list exceeded the item bound".into());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err("tools/list exceeded the page bound".into())
    }

    pub(crate) async fn tool_contract(&self, expected: &str) -> Result<Option<String>, String> {
        let peer = self.service.as_ref().expect("runner active").peer();
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let mut cursor = None;
        let mut count = 0usize;
        for _ in 0..MAX_PAGES {
            let params = cursor
                .take()
                .map(|cursor| PaginatedRequestParams::default().with_cursor(Some(cursor)));
            let page = tokio::time::timeout_at(deadline, peer.list_tools(params))
                .await
                .map_err(|_| "tools/list timed out".to_string())?
                .map_err(|error| error.to_string())?;
            for tool in page.tools {
                count += 1;
                if count > MAX_TOOLS {
                    return Err("tools/list exceeded the item bound".into());
                }
                if tool.name.as_ref() == expected {
                    return serde_json::to_string(&tool)
                        .map(Some)
                        .map_err(|error| error.to_string());
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(None);
            }
        }
        Err("tools/list exceeded the page bound".into())
    }

    pub(crate) async fn call(
        &self,
        service: &str,
        action: &str,
        params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, String> {
        let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
        let _outstanding = self
            .outstanding
            .try_acquire()
            .map_err(|_| "MCP action runner outstanding request bound exceeded".to_string())?;
        let _permit = tokio::time::timeout_at(deadline, self.concurrency.acquire())
            .await
            .map_err(|_| "tools/call timed out while queued".to_string())?
            .map_err(|_| "MCP action runner is shutting down".to_string())?;
        let arguments = serde_json::json!({"action": action, "params": params})
            .as_object()
            .expect("object")
            .clone();
        let request = CallToolRequestParams::new(service.to_string()).with_arguments(arguments);
        let result = tokio::time::timeout_at(
            deadline,
            self.service
                .as_ref()
                .expect("runner active")
                .peer()
                .call_tool(request),
        )
        .await
        .map_err(|_| "tools/call timed out".to_string())?
        .map_err(|error| error.to_string())?;
        Ok(result)
    }

    pub(crate) async fn finish(mut self) -> CleanupResult {
        if let Some(service) = self.service.take() {
            drop(tokio::time::timeout(REQUEST_TIMEOUT, service.cancel()).await);
        }
        self.guard.take().expect("runner active").finish().await
    }

    pub(crate) async fn disconnect(mut self) {
        if let Some(service) = self.service.take() {
            drop(tokio::time::timeout(REQUEST_TIMEOUT, service.cancel()).await);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_body_cap_matches_runner_contract() {
        assert_eq!(capped_http_client().max_bytes(), MAX_RESPONSE_BYTES);
    }

    #[tokio::test]
    async fn outstanding_admission_is_bounded_without_an_unbounded_queue() {
        let outstanding = tokio::sync::Semaphore::new(MAX_OUTSTANDING);
        let held = (0..MAX_OUTSTANDING)
            .map(|_| outstanding.try_acquire())
            .collect::<Result<Vec<_>, _>>()
            .expect("configured outstanding slots");
        assert!(outstanding.try_acquire().is_err());
        drop(held);
        assert!(outstanding.try_acquire().is_ok());
    }

    #[tokio::test]
    async fn queue_wait_consumes_the_absolute_request_deadline() {
        let concurrency = tokio::sync::Semaphore::new(1);
        let held = concurrency.acquire().await.expect("initial permit");
        let deadline = tokio::time::Instant::now() + Duration::from_millis(20);
        let queued = tokio::time::timeout_at(deadline, concurrency.acquire()).await;
        assert!(queued.is_err());
        drop(held);
    }
}
