//! Thin HTTP projection of project-credential lifecycle transactions.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, routing};
use labby_auth::AuthContext;
use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::access::{CredentialLifecycleError, IssueCredentialInput, MutationOutcome};
use crate::api::state::AppState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueRequest {
    credential_id: String,
    credential_digest_hex: String,
    project_id: String,
    route_id: String,
    resource: String,
    audience: String,
    scopes: Vec<String>,
    expires_at: i64,
    idempotency_key: String,
}

#[derive(Serialize)]
struct IssueResponse {
    status: &'static str,
    credential_id: String,
    credential_generation: u64,
    expires_at: i64,
}

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(descriptors.next().unwrap(), routing::post(issue))
        .route(descriptors.next().unwrap(), routing::get(self_introspect))
        .route(descriptors.next().unwrap(), routing::delete(revoke))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};

    vec![
        RouteDescriptor::new("POST", "/", "credential_issue", "access", RouteAuth::V1)
            .private_no_store()
            .non_enumerating()
            .side_effects("credential creation; exact retry idempotent"),
        RouteDescriptor::new("GET", "/self", "credential_self", "access", RouteAuth::V1)
            .private_no_store()
            .non_enumerating(),
        RouteDescriptor::new(
            "DELETE",
            "/{credential_id}",
            "credential_revoke",
            "access",
            RouteAuth::V1,
        )
        .private_no_store()
        .non_enumerating()
        .side_effects("immediate credential revocation; exact retry idempotent"),
    ]
}

async fn issue(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
    request: Result<Json<IssueRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some((auth, source, bound)) = authenticated(auth, source, bound) else {
        audit_denial(&state, "credential_issue", "authentication_denied", [0; 32]).await;
        return denied();
    };
    let now = labby_auth::util::now_unix();
    let global: [u8; 32] = Sha256::digest(b"labby-credential-issue-global-v1").into();
    let target: [u8; 32] = Sha256::digest(source.credential_id.as_bytes()).into();
    let admitted = state
        .access_runtime
        .admit_security_operation("credential_global".into(), global, now, 60, 64)
        .await
        .ok()
        == Some(true)
        && state
            .access_runtime
            .admit_security_operation("credential_peer".into(), target, now, 60, 16)
            .await
            .ok()
            == Some(true);
    if !admitted {
        let _ = state
            .access_runtime
            .record_security_event(
                "credential_issue".into(),
                "deny".into(),
                "rate_limited".into(),
                target,
                None,
                now,
            )
            .await;
        return denied();
    }
    if !valid_mutation_csrf(&headers, &auth) {
        audit_denial(&state, "credential_issue", "csrf_denied", target).await;
        return denied();
    }
    let Ok(Json(request)) = request else {
        audit_denial(&state, "credential_issue", "invalid_request", target).await;
        return invalid("invalid credential issue request");
    };
    let Some(credential_digest) = decode_digest(&request.credential_digest_hex) else {
        return invalid("invalid credential issue request");
    };
    if request.project_id != bound.project_id
        || request.route_id != bound.route_id
        || request.resource != bound.resource
        || request.audience != bound.audience
        || request.expires_at <= now
        || u64::try_from(request.expires_at)
            .ok()
            .is_none_or(|expiry| expiry > bound.expires_at)
        || !canonical_scopes(&request.scopes)
        || !request
            .scopes
            .iter()
            .all(|scope| bound.scopes.iter().any(|granted| granted == scope))
        || request.idempotency_key.is_empty()
        || request.idempotency_key.len() > 160
    {
        audit_denial(&state, "credential_issue", "binding_denied", target).await;
        return denied();
    }
    let scopes_json = match serde_json::to_string(&request.scopes) {
        Ok(value) => value,
        Err(_) => return invalid("invalid credential issue request"),
    };
    let mut request_hasher = Sha256::new();
    let expires_at = request.expires_at.to_string();
    for field in [
        request.credential_id.as_bytes(),
        request.project_id.as_bytes(),
        request.route_id.as_bytes(),
        request.resource.as_bytes(),
        request.audience.as_bytes(),
        scopes_json.as_bytes(),
        expires_at.as_bytes(),
    ] {
        request_hasher.update(field);
        request_hasher.update([0]);
    }
    request_hasher.update(credential_digest);
    let request_digest: [u8; 32] = request_hasher.finalize().into();
    let idempotency_digest: [u8; 32] = Sha256::digest(request.idempotency_key.as_bytes()).into();
    let input = IssueCredentialInput {
        actor_credential_id: source.credential_id,
        actor_credential_generation: i64::try_from(source.credential_generation).unwrap_or(0),
        credential_id: request.credential_id.clone(),
        credential_digest,
        credential_generation: 1,
        scopes_json,
        issued_at: now,
        expires_at: request.expires_at,
        idempotency_digest,
        request_digest,
    };
    match state.access_runtime.issue_project_credential(input).await {
        Ok(outcome) => json(
            StatusCode::CREATED,
            &IssueResponse {
                status: mutation_status(outcome),
                credential_id: request.credential_id,
                credential_generation: 1,
                expires_at: request.expires_at,
            },
        ),
        Err(CredentialLifecycleError::NotAuthorized) => {
            audit_denial(&state, "credential_issue", "authorization_denied", target).await;
            denied()
        }
        Err(CredentialLifecycleError::Invalid) => {
            audit_denial(&state, "credential_issue", "invalid_request", target).await;
            invalid("invalid credential issue request")
        }
        Err(_) => {
            audit_denial(&state, "credential_issue", "recovery_unavailable", target).await;
            unavailable()
        }
    }
}

async fn self_introspect(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
) -> Response {
    let Some((_auth, source, bound)) = authenticated(auth, source, bound) else {
        audit_denial(
            &state,
            "credential_verify",
            "authentication_denied",
            [0; 32],
        )
        .await;
        return denied();
    };
    let target: [u8; 32] = Sha256::digest(source.credential_id.as_bytes()).into();
    match state
        .access_runtime
        .introspect_project_credential(
            source.credential_id,
            i64::try_from(source.credential_generation).unwrap_or(0),
            labby_auth::util::now_unix(),
        )
        .await
    {
        Ok(Some(snapshot)) if snapshot.project_id == bound.project_id => json(
            StatusCode::OK,
            &serde_json::json!({
                "credential_id": snapshot.credential_id,
                "credential_generation": snapshot.credential_generation,
                "installation_id": snapshot.installation_id,
                "organization_id": snapshot.organization_id,
                "project_id": snapshot.project_id,
                "loadout_id": snapshot.loadout_id,
                "route_id": snapshot.route_id,
                "resource": snapshot.resource,
                "audience": snapshot.audience,
                "scopes": serde_json::from_str::<Vec<String>>(&snapshot.scopes_json).unwrap_or_default(),
                "expires_at": snapshot.expires_at,
                "revocation_generation": snapshot.revocation_generation,
                "status": "active"
            }),
        ),
        Ok(_) | Err(CredentialLifecycleError::NotAuthorized) => {
            audit_denial(&state, "credential_verify", "self_denied", target).await;
            denied()
        }
        Err(_) => {
            audit_denial(&state, "credential_verify", "recovery_unavailable", target).await;
            unavailable()
        }
    }
}

async fn revoke(
    State(state): State<AppState>,
    Path(target_id): Path<String>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
) -> Response {
    let Some((auth, source, _bound)) = authenticated(auth, source, bound) else {
        audit_denial(
            &state,
            "credential_revoke",
            "authentication_denied",
            [0; 32],
        )
        .await;
        return denied();
    };
    let target: [u8; 32] = Sha256::digest(target_id.as_bytes()).into();
    if auth.via_session {
        if !valid_mutation_csrf(&headers, &auth) {
            audit_denial(&state, "credential_revoke", "csrf_denied", target).await;
            return denied();
        }
    } else if target_id != source.credential_id {
        audit_denial(&state, "credential_revoke", "binding_denied", target).await;
        return denied();
    }
    match state
        .access_runtime
        .revoke_project_credential(
            source.credential_id,
            i64::try_from(source.credential_generation).unwrap_or(0),
            target_id,
            labby_auth::util::now_unix(),
        )
        .await
    {
        Ok(outcome) => json(
            StatusCode::OK,
            &serde_json::json!({"status": mutation_status(outcome)}),
        ),
        Err(CredentialLifecycleError::NotAuthorized | CredentialLifecycleError::Invalid) => {
            audit_denial(&state, "credential_revoke", "authorization_denied", target).await;
            denied()
        }
        Err(_) => {
            audit_denial(&state, "credential_revoke", "recovery_unavailable", target).await;
            unavailable()
        }
    }
}

async fn audit_denial(
    state: &AppState,
    event_kind: &'static str,
    reason: &'static str,
    target: [u8; 32],
) {
    if state
        .access_runtime
        .record_security_event(
            event_kind.into(),
            "deny".into(),
            reason.into(),
            target,
            None,
            labby_auth::util::now_unix(),
        )
        .await
        .is_err()
    {
        tracing::warn!(phase = "security_audit", "security event unavailable");
    }
}

fn authenticated(
    auth: Option<Extension<AuthContext>>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
) -> Option<(AuthContext, ProductCredentialGrant, BoundAccessGrant)> {
    let (Extension(auth), Extension(source), Extension(bound)) = (auth?, source?, bound?);
    let principal_matches = auth.sub == bound.principal_id;
    let issuer_matches = auth.issuer == bound.issuer;
    let credential_matches = source.credential_id == bound.credential_id;
    let generation_matches = source.credential_generation == bound.credential_generation;
    (principal_matches && issuer_matches && credential_matches && generation_matches)
        .then_some((auth, source, bound))
}

fn valid_mutation_csrf(headers: &HeaderMap, auth: &AuthContext) -> bool {
    !auth.via_session
        || auth.csrf_token.as_deref()
            == headers
                .get(labby_auth::session::BROWSER_CSRF_HEADER_NAME)
                .and_then(|value| value.to_str().ok())
}

fn canonical_scopes(scopes: &[String]) -> bool {
    !scopes.is_empty()
        && scopes.iter().all(|scope| {
            !scope.is_empty() && scope.len() <= 160 && !scope.chars().any(char::is_control)
        })
        && scopes.windows(2).all(|pair| pair[0] < pair[1])
}

fn decode_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(digest)
}

fn mutation_status(outcome: MutationOutcome) -> &'static str {
    match outcome {
        MutationOutcome::Created => "created",
        MutationOutcome::AlreadyApplied => "already_applied",
    }
}

fn denied() -> Response {
    response(
        StatusCode::NOT_FOUND,
        serde_json::json!({"kind":"not_found","message":"credential operation denied"}),
    )
}

fn invalid(message: &'static str) -> Response {
    response(
        StatusCode::UNPROCESSABLE_ENTITY,
        serde_json::json!({"kind":"validation_failed","message":message}),
    )
}

fn unavailable() -> Response {
    response(
        StatusCode::SERVICE_UNAVAILABLE,
        serde_json::json!({"kind":"service_unavailable","message":"credential service unavailable"}),
    )
}

fn json(status: StatusCode, value: &impl Serialize) -> Response {
    let body = serde_json::to_value(value).unwrap_or_else(|_| serde_json::json!({}));
    response(status, body)
}

fn response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants() -> (AuthContext, ProductCredentialGrant, BoundAccessGrant) {
        let source = ProductCredentialGrant {
            issuer: "https://issuer.example".into(),
            subject: "operator-1".into(),
            credential_id: "credential-1".into(),
            credential_generation: 1,
            scopes: vec!["lab:read".into()],
            resource: "lab://project-1".into(),
            audience: "labby".into(),
            expires_at: u64::try_from(labby_auth::util::now_unix() + 600).unwrap(),
        };
        let bound = BoundAccessGrant {
            installation_id: "installation-1".into(),
            issuer: source.issuer.clone(),
            subject: source.subject.clone(),
            principal_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            project_id: "project-1".into(),
            loadout_id: "loadout-1".into(),
            loadout_generation: 1,
            assignment_generation: 1,
            catalog_generation: 1,
            route_id: "route-1".into(),
            route_generation: 1,
            membership_epoch: 1,
            organization_policy_epoch: 0,
            project_policy_epoch: 0,
            credential_id: source.credential_id.clone(),
            credential_generation: source.credential_generation,
            scopes: source.scopes.clone(),
            resource: source.resource.clone(),
            audience: source.audience.clone(),
            expires_at: source.expires_at,
            requires_admin: false,
            destructive: false,
        };
        let auth = AuthContext {
            sub: bound.principal_id.clone(),
            actor_key: None,
            scopes: bound.scopes.clone(),
            issuer: bound.issuer.clone(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        (auth, source, bound)
    }

    #[test]
    fn routes_and_bounded_request_vocabulary_are_exact() {
        let paths = routes(AppState::new())
            .descriptors
            .into_iter()
            .map(|descriptor| (descriptor.method, descriptor.path))
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                ("POST", "/".into()),
                ("GET", "/self".into()),
                ("DELETE", "/{credential_id}".into())
            ]
        );
        assert_eq!(decode_digest(&"07".repeat(32)), Some([7; 32]));
        assert!(decode_digest("07").is_none());
        assert!(decode_digest(&"AB".repeat(32)).is_none());
        assert!(canonical_scopes(&["lab:admin".into(), "lab:read".into()]));
        assert!(!canonical_scopes(&["lab:read".into(), "lab:read".into()]));
    }

    #[tokio::test]
    async fn missing_or_mixed_authority_and_browser_csrf_fail_before_store_mutation() {
        let missing = self_introspect(State(AppState::new()), None, None, None).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            missing.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );

        let (mut auth, source, bound) = grants();
        let bearer_foreign = revoke(
            State(AppState::new()),
            Path("credential-foreign".into()),
            HeaderMap::new(),
            Some(Extension(auth.clone())),
            Some(Extension(source.clone())),
            Some(Extension(bound.clone())),
        )
        .await;
        assert_eq!(bearer_foreign.status(), StatusCode::NOT_FOUND);

        auth.via_session = true;
        auth.csrf_token = Some("csrf".into());
        let browser_without_csrf = revoke(
            State(AppState::new()),
            Path(source.credential_id.clone()),
            HeaderMap::new(),
            Some(Extension(auth)),
            Some(Extension(source)),
            Some(Extension(bound)),
        )
        .await;
        assert_eq!(browser_without_csrf.status(), StatusCode::NOT_FOUND);
    }
}
