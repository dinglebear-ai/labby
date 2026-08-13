//! Public, explicitly-authorized stdio MCP connection API.

use std::ffi::OsString;
use std::path::PathBuf;

use rmcp::service::Peer;
use rmcp::{ClientHandler, RoleClient};

use super::pool::UpstreamConnection;

/// An argv-based stdio command entered explicitly by a local operator.
#[derive(Debug, Clone)]
pub struct DirectStdioCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    pub inherit_env: Vec<OsString>,
    pub display: String,
}

/// A discovered MCP peer together with ownership of its child process tree.
pub struct DirectStdioConnection<H = ()>
where
    H: ClientHandler,
{
    inner: UpstreamConnection<H>,
}

impl<H> std::fmt::Debug for DirectStdioConnection<H>
where
    H: ClientHandler,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectStdioConnection")
            .field("child_pid", &self.child_pid())
            .field("protocol_version", &self.protocol_version())
            .finish_non_exhaustive()
    }
}

impl<H> DirectStdioConnection<H>
where
    H: ClientHandler,
{
    pub(crate) fn new(inner: UpstreamConnection<H>) -> Self {
        Self { inner }
    }

    #[must_use]
    pub fn peer(&self) -> &Peer<RoleClient> {
        &self.inner.peer
    }

    #[must_use]
    pub fn child_pid(&self) -> Option<u32> {
        self.inner.runtime.pid
    }

    #[must_use]
    pub fn protocol_version(&self) -> Option<rmcp::model::ProtocolVersion> {
        self.inner
            .peer
            .peer_info()
            .map(|info| info.protocol_version.clone())
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.peer.is_transport_closed()
    }

    pub async fn shutdown(self) {
        self.inner.shutdown("direct-stdio", "proxy_shutdown").await;
    }
}

/// Spawn, negotiate, and discover an explicitly-authorized local stdio server.
///
/// Dropping the returned connection is fail-safe: the same Unix process-group
/// and Windows Job Object ownership used by the gateway pool reaps descendants.
pub async fn connect_direct_stdio<H>(
    command: DirectStdioCommand,
    handler: H,
) -> anyhow::Result<(DirectStdioConnection<H>, Vec<rmcp::model::Tool>)>
where
    H: ClientHandler + Clone,
{
    let (connection, tools) = super::pool::connect_direct_stdio(command, handler).await?;
    Ok((DirectStdioConnection::new(connection), tools))
}
