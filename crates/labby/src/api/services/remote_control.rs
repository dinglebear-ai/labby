//! Thin HTTP adapters for remote Artifact control-plane services.

use std::{net::SocketAddr, sync::LazyLock};

use axum::{
    Extension, Json,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, header},
    routing::{post, put},
};
use futures::StreamExt as _;
use labby_auth::VerifiedIdentity;
use serde::Deserialize;
use serde_json::Value;
use sha2::Digest as _;
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};
use tokio_util::io::ReaderStream;

use crate::access::Permission;
use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;

const MAX_UPLOAD_BYTES: usize = 50_000_000;
static UPLOAD_ADMISSION: LazyLock<tokio::sync::Semaphore> =
    LazyLock::new(|| tokio::sync::Semaphore::new(16));

pub fn routes(service: &'static str, _state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let handler = match service {
        "sources" => post(handle_sources),
        "jobs" => post(handle_jobs),
        "uploads" => post(handle_uploads),
        "bundles" => post(handle_bundles),
        _ => return RouteGroup::empty(),
    };
    let mut route_descriptors = descriptors(service).into_iter();
    let group = RouteGroup::empty().route(route_descriptors.next().unwrap(), handler);
    if service == "uploads" {
        group.route(
            route_descriptors.next().unwrap(),
            put(upload_bytes).layer(axum::extract::DefaultBodyLimit::max(50_000_000)),
        )
    } else {
        group
    }
}

pub(crate) fn descriptors(
    service: &'static str,
) -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    let mut descriptors = vec![
        RouteDescriptor::new("POST", "/", "handle", service, RouteAuth::V1)
            .feature("skills")
            .when("mounted only when API authentication is configured")
            .host_validated()
            .private_no_store(),
    ];
    if service == "uploads" {
        descriptors.push(
            RouteDescriptor::new("PUT", "/{id}", "upload_bytes", service, RouteAuth::V1)
                .feature("skills")
                .when("mounted only when API authentication is configured")
                .host_validated()
                .private_no_store()
                .side_effects("stores bounded bytes in a principal-bound remote upload slot"),
        );
    }
    descriptors
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    connection_id: Option<String>,
}

async fn upload_bytes(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    let is_admin = auth
        .as_ref()
        .is_some_and(|Extension(auth)| auth.scopes.iter().any(|scope| scope == "lab:admin"));
    if !is_admin {
        return Err(ToolError::Forbidden {
            message: "Artifact uploads require lab:admin scope".to_owned(),
            required_scopes: vec!["lab:admin".to_owned()],
        }
        .into());
    }
    require_session_csrf(
        "uploads.put",
        &headers,
        auth.as_ref().map(|Extension(auth)| auth),
    )?;
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if content_length.is_some_and(|length| length > MAX_UPLOAD_BYTES as u64) {
        return Err(ToolError::InvalidParam {
            message: "Artifact upload exceeds 50000000 bytes".to_owned(),
            param: "body".to_owned(),
        }
        .into());
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream");
    let project_id = headers
        .get("x-labby-project-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ToolError::Forbidden {
            message: "Artifact uploads require project context".to_owned(),
            required_scopes: vec!["lab:admin".to_owned()],
        })?;
    let selected_team_id = selected_team_id_header(&headers)?;
    let context = authorize_authority_context(
        &state.access_runtime,
        identity.map(|Extension(identity)| identity),
        Some(project_id),
        selected_team_id,
        Permission::ProjectManage,
    )
    .await?;
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    let controls =
        crate::dispatch::skill_library::process_controls().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Remote Artifact control plane is unavailable".to_owned(),
        })?;
    let _admission = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        UPLOAD_ADMISSION.acquire(),
    )
    .await
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "queue_saturated".to_owned(),
        message: "Artifact upload queue is saturated".to_owned(),
    })?
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "source_unavailable".to_owned(),
        message: "Artifact upload queue is unavailable".to_owned(),
    })?;
    tracing::info!(surface = "api", service = "uploads", action = "uploads.put", request_id, actor_id = %context.actor_id, project_id = %context.project_id, "remote upload started");
    // Spool-then-send: the delegated upload assertion binds the full SHA-256
    // digest and exact length into the outbound request headers, so the whole
    // body must be seen before the first outbound byte. Spooling to a private
    // temp file keeps that requirement without holding up to 50 MB in RAM per
    // in-flight upload.
    let spooled = spool_upload_body(body, MAX_UPLOAD_BYTES).await?;
    let actual_length = spooled.length;
    verify_declared_length(content_length, actual_length)?;
    let content_digest = spooled.digest.clone();
    let result = controls
        .upload(
            query.connection_id.as_deref(),
            &id,
            spooled.into_body(),
            Some(actual_length),
            content_type,
            &content_digest,
            &context,
        )
        .await;
    match &result {
        Ok(_) => {
            tracing::info!(surface = "api", service = "uploads", action = "uploads.put", request_id, actor_id = %context.actor_id, project_id = %context.project_id, "remote upload completed")
        }
        Err(error) => {
            tracing::warn!(surface = "api", service = "uploads", action = "uploads.put", request_id, actor_id = %context.actor_id, project_id = %context.project_id, kind = error.kind(), "remote upload failed")
        }
    }
    result.map(Json).map_err(Into::into)
}

async fn handle_sources(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("sources", state, peer, headers, auth, identity, body).await
}
async fn handle_jobs(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("jobs", state, peer, headers, auth, identity, body).await
}
async fn handle_uploads(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("uploads", state, peer, headers, auth, identity, body).await
}
async fn handle_bundles(
    state: State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    body: Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    handle("bundles", state, peer, headers, auth, identity, body).await
}

async fn handle(
    service: &'static str,
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let identity = identity.map(|Extension(identity)| identity);
    let project_id = headers
        .get("x-labby-project-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // Parsed here, surfaced inside the dispatch closure so an invalid header
    // takes the same service/action error path as any other dispatch failure.
    let selected_team_id = selected_team_id_header(&headers).map(|value| value.map(str::to_owned));
    let request_headers = headers.clone();
    let request_auth = auth.clone();
    handle_action_with_meta(
        service,
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::remote_control::actions(service),
        move |action, params| async move {
            let spec = crate::dispatch::remote_control::actions(service)
                .iter()
                .find(|candidate| candidate.name == action);
            let operation = crate::dispatch::remote_control::operation(service, &action)
                .ok_or_else(|| ToolError::UnknownAction {
                    message: format!("Unknown action: {action}"),
                    valid: Vec::new(),
                    hint: None,
                })?;
            let permission = crate::dispatch::artifact_control::operation_permission(operation);
            let selected_team_id = selected_team_id?;
            if spec.is_some_and(|spec| spec.requires_admin) {
                require_session_csrf(
                    &action,
                    &request_headers,
                    request_auth.as_ref().map(|Extension(auth)| auth),
                )?;
            }
            let context = authorize_authority_context(
                &state.access_runtime,
                identity,
                project_id.as_deref(),
                selected_team_id.as_deref(),
                permission,
            )
            .await?;
            crate::dispatch::remote_control::dispatch_with_context(
                service,
                &action,
                params,
                Some(&context),
            )
            .await
        },
    )
    .await
}

/// Read the optional `x-labby-team-id` header, rejecting values that are not
/// valid visible ASCII instead of silently dropping the team context.
fn selected_team_id_header(headers: &HeaderMap) -> Result<Option<&str>, ToolError> {
    headers
        .get("x-labby-team-id")
        .map(|value| {
            value.to_str().map_err(|_| ToolError::InvalidParam {
                message: "team context header is invalid".to_owned(),
                param: "x-labby-team-id".to_owned(),
            })
        })
        .transpose()
}

/// A request body spooled to a private, unlinked temp file together with the
/// exact byte length and `sha256:<hex>` digest observed while spooling.
///
/// The temp file is removed by the OS when the handle is dropped, including
/// when the request future is cancelled or the body errors mid-stream.
#[derive(Debug)]
struct SpooledUpload {
    file: tokio::fs::File,
    length: u64,
    digest: String,
}

impl SpooledUpload {
    /// Stream the spooled bytes back out as an outbound request body.
    fn into_body(self) -> reqwest::Body {
        reqwest::Body::wrap_stream(ReaderStream::new(self.file))
    }
}

/// Read `body` chunk by chunk, enforcing `max_bytes` incrementally, hashing as
/// it goes, and spooling the bytes to a private temp file.
///
/// The cap is checked before each chunk is written, so an oversized stream
/// fails as soon as the running total would exceed the limit and never lands
/// on disk in full.
async fn spool_upload_body(body: Body, max_bytes: usize) -> Result<SpooledUpload, ToolError> {
    let spool = tokio::task::spawn_blocking(tempfile::tempfile)
        .await
        .map_err(|_| spool_unavailable())?
        .map_err(|_| spool_unavailable())?;
    let mut file = tokio::fs::File::from_std(spool);
    let mut hasher = sha2::Sha256::new();
    let mut total = 0usize;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ToolError::InvalidParam {
            message: "Artifact upload body could not be read".to_owned(),
            param: "body".to_owned(),
        })?;
        total = total
            .checked_add(chunk.len())
            .filter(|running| *running <= max_bytes)
            .ok_or_else(|| ToolError::InvalidParam {
                message: format!("Artifact upload exceeds {max_bytes} bytes"),
                param: "body".to_owned(),
            })?;
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|_| spool_unavailable())?;
    }
    file.flush().await.map_err(|_| spool_unavailable())?;
    file.rewind().await.map_err(|_| spool_unavailable())?;
    let length = u64::try_from(total).map_err(|_| ToolError::InvalidParam {
        message: "Artifact upload length is invalid".to_owned(),
        param: "body".to_owned(),
    })?;
    Ok(SpooledUpload {
        file,
        length,
        digest: format!("sha256:{}", hex::encode(hasher.finalize())),
    })
}

/// Reject a body whose observed length differs from the declared
/// `content-length`, when one was declared.
fn verify_declared_length(declared: Option<u64>, actual: u64) -> Result<(), ToolError> {
    if declared.is_some_and(|declared| declared != actual) {
        return Err(ToolError::InvalidParam {
            message: "Artifact upload content length does not match its body".to_owned(),
            param: "content-length".to_owned(),
        });
    }
    Ok(())
}

fn spool_unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".to_owned(),
        message: "Artifact upload spool is unavailable".to_owned(),
    }
}

pub(crate) fn require_session_csrf(
    action: &str,
    headers: &HeaderMap,
    auth: Option<&AuthContext>,
) -> Result<(), ToolError> {
    super::require_session_csrf(action, headers, auth)
}

pub(crate) async fn authorize_authority_context(
    runtime: &crate::access::AccessRuntime,
    identity: Option<VerifiedIdentity>,
    project_id: Option<&str>,
    selected_team_id: Option<&str>,
    permission: Permission,
) -> Result<crate::dispatch::artifact_control::AuthorityContext, ToolError> {
    let identity = identity.ok_or_else(|| ToolError::Forbidden {
        message: "Remote Artifact operations require verified identity".to_owned(),
        required_scopes: vec!["lab:read".to_owned()],
    })?;
    let project_id = project_id
        .filter(|project_id| !project_id.trim().is_empty())
        .ok_or_else(|| ToolError::Forbidden {
            message: "Remote Artifact operations require project context".to_owned(),
            required_scopes: vec!["lab:read".to_owned()],
        })?;
    crate::dispatch::artifact_control::authorize_authority_context(
        runtime,
        identity,
        project_id,
        selected_team_id,
        permission,
    )
    .await
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;
    use http_body_util::BodyExt as _;

    use super::*;

    fn auth(scopes: &[&str]) -> Option<Extension<AuthContext>> {
        Some(Extension(AuthContext {
            sub: "operator".to_owned(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            issuer: "test".to_owned(),
            via_session: false,
            csrf_token: None,
            email: None,
        }))
    }

    #[test]
    fn browser_mutations_require_the_shared_session_csrf_header() {
        let mut session = auth(&["lab:admin"]).unwrap().0;
        session.via_session = true;
        session.csrf_token = Some("csrf-secret".to_owned());
        let mut headers = HeaderMap::new();
        assert!(require_session_csrf("jobs.start", &headers, Some(&session)).is_err());
        headers.insert(
            labby_auth::session::BROWSER_CSRF_HEADER_NAME,
            "csrf-secret".parse().unwrap(),
        );
        assert!(require_session_csrf("jobs.start", &headers, Some(&session)).is_ok());

        session.csrf_token = None;
        headers.remove(labby_auth::session::BROWSER_CSRF_HEADER_NAME);
        assert!(require_session_csrf("jobs.start", &headers, Some(&session)).is_err());
    }

    #[test]
    fn bearer_mutations_do_not_require_browser_csrf() {
        let bearer = auth(&["lab:admin"]).unwrap().0;
        assert!(require_session_csrf("jobs.start", &HeaderMap::new(), Some(&bearer)).is_ok());
    }

    #[test]
    fn every_remote_authority_route_is_private_no_store() {
        for service in ["sources", "jobs", "uploads", "bundles"] {
            for descriptor in descriptors(service) {
                assert_eq!(descriptor.cache_posture, "private, no-store", "{service}");
            }
        }
    }

    #[tokio::test]
    async fn raw_upload_requires_admin_before_remote_dispatch() {
        let error = upload_bytes(
            State(AppState::default()),
            None,
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Body::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn raw_upload_rejects_non_admin_execution_scope() {
        let error = upload_bytes(
            State(AppState::default()),
            auth(&["lab"]),
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            HeaderMap::new(),
            Body::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn raw_upload_rejects_invalid_team_header_before_authority_lookup() {
        let mut headers = HeaderMap::new();
        headers.insert("x-labby-project-id", "project-1".parse().unwrap());
        headers.insert(
            "x-labby-team-id",
            axum::http::HeaderValue::from_bytes(b"team-\xc3\xa9").unwrap(),
        );
        let error = upload_bytes(
            State(AppState::default()),
            auth(&["lab:admin"]),
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            headers,
            Body::from("payload"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.error.kind(), "invalid_param");
        assert!(matches!(
            &error.error,
            ToolError::InvalidParam { param, .. } if param == "x-labby-team-id"
        ));
    }

    #[tokio::test]
    async fn dispatch_rejects_invalid_team_header_through_action_error_path() {
        let mut headers = HeaderMap::new();
        headers.insert("x-labby-project-id", "project-1".parse().unwrap());
        headers.insert(
            "x-labby-team-id",
            axum::http::HeaderValue::from_bytes(b"team-\xff").unwrap(),
        );
        let error = handle(
            "sources",
            State(AppState::default()),
            None,
            headers,
            auth(&["lab:admin"]),
            None,
            Json(ActionRequest {
                action: "sources.list".to_owned(),
                params: Value::Object(serde_json::Map::new()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.error.kind(), "invalid_param");
        assert!(matches!(
            &error.error,
            ToolError::InvalidParam { param, .. } if param == "x-labby-team-id"
        ));
        assert_eq!(
            error.body().get("service").and_then(Value::as_str),
            Some("sources")
        );
    }

    #[tokio::test]
    async fn valid_team_header_is_passed_through() {
        let mut headers = HeaderMap::new();
        assert_eq!(selected_team_id_header(&headers).unwrap(), None);
        headers.insert("x-labby-team-id", "team-1".parse().unwrap());
        assert_eq!(selected_team_id_header(&headers).unwrap(), Some("team-1"));
    }

    #[tokio::test]
    async fn spooled_upload_reports_exact_length_and_digest() {
        let spooled = spool_upload_body(Body::from("hello"), 16).await.unwrap();
        assert_eq!(spooled.length, 5);
        assert_eq!(
            spooled.digest,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let replayed = axum::body::to_bytes(Body::from_stream(ReaderStream::new(spooled.file)), 64)
            .await
            .unwrap();
        assert_eq!(&replayed[..], b"hello");
    }

    #[tokio::test]
    async fn spooled_upload_replays_chunked_bodies_in_order() {
        let chunks = futures::stream::iter(
            [b"ab".as_slice(), b"cd", b"ef"]
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(bytes::Bytes::from_static(chunk))),
        );
        let spooled = spool_upload_body(Body::from_stream(chunks), 6)
            .await
            .unwrap();
        assert_eq!(spooled.length, 6);
        assert_eq!(
            spooled.digest,
            format!("sha256:{}", hex::encode(sha2::Sha256::digest(b"abcdef")))
        );
        let replayed = spooled.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&replayed[..], b"abcdef");
    }

    #[tokio::test]
    async fn spooled_upload_rejects_oversized_body_incrementally() {
        let chunks = futures::stream::iter(
            [b"aaaa".as_slice(), b"bbbb", b"cccc"]
                .into_iter()
                .map(|chunk| Ok::<_, std::io::Error>(bytes::Bytes::from_static(chunk))),
        );
        let error = spool_upload_body(Body::from_stream(chunks), 10)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(matches!(&error, ToolError::InvalidParam { param, .. } if param == "body"));
        assert_eq!(
            ApiError::from(error).into_response().status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn spooled_upload_rejects_body_read_errors() {
        let chunks = futures::stream::iter([
            Ok(bytes::Bytes::from_static(b"ok")),
            Err(std::io::Error::other("client aborted")),
        ]);
        let error = spool_upload_body(Body::from_stream(chunks), 10)
            .await
            .unwrap_err();
        assert!(matches!(&error, ToolError::InvalidParam { param, .. } if param == "body"));
    }

    #[test]
    fn declared_length_must_match_observed_length() {
        assert!(verify_declared_length(None, 5).is_ok());
        assert!(verify_declared_length(Some(5), 5).is_ok());
        let error = verify_declared_length(Some(3), 5).unwrap_err();
        assert!(
            matches!(&error, ToolError::InvalidParam { param, .. } if param == "content-length")
        );
        assert_eq!(
            ApiError::from(error).into_response().status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn raw_upload_enforces_bound_before_remote_dispatch() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, "50000001".parse().unwrap());
        let error = upload_bytes(
            State(AppState::default()),
            auth(&["lab:admin"]),
            None,
            Path("upload-1".to_owned()),
            Query(UploadQuery {
                connection_id: None,
            }),
            headers,
            Body::empty(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.into_response().status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }
}
