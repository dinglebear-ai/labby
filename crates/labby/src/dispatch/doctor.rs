//! Shared dispatch layer for the `doctor` service.
//!
//! Doctor is a Bootstrap utility: no external service URL, no feature gate.
//! `system.checks` reads local state; `audit.full` combines the checks that
//! actually exist in the slim product (system, auth, access, gateway, and relay).

mod access;
mod catalog;
mod client;
mod dispatch;
pub mod gateway;
mod params;
mod preflight;
pub mod proxy;
mod relay;
pub mod service;
mod system;
mod types;

pub use catalog::ACTIONS;
pub use dispatch::{
    dispatch, dispatch_with_clients, dispatch_with_clients_and_relay, dispatch_with_surface,
};
pub use relay::check_public_relay;
pub use system::{run_auth_checks, run_auth_checks_with_config, run_system_checks};
pub use types::{Finding, Report, Severity};

use labby_primitives::plugin::{Category, PluginMeta};

/// Compile-time metadata for the doctor Bootstrap service.
pub const META: PluginMeta = PluginMeta {
    name: "doctor",
    display_name: "Doctor",
    description: "Comprehensive health audit: env vars, system, access store, gateway, and OAuth relay checks",
    category: Category::Bootstrap,
    docs_url: "https://github.com/dinglebear-ai/labby",
    required_env: &[],
    optional_env: &[],
    default_port: None,
    supports_multi_instance: false,
};
