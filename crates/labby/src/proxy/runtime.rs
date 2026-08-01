//! Foreground loopback runtime for one explicitly selected stdio MCP child.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::{Json, Router, routing::get};
use labby_auth::{AuthLayer, state::AuthState, types::ProtectedResourceMetadata};
use labby_gateway::upstream::direct_stdio::{
    DirectStdioCommand, DirectStdioConnection, connect_direct_stdio,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use tokio_util::sync::CancellationToken;

use crate::mcp::bridge::{BridgeClientHandler, BridgeServerHandler};
use crate::proxy::command::ProxyCommand;
use crate::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPreferences};

pub struct LocalProxyOptions {
    pub command: ProxyCommand,
    pub preferences: ProxyPreferences,
    pub bearer_token: Option<String>,
    pub explicit_env: Vec<(OsString, OsString)>,
    pub inherit_env: Vec<OsString>,
}

#[derive(Clone)]
pub enum LocalProxyAuthPolicy {
    None,
    Bearer {
        token: Arc<str>,
        resource: url::Url,
    },
    Oauth {
        auth_state: Arc<AuthState>,
        resource: url::Url,
        issuer: url::Url,
        required_scopes: Vec<String>,
    },
}

impl std::fmt::Debug for LocalProxyAuthPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Bearer { resource, .. } => f
                .debug_struct("Bearer")
                .field("resource", resource)
                .finish_non_exhaustive(),
            Self::Oauth {
                resource,
                issuer,
                required_scopes,
                ..
            } => f
                .debug_struct("Oauth")
                .field("resource", resource)
                .field("issuer", issuer)
                .field("required_scopes", required_scopes)
                .finish_non_exhaustive(),
        }
    }
}

pub struct PreparedLocalProxy {
    display: String,
    connection: Option<DirectStdioConnection<BridgeClientHandler>>,
    listener: Option<tokio::net::TcpListener>,
    local_addr: std::net::SocketAddr,
    local_url: url::Url,
    path: String,
    protocol_version: rmcp::model::ProtocolVersion,
    child_pid: Option<u32>,
}

impl std::fmt::Debug for PreparedLocalProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedLocalProxy")
            .field("display", &self.display)
            .field("connection", &self.connection.is_some())
            .field("listener", &self.listener.is_some())
            .field("local_addr", &self.local_addr)
            .field("local_url", &self.local_url)
            .field("path", &self.path)
            .field("protocol_version", &self.protocol_version)
            .field("child_pid", &self.child_pid)
            .finish()
    }
}

#[derive(Clone)]
pub struct LocalProxyInfo {
    pub url: url::Url,
    pub local_addr: std::net::SocketAddr,
    pub command: String,
    pub child_pid: Option<u32>,
    pub protocol_version: rmcp::model::ProtocolVersion,
    pub auth: ProxyAuthMode,
}

impl std::fmt::Debug for LocalProxyInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProxyInfo")
            .field("url", &self.url)
            .field("local_addr", &self.local_addr)
            .field("command", &self.command)
            .field("child_pid", &self.child_pid)
            .field("protocol_version", &self.protocol_version)
            .field("auth", &self.auth)
            .finish()
    }
}

pub struct LocalProxy {
    info: LocalProxyInfo,
    path: String,
    connection: Option<DirectStdioConnection<BridgeClientHandler>>,
    cancellation: CancellationToken,
    server_task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl std::fmt::Debug for LocalProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProxy")
            .field("info", &self.info)
            .field("path", &self.path)
            .field("connection", &self.connection.is_some())
            .field("cancellation", &self.cancellation.is_cancelled())
            .field("server_task", &self.server_task.is_some())
            .finish()
    }
}

impl LocalProxy {
    pub async fn start(options: LocalProxyOptions) -> Result<Self> {
        if options.preferences.exposure != ProxyExposure::Local {
            bail!(
                "proxy exposure {:?} is unsupported in this runtime slice",
                options.preferences.exposure
            );
        }
        if !matches!(
            options.preferences.auth,
            ProxyAuthMode::None | ProxyAuthMode::Bearer
        ) {
            bail!(
                "proxy auth {:?} is unsupported in this runtime slice",
                options.preferences.auth
            );
        }
        let auth = match options.preferences.auth {
            ProxyAuthMode::Bearer => {
                let token = options
                    .bearer_token
                    .as_ref()
                    .filter(|token| !token.is_empty())
                    .cloned()
                    .context("bearer auth requires a non-empty proxy token")?;
                LocalProxyAuthPolicy::Bearer {
                    token: Arc::from(token),
                    resource: url::Url::parse("http://127.0.0.1/")?,
                }
            }
            ProxyAuthMode::None => LocalProxyAuthPolicy::None,
            _ => unreachable!("unsupported auth modes rejected above"),
        };

        let prepared = Self::prepare(options).await?;
        let auth = match auth {
            LocalProxyAuthPolicy::Bearer { token, .. } => LocalProxyAuthPolicy::Bearer {
                token,
                resource: prepared.local_url.clone(),
            },
            other => other,
        };
        prepared.start(auth)
    }

    pub async fn prepare(options: LocalProxyOptions) -> Result<PreparedLocalProxy> {
        if options.preferences.exposure != ProxyExposure::Local {
            bail!(
                "proxy exposure {:?} is unsupported in this runtime slice",
                options.preferences.exposure
            );
        }

        let display = options.command.display.clone();
        let (connection, _discovered_tools) = connect_direct_stdio(
            DirectStdioCommand {
                program: options.command.program,
                args: options.command.args,
                cwd: options.command.cwd,
                env: options.explicit_env,
                inherit_env: options.inherit_env,
                display: display.clone(),
            },
            BridgeClientHandler::new(),
        )
        .await
        .context("proxy child startup and MCP discovery failed")?;

        let protocol_version = connection
            .protocol_version()
            .unwrap_or(rmcp::model::ProtocolVersion::V_2026_07_28);
        let child_pid = connection.child_pid();
        let bind_port = options.preferences.port.fixed().unwrap_or(0);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", bind_port))
            .await
            .context("failed to bind proxy loopback listener")?;
        let local_addr = listener.local_addr()?;
        let url = url::Url::parse(&format!("http://{local_addr}{}", options.preferences.path))?;

        Ok(PreparedLocalProxy {
            display,
            connection: Some(connection),
            listener: Some(listener),
            local_addr,
            local_url: url,
            path: options.preferences.path,
            protocol_version,
            child_pid,
        })
    }

    #[must_use]
    pub fn info(&self) -> &LocalProxyInfo {
        &self.info
    }

    #[must_use]
    pub fn url(&self) -> &url::Url {
        &self.info.url
    }

    /// Wait until a supervised component exits unexpectedly.
    pub async fn wait_for_failure(&self) -> Result<()> {
        loop {
            if self
                .connection
                .as_ref()
                .is_none_or(DirectStdioConnection::is_closed)
            {
                bail!("proxy stdio child connection closed unexpectedly");
            }
            if self
                .server_task
                .as_ref()
                .is_none_or(tokio::task::JoinHandle::is_finished)
            {
                bail!("proxy HTTP listener exited unexpectedly");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn stop_http(&mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(server_task) = self.server_task.take() {
            match tokio::time::timeout(Duration::from_secs(3), server_task).await {
                Ok(result) => result.context("proxy HTTP task panicked")??,
                Err(_) => bail!("proxy HTTP server did not stop within 3 seconds"),
            }
        }
        Ok(())
    }

    pub async fn stop_child(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.shutdown().await;
        }
    }

    pub async fn shutdown(mut self) -> Result<()> {
        let http = self.stop_http().await;
        self.stop_child().await;
        http
    }

    pub async fn rollback_to_prepared(mut self) -> Result<PreparedLocalProxy> {
        self.stop_http().await?;
        let listener = tokio::net::TcpListener::bind(self.info.local_addr)
            .await
            .context("failed to rebind prepared proxy loopback listener")?;
        let connection = self
            .connection
            .take()
            .context("proxy child is no longer owned during rollback")?;
        Ok(PreparedLocalProxy {
            display: self.info.command.clone(),
            connection: Some(connection),
            listener: Some(listener),
            local_addr: self.info.local_addr,
            local_url: self.info.url.clone(),
            path: self.path.clone(),
            protocol_version: self.info.protocol_version.clone(),
            child_pid: self.info.child_pid,
        })
    }
}

impl PreparedLocalProxy {
    #[must_use]
    pub const fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    #[must_use]
    pub fn local_url(&self) -> &url::Url {
        &self.local_url
    }

    pub fn start(mut self, auth: LocalProxyAuthPolicy) -> Result<LocalProxy> {
        let listener = self
            .listener
            .take()
            .context("prepared proxy listener is no longer owned")?;
        let connection = self
            .connection
            .take()
            .context("prepared proxy child is no longer owned")?;
        let peer = connection.peer().clone();

        let cancellation = CancellationToken::new();
        let mut allowed_hosts = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
            self.local_addr.to_string(),
        ];
        let mut allowed_origins = vec![
            format!("http://127.0.0.1:{}", self.local_addr.port()),
            format!("http://localhost:{}", self.local_addr.port()),
        ];
        let external_resource = match &auth {
            LocalProxyAuthPolicy::None => None,
            LocalProxyAuthPolicy::Bearer { resource, .. }
            | LocalProxyAuthPolicy::Oauth { resource, .. } => Some(resource),
        };
        if let Some(resource) = external_resource {
            if let Some(host) = resource.host_str() {
                allowed_hosts.push(match resource.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_string(),
                });
            }
            let origin = resource.origin().ascii_serialization();
            if origin != "null" {
                allowed_origins.push(origin);
            }
        }
        let service_config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(allowed_hosts)
            .with_allowed_origins(allowed_origins)
            .with_legacy_session_mode(false)
            .with_json_response(false)
            .with_cancellation_token(cancellation.clone());
        let session_manager = Arc::new(NeverSessionManager::default());
        let mcp_service = StreamableHttpService::new(
            move || Ok(BridgeServerHandler::from_peer(peer.clone())),
            session_manager,
            service_config,
        );

        let mut mcp_router = Router::new().nest_service(&self.path, mcp_service);
        let auth_mode = match auth {
            LocalProxyAuthPolicy::None => ProxyAuthMode::None,
            LocalProxyAuthPolicy::Bearer { token, resource } => {
                mcp_router = mcp_router.layer(
                    AuthLayer::new()
                        .with_static_token(Some(token))
                        .with_resource_url(Some(Arc::from(resource.as_str()))),
                );
                ProxyAuthMode::Bearer
            }
            LocalProxyAuthPolicy::Oauth {
                auth_state,
                resource,
                issuer,
                required_scopes,
            } => {
                let configured_issuer = auth_state
                    .config
                    .public_url
                    .as_ref()
                    .context("OAuth auth state has no stable public issuer")?
                    .as_str()
                    .trim_end_matches('/');
                if configured_issuer != issuer.as_str().trim_end_matches('/') {
                    bail!("OAuth auth state issuer does not match the stable issuer");
                }
                let metadata_url = root_metadata_url(&resource)?;
                let layer = AuthLayer::from_state(auth_state)
                    .with_resource_url(Some(Arc::from(resource.as_str())))
                    .with_required_scopes(required_scopes.clone())
                    .with_protected_resource_metadata_url(Some(Arc::from(metadata_url.as_str())));
                mcp_router = mcp_router.layer(layer);
                let metadata = ProtectedResourceMetadata {
                    resource: resource.to_string(),
                    authorization_servers: vec![issuer.to_string().trim_end_matches('/').into()],
                    scopes_supported: required_scopes,
                    bearer_methods_supported: vec!["header".to_string()],
                };
                let router = mcp_router.route(
                    "/.well-known/oauth-protected-resource",
                    get(move || async move { Json(metadata.clone()) }),
                );
                return self.finish_start(
                    connection,
                    cancellation,
                    listener,
                    router,
                    ProxyAuthMode::Oauth,
                );
            }
        };

        self.finish_start(connection, cancellation, listener, mcp_router, auth_mode)
    }

    fn finish_start(
        self,
        connection: DirectStdioConnection<BridgeClientHandler>,
        cancellation: CancellationToken,
        listener: tokio::net::TcpListener,
        router: Router,
        auth: ProxyAuthMode,
    ) -> Result<LocalProxy> {
        let shutdown = cancellation.clone();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .context("proxy HTTP server failed")
        });

        Ok(LocalProxy {
            info: LocalProxyInfo {
                url: self.local_url.clone(),
                local_addr: self.local_addr,
                command: self.display.clone(),
                child_pid: self.child_pid,
                protocol_version: self.protocol_version.clone(),
                auth,
            },
            path: self.path.clone(),
            connection: Some(connection),
            cancellation,
            server_task: Some(server_task),
        })
    }
}

fn root_metadata_url(resource: &url::Url) -> Result<url::Url> {
    let mut metadata = resource.clone();
    metadata.set_path("/.well-known/oauth-protected-resource");
    metadata.set_query(None);
    metadata.set_fragment(None);
    Ok(metadata)
}

impl Drop for LocalProxy {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
        // DirectStdioConnection::Drop owns fail-safe process-tree cleanup.
    }
}

impl Drop for PreparedLocalProxy {
    fn drop(&mut self) {
        // Dropping the listener closes the unserved socket. DirectStdioConnection::Drop
        // owns fail-safe process-tree cleanup when preparation is rolled back.
        self.listener.take();
    }
}
