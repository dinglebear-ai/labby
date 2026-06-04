//! McpRegistry client construction helpers for `mcp.*` actions in marketplace dispatch.
//!
//! Uses `[mcpregistry].url` from `config.toml`, falling back to the official
//! public registry URL when the setting is absent.

#[cfg(feature = "mcpregistry")]
use lab_apis::mcpregistry::client::McpRegistryClient;

use crate::config;
use crate::dispatch::error::ToolError;

/// Return a McpRegistry client using config.toml, or the official default URL.
#[cfg(feature = "mcpregistry")]
pub fn require_mcp_client() -> Result<McpRegistryClient, ToolError> {
    let url = configured_registry_url()?;
    McpRegistryClient::new(&url).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("McpRegistry client init failed: {e}"),
    })
}

#[cfg(feature = "mcpregistry")]
pub fn configured_registry_url() -> Result<String, ToolError> {
    let cfg = config::load_toml(&config::toml_candidates()).map_err(|e| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("load config.toml: {e}"),
    })?;
    Ok(config::mcpregistry_url(&cfg).to_string())
}

/// Structured `not_configured` error when the mcpregistry feature is disabled.
#[cfg(not(feature = "mcpregistry"))]
pub fn not_configured_error() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_configured".to_string(),
        message: "mcpregistry feature is disabled".to_string(),
    }
}

// Suppress unused warning when mcpregistry feature is off.
#[cfg(not(feature = "mcpregistry"))]
pub fn require_mcp_client() -> Result<(), ToolError> {
    Err(not_configured_error())
}

// TODO(lab-zxx5.3): Add ACP client stub/placeholder here once ACP registry
// client is available in lab-apis. The three-client architecture will be:
// 1. Marketplace filesystem client (always available, in client.rs)
// 2. McpRegistryClient (this file, configured through config.toml)
// 3. AcpRegistryClient (placeholder — lab-zxx5.3)
