//! Authenticated Palette integration identity.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::Path;

use axum::{Json, Router, extract::State, routing::get};
use labby_auth::config::AuthMode;
use serde::Serialize;

use crate::api::state::AppState;

const CONTRACT_VERSION: &str = "1.0.0";
const REDACTED_CREDENTIAL_GENERATION: &str = "redacted";

#[derive(Debug, Serialize)]
struct IntegrationIdentity {
    contract_version: &'static str,
    product: &'static str,
    server_id: String,
    product_version: &'static str,
    api_version: ApiVersion,
    capabilities: Vec<&'static str>,
    auth: IntegrationAuth,
    streams: IntegrationStreams,
}

#[derive(Debug, Serialize)]
struct ApiVersion {
    major: u8,
    minor: u8,
}

#[derive(Debug, Serialize)]
struct IntegrationAuth {
    modes: Vec<&'static str>,
    issuer: Option<String>,
    audience: Option<String>,
    token_endpoint_origin: Option<String>,
    principal_cache_scope: &'static str,
    credential_generation: &'static str,
}

#[derive(Debug, Serialize)]
struct IntegrationStreams {
    transport: &'static str,
    resume: &'static str,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/identity", get(identity))
}

async fn identity(State(state): State<AppState>) -> Json<IntegrationIdentity> {
    Json(IntegrationIdentity {
        contract_version: CONTRACT_VERSION,
        product: "labby",
        server_id: state.integration_server_id.to_string(),
        product_version: env!("CARGO_PKG_VERSION"),
        api_version: ApiVersion { major: 1, minor: 0 },
        capabilities: capabilities(&state),
        auth: integration_auth(&state),
        streams: IntegrationStreams {
            transport: "none",
            resume: "none",
        },
    })
}

fn capabilities(state: &AppState) -> Vec<&'static str> {
    #[cfg(feature = "gateway")]
    if state.gateway_manager.is_some() {
        return vec![
            "gateway_management",
            "exact_tool_call",
            "catalog",
            "snippets",
            "loadouts",
        ];
    }

    Vec::new()
}

fn integration_auth(state: &AppState) -> IntegrationAuth {
    let oauth = state
        .auth_config
        .as_ref()
        .filter(|config| config.mode == AuthMode::OAuth);

    let mut modes = Vec::with_capacity(2);
    if state.bearer_token.is_some() {
        modes.push("static_bearer");
    }
    if oauth.is_some() {
        modes.push("oauth2");
    }
    if modes.is_empty() {
        modes.push("service_identity");
    }

    let issuer = oauth
        .and_then(|config| config.public_url.as_ref())
        .map(|url| url.as_str().trim_end_matches('/').to_owned());
    let audience = issuer.as_ref().map(|base| format!("{base}/v1"));
    let token_endpoint_origin = issuer.clone();
    let principal_cache_scope = if oauth.is_some() {
        "issuer-subject"
    } else if state.bearer_token.is_some() {
        "static-bearer"
    } else {
        "service-identity"
    };

    IntegrationAuth {
        modes,
        issuer,
        audience,
        token_endpoint_origin,
        principal_cache_scope,
        credential_generation: REDACTED_CREDENTIAL_GENERATION,
    }
}

pub(crate) fn load_or_create_server_id(path: &Path) -> anyhow::Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        return validate_server_id(existing.trim()).map(str::to_owned);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let candidate = format!("labby_{}", uuid::Uuid::new_v4().simple());
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => {
            file.write_all(candidate.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            Ok(candidate)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let existing = std::fs::read_to_string(path)?;
            validate_server_id(existing.trim()).map(str::to_owned)
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_server_id(value: &str) -> anyhow::Result<&str> {
    let suffix = value
        .strip_prefix("labby_")
        .filter(|suffix| (16..=128).contains(&suffix.len()))
        .filter(|suffix| {
            suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
    suffix
        .map(|_| value)
        .ok_or_else(|| anyhow::anyhow!("invalid persisted Labby integration server ID"))
}

#[cfg(test)]
#[path = "integration_identity_tests.rs"]
mod tests;
