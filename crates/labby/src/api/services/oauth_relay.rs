use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    Extension, Json, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderName, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

use crate::api::error::{ApiError, ToolError};
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;
use crate::oauth::public_relay::{
    ForwardRequest, ImportReport, MachineId, MutationReport, PUBLIC_QUERY_LIMIT_BYTES,
    PUBLIC_REQUEST_BODY_LIMIT_BYTES, PublicRelayEntry, PublicRelayError, PublicRelayHealth,
    PublicRelayRegistryManager, PublicRelayRegistryStore, RegistryWriteOutcome,
    suffix_after_machine,
};

pub fn public_routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/callback/{machine_id}", get(callback).post(callback))
        .route(
            "/callback/{machine_id}/{*suffix}",
            get(callback).post(callback),
        )
}

pub fn admin_routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route(
            "/machines",
            get(list_machines).post(upsert_machine_without_path),
        )
        .route(
            "/machines/{machine_id}",
            get(get_machine)
                .put(upsert_machine_with_path)
                .delete(remove_machine),
        )
        .route("/machines/{machine_id}/disable", post(disable_machine))
        .route("/machines/{machine_id}/enable", post(enable_machine))
        .route("/import", post(import_registry))
}

/// Shallow health probe for the unauthenticated public callback surface.
///
/// Per `docs/runtime/OAUTH.md`, `/healthz` only reports process-alive,
/// relay-enabled, and registry-loaded from the already-validated in-memory
/// manager snapshot — it never touches disk. Deep persisted-vs-live
/// staleness detection belongs to `labby doctor oauth-relay`
/// (`crate::dispatch::doctor::relay::check_public_relay`), which is an
/// authenticated/operator-triggered check, not a public unauthenticated one.
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    match state.public_relay.as_ref() {
        Some(manager) => {
            let machines = manager.count().await;
            (
                StatusCode::OK,
                Json(PublicRelayHealth {
                    status: "ok",
                    relay: "enabled",
                    registry: "loaded",
                    machines,
                }),
            )
                .into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(PublicRelayHealth {
                status: "unavailable",
                relay: "disabled",
                registry: "missing",
                machines: 0,
            }),
        )
            .into_response(),
    }
}

async fn callback(
    State(state): State<AppState>,
    Path(params): Path<HashMap<String, String>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let started = Instant::now();
    let machine_raw = params.get("machine_id").map(String::as_str).unwrap_or("");
    let machine_id = match MachineId::parse(machine_raw) {
        Ok(machine_id) => machine_id,
        Err(error) => {
            return public_error_response_logged(error, started, None, machine_raw, &method, None);
        }
    };
    if uri
        .query()
        .is_some_and(|query| query.len() > PUBLIC_QUERY_LIMIT_BYTES)
    {
        return public_error_response_logged(
            PublicRelayError::BodyTooLarge,
            started,
            Some(&machine_id),
            machine_raw,
            &method,
            None,
        );
    }
    if content_length_exceeds_limit(&headers, PUBLIC_REQUEST_BODY_LIMIT_BYTES) {
        return public_error_response_logged(
            PublicRelayError::BodyTooLarge,
            started,
            Some(&machine_id),
            machine_raw,
            &method,
            None,
        );
    }
    let suffix_path = match suffix_after_machine(uri.path(), &machine_id) {
        Ok(suffix) => suffix,
        Err(error) => {
            return public_error_response_logged(
                error,
                started,
                Some(&machine_id),
                machine_raw,
                &method,
                None,
            );
        }
    };
    let Some(manager) = state.public_relay.clone() else {
        return public_error_response_logged(
            PublicRelayError::RegistryUnavailable(
                "public relay registry manager is not loaded".into(),
            ),
            started,
            Some(&machine_id),
            machine_raw,
            &method,
            None,
        );
    };
    let target = match manager.resolve(&machine_id).await {
        Ok(target) => target,
        Err(error) => {
            return public_error_response_logged(
                error,
                started,
                Some(&machine_id),
                machine_raw,
                &method,
                None,
            );
        }
    };
    // Acquire the per-machine/global concurrency permit *before* buffering
    // the request body: a request to an already-saturated machine should be
    // rejected with 429 before doing any buffering work, not after. Since
    // `/callback/*` is public and unauthenticated, buffering first would let
    // requests to a saturated machine still pay the full body-read cost
    // (up to `PUBLIC_REQUEST_BODY_LIMIT_BYTES`) before being turned away.
    let _permit = match manager.acquire_forward_permit(&machine_id).await {
        Ok(permit) => permit,
        Err(error) => {
            return public_error_response_logged(
                error,
                started,
                Some(&machine_id),
                machine_raw,
                &method,
                None,
            );
        }
    };
    let body = match axum::body::to_bytes(body, PUBLIC_REQUEST_BODY_LIMIT_BYTES).await {
        Ok(body) => body,
        Err(error) => {
            let relay_error = body_read_error_to_public(&error);
            return public_error_response_logged(
                relay_error,
                started,
                Some(&machine_id),
                machine_raw,
                &method,
                Some(error.to_string()),
            );
        }
    };

    let result = state
        .public_relay_forwarder
        .forward(ForwardRequest {
            method: method.clone(),
            target,
            suffix_path,
            query: uri.query().map(str::to_string),
            headers,
            body,
        })
        .await;

    match result {
        Ok(forwarded) => {
            tracing::info!(
                surface = "api",
                service = "oauth_relay",
                action = "callback.forward",
                machine_id = %machine_id,
                method = %method,
                status = forwarded.status.as_u16(),
                elapsed_ms = started.elapsed().as_millis(),
                "public oauth callback relay forward complete"
            );
            build_forward_response(forwarded.status, &forwarded.headers, forwarded.body)
        }
        Err(error) => {
            tracing::warn!(
                surface = "api",
                service = "oauth_relay",
                action = "callback.forward",
                machine_id = %machine_id,
                method = %method,
                kind = error.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "public oauth callback relay forward failed"
            );
            public_error_response(error)
        }
    }
}

fn build_forward_response(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().extend(headers.clone());
    apply_public_callback_security_headers(response.headers_mut());
    response
}

/// Fails open (`false`) for a missing, non-UTF-8, or unparseable
/// `Content-Length` -- not exploitable, since the real backstop is the
/// `axum::body::to_bytes(body, LIMIT)` read cap applied regardless of this
/// header. Still logs the malformed cases at debug so a client sending a
/// garbage `Content-Length` leaves a trace instead of silently no-op'ing.
fn content_length_exceeds_limit(headers: &HeaderMap, limit: usize) -> bool {
    let Some(value) = headers.get(header::CONTENT_LENGTH) else {
        return false;
    };
    let value = match value.to_str() {
        Ok(value) => value,
        Err(error) => {
            tracing::debug!(
                surface = "api",
                service = "oauth_relay",
                error = %error,
                "content-length header is not valid UTF-8; treating as absent (fails open)"
            );
            return false;
        }
    };
    match value.parse::<usize>() {
        Ok(length) => length > limit,
        Err(error) => {
            tracing::debug!(
                surface = "api",
                service = "oauth_relay",
                value,
                error = %error,
                "content-length header failed to parse as usize; treating as absent (fails open)"
            );
            false
        }
    }
}

fn apply_public_callback_security_headers(headers: &mut HeaderMap) {
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
    headers.insert(header::EXPIRES, header::HeaderValue::from_static("0"));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        header::HeaderValue::from_static(
            "default-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; worker-src 'none'",
        ),
    );
}

fn public_error_status(error: &PublicRelayError) -> StatusCode {
    match error {
        PublicRelayError::BodyTooLarge | PublicRelayError::ResponseTooLarge => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        PublicRelayError::InvalidRequestBody(_) => StatusCode::BAD_REQUEST,
        PublicRelayError::Overloaded => StatusCode::TOO_MANY_REQUESTS,
        PublicRelayError::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
        PublicRelayError::InvalidMachineId(_)
        | PublicRelayError::InvalidSuffix(_)
        | PublicRelayError::UnknownMachine
        | PublicRelayError::DisabledMachine => StatusCode::NOT_FOUND,
        PublicRelayError::InvalidRegistryInput(_) => StatusCode::UNPROCESSABLE_ENTITY,
        PublicRelayError::InvalidTarget(_)
        | PublicRelayError::RegistryUnavailable(_)
        | PublicRelayError::ForwarderInitFailed(_)
        | PublicRelayError::UpstreamError => StatusCode::BAD_GATEWAY,
    }
}

fn public_error_response(error: PublicRelayError) -> Response {
    let status = public_error_status(&error);
    let mut response = (
        status,
        Json(json!({
            "detail": error.public_message(),
        })),
    )
        .into_response();
    // Same header set as the success path (`build_forward_response`) --
    // there's no reason a fixed, tiny JSON error body should skip the
    // CSP/nosniff headers the success path gets.
    apply_public_callback_security_headers(response.headers_mut());
    response
}

fn public_error_response_logged(
    error: PublicRelayError,
    started: Instant,
    machine_id: Option<&MachineId>,
    machine_raw: &str,
    method: &Method,
    source: Option<String>,
) -> Response {
    let status = public_error_status(&error);
    let machine_label = public_log_machine_label(machine_id, machine_raw);
    tracing::warn!(
        surface = "api",
        service = "oauth_relay",
        action = "callback",
        machine_id = machine_label.as_str(),
        method = %method,
        status = status.as_u16(),
        kind = error.kind(),
        source = source.as_deref(),
        elapsed_ms = started.elapsed().as_millis(),
        "public oauth callback relay rejected request"
    );
    public_error_response(error)
}

fn public_log_machine_label(machine_id: Option<&MachineId>, machine_raw: &str) -> String {
    machine_id.map(ToString::to_string).unwrap_or_else(|| {
        if machine_raw.is_empty() {
            "<missing>"
        } else {
            "<invalid>"
        }
        .into()
    })
}

fn body_read_error_to_public(error: &(dyn std::error::Error + 'static)) -> PublicRelayError {
    if error_is_length_limit(error) {
        PublicRelayError::BodyTooLarge
    } else {
        PublicRelayError::InvalidRequestBody(error.to_string())
    }
}

fn error_is_length_limit(error: &(dyn std::error::Error + 'static)) -> bool {
    if error.is::<http_body_util::LengthLimitError>() {
        return true;
    }
    let mut current = error.source();
    while let Some(source) = current {
        if source.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        current = source.source();
    }
    false
}

async fn list_machines(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let action = "oauth.relay.list";
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    let started = Instant::now();
    let result = admin_list_machines(&state).await;
    log_admin_read(action, request_id, started, &result);
    result.map(Json).map_err(ApiError)
}

async fn admin_list_machines(state: &AppState) -> Result<serde_json::Value, ToolError> {
    let manager = require_manager(state)?;
    Ok(json!({ "machines": manager.list().await }))
}

async fn get_machine(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let action = "oauth.relay.get";
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    let started = Instant::now();
    let result = admin_get_machine(&state, &machine_id).await;
    log_admin_read(action, request_id, started, &result);
    result.map(Json).map_err(ApiError)
}

async fn admin_get_machine(
    state: &AppState,
    machine_id: &str,
) -> Result<serde_json::Value, ToolError> {
    let manager = require_manager(state)?;
    let machine_id = MachineId::parse(machine_id).map_err(|error| error.to_tool_error())?;
    let entry = manager
        .entry(&machine_id)
        .await
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: "relay machine is not registered".into(),
        })?;
    Ok(json!({ "machine": crate::oauth::public_relay::PublicRelayMachineView::from_entry(&entry) }))
}

/// Standard dispatch log event for admin read endpoints (`list_machines`,
/// `get_machine`). Mutations already get equivalent coverage from
/// `audit_admin_mutation`; reads previously emitted nothing at the handler
/// level, which is inconsistent with `docs/dev/OBSERVABILITY.md` (every
/// user-visible action should emit one structured dispatch event).
fn log_admin_read(
    action: &'static str,
    request_id: Option<&str>,
    started: Instant,
    result: &Result<serde_json::Value, ToolError>,
) {
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(_) => tracing::info!(
            surface = "api",
            service = "oauth_relay",
            action,
            request_id,
            elapsed_ms,
            "oauth relay admin read complete"
        ),
        Err(error) if error.is_internal() => tracing::error!(
            surface = "api",
            service = "oauth_relay",
            action,
            request_id,
            elapsed_ms,
            kind = error.kind(),
            "oauth relay admin read failed"
        ),
        Err(error) => tracing::warn!(
            surface = "api",
            service = "oauth_relay",
            action,
            request_id,
            elapsed_ms,
            kind = error.kind(),
            "oauth relay admin read failed"
        ),
    }
}

async fn upsert_machine_without_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<UpsertMachineRequest>,
) -> Result<Json<MutationReport>, ApiError> {
    let action = "oauth.relay.upsert";
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    let machine_hint = body.machine_id.clone();
    audit_admin_mutation(
        action,
        request_id,
        auth.as_ref(),
        machine_hint.clone(),
        async move {
            let machine_id = body
                .machine_id
                .clone()
                .ok_or_else(|| ToolError::MissingParam {
                    message: "missing required parameter `machine_id`".into(),
                    param: "machine_id".into(),
                })?;
            upsert_machine(state, machine_id, body).await
        },
    )
    .await
}

async fn upsert_machine_with_path(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<UpsertMachineRequest>,
) -> Result<Json<MutationReport>, ApiError> {
    let action = "oauth.relay.upsert";
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    audit_admin_mutation(
        action,
        request_id,
        auth.as_ref(),
        Some(machine_id.clone()),
        async move { upsert_machine(state, machine_id, body).await },
    )
    .await
}

async fn remove_machine(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<MutationReport>, ApiError> {
    let action = "oauth.relay.remove";
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    audit_admin_mutation(
        action,
        request_id,
        auth.as_ref(),
        Some(machine_id.clone()),
        async move {
            let manager = require_manager(&state)?;
            let machine_id =
                MachineId::parse(&machine_id).map_err(|error| error.to_tool_error())?;
            let outcome = manager
                .remove(&machine_id)
                .await
                .map_err(|error| error.to_tool_error())?;
            Ok(mutation_response(outcome))
        },
    )
    .await
}

async fn disable_machine(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<MutationReport>, ApiError> {
    set_machine_disabled(state, headers, auth, machine_id, true).await
}

async fn enable_machine(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<MutationReport>, ApiError> {
    set_machine_disabled(state, headers, auth, machine_id, false).await
}

async fn import_registry(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let action = "oauth.relay.import";
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    audit_admin_mutation(action, request_id, auth.as_ref(), None, async move {
        let manager = require_manager(&state)?;
        let report = crate::oauth::public_relay::store::parse_registry_value(body)
            .map_err(|error| error.to_tool_error())?;
        let output_report = ImportReport {
            accepted: report.accepted.clone(),
            quarantined: report.quarantined.clone(),
            entries: Vec::new(),
        };
        let outcome = manager
            .import_report(report)
            .await
            .map_err(|error| error.to_tool_error())?;
        Ok(Json(json!({
            "report": output_report,
            "restart_required": false,
            "outcome": outcome,
        })))
    })
    .await
}

async fn upsert_machine(
    state: AppState,
    machine_id: String,
    body: UpsertMachineRequest,
) -> Result<Json<MutationReport>, ToolError> {
    let manager = require_manager(&state)?;
    let machine_id = MachineId::parse(&machine_id).map_err(|error| error.to_tool_error())?;
    let entry = PublicRelayEntry::new(
        machine_id,
        body.target_url,
        body.description,
        body.disabled.unwrap_or(false),
    );
    let outcome = manager
        .upsert(entry)
        .await
        .map_err(|error| error.to_tool_error())?;
    Ok(mutation_response(outcome))
}

async fn set_machine_disabled(
    state: AppState,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    machine_id: String,
    disabled: bool,
) -> Result<Json<MutationReport>, ApiError> {
    let action = if disabled {
        "oauth.relay.disable"
    } else {
        "oauth.relay.enable"
    };
    let request_id = request_id(&headers);
    require_lab_admin(action, request_id, auth.as_ref())?;
    audit_admin_mutation(
        action,
        request_id,
        auth.as_ref(),
        Some(machine_id.clone()),
        async move {
            let manager = require_manager(&state)?;
            let machine_id =
                MachineId::parse(&machine_id).map_err(|error| error.to_tool_error())?;
            let outcome = manager
                .set_disabled(&machine_id, disabled)
                .await
                .map_err(|error| error.to_tool_error())?;
            Ok(mutation_response(outcome))
        },
    )
    .await
}

async fn audit_admin_mutation<T, F>(
    action: &'static str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
    machine_id: Option<String>,
    operation: F,
) -> Result<T, ApiError>
where
    F: Future<Output = Result<T, ToolError>>,
{
    let actor_key = auth
        .and_then(|Extension(ctx)| ctx.actor_key.as_deref())
        .map(str::to_owned);
    let started = Instant::now();
    tracing::info!(
        surface = "api",
        service = "oauth_relay",
        action,
        request_id,
        actor_key = actor_key.as_deref(),
        machine_id = machine_id.as_deref(),
        "oauth relay admin mutation start"
    );
    let result = operation.await;
    let elapsed_ms = started.elapsed().as_millis();
    match &result {
        Ok(_) => tracing::info!(
            surface = "api",
            service = "oauth_relay",
            action,
            request_id,
            actor_key = actor_key.as_deref(),
            machine_id = machine_id.as_deref(),
            elapsed_ms,
            "oauth relay admin mutation complete"
        ),
        Err(error) if error.is_internal() => tracing::error!(
            surface = "api",
            service = "oauth_relay",
            action,
            request_id,
            actor_key = actor_key.as_deref(),
            machine_id = machine_id.as_deref(),
            elapsed_ms,
            kind = error.kind(),
            "oauth relay admin mutation failed"
        ),
        Err(error) => tracing::warn!(
            surface = "api",
            service = "oauth_relay",
            action,
            request_id,
            actor_key = actor_key.as_deref(),
            machine_id = machine_id.as_deref(),
            elapsed_ms,
            kind = error.kind(),
            "oauth relay admin mutation failed"
        ),
    }
    result.map_err(ApiError)
}

fn mutation_response(outcome: RegistryWriteOutcome) -> Json<MutationReport> {
    Json(MutationReport {
        restart_required: false,
        outcome,
    })
}

fn require_manager(state: &AppState) -> Result<Arc<PublicRelayRegistryManager>, ToolError> {
    state.public_relay.clone().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "relay_registry_unavailable".into(),
        message: format!(
            "public relay registry manager is not loaded at {}",
            PublicRelayRegistryStore::default_path().display()
        ),
    })
}

fn require_lab_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    match auth {
        None => {
            tracing::warn!(
                surface = "api",
                service = "oauth_relay",
                action,
                request_id,
                kind = "auth_failed",
                "oauth relay admin action rejected: authentication required"
            );
            Err(ToolError::Sdk {
                sdk_kind: "auth_failed".into(),
                message: "oauth relay admin API requires authentication".into(),
            })
        }
        Some(auth) if auth.0.scopes.iter().any(|scope| scope == "lab:admin") => Ok(()),
        Some(_) => {
            tracing::warn!(
                surface = "api",
                service = "oauth_relay",
                action,
                request_id,
                kind = "forbidden",
                "oauth relay admin action rejected: lab:admin scope required"
            );
            Err(ToolError::Sdk {
                sdk_kind: "forbidden".into(),
                message: "oauth relay admin API requires `lab:admin` scope".into(),
            })
        }
    }
}

fn request_id(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("x-request-id")?;
    match value.to_str() {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(
                surface = "api",
                service = "oauth_relay",
                error = %error,
                "x-request-id header is not valid UTF-8; treating as absent"
            );
            None
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpsertMachineRequest {
    #[serde(default)]
    machine_id: Option<String>,
    target_url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    disabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{HeaderValue, Request, header},
    };
    use tower::ServiceExt;
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    fn admin_auth_context() -> AuthContext {
        AuthContext {
            sub: "admin-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("admin@example.com".to_string()),
        }
    }

    fn read_only_auth_context() -> AuthContext {
        AuthContext {
            sub: "read-only-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("reader@example.com".to_string()),
        }
    }

    async fn test_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();
        (
            dir,
            AppState::new().with_public_relay_manager(Arc::new(manager)),
        )
    }

    #[tokio::test]
    async fn oauth_relay_admin_requires_lab_admin_scope() {
        let (_dir, state) = test_state().await;
        let app = admin_routes(state.clone()).with_state(state.clone());

        let unauthenticated = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/machines")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let read_only = admin_routes(state.clone())
            .layer(Extension(read_only_auth_context()))
            .with_state(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/machines")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read_only.status(), StatusCode::FORBIDDEN);

        let admin = admin_routes(state.clone())
            .layer(Extension(admin_auth_context()))
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/machines")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oauth_relay_admin_imports_valid_registry() {
        let (_dir, state) = test_state().await;
        let body = json!({
            "dookie": "http://100.88.16.79:38935/callback/dookie"
        });
        let response = admin_routes(state.clone())
            .layer(Extension(admin_auth_context()))
            .with_state(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/import")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn oauth_relay_admin_rejects_partial_import_without_replacing_registry() {
        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        manager
            .upsert(PublicRelayEntry::new(
                MachineId::parse("squirts").unwrap(),
                "http://100.75.111.118:38935/callback/squirts",
                None,
                false,
            ))
            .await
            .unwrap();
        let body = json!({
            "dookie": "http://100.88.16.79:38935/callback/dookie",
            "bad": "http://127.0.0.1:38935/callback/bad"
        });
        let response = admin_routes(state.clone())
            .layer(Extension(admin_auth_context()))
            .with_state(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/import")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            manager
                .entry(&MachineId::parse("squirts").unwrap())
                .await
                .is_some()
        );
        assert!(
            manager
                .entry(&MachineId::parse("dookie").unwrap())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn oauth_relay_admin_rejects_invalid_import_schema_as_caller_error() {
        let (_dir, state) = test_state().await;
        let response = admin_routes(state.clone())
            .layer(Extension(admin_auth_context()))
            .with_state(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/import")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"entries":"not-an-array"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn oauth_relay_admin_mutation_flow_updates_live_registry() {
        let (_dir, state) = test_state().await;
        let app = admin_routes(state.clone())
            .layer(Extension(admin_auth_context()))
            .with_state(state.clone());

        let create = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/machines")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"machine_id":"dookie","target_url":"http://100.88.16.79:38935/callback/dookie"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);

        let read = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/machines/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);

        let disable = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/machines/dookie/disable")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disable.status(), StatusCode::OK);

        let callback_while_disabled = public_routes(state.clone())
            .with_state(state.clone())
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/callback/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback_while_disabled.status(), StatusCode::NOT_FOUND);

        let enable = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/machines/dookie/enable")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(enable.status(), StatusCode::OK);

        let remove = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/machines/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(remove.status(), StatusCode::OK);

        let remove_unknown = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/machines/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(remove_unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn public_callback_bypasses_bearer_auth_and_protected_intercept() {
        let state = AppState::new();
        let app =
            crate::api::router::build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/callback/dookie?code=abc&state=secret-state")
                    .header("x-forwarded-host", "callback.tootie.tv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn public_callback2_does_not_match_and_healthz_returns_json() {
        let state = AppState::new();
        let app =
            crate::api::router::build_router_with_bearer(state, Some("secret-token".into()), None);

        let callback2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/callback2/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback2.status(), StatusCode::NOT_FOUND);

        let healthz = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(healthz.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            healthz.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    #[tokio::test]
    async fn public_healthz_stays_shallow_and_does_not_read_disk() {
        // lab-k96pn: /healthz must report from the already-validated in-memory
        // manager snapshot only. Corrupting the on-disk registry file must have
        // no effect on the public health response — proving no disk I/O happens
        // on this unauthenticated hot path.
        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        manager
            .upsert(PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            ))
            .await
            .unwrap();
        std::fs::write(manager.store().path(), "{not valid json").unwrap();
        let response = public_routes(state.clone())
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["registry"], "loaded");
        assert_eq!(value["machines"], 1);
    }

    #[test]
    fn public_callback_error_status_mapping_is_explicit() {
        assert_eq!(
            public_error_status(&PublicRelayError::ResponseTooLarge),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            public_error_status(&PublicRelayError::UpstreamTimeout),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            public_error_status(&PublicRelayError::UpstreamError),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            public_error_status(&PublicRelayError::RegistryUnavailable("missing".into())),
            StatusCode::BAD_GATEWAY
        );
    }

    #[tokio::test]
    async fn public_callback_rejects_large_content_length_before_registry_lookup() {
        let state = AppState::new();
        let app =
            crate::api::router::build_router_with_bearer(state, Some("secret-token".into()), None);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/callback/dookie")
                    .header(header::CONTENT_LENGTH, "65537")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn public_callback_maps_unknown_disabled_and_large_query_failures() {
        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        manager
            .upsert(PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                true,
            ))
            .await
            .unwrap();
        let app = public_routes(state.clone()).with_state(state);

        let unknown = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/callback/squirts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

        let disabled = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/callback/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disabled.status(), StatusCode::NOT_FOUND);

        let large_query = "a".repeat(PUBLIC_QUERY_LIMIT_BYTES + 1);
        let too_large = app
            .oneshot(
                Request::builder()
                    .uri(format!("/callback/dookie?{large_query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn public_callback_rejects_chunked_body_without_content_length_via_streaming_backstop() {
        // `public_callback_rejects_large_content_length_before_registry_lookup`
        // only exercises the `Content-Length` header fast path. A client
        // that streams a body without declaring `Content-Length` (chunked
        // transfer, or any stream axum can't size up front) bypasses that
        // header check entirely -- this test proves the real backstop,
        // `axum::body::to_bytes(body, PUBLIC_REQUEST_BODY_LIMIT_BYTES)`,
        // still catches it, and that `error_is_length_limit`'s error
        // source-chain walk correctly recognizes the resulting
        // `LengthLimitError` so it maps to `BodyTooLarge` (413) rather than
        // falling through to the generic `InvalidRequestBody` (400) path.
        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        manager
            .upsert(PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            ))
            .await
            .unwrap();
        let app = public_routes(state.clone()).with_state(state);

        let chunk = Bytes::from(vec![0u8; 4096]);
        let chunk_count = PUBLIC_REQUEST_BODY_LIMIT_BYTES / 4096 + 4;
        let body_stream = futures::stream::iter(
            (0..chunk_count).map(move |_| Ok::<_, std::io::Error>(chunk.clone())),
        );
        let request = Request::builder()
            .method("POST")
            .uri("/callback/dookie")
            .body(Body::from_stream(body_stream))
            .unwrap();
        assert!(
            request.headers().get(header::CONTENT_LENGTH).is_none(),
            "test body must stream without a Content-Length header"
        );

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn public_callback_returns_429_when_machine_forward_permits_are_exhausted() {
        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        let machine = MachineId::parse("dookie").unwrap();
        manager
            .upsert(PublicRelayEntry::new(
                machine.clone(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            ))
            .await
            .unwrap();
        let _permit_one = manager.acquire_forward_permit(&machine).await.unwrap();
        let _permit_two = manager.acquire_forward_permit(&machine).await.unwrap();
        let app = public_routes(state.clone()).with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/callback/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn public_forwarded_success_forces_safe_cache_and_content_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("public"));
        headers.insert(header::PRAGMA, HeaderValue::from_static("cache"));
        headers.insert(
            header::EXPIRES,
            HeaderValue::from_static("Wed, 01 Jan 3000 00:00:00 GMT"),
        );
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));

        let response = build_forward_response(
            StatusCode::OK,
            &headers,
            Bytes::from_static(b"<html></html>"),
        );

        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert_eq!(response.headers().get(header::PRAGMA).unwrap(), "no-cache");
        assert_eq!(response.headers().get(header::EXPIRES).unwrap(), "0");
        assert_eq!(
            response
                .headers()
                .get(HeaderName::from_static("x-content-type-options"))
                .unwrap(),
            "nosniff"
        );
        assert!(
            response
                .headers()
                .get(HeaderName::from_static("content-security-policy"))
                .unwrap()
                .to_str()
                .unwrap()
                .contains("default-src 'none'")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn relay_observability_callback_logs_do_not_include_code_state_or_target_url() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);

        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        manager
            .upsert(PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            ))
            .await
            .unwrap();
        let app =
            crate::api::router::build_router_with_bearer(state, Some("secret-token".into()), None);
        drop(
            app.oneshot(
                Request::builder()
                    .uri("/callback/dookie?code=abc&state=secret-state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        );

        drop(_guard);
        let logs = crate::test_support::captured_logs(&buf);
        assert!(!logs.contains("code=abc"), "{logs}");
        assert!(!logs.contains("secret-state"), "{logs}");
        assert!(
            !logs.contains("http://100.88.16.79:38935/callback/dookie"),
            "{logs}"
        );
    }

    // lab-uvscv: admin read endpoints (`list_machines`, `get_machine`) must
    // emit a dispatch log event, matching mutation coverage from
    // `audit_admin_mutation` and the observability contract that every
    // user-visible action logs surface/service/action/elapsed_ms.
    #[tokio::test(flavor = "current_thread")]
    async fn oauth_relay_admin_reads_emit_dispatch_log_events() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = crate::test_support::SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );
        let _guard = tracing::subscriber::set_default(subscriber);

        let (_dir, state) = test_state().await;
        let manager = state.public_relay.as_ref().unwrap().clone();
        manager
            .upsert(PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            ))
            .await
            .unwrap();
        let app = admin_routes(state.clone())
            .layer(Extension(admin_auth_context()))
            .with_state(state);

        let list = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/machines")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);

        let get = app
            .oneshot(
                Request::builder()
                    .uri("/machines/dookie")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);

        drop(_guard);
        let logs = crate::test_support::captured_logs(&buf);
        assert!(
            logs.contains("\"action\":\"oauth.relay.list\"")
                && logs.contains("oauth relay admin read complete"),
            "expected list_machines dispatch log, got: {logs}"
        );
        assert!(
            logs.contains("\"action\":\"oauth.relay.get\"")
                && logs.contains("oauth relay admin read complete"),
            "expected get_machine dispatch log, got: {logs}"
        );
    }
}
