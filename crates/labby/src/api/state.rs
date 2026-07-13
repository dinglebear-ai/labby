//! Shared application state for axum handlers.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::catalog::{Catalog, build_catalog};
use crate::config::LabConfig;
use crate::dispatch::clients::ServiceClients;
use crate::registry::{ToolRegistry, build_default_registry};

const DEFAULT_PROTECTED_MCP_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Application state passed to every axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    /// Pre-built service+action catalog for discovery endpoints.
    pub catalog: Arc<Catalog>,
    /// Tool registry with dispatch functions for each service.
    ///
    /// Used by `build_router_with_bearer` to enforce runtime service filtering:
    /// only services present in the registry get their HTTP routes mounted,
    /// even when their compile-time feature flag is enabled.
    pub registry: Arc<ToolRegistry>,
    /// Pre-built service clients for connection pool reuse.
    pub clients: Arc<ServiceClients>,
    /// Shared HTTP client for protected MCP reverse proxy requests.
    pub protected_mcp_http_client: reqwest::Client,
    /// Shared public OAuth callback relay forwarder.
    pub public_relay_forwarder: Arc<crate::oauth::public_relay::PublicRelayForwarder>,
    /// Live public OAuth callback relay registry manager.
    ///
    /// `None` means the public relay is not enabled for this process.
    pub public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
    /// Router containing protected route scoped MCP services, mounted by
    /// host/path after protected route auth.
    pub protected_mcp_router: Option<Arc<axum::Router>>,
    /// Runtime-enabled service names derived from the registry.
    ///
    /// The HTTP router checks this set to decide which per-service route groups
    /// to mount.  When `--services` filtering is applied, only the listed names
    /// appear here, so filtered-out services have no reachable POST endpoint.
    #[allow(dead_code)]
    pub enabled_services: Arc<HashSet<String>>,
    /// Resolved auth configuration, if present.
    ///
    /// Stored in `AppState` so that handlers (e.g. protected resource metadata,
    /// WWW-Authenticate headers) can read from resolved config rather than
    /// re-reading env vars at request time.
    pub auth_config: Option<Arc<labby_auth::config::AuthConfig>>,
    /// Resolved lab configuration loaded at server startup.
    pub config: Arc<LabConfig>,
    /// OAuth-mode auth server state, mounted only when LABBY_AUTH_MODE=oauth.
    pub oauth_state: Option<Arc<labby_auth::state::AuthState>>,
    /// Cached actor-key deriver used at authenticated bind boundaries.
    pub actor_key_deriver: Option<Arc<crate::observability::activity::ActorKeyDeriver>>,
    /// Shared gateway manager for runtime upstream pool access and config mutation.
    ///
    /// `None` when gateway management is not wired for this process.
    #[cfg(feature = "gateway")]
    pub gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
    /// Optional directory containing exported Labby web assets.
    pub web_assets_dir: Option<Arc<PathBuf>>,
    /// Whether to serve Labby assets embedded into the lab binary.
    pub embedded_web_assets: bool,
    /// Instant at which the server became ready (used by `/health` uptime_s).
    pub server_start: std::time::Instant,
    /// Canonical absolute path of the configured workspace root, or
    /// `None` when `workspace.root` is invalid at startup.
    /// Backs the `dispatch/fs/` service (workspace filesystem browser).
    #[allow(dead_code)] // Used by fs HTTP routes when that surface is mounted.
    pub workspace_root: Option<Arc<PathBuf>>,
    /// When true, `/v1/*` skips auth middleware for hosted UI requests.
    pub web_ui_auth_disabled: bool,
    /// Static bearer token (LABBY_MCP_HTTP_TOKEN), if configured.
    ///
    /// Stored on AppState so handlers outside the auth middleware
    /// (e.g. `/auth/session`) can validate the same token. The middleware
    /// remains the canonical enforcement point for `/v1/*`.
    pub bearer_token: Option<Arc<str>>,
    /// HTTP bind host resolved by `labby serve`.
    pub http_bind_host: Option<Arc<String>>,
}

impl AppState {
    /// Build state from the default (all enabled features) registry.
    #[must_use]
    pub fn new() -> Self {
        let registry = build_default_registry();
        Self::from_registry(registry)
    }

    /// Build state from a pre-filtered or pre-built registry.
    ///
    /// Use this when the caller has already applied service filtering (e.g.
    /// `--services` on `labby serve`) so that the HTTP surface
    /// respects the same service set as the stdio surface.
    ///
    /// `enabled_services` is derived from the registry entries so the router
    /// can skip mounting handlers for services that were filtered out.
    ///
    #[must_use]
    pub fn from_registry(registry: ToolRegistry) -> Self {
        let enabled_services: HashSet<String> = registry
            .services()
            .iter()
            .map(|e| e.name.to_string())
            .collect();
        let catalog = Arc::new(build_catalog(&registry));
        let clients = Arc::new(ServiceClients::from_env());
        let protected_mcp_http_client = build_protected_mcp_http_client();
        Self {
            catalog,
            registry: Arc::new(registry),
            clients,
            protected_mcp_http_client,
            public_relay_forwarder: Arc::new(
                crate::oauth::public_relay::PublicRelayForwarder::default(),
            ),
            public_relay: None,
            protected_mcp_router: None,
            enabled_services: Arc::new(enabled_services),
            auth_config: None,
            config: Arc::new(LabConfig::default()),
            oauth_state: None,
            actor_key_deriver: None,
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            web_assets_dir: None,
            embedded_web_assets: false,
            workspace_root: None,
            web_ui_auth_disabled: false,
            bearer_token: None,
            http_bind_host: None,
            server_start: std::time::Instant::now(),
        }
    }

    /// Attach the resolved auth configuration.
    #[must_use]
    pub fn with_auth_config(mut self, config: labby_auth::config::AuthConfig) -> Self {
        self.auth_config = Some(Arc::new(config));
        self
    }

    #[must_use]
    pub fn with_config(mut self, config: LabConfig) -> Self {
        self.config = Arc::new(config);
        self
    }

    #[must_use]
    pub fn with_protected_mcp_router(mut self, router: axum::Router) -> Self {
        self.protected_mcp_router = Some(Arc::new(router));
        self
    }

    #[must_use]
    pub fn with_public_relay_manager(
        mut self,
        manager: Arc<crate::oauth::public_relay::PublicRelayRegistryManager>,
    ) -> Self {
        self.public_relay = Some(manager);
        self
    }

    #[must_use]
    pub fn with_oauth_state(mut self, auth_state: labby_auth::state::AuthState) -> Self {
        self.oauth_state = Some(Arc::new(auth_state));
        self
    }

    #[must_use]
    pub fn with_actor_key_deriver(
        mut self,
        deriver: crate::observability::activity::ActorKeyDeriver,
    ) -> Self {
        self.actor_key_deriver = Some(Arc::new(deriver));
        self
    }

    /// Attach the shared gateway manager.
    #[cfg(feature = "gateway")]
    #[must_use]
    #[allow(dead_code)] // Called by `labby serve` when gateway runtime is wired.
    pub fn with_gateway_manager(
        mut self,
        manager: Arc<crate::dispatch::gateway::manager::GatewayManager>,
    ) -> Self {
        self.gateway_manager = Some(manager);
        self
    }

    /// Attach an exported Labby assets directory for static web serving.
    #[must_use]
    pub fn with_web_assets_dir(mut self, dir: PathBuf) -> Self {
        self.web_assets_dir = Some(Arc::new(dir));
        self.embedded_web_assets = false;
        self
    }

    /// Enable Labby assets embedded into the lab binary.
    #[must_use]
    pub fn with_embedded_web_assets(mut self) -> Self {
        self.embedded_web_assets = true;
        self
    }

    #[must_use]
    pub fn web_assets_enabled(&self) -> bool {
        self.web_assets_dir.is_some() || self.embedded_web_assets
    }

    /// Attach the canonical workspace-root path for the filesystem browser
    /// service. Callers should pass an already-canonicalized, existing
    /// absolute path — the fs service assumes `starts_with` checks against
    /// this value are sound.
    #[must_use]
    #[allow(dead_code)] // Called by `labby serve` when fs HTTP routes are enabled.
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = Some(Arc::new(root));
        self
    }

    /// Disable auth on `/v1/*` while leaving `/mcp` auth unchanged.
    #[must_use]
    pub fn with_web_ui_auth_disabled(mut self, disabled: bool) -> Self {
        self.web_ui_auth_disabled = disabled;
        self
    }

    /// Attach the static bearer token (LABBY_MCP_HTTP_TOKEN) so handlers
    /// outside the auth middleware can validate it.
    #[must_use]
    pub fn with_bearer_token(mut self, token: Option<Arc<str>>) -> Self {
        self.bearer_token = token;
        self
    }

    #[must_use]
    pub fn with_http_bind_host(mut self, host: impl Into<String>) -> Self {
        self.http_bind_host = Some(Arc::new(host.into()));
        self
    }
}

fn protected_mcp_connect_timeout() -> Duration {
    crate::config::resolved_protected_mcp_connect_timeout_secs()
        .filter(|seconds| *seconds > 0)
        .map_or(
            Duration::from_secs(DEFAULT_PROTECTED_MCP_CONNECT_TIMEOUT_SECS),
            Duration::from_secs,
        )
}

fn build_protected_mcp_http_client() -> reqwest::Client {
    // See entrypoint.rs::run for why this call is needed under
    // "rustls-no-provider" -- idempotent, safe to ignore Err. entrypoint::run
    // already installs it for the real binary; test binaries don't go
    // through it.
    drop(rustls::crypto::ring::default_provider().install_default());
    reqwest::Client::builder()
        // Keep long-lived MCP streams possible, but fail unreachable upstreams
        // instead of letting proxy connection attempts hang indefinitely.
        .connect_timeout(protected_mcp_connect_timeout())
        .build()
        .expect("protected MCP HTTP client configuration is valid")
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
