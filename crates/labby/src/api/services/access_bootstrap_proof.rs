//! Direct-local HTTP adapter for daemon-owned bootstrap-proof orchestration.

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, routing};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;

use crate::api::state::AppState;
pub(crate) use crate::dispatch::access_bootstrap::AccessBootstrapProofService;
#[cfg(test)]
use crate::dispatch::access_bootstrap::ProofServiceFuture;
use crate::dispatch::access_bootstrap::{ProofMetadata, ProofServiceError};
use crate::dispatch::setup::AccessBootstrapManifest;

const PROOF_HEADER: &str = "x-labby-bootstrap-proof";
const MAX_PROOF_LEN: usize = 192;
const MAX_BODY: usize = 8 * 1024;
const MAX_PREAUTH_CONCURRENCY: usize = 8;
const PREAUTH_WINDOW_SECONDS: i64 = 60;
const PREAUTH_GLOBAL_LIMIT: i64 = 64;
const PREAUTH_PEER_LIMIT: i64 = 16;
static PREAUTH: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_PREAUTH_CONCURRENCY)));

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareIdRequest {
    prepare_id: String,
}

/// TCP serving installs `ConnectInfo<SocketAddr>`; its absence denotes the
/// separately configured Unix-socket serving path. Both are direct peers.
struct DirectPeer(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for DirectPeer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

pub(crate) fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(descriptors.next().unwrap(), routing::post(consume))
        .route(descriptors.next().unwrap(), routing::post(status))
        .route(descriptors.next().unwrap(), routing::post(cleanup))
        .map_router(|router| {
            router
                .layer(DefaultBodyLimit::max(MAX_BODY))
                .layer(axum::middleware::from_fn(harden_response))
        })
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    [
        ("/consume", "access_bootstrap_proof_consume"),
        ("/status", "access_bootstrap_proof_status"),
        ("/cleanup", "access_bootstrap_proof_cleanup"),
    ]
    .into_iter()
    .map(|(path, handler)| {
        RouteDescriptor::new("POST", path, handler, "access", RouteAuth::BootstrapProof)
            .host_validated()
            .private_no_store()
            .non_enumerating()
            .side_effects(match path {
                "/status" => "none_expected",
                "/consume" => "atomic owner and credential creation; exact retry idempotent",
                _ => "tombstone before exact-file cleanup; exact retry idempotent",
            })
            .when("direct loopback or Unix peer with daemon proof orchestration")
    })
    .collect()
}

async fn consume(
    State(state): State<AppState>,
    DirectPeer(peer): DirectPeer,
    headers: HeaderMap,
    request: Result<Json<AccessBootstrapManifest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some((service, proof, _permit)) = admit(&state, peer, &headers).await else {
        return denied();
    };
    let Ok(Json(manifest)) = request else {
        return denied();
    };
    finish(service.consume(&proof, manifest).await)
}

async fn status(
    State(state): State<AppState>,
    DirectPeer(peer): DirectPeer,
    headers: HeaderMap,
    request: Result<Json<PrepareIdRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    operate(state, peer, headers, request, false).await
}

async fn cleanup(
    State(state): State<AppState>,
    DirectPeer(peer): DirectPeer,
    headers: HeaderMap,
    request: Result<Json<PrepareIdRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    operate(state, peer, headers, request, true).await
}

async fn operate(
    state: AppState,
    peer: Option<SocketAddr>,
    headers: HeaderMap,
    request: Result<Json<PrepareIdRequest>, axum::extract::rejection::JsonRejection>,
    cleanup: bool,
) -> Response {
    let Some((service, proof, _permit)) = admit(&state, peer, &headers).await else {
        return denied();
    };
    let Ok(Json(request)) = request else {
        return denied();
    };
    if !valid_public_id(&request.prepare_id) {
        return denied();
    }
    let result = if cleanup {
        service.cleanup(&proof, &request.prepare_id).await
    } else {
        service.status(&proof, &request.prepare_id).await
    };
    finish(result)
}

async fn admit(
    state: &AppState,
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
) -> Option<(
    Arc<dyn AccessBootstrapProofService>,
    String,
    tokio::sync::OwnedSemaphorePermit,
)> {
    let permit = Arc::clone(&PREAUTH).try_acquire_owned().ok()?;
    let now = labby_auth::util::now_unix();
    let global: [u8; 32] = Sha256::digest(b"labby-proof-global-v1").into();
    let peer_fingerprint: [u8; 32] = Sha256::digest(
        peer.map_or_else(|| "unix".to_owned(), |value| value.ip().to_string())
            .as_bytes(),
    )
    .into();
    let global_admitted = state
        .access_runtime
        .admit_security_operation(
            "proof_global".into(),
            global,
            now,
            PREAUTH_WINDOW_SECONDS,
            PREAUTH_GLOBAL_LIMIT,
        )
        .await
        .ok()?;
    let peer_admitted = state
        .access_runtime
        .admit_security_operation(
            "proof_peer".into(),
            peer_fingerprint,
            now,
            PREAUTH_WINDOW_SECONDS,
            PREAUTH_PEER_LIMIT,
        )
        .await
        .ok()?;
    if !global_admitted || !peer_admitted {
        let _ = state
            .access_runtime
            .record_security_event(
                "proof".into(),
                "deny".into(),
                "rate_limited".into(),
                global,
                Some(peer_fingerprint),
                now,
            )
            .await;
        return None;
    }
    if peer.is_some_and(|peer| !peer.ip().is_loopback())
        || state
            .http_bind_host
            .as_deref()
            .is_some_and(|host| !crate::api::host_validation::is_loopback_host_value(host))
        || has_forwarding_headers(headers)
        || !literal_origin_matches_host(headers)
    {
        return None;
    }
    let proof = headers.get(PROOF_HEADER)?.to_str().ok()?;
    if !valid_proof(proof) {
        return None;
    }
    Some((
        state.access_bootstrap_proof.clone()?,
        proof.to_owned(),
        permit,
    ))
}

fn valid_proof(proof: &str) -> bool {
    const PREFIX: &str = "lby_bp_v1_";
    const SECRET_LEN: usize = 43;
    if proof.len() > MAX_PROOF_LEN || !proof.is_ascii() {
        return false;
    }
    let Some(payload) = proof.strip_prefix(PREFIX) else {
        return false;
    };
    let Some(split) = payload.len().checked_sub(SECRET_LEN + 1) else {
        return false;
    };
    payload.as_bytes().get(split) == Some(&b'_')
        && valid_public_id(&payload[..split])
        && payload[split + 1..]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn has_forwarding_headers(headers: &HeaderMap) -> bool {
    [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
}

fn literal_origin_matches_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if !crate::api::host_validation::is_loopback_host_value(host) {
        return false;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    let Ok(host_url) = url::Url::parse(&format!("http://{host}")) else {
        return false;
    };
    url.scheme() == "http"
        && url.host_str() == host_url.host_str()
        && url.port_or_known_default() == host_url.port_or_known_default()
}

fn valid_public_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn finish(result: Result<ProofMetadata, ProofServiceError>) -> Response {
    match result {
        Ok(metadata) => secure((StatusCode::OK, Json(metadata)).into_response()),
        Err(_) => denied(),
    }
}

fn denied() -> Response {
    secure((StatusCode::FORBIDDEN, Json(serde_json::json!({"error":{"kind":"access_denied","message":"bootstrap request denied"}}))).into_response())
}

fn secure(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

async fn harden_response(
    request: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    secure(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockService(AtomicUsize);

    impl AccessBootstrapProofService for MockService {
        fn consume<'a>(
            &'a self,
            _proof: &'a str,
            manifest: AccessBootstrapManifest,
        ) -> ProofServiceFuture<'a> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move {
                Ok(ProofMetadata {
                    status: "consumed".into(),
                    prepare_id: manifest.idempotency_key,
                    credential_id: Some(manifest.credential_id),
                })
            })
        }
        fn status<'a>(&'a self, _proof: &'a str, prepare_id: &'a str) -> ProofServiceFuture<'a> {
            self.0.fetch_add(1, Ordering::Relaxed);
            let prepare_id = prepare_id.to_owned();
            Box::pin(async move {
                Ok(ProofMetadata {
                    status: "active".into(),
                    prepare_id,
                    credential_id: None,
                })
            })
        }
        fn cleanup<'a>(&'a self, _proof: &'a str, prepare_id: &'a str) -> ProofServiceFuture<'a> {
            self.0.fetch_add(1, Ordering::Relaxed);
            let prepare_id = prepare_id.to_owned();
            Box::pin(async move {
                Ok(ProofMetadata {
                    status: "cleaned".into(),
                    prepare_id,
                    credential_id: None,
                })
            })
        }
    }

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8765"));
        headers.insert(
            PROOF_HEADER,
            HeaderValue::from_str(&format!("lby_bp_v1_proof_{}", "A".repeat(43))).unwrap(),
        );
        headers
    }

    fn manifest() -> AccessBootstrapManifest {
        AccessBootstrapManifest {
            version: 1,
            installation_id: "installation".into(),
            canonical_issuer: "issuer".into(),
            organization_name: "Organization".into(),
            project_name: "Project".into(),
            subject: "subject".into(),
            loadout_id: "loadout".into(),
            route_id: "route".into(),
            resource: "lab://project".into(),
            scopes: vec!["lab:read".into()],
            ttl_seconds: 300,
            credential_id: "credential".into(),
            idempotency_key: "prepare-id".into(),
        }
    }

    async fn isolated_admitted_state(service: Arc<MockService>) -> (tempfile::TempDir, AppState) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let store = crate::access::AccessStore::open(path.clone())
            .await
            .unwrap();
        store
            .activate_bootstrap_proof(crate::access::ActivateProofInput {
                proof_id: format!("proof-{}", ulid::Ulid::new()),
                prepare_id: format!("prepare-{}", ulid::Ulid::new()),
                installation_id: format!("installation-{}", ulid::Ulid::new()),
                installation_generation: 1,
                proof_digest: [1; 32],
                manifest_digest: [2; 32],
                request_digest: [3; 32],
                idempotency_digest: [4; 32],
                credential_id: format!("credential-{}", ulid::Ulid::new()),
                credential_digest: [5; 32],
                proof_generation: 1,
                created_at: 1,
                expires_at: i64::MAX,
            })
            .await
            .unwrap();
        drop(store);
        let runtime = Arc::new(crate::access::AccessRuntime::initialize(path).await);
        (
            directory,
            AppState::new()
                .with_access_runtime(runtime)
                .with_access_bootstrap_proof(service),
        )
    }

    #[test]
    fn forwarding_and_nonliteral_origin_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8765"));
        assert!(literal_origin_matches_host(&headers));
        headers.insert("forwarded", HeaderValue::from_static("for=127.0.0.1"));
        assert!(has_forwarding_headers(&headers));
        headers.remove("forwarded");
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://127.0.0.1:8765"),
        );
        assert!(!literal_origin_matches_host(&headers));
    }

    #[test]
    fn proof_header_requires_the_exact_canonical_shape() {
        assert!(valid_proof(&format!(
            "lby_bp_v1_proof-id_{}",
            "A".repeat(43)
        )));
        for invalid in [
            "lby_bp_v1_",
            "lby_bp_v1_bad_id_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "lby_bp_v1_proof_short",
            "wrong_proof_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            assert!(!valid_proof(invalid), "accepted {invalid}");
        }
    }

    #[test]
    fn errors_are_uniform_and_hardened() {
        for error in [ProofServiceError::Denied, ProofServiceError::Unavailable] {
            let response = finish(Err(error));
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
            assert!(
                !response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            );
        }
    }

    #[tokio::test]
    async fn valid_direct_request_calls_only_the_orchestration_seam() {
        let service = Arc::new(MockService(AtomicUsize::new(0)));
        let (_directory, state) = isolated_admitted_state(service.clone()).await;
        let response = consume(
            State(state),
            DirectPeer(Some("127.0.0.1:42000".parse().unwrap())),
            headers(),
            Ok(Json(manifest())),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(service.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
    }

    #[tokio::test]
    async fn forwarded_request_is_denied_before_orchestration() {
        let service = Arc::new(MockService(AtomicUsize::new(0)));
        let (_directory, state) = isolated_admitted_state(service.clone()).await;
        let mut request_headers = headers();
        request_headers.insert("x-forwarded-for", HeaderValue::from_static("127.0.0.1"));
        let response = consume(
            State(state),
            DirectPeer(Some("127.0.0.1:42000".parse().unwrap())),
            request_headers,
            Ok(Json(manifest())),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(service.0.load(Ordering::Relaxed), 0);
    }
}
