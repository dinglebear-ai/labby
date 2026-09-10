//! Authenticated, streaming HTTP adapter for the principal-scoped File Stash.

use axum::{
    Json,
    body::Body,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use futures::TryStreamExt;
use labby_auth::{AuthContext, VerifiedIdentity};
use serde::Deserialize;
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::{
    io::{ReaderStream, StreamReader},
    sync::CancellationToken,
};
use tracing::Instrument as _;

use crate::{
    api::{
        error::{ApiError, ToolError},
        route_registry::{RouteAuth, RouteDescriptor, RouteGroup},
        state::AppState,
    },
    dispatch::file_stash::FileStashService,
};

pub fn routes(_state: AppState) -> RouteGroup {
    descriptors()
        .into_iter()
        .fold(RouteGroup::empty(), |group, descriptor| {
            let method = match (descriptor.method, descriptor.path.as_str()) {
                ("GET", "/") => get(list),
                ("POST", "/") => post(action),
                ("GET", "/stats") => get(stats),
                ("POST", "/recipients") => post(recipients),
                ("POST", "/uploads") => post(upload),
                ("GET", "/files/{file_id}") => get(metadata),
                ("GET", "/files/{file_id}/content") => get(download),
                ("PATCH", "/files/{file_id}") => patch(rename),
                ("DELETE", "/files/{file_id}") => delete(remove),
                ("POST", "/files/{file_id}/grants") => post(create_grant),
                ("GET", "/files/{file_id}/grants") => get(list_grants),
                ("DELETE", "/files/{file_id}/grants/{grant_id}") => delete(revoke_grant),
                _ => unreachable!("descriptor and route table must stay aligned"),
            };
            group.route(descriptor, method)
        })
}

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    [
        ("GET", "/", "stash_list", "none_expected"),
        ("POST", "/", "stash_action", "action-defined"),
        ("GET", "/stats", "stash_stats", "none_expected"),
        (
            "POST",
            "/recipients",
            "stash_recipients",
            "directory lookup",
        ),
        ("POST", "/uploads", "stash_upload", "creates a file"),
        ("GET", "/files/{file_id}", "stash_metadata", "none_expected"),
        (
            "GET",
            "/files/{file_id}/content",
            "stash_download",
            "none_expected",
        ),
        (
            "PATCH",
            "/files/{file_id}",
            "stash_rename",
            "renames a file",
        ),
        (
            "DELETE",
            "/files/{file_id}",
            "stash_delete",
            "deletes a file and grants",
        ),
        (
            "POST",
            "/files/{file_id}/grants",
            "stash_grant_create",
            "creates a read grant",
        ),
        (
            "GET",
            "/files/{file_id}/grants",
            "stash_grant_list",
            "none_expected",
        ),
        (
            "DELETE",
            "/files/{file_id}/grants/{grant_id}",
            "stash_grant_revoke",
            "revokes a grant",
        ),
    ]
    .into_iter()
    .map(|(method, path, handler, effects)| {
        RouteDescriptor::new(method, path, handler, "stash", RouteAuth::V1)
            .when("Linux with API auth configured; operations require runtime readiness")
            .private_no_store()
            .non_enumerating()
            .side_effects(effects)
    })
    .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageQuery {
    cursor: Option<String>,
    limit: Option<usize>,
    query: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerQuery {
    owner_kind: Option<String>,
    owner_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipientQuery {
    query: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameRequest {
    display_name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRequest {
    grantee_principal_id: String,
}

macro_rules! observed_handler {
    ($name:ident, $inner:ident, $action:literal, $destructive:literal, ($($arg:ident : $ty:ty),* $(,)?)) => {
        async fn $name($($arg: $ty),*) -> Result<Response, ApiError> {
            observe_api($action, None, None, $destructive, $inner($($arg),*)).await
        }
    };
}

async fn action(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    request: Result<Json<crate::api::ActionRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let action = request
        .as_ref()
        .map_or("stash.action", |request| request.action.as_str())
        .to_owned();
    let destructive = action == "stash.delete";
    observe_api(&action, None, None, destructive, async move {
        let request = request.map_err(|_| stable("invalid_param"))?;
        action_impl(state, headers, auth, identity, request).await
    })
    .await
}

async fn list(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let action = if query.as_ref().is_ok_and(|query| query.query.is_some()) {
        "stash.search"
    } else {
        "stash.list"
    };
    observe_api(action, None, None, false, async move {
        let query = query.map_err(|_| stable("invalid_param"))?;
        list_impl(state, headers, auth, identity, query).await
    })
    .await
}
observed_handler!(stats, stats_impl, "stash.stats", false, (
    state: State<AppState>, headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
));
async fn recipients(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    query: Result<Json<RecipientQuery>, JsonRejection>,
) -> Result<Response, ApiError> {
    observe_api("stash.recipients.search", None, None, false, async move {
        recipients_impl(
            state,
            headers,
            auth,
            identity,
            query.map_err(|_| stable("invalid_param"))?,
        )
        .await
    })
    .await
}
async fn metadata(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.metadata",
        Some(&object_id),
        None,
        false,
        metadata_impl(state, headers, auth, identity, file_id),
    )
    .await
}
async fn rename(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
    body: Result<Json<RenameRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api("stash.rename", Some(&object_id), None, false, async move {
        rename_impl(
            state,
            headers,
            auth,
            identity,
            file_id,
            body.map_err(|_| stable("invalid_param"))?,
        )
        .await
    })
    .await
}
async fn remove(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.delete",
        Some(&object_id),
        None,
        true,
        remove_impl(state, headers, auth, identity, file_id),
    )
    .await
}
async fn create_grant(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
    body: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.grants.create",
        Some(&object_id),
        None,
        false,
        async move {
            create_grant_impl(
                state,
                headers,
                auth,
                identity,
                file_id,
                body.map_err(|_| stable("invalid_param"))?,
            )
            .await
        },
    )
    .await
}
async fn list_grants(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    file_id: Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let object_id = file_id.0.clone();
    observe_api(
        "stash.grants.list",
        Some(&object_id),
        None,
        false,
        async move {
            list_grants_impl(
                state,
                headers,
                auth,
                identity,
                file_id,
                query.map_err(|_| stable("invalid_param"))?,
            )
            .await
        },
    )
    .await
}
async fn revoke_grant(
    state: State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    path: Path<(String, String)>,
) -> Result<Response, ApiError> {
    let object_id = path.0.0.clone();
    let grant_id = path.0.1.clone();
    observe_api(
        "stash.grants.revoke",
        Some(&object_id),
        Some(&grant_id),
        false,
        revoke_grant_impl(state, headers, auth, identity, path),
    )
    .await
}
observed_handler!(upload, upload_impl, "stash.upload", false, (
    state: State<AppState>, headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>, body: Body,
));
async fn action_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Json(request): Json<crate::api::ActionRequest>,
) -> Result<Response, ApiError> {
    if matches!(
        request.action.as_str(),
        "stash.rename" | "stash.delete" | "stash.grants.create" | "stash.grants.revoke"
    ) {
        mutation_csrf(&headers, auth.as_ref(), &request.action)?;
    }
    let action = request.action;
    let recipient_identity = identity.clone();
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        request
            .params
            .get("owner_kind")
            .and_then(serde_json::Value::as_str),
        request
            .params
            .get("owner_id")
            .and_then(serde_json::Value::as_str),
        &action,
    )
    .await?;
    let validated_grantee = if action == "stash.grants.create" {
        let recipient = request
            .params
            .get("grantee_principal_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| stable("invalid_param"))?
            .to_owned();
        let (_owner, recipient, lease) =
            principal_and_recipient(&state, recipient_identity, recipient).await?;
        Some((recipient, lease))
    } else {
        None
    };
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let response = crate::dispatch::file_stash::dispatch_for_principal(
        &service(&state),
        &principal,
        "api",
        &action,
        request.params,
        validated_grantee
            .as_ref()
            .map(|(recipient, _lease)| recipient),
    )
    .await
    .map_err(|error| ApiError::new(error).with_service_action("stash", &action))?;
    Ok(result(response))
}

async fn principal_and_recipient(
    state: &AppState,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    recipient: String,
) -> Result<
    (
        crate::access::AccessPrincipalId,
        crate::access::AccessPrincipalId,
        crate::access::ActiveFileStashPrincipalLease,
    ),
    ApiError,
> {
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    state
        .access_runtime
        .resolve_and_lease_file_stash_participants(identity, recipient)
        .await
        .map_err(map_principal_error)
}

fn map_principal_error(error: crate::access::FileStashPrincipalResolutionError) -> ApiError {
    match error {
        crate::access::FileStashPrincipalResolutionError::IdentityUnavailable => {
            stable("not_found")
        }
        crate::access::FileStashPrincipalResolutionError::StoreUnavailable
        | crate::access::FileStashPrincipalResolutionError::Runtime(_) => {
            stable("service_unavailable")
        }
    }
}

fn service(state: &AppState) -> FileStashService {
    FileStashService::new(
        state.file_stash_runtime.clone(),
        state.access_runtime.clone(),
        usize::from(state.config.file_stash.page_size),
        state.config.file_stash.max_query_bytes,
    )
}

async fn observe_api<T>(
    action: &str,
    object_id: Option<&str>,
    grant_id: Option<&str>,
    destructive: bool,
    future: impl Future<Output = Result<T, ApiError>>,
) -> Result<T, ApiError> {
    let started = std::time::Instant::now();
    let (result, details) = crate::dispatch::file_stash::collect_observation_details(future).await;
    crate::dispatch::file_stash::observe_operation(
        "api",
        action,
        if result.is_ok() { "success" } else { "error" },
        details.object_id.as_deref().or(object_id),
        details.grant_id.as_deref().or(grant_id),
        details.byte_count,
        destructive,
        u64::try_from(started.elapsed().as_millis())
            .unwrap_or(u64::MAX)
            .max(1),
        result.as_ref().err().map(|error| error.error.kind()),
    );
    result
}

/// Resolve the caller's selected owner scope and authorize `action` in it.
/// The selection (params, query, or `x-labby-owner-*` headers) never grants
/// authority; the verified identity and durable roles decide.
async fn selected_principal(
    state: &AppState,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    auth: Option<&axum::Extension<AuthContext>>,
    kind: Option<&str>,
    owner_id: Option<&str>,
    action: &str,
) -> Result<crate::access::FileStashOwnerAuthorization, ApiError> {
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    let ceiling = auth.map_or_else(crate::access::AuthorityCeiling::trusted_local, |auth| {
        crate::access::AuthorityCeiling::from_auth_context(&auth.0)
    });
    crate::dispatch::file_stash::authorize_owner(
        &state.access_runtime,
        identity,
        ceiling,
        kind,
        owner_id,
        action,
    )
    .await
    .map_err(|error| ApiError::new(error).with_service_action("stash", action))
}

fn selected_owner_headers(headers: &HeaderMap) -> (Option<&str>, Option<&str>) {
    (
        headers
            .get("x-labby-owner-kind")
            .and_then(|value| value.to_str().ok()),
        headers
            .get("x-labby-owner-id")
            .and_then(|value| value.to_str().ok()),
    )
}

fn stable(kind: &str) -> ApiError {
    ApiError::new(ToolError::Sdk {
        sdk_kind: kind.to_owned(),
        message: "File Stash operation failed".to_owned(),
    })
    .with_service_action("stash", "stash.http")
}
fn result<T: serde::Serialize>(value: T) -> Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
fn mutation_csrf(
    headers: &HeaderMap,
    auth: Option<&axum::Extension<AuthContext>>,
    action: &str,
) -> Result<(), ApiError> {
    crate::api::services::require_session_csrf(action, headers, auth.map(|v| &v.0))
        .map_err(ApiError::from)
}

async fn list_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let action = if q.query.is_some() {
        "stash.search"
    } else {
        "stash.list"
    };
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(&state, identity, auth.as_ref(), kind, id, action).await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let page = if let Some(query) = q.query {
        let stash = service(&state);
        let page = crate::dispatch::file_stash::observe_result(
            "api",
            "stash.search",
            None,
            None,
            None,
            false,
            stash.search(&principal, &query, q.cursor.as_deref(), q.limit),
        )
        .await?;
        crate::dispatch::file_stash::capture_observation_details(None, None, None);
        page
    } else {
        let stash = service(&state);
        let page = crate::dispatch::file_stash::observe_result(
            "api",
            "stash.list",
            None,
            None,
            None,
            false,
            stash.list(&principal, q.cursor.as_deref(), q.limit),
        )
        .await?;
        crate::dispatch::file_stash::capture_observation_details(None, None, None);
        page
    };
    Ok(result(page))
}
async fn stats_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.stats").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let stash = service(&state);
    let stats = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.stats",
        None,
        None,
        None,
        false,
        stash.stats(&principal),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        None,
        None,
        Some(stats.owned_committed_bytes),
    );
    Ok(result(stats))
}
async fn recipients_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Json(q): Json<RecipientQuery>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.recipients.search")?;
    if !auth
        .as_ref()
        .is_some_and(|context| context.0.scopes.iter().any(|scope| scope == "lab:admin"))
    {
        return Err(stable("not_found"));
    }
    let Some(axum::Extension(identity)) = identity else {
        return Err(stable("not_found"));
    };
    // Recipient search performs its own bounded AccessStore operation. Resolve
    // the caller without retaining the connection-admission lease so the
    // search can acquire it and install a cancellable SQLite deadline.
    let principal = state
        .access_runtime
        .resolve_file_stash_principal(identity)
        .await
        .map_err(map_principal_error)?;
    let query = q.query.trim();
    if query.chars().count() < 3 || query.len() > 128 {
        return Err(stable("invalid_param"));
    }
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| stable("service_unavailable"))?;
    let values = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.recipients.search",
        None,
        None,
        None,
        false,
        async {
            store
                .search_file_stash_recipients(
                    principal,
                    query.to_owned(),
                    20,
                    std::time::Duration::from_millis(state.config.file_stash.database_deadline_ms),
                )
                .await
                .map_err(|error| ToolError::Sdk {
                    sdk_kind: if error.to_string().contains("deadline exceeded") {
                        "busy"
                    } else {
                        "service_unavailable"
                    }
                    .into(),
                    message: "File Stash operation failed".into(),
                })
        },
    )
    .await?;
    Ok(result(serde_json::json!({"recipients": values})))
}
async fn metadata_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.metadata").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let stash = service(&state);
    let file = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.metadata",
        Some(&file_id),
        None,
        None,
        false,
        stash.metadata(&principal, &file_id),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        Some(&file_id),
        None,
        Some(file.size_bytes),
    );
    Ok(result(file))
}
async fn rename_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Json(body): Json<RenameRequest>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.rename")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.rename").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let stash = service(&state);
    let file = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.rename",
        Some(&file_id),
        None,
        None,
        false,
        stash.rename(&principal, &file_id, &body.display_name),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        Some(&file_id),
        None,
        Some(file.size_bytes),
    );
    Ok(result(file))
}
async fn remove_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.delete")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.delete").await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let stash = service(&state);
    crate::dispatch::file_stash::observe_result(
        "api",
        "stash.delete",
        Some(&file_id),
        None,
        None,
        true,
        stash.delete(&principal, &file_id),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), None, None);
    Ok(StatusCode::NO_CONTENT.into_response())
}
async fn create_grant_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Json(body): Json<GrantRequest>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.grants.create")?;
    let recipient_identity = identity.clone();
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        kind,
        id,
        "stash.grants.create",
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let (_owner, grantee, _lease) =
        principal_and_recipient(&state, recipient_identity, body.grantee_principal_id).await?;
    let stash = service(&state);
    let grant = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.grants.create",
        Some(&file_id),
        None,
        None,
        false,
        stash.create_grant_validated(&principal, &file_id, &grantee),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(
        Some(&file_id),
        Some(&grant.grant_id),
        None,
    );
    Ok((StatusCode::CREATED, result(grant)).into_response())
}
async fn list_grants_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path(file_id): Path<String>,
    Query(q): Query<PageQuery>,
) -> Result<Response, ApiError> {
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        kind,
        id,
        "stash.grants.list",
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let stash = service(&state);
    let grants = crate::dispatch::file_stash::observe_result(
        "api",
        "stash.grants.list",
        Some(&file_id),
        None,
        None,
        false,
        stash.grants(&principal, &file_id, q.cursor.as_deref(), q.limit),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), None, None);
    Ok(result(grants))
}
async fn revoke_grant_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    Path((file_id, grant_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    mutation_csrf(&headers, auth.as_ref(), "stash.grants.revoke")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal = selected_principal(
        &state,
        identity,
        auth.as_ref(),
        kind,
        id,
        "stash.grants.revoke",
    )
    .await?;
    principal
        .validate_before_commit()
        .await
        .map_err(map_principal_error)?;
    let stash = service(&state);
    crate::dispatch::file_stash::observe_result(
        "api",
        "stash.grants.revoke",
        Some(&file_id),
        Some(&grant_id),
        None,
        false,
        stash.revoke_grant(&principal, &file_id, &grant_id),
    )
    .await?;
    crate::dispatch::file_stash::capture_observation_details(Some(&file_id), Some(&grant_id), None);
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn upload_impl(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<axum::Extension<AuthContext>>,
    identity: Option<axum::Extension<VerifiedIdentity>>,
    body: Body,
) -> Result<Response, ApiError> {
    validate_header_budget(&headers, state.config.file_stash.max_header_bytes)?;
    mutation_csrf(&headers, auth.as_ref(), "stash.upload")?;
    let (kind, id) = selected_owner_headers(&headers);
    let principal =
        selected_principal(&state, identity, auth.as_ref(), kind, id, "stash.upload").await?;
    let display_name = headers
        .get("x-labby-stash-filename")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .ok()
        })
        .map(|value| value.into_owned())
        .ok_or_else(|| stable("validation_failed"))?;
    let declared = exact_content_length(&headers)?;
    crate::dispatch::file_stash::capture_observation_details(None, None, Some(declared));
    validate_transfer_headers(&headers)?;
    let svc = service(&state);
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    let cancel = CancellationToken::new();
    let mut guard = CancelOnDrop(Some(cancel.clone()));
    let owner: crate::access::AccessPrincipalId = (*principal).clone();
    // Own the complete side-effect transaction in one spawned task. Reserving,
    // finalizing, final authority validation, and compensating cleanup must all
    // outlive the cancellable HTTP request future; otherwise a disconnect after
    // the file commit can skip the revocation-at-commit check and leave a file
    // behind under authority the caller no longer holds.
    let upload = tokio::spawn(async move {
        let (reservation, admission) = svc.reserve_upload(&owner, &display_name, declared).await?;
        let file_id = svc
            .finalize_upload(reservation, admission, reader, cancel)
            .await?;
        if let Err(error) = principal.validate_before_commit().await {
            if let Err(cleanup) = svc.delete(&principal, &file_id).await {
                tra