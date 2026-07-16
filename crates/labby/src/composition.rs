//! Product-level wiring shared by the CLI, HTTP daemon, and MCP surface.
//!
//! Surface adapters must not import sibling adapters directly. Concrete
//! protocol implementations are selected here and exposed through neutral
//! runtime seams.

#[cfg(feature = "gateway")]
pub(crate) fn in_process_connector() -> crate::dispatch::upstream::pool::InProcessConnector {
    crate::mcp::in_process_peer::connector()
}
