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

pub fn personal_readiness_finding(findings: &[Finding], browser_oauth_expected: bool) -> Finding {
    let failures = findings
        .iter()
        .filter(|finding| matches!(finding.severity, Severity::Fail))
        .count();
    let warnings = findings
        .iter()
        .filter(|finding| matches!(finding.severity, Severity::Warn))
        .count();
    let (severity, message) = if failures > 0 {
        (
            Severity::Fail,
            format!(
                "Labby is not operational yet: {failures} blocking check(s) failed; resolve them before relying on this installation"
            ),
        )
    } else if browser_oauth_expected {
        (
            Severity::Ok,
            format!(
                "Labby is operational for the configured Browser + ChatGPT OAuth workflow; {warnings} recommendation(s) remain"
            ),
        )
    } else {
        (
            Severity::Ok,
            format!(
                "Labby is operational for local/bearer workflows; Browser + ChatGPT public OAuth is optional and {warnings} recommendation(s) remain"
            ),
        )
    };
    Finding {
        service: "doctor".into(),
        check: "readiness:personal".into(),
        severity,
        message,
    }
}

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
