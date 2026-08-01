//! Foreground loopback runtime for one explicitly selected stdio MCP child.

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Router;
use labby_auth::AuthLayer;
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
    connection: Option<DirectStdioConnection<BridgeClientHandler>>,
    cancellation: CancellationToken,
    server_task: Option<tokio::task::JoinHandle<Result<()>>>,
}

impl std::fmt::Debug for LocalProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProxy")
            .field("info", &self.info)
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
        let bearer_token = match options.preferences.auth {
            ProxyAuthMode::Bearer => Some(
                options
                    .bearer_token
                    .filter(|token| !token.is_empty())
                    .context("bearer auth requires a non-empty proxy token")?,
            ),
            ProxyAuthMode::None => None,
            _ => unreachable!("unsupported auth modes rejected above"),
        };

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
        let peer = connection.peer().clone();

        let bind_port = options.preferences.port.fixed().unwrap_or(0);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", bind_port))
            .await
            .context("failed to bind proxy loopback listener")?;
        let local_addr = listener.local_addr()?;
        let url = url::Url::parse(&format!("http://{local_addr}{}", options.preferences.path))?;

        let cancellation = CancellationToken::new();
        let service_config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "::1".to_string(),
                local_addr.to_string(),
            ])
            .with_allowed_origins([
                format!("http://127.0.0.1:{}", local_addr.port()),
                format!("http://localhost:{}", local_addr.port()),
            ])
            .with_legacy_session_mode(false)
            .with_json_response(false)
            .with_cancellation_token(cancellation.clone());
        let session_manager = Arc::new(NeverSessionManager::default());
        let mcp_service = StreamableHttpService::new(
            move || Ok(BridgeServerHandler::from_peer(peer.clone())),
            session_manager,
            service_config,
        );

        let mut router = Router::new().nest_service(&options.preferences.path, mcp_service);
        if let Some(token) = bearer_token {
            router = router.layer(
                AuthLayer::new()
                    .with_static_token(Some(Arc::<str>::from(token)))
                    .with_resource_url(Some(Arc::<str>::from(url.as_str()))),
            );
        }

        let shutdown = cancellation.clone();
        let server_task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .context("proxy HTTP server failed")
        });

        Ok(Self {
            info: LocalProxyInfo {
                url,
                local_addr,
                command: display,
                child_pid,
                protocol_version,
                auth: options.preferences.auth,
            },
            connection: Some(connection),
            cancellation,
            server_task: Some(server_task),
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

    pub async fn shutdown(mut self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(server_task) = self.server_task.take() {
            match tokio::time::timeout(Duration::from_secs(3), server_task).await {
                Ok(result) => result.context("proxy HTTP task panicked")??,
                Err(_) => bail!("proxy HTTP server did not stop within 3 seconds"),
            }
        }
        if let Some(connection) = self.connection.take() {
            connection.shutdown().await;
        }
        Ok(())
    }
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
