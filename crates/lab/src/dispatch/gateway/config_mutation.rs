use std::collections::{BTreeMap, HashMap};

use lab_apis::extract::types::ServiceCreds;

use crate::config::UpstreamConfig;
use crate::dispatch::error::ToolError;

pub(super) fn ensure_stdio_admin_ack(
    action: &str,
    upstream: &UpstreamConfig,
    allow_stdio: bool,
) -> Result<(), ToolError> {
    if upstream.command.is_none() || !upstream.enabled || allow_stdio {
        return Ok(());
    }

    Err(ToolError::InvalidParam {
        message: format!(
            "{action} would execute local command `{}` for stdio gateway `{}`; resend with `allow_stdio: true` only after operator approval",
            upstream.command.as_deref().unwrap_or("<unknown>"),
            upstream.name
        ),
        param: "allow_stdio".to_string(),
    })
}

pub(super) fn read_env_values(
    path: &std::path::Path,
) -> Result<HashMap<String, String>, ToolError> {
    Ok(dotenvy::from_path_iter(path)
        .ok()
        .map(|iter| iter.filter_map(Result::ok).collect())
        .unwrap_or_default())
}

pub(super) fn values_to_service_creds(
    service: &str,
    values: &BTreeMap<String, String>,
) -> Vec<ServiceCreds> {
    values
        .iter()
        .map(|(field, value)| {
            let url = if field == &format!("{}_URL", service.to_uppercase()) {
                Some(value.clone())
            } else {
                None
            };
            let secret = if url.is_some() {
                None
            } else {
                Some(value.clone())
            };
            ServiceCreds {
                service: service.to_string(),
                url,
                secret,
                env_field: field.clone(),
                source_host: None,
                probe_host: None,
                runtime: None,
                url_verified: false,
            }
        })
        .collect()
}
