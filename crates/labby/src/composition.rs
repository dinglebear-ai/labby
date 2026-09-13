//! Product-level wiring shared by the CLI, HTTP daemon, and MCP surface.
//!
//! Surface adapters must not import sibling adapters directly. Concrete
//! protocol implementations are selected here and exposed through neutral
//! runtime seams.

/// Offline `labby serve` config validation shared by `setup check` and
/// `doctor system.checks`. It composes several subsystems' startup checks, so
/// it lives here rather than inside any one dispatch service.
pub(crate) mod config_check;

#[cfg(feature = "gateway")]
pub(crate) fn in_process_connector() -> crate::dispatch::upstream::pool::InProcessConnector {
    crate::mcp::in_process_peer::connector()
}
