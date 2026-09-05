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
pub mod provider;
pub mod proxy;
mod relay;
pub mod service;
mod system;
mod types;

pub use catalog::ACTIONS;
pub use dispatch::{
    AuthConfigSource, dispatch, dispatch_with_clients, dispatch_with_clients_and_relay,
    dispatch_with_clients_relay_and_auth, dispatch_with_surface,
};
pub use relay::check_public_relay;
pub use system::{run_auth_checks, run_auth_checks_with_config, run_system_checks};
pub use types::{Finding, Report, Severity};

pub fn auth_config_error_finding(error: &str) -> Finding {
    let error = labby_runtime::agent_error::sanitize_error_text(error, 1024);
    tracing::warn!(
        surface = "doctor",
        phase = "auth.config.resolve",
        kind = "config_error",
        error = %error,
        "auth configuration resolution failed"
    );
    Finding {
        service: "auth".into(),
        check: "auth:config".into(),
        severity: Severity::Fail,
        message: "kind=config_error; auth configuration is invalid; verify provider selection and provider-specific settings".into(),
    }
}

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
