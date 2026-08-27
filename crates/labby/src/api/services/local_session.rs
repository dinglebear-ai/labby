//! Thin HTTPS adapter for source-bound project browser sessions.

use axum::Extension;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use labby_auth::{AuthContext, Authenticator, VerifiedIdentity};
use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};

use crate::api::state::AppState;

pub(crate) fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;

    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(
            descriptors.next().unwrap(),
            axum::routing::post(create_local_session),
        )
        .route(
            descriptors.next().unwrap(),
            axum::routing::delete(logout_local_session),
        )
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};

    vec![
        RouteDescriptor::new(
            "POST",
            "/auth/local-session",
            "local_session_create",
            "access",
            RouteAuth::BearerOnly,
        )
        .host_validated()
        .private_no_store()
        .non_enumerating()
        .side_effects("source-bound browser session creation"),
        RouteDescriptor::new(
            "DELETE",
            "/auth/local-session",
            "local_session_logout",
            "access",
            RouteAuth::BrowserSession,
        )
        .host_validated()
        .private_no_store()
        .non_enumerating()
        .side_effects("browser session revocation"),
    ]
}

fn denied(message: &'static str) -> Response {
    response(
        StatusCode::UNAUTHORIZED,
        serde_json::json!({"kind":"auth_failed","message":message}),
    )
}

fn response(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, axum::Json(body)).into_response();
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

fn invalid_browser_origin(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    if [
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
    {
        return Some(denied("local project session request denied"));
    }
    let Some(public_url) = state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
    else {
        return Some(response(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"kind":"service_unavailable","message":"secure project sessions are unavailable"}),
        ));
    };
    if public_url.scheme() != "https" {
        return Some(response(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"kind":"service_unavailable","message":"secure project sessions require HTTPS"}),
        ));
    }
    let expected_origin = public_url.origin().ascii_serialization();
    let expected_host = expected_origin.strip_prefix("https://").unwrap_or_default();
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    if origin != Some(expected_origin.as_str()) || host != Some(expected_host) {
        return Some(denied("local project session request denied"));
    }
    None
}

pub async fn create_local_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
) -> Response {
    if headers.contains_key(header::COOKIE) {
        return denied("local project session request denied");
    }
    if let Some(response) = invalid_browser_origin(&state, &headers) {
        return response;
    }
    let (
        Some(Extension(auth)),
        Some(Extension(identity)),
        Some(Extension(source)),
        Some(Extension(bound)),
    ) = (auth, identity, source, bound)
    else {
        return denied("local project session request denied");
    };
    let context_matches_principal = auth.sub == bound.principal_id;
    if auth.via_session
        || identity.authenticator() != Authenticator::ProductCredential
        || source.credential_id != bound.credential_id
        || source.credential_generation != bound.credential_generation
        || !context_matches_principal
    {
        return denied("local project session request denied");
    }
    let Some(session_state) = state.project_session_state.as_ref() else {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"kind":"service_unavailable","message":"project session store is unavailable"}),
        );
    };
    let session = match session_state.create(&bound).await {
        Ok(session) => session,
        Err(error) => {
            tracing::warn!(
                error_kind = error.kind(),
                "project session persistence failed"
            );
            return response(
                StatusCode::SERVICE_UNAVAILABLE,
                serde_json::json!({"kind":"service_unavailable","message":"project session creation failed"}),
            );
        }
    };
    let max_age = u64::try_from(session.expires_at.saturating_sub(session.created_at)).unwrap_or(0);
    let mut response = response(
        StatusCode::CREATED,
        serde_json::json!({"csrf_token":session.csrf_token,"expires_at":session.expires_at}),
    );
    if let Ok(cookie) =
        HeaderValue::from_str(&session_state.set_cookie(&session.session_id, max_age))
    {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

pub async fn logout_local_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    bound: Option<Extension<BoundAccessGrant>>,
) -> Response {
    if let Some(response) = invalid_browser_origin(&state, &headers) {
        return response;
    }
    let (Some(Extension(auth)), Some(Extension(identity)), Some(Extension(bound))) =
        (auth, identity, bound)
    else {
        return denied("local project session logout denied");
    };
    if !auth.via_session
        || identity.authenticator() != Authenticator::BrowserSession
        || auth.csrf_token.as_deref()
            != headers
                .get(labby_auth::session::BROWSER_CSRF_HEADER_NAME)
                .and_then(|value| value.to_str().ok())
    {
        return denied("local project session logout denied");
    }
    let Some(session_state) = state.project_session_state.as_ref() else {
        return response(
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"kind":"service_unavailable","message":"project session store is unavailable"}),
        );
    };
    let Some(session_id) = labby_auth::session::read_cookie(&headers, &session_state.cookie_name)
    else {
        return denied("local project session logout denied");
    };
    let owned = session_state
        .store
        .find_browser_session(&session_id)
        .await
        .ok()
        .flatten()
        .and_then(|row| row.project_binding)
        .is_some_and(|binding| {
            let same_credential = binding.source_credential_id == bound.credential_id;
            binding.principal_id == bound.principal_id
                && same_credential
                && binding.source_credential_generation == bound.credential_generation
        });
    if !owned
        || session_state
            .store
            .revoke_browser_session(&session_id)
            .await
            .is_err()
    {
        return denied("local project session logout denied");
    }
    let mut response = response(StatusCode::NO_CONTENT, serde_json::Value::Null);
    if let Ok(cookie) = HeaderValue::from_str(&session_state.clear_cookie()) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers(origin: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        headers.insert(header::HOST, HeaderValue::from_static("lab.example.com"));
        headers
    }

    fn grants() -> (ProductCredentialGrant, BoundAccessGrant) {
        let expires_at = u64::try_from(labby_auth::util::now_unix() + 600).unwrap();
        let source = ProductCredentialGrant {
            issuer: "https://issuer.example".into(),
            subject: "operator-1".into(),
            credential_id: "credential-1".into(),
            credential_generation: 2,
            scopes: vec!["lab:read".into()],
            resource: "lab://project-1".into(),
            audience: "labby".into(),
            expires_at,
        };
        let bound = BoundAccessGrant {
            installation_id: "installation-1".into(),
            issuer: source.issuer.clone(),
            subject: source.subject.clone(),
            principal_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            project_id: "project-1".into(),
            loadout_id: "loadout-1".into(),
            loadout_generation: 3,
            assignment_generation: 4,
            catalog_generation: 5,
            route_id: "route-1".into(),
            route_generation: 6,
            membership_epoch: 7,
            organization_policy_epoch: 8,
            project_policy_epoch: 9,
            credential_id: source.credential_id.clone(),
            credential_generation: source.credential_generation,
            scopes: source.scopes.clone(),
            resource: source.resource.clone(),
            audience: source.audience.clone(),
            expires_at,
            requires_admin: false,
            destructive: false,
        };
        (source, bound)
    }

    async fn state(scheme: &str) -> (AppState, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let session_state = labby_auth::project_session::ProjectSessionState::open(
            directory.path().join("auth.db"),
            "__Host-labby-session",
        )
        .await
        .unwrap();
        let config = labby_auth::config::AuthConfig {
            public_url: Some(url::Url::parse(&format!("{scheme}://lab.example.com")).unwrap()),
            ..labby_auth::config::AuthConfig::default()
        };
        (
            AppState::new()
                .with_auth_config(config)
                .with_project_session_state(session_state),
            directory,
        )
    }

    #[tokio::test]
    async fn creates_secure_source_bound_session_then_owner_scoped_logout_revokes_it() {
        let (state, _directory) = state("https").await;
        let (source, bound) = grants();
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::ProductCredential,
            source.issuer.clone(),
            source.credential_id.clone(),
        )
        .unwrap();
        let auth = AuthContext {
            sub: bound.principal_id.clone(),
            actor_key: None,
            scopes: bound.scopes.clone(),
            issuer: bound.issuer.clone(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let created = create_local_session(
            State(state.clone()),
            headers("https://lab.example.com"),
            Some(Extension(auth)),
            Some(Extension(identity)),
            Some(Extension(source)),
            Some(Extension(bound.clone())),
        )
        .await;
        assert_eq!(created.status(), StatusCode::CREATED);
        let cookie = created
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.starts_with("__Host-labby-session="));
        assert!(cookie.contains("; Path=/; HttpOnly; Secure; SameSite=Strict;"));
        assert!(!cookie.contains("Domain="));
        let session_id = cookie.split_once('=').unwrap().1.split(';').next().unwrap();
        let row = state
            .project_session_state
            .as_ref()
            .unwrap()
            .store
            .find_browser_session(session_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            row.project_binding
                .as_ref()
                .is_some_and(|binding| binding.source_credential_id == bound.credential_id)
        );
        let mut logout_headers = headers("https://lab.example.com");
        logout_headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("__Host-labby-session={session_id}")).unwrap(),
        );
        logout_headers.insert(
            labby_auth::session::BROWSER_CSRF_HEADER_NAME,
            HeaderValue::from_str(&row.csrf_token).unwrap(),
        );
        let logout_auth = AuthContext {
            sub: bound.principal_id.clone(),
            actor_key: None,
            scopes: bound.scopes.clone(),
            issuer: bound.issuer.clone(),
            via_session: true,
            csrf_token: Some(row.csrf_token),
            email: None,
        };
        let browser_identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::BrowserSession,
            bound.issuer.clone(),
            bound.credential_id.clone(),
        )
        .unwrap();
        let logged_out = logout_local_session(
            State(state.clone()),
            logout_headers,
            Some(Extension(logout_auth)),
            Some(Extension(browser_identity)),
            Some(Extension(bound)),
        )
        .await;
        assert_eq!(logged_out.status(), StatusCode::NO_CONTENT);
        assert!(
            state
                .project_session_state
                .as_ref()
                .unwrap()
                .store
                .find_browser_session(session_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejects_http_cross_origin_static_legacy_and_mixed_requests() {
        let (http_state, _directory) = state("http").await;
        let response = create_local_session(
            State(http_state),
            headers("http://lab.example.com"),
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let (state, _directory) = state("https").await;
        let static_response = create_local_session(
            State(state.clone()),
            headers("https://lab.example.com"),
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(static_response.status(), StatusCode::UNAUTHORIZED);
        let mut mixed = headers("https://lab.example.com");
        mixed.insert(header::COOKIE, HeaderValue::from_static("legacy=value"));
        let mixed_response =
            create_local_session(State(state.clone()), mixed, None, None, None, None).await;
        assert_eq!(mixed_response.status(), StatusCode::UNAUTHORIZED);
        let (source, bound) = grants();
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::ProductCredential,
            source.issuer.clone(),
            source.credential_id.clone(),
        )
        .unwrap();
        let auth = AuthContext {
            sub: bound.principal_id.clone(),
            actor_key: None,
            scopes: bound.scopes.clone(),
            issuer: bound.issuer.clone(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let mut fixation = headers("https://lab.example.com");
        fixation.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-labby-session=attacker-fixed"),
        );
        let fixation_response = create_local_session(
            State(state.clone()),
            fixation,
            Some(Extension(auth)),
            Some(Extension(identity)),
            Some(Extension(source)),
            Some(Extension(bound)),
        )
        .await;
        assert_eq!(fixation_response.status(), StatusCode::UNAUTHORIZED);
        let cross_origin = create_local_session(
            State(state),
            headers("https://evil.example"),
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(cross_origin.status(), StatusCode::UNAUTHORIZED);
    }
}
