//! Spawn-time resolution of secret stdio environment values.
//!
//! `UpstreamConfig::secret_env` stores only names: the child variable and the
//! credential variable that holds its value. Values are read from the selected
//! installation `.env` (or a genuine external override) on every spawn, so a
//! rotation takes effect on the next reconnect and no value is cached here.

use std::ffi::OsString;
use std::path::Path;

use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;

use crate::security::stdio_secret_env::{
    validate_secret_env_credential_name, validate_secret_env_name,
};
use crate::upstream::auth::configured_raw_credential;

/// Resolve every secret value for `config`, verbatim and in name order.
///
/// A missing or empty value refuses the spawn. The error names the variables
/// but never a value. `dotenv_path` overrides the installation `.env` path.
pub(super) fn secret_environment(
    config: &UpstreamConfig,
    dotenv_path: Option<&Path>,
) -> Result<Vec<(OsString, OsString)>, ToolError> {
    config
        .secret_env
        .iter()
        .map(|(child_name, credential_name)| {
            // Write paths validate names; recheck so a hand-edited config
            // cannot use a secret to replace a launcher or sandbox variable.
            validate_secret_env_name(child_name)?;
            validate_secret_env_credential_name(child_name, credential_name)?;
            configured_raw_credential(credential_name, dotenv_path)
                .filter(|value| !value.trim().is_empty())
                .map(|value| (OsString::from(child_name), OsString::from(value)))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "upstream_credential_missing".to_string(),
                    message: format!(
                        "Secret env var `{child_name}` for upstream `{}` has no value in credential `{credential_name}`. Set it with `labby server set {} --env {child_name}` and reload the upstream.",
                        config.name, config.name
                    ),
                })
        })
        .collect()
}
