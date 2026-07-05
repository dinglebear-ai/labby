//! Shared dispatch layer for the `doctor` service.
//!
//! Doctor is a Bootstrap utility: no external service URL, no feature gate.
//! `system.checks` reads local state; `service.probe` and `audit.full` use
//! pre-built `ServiceClients`.

mod catalog;
mod client;
mod dispatch;
pub mod gateway;
mod params;
pub mod proxy;
pub mod service;
mod system;
mod types;

pub use catalog::ACTIONS;
pub use dispatch::{dispatch, dispatch_with_clients};
pub use system::{run_auth_checks, run_system_checks};
pub use types::{Finding, Report, Severity};

use labby_primitives::plugin::{Category, PluginMeta};

/// Compile-time metadata for the doctor Bootstrap service.
pub const META: PluginMeta = PluginMeta {
    name: "doctor",
    display_name: "Doctor",
    description: "Comprehensive health audit: env vars, system probes, and service reachability",
    category: Category::Bootstrap,
    docs_url: "https://github.com/jmagar/lab",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
