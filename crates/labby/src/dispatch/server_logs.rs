//! Shared dispatch layer for the `server_logs` operator tool.
//!
//! This is intentionally narrow: it reads Labby's own rolling process logs.
//! It does not reintroduce syslog ingestion, fleet log storage, or external
//! host log collection.

mod catalog;
mod client;
mod dispatch;
mod params;

pub use catalog::ACTIONS;
pub use dispatch::dispatch;

use labby_primitives::plugin::{Category, PluginMeta};

/// Compile-time metadata for the server log viewer.
pub const META: PluginMeta = PluginMeta {
    name: "server_logs",
    display_name: "Server Logs",
    description: "View and filter Labby's own rolling server process logs",
    category: Category::Bootstrap,
    docs_url: "https://github.com/jmagar/lab",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
