//! Staging of write-only secret stdio environment values.
//!
//! `gateway.add` and `gateway.update` accept `secret_env_values` (child
//! variable name to value). The values go to the credential file through the
//! config transaction; the upstream keeps only the names in `secret_env`.

use std::collections::BTreeMap;

use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;

use crate::gateway::config::default_secret_env_credential_name;
use crate::security::stdio_secret_env::{
    MAX_SECRET_ENV_ENTRIES, validate_secret_env_name, validate_secret_env_value,
};

/// One credential-file write: `(credential variable, value)`.
pub(super) type CredentialWrite = (String, String);

/// Credential-file writes committed with one config transaction.
#[derive(Default)]
pub(super) struct CredentialWrites {
    /// Bearer token value, normalized to an `Authorization` header on write.
    pub bearer: Option<CredentialWrite>,
    /// Secret stdio env values, written verbatim.
    pub raw: Vec<CredentialWrite>,
}

impl CredentialWrites {
    pub(super) fn is_empty(&self) -> bool {
        self.bearer.is_none() && self.raw.is_empty()
    }
}

/// Validate `values` and record them on `spec`.
///
/// Each child variable keeps its existing credential name, so a renamed
/// upstream rotates the same key, or gets a derived name on first write. A
/// plain `env` entry with the same name is replaced. Errors never include a
/// value.
pub(super) fn stage_secret_env_values(
    spec: &mut UpstreamConfig,
    values: BTreeMap<String, String>,
) -> Result<Vec<CredentialWrite>, ToolError> {
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let invalid = |message: String| ToolError::InvalidParam {
        message,
        param: "secret_env_values".to_string(),
    };
    if spec.command.is_none() {
        return Err(invalid(
            "secret env values apply only to stdio upstreams; set `command`".to_string(),
        ));
    }
    if values.len() > MAX_SECRET_ENV_ENTRIES {
        return Err(invalid(format!(
            "at most {MAX_SECRET_ENV_ENTRIES} secret env values may be set at once"
        )));
    }
    for (child, value) in &values {
        validate_secret_env_name(child)?;
        validate_secret_env_value(child, value)?;
    }
    Ok(values
        .into_iter()
        .map(|(child, value)| {
            let credential = spec
                .secret_env
                .get(&child)
                .cloned()
                .unwrap_or_else(|| default_secret_env_credential_name(&spec.name, &child));
            spec.env.remove(&child);
            spec.secret_env.insert(child, credential.clone());
            (credential, value)
        })
        .collect())
}
