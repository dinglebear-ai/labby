//! The public dispatch entry point for the `openapi` provider.

use crate::error::OpenApiError;
use crate::registry::OpenApiRegistry;

/// Dispatch one `openapi::<label>.<operationId>` call: resolve the operation
/// (deny-by-default allowlist enforced at load), then execute it through the
/// hardened client. Emits one scrubbed dispatch event on BOTH the success and
/// failure path (`service`/`action`/`label`/`host`/`method`/`status`/
/// `elapsed_ms`, plus `kind` on error) — never a body, query-with-auth, or
/// credential.
///
/// The SSRF pin + peer re-check are ALWAYS enforced here (the production path).
/// Tests that need to reach a loopback mock use
/// [`dispatch_openapi_call_no_ssrf`], keeping the SSRF branch out of this
/// shipping entry point.
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
    let host = handle.base_url.host_str().unwrap_or_default().to_string();
    let method = handle.method.clone();
    let started = std::time::Instant::now();
    let result = crate::http::execute_operation(client, handle, params).await;
    log_dispatch(label, operation_id, &host, &method, started, &result);
    result
}

/// TEST-ONLY dispatch that skips the SSRF pin so a loopback wiremock server is
/// reachable. Never compiled into a release build; keeps the production
/// [`dispatch_openapi_call`] free of any `cfg(test)` branch.
#[cfg(test)]
pub(crate) async fn dispatch_openapi_call_no_ssrf(
    registry: &OpenApiRegistry,
    client: &reqwest::Client,
    label: &str,
    operation_id: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, OpenApiError> {
    let handle = registry.operation(label, operation_id)?;
    let host = handle.base_url.host_str().unwrap_or_default().to_string();
    let method = handle.method.clone();
    let started = std::time::Instant::now();
    let result = crate::http::execute_operation_no_ssrf(client, handle, params).await;
    log_dispatch(label, operation_id, &host, &method, started, &result);
    result
}

/// Emit the scrubbed dispatch event — INFO on success, WARN on error with the
/// stable `kind`. Shared by the production and test dispatch paths.
fn log_dispatch(
    label: &str,
    operation_id: &str,
    host: &str,
    method: &reqwest::Method,
    started: std::time::Instant,
    result: &Result<serde_json::Value, OpenApiError>,
) {
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(_) => tracing::info!(
            service = "openapi",
            action = operation_id,
            label = %label,
            host = %host,
            method = %method,
            status = "ok",
            elapsed_ms = elapsed_ms as u64,
            "openapi dispatch complete"
        ),
        Err(e) => tracing::warn!(
            service = "openapi",
            action = operation_id,
            label = %label,
            host = %host,
            method = %method,
            status = "error",
            kind = e.kind(),
            elapsed_ms = elapsed_ms as u64,
            "openapi dispatch failed"
        ),
    }
}
