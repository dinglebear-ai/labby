//! The public dispatch entry point for the `openapi` provider.

use crate::error::OpenApiError;
use crate::registry::OpenApiRegistry;

/// Dispatch one `openapi::<label>.<operationId>` call: resolve the operation
/// (deny-by-default allowlist enforced at load), then execute it through the
/// hardened client. Logs a scrubbed dispatch event (method / host / status only
/// — no body, no query-with-auth, no credential).
///
/// # Errors
/// Returns a scrubbed [`OpenApiError`] for an unknown label/operation, SSRF
/// rejection, timeout, or transport/status failure.
pub async fn dispatch_openapi_call(
    registry: &OpenApiRegistry,
    client: &reqwest::Client,
    label: &str,
    operation_id: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, OpenApiError> {
    let handle = registry.operation(label, operation_id)?;
    #[cfg(test)]
    let out = crate::http::execute_operation_no_ssrf(client, handle, params).await?;
    #[cfg(not(test))]
    let out = crate::http::execute_operation(client, handle, params).await?;
    tracing::info!(
        service = "openapi",
        action = operation_id,
        label = %label,
        host = %handle.base_url.host_str().unwrap_or_default(),
        status = "ok",
        "openapi dispatch complete"
    );
    Ok(out)
}
