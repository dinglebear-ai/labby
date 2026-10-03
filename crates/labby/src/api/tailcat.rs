//! Restricted HTTP projection. Native session policy belongs to dispatch.

use axum::{Router, body::Body};
#[cfg(all(feature = "tailcat", unix))]
mod control;
#[cfg(all(feature = "tailcat", unix))]
pub(crate) use control::ControlListener;

/// Construct only the selected protected MCP projection. Publication is owned
/// by the native session manager; this never mounts the operator API router.
pub(crate) fn restricted_router(
    state: super::AppState,
    route: crate::config::ProtectedMcpRouteConfig,
    authority: std::sync::Arc<crate::dispatch::tailcat::RequestAuthority>,
) -> Result<Router, crate::dispatch::tailcat::PairingError> {
    use axum::{
        http::{HeaderValue, StatusCode, header},
        response::IntoResponse as _,
    };
    use base64::Engine as _;
    let source = authority.approved.source();
    if state.web_ui_auth_disabled
        || !state
            .oauth_state
            .as_ref()
            .is_some_and(|auth| auth.config.mode == labby_auth::config::AuthMode::OAuth)
        || !state
            .access_credential_adapter
            .as_ref()
            .is_some_and(|adapter| std::sync::Arc::ptr_eq(adapter, &authority.adapter))
        || !route.enabled
        || route.name != source.route_id
        || route.public_resource() != source.resource
        || state.installation_id.as_deref() != Some(source.installation_id.as_str())
    {
        return Err(crate::dispatch::tailcat::PairingError);
    }
    let path: axum::http::Uri = route
        .public_path
        .parse()
        .map_err(|_| crate::dispatch::tailcat::PairingError)?;
    let host = HeaderValue::from_str(&route.public_host)
        .map_err(|_| crate::dispatch::tailcat::PairingError)?;
    let cleanup = std::sync::Arc::new(crate::dispatch::tailcat::cleanup::CleanupSession::new(
        authority.approved.upstream(),
    ));
    Ok(Router::new().route(
        "/mcp",
        axum::routing::any(move |mut request: axum::http::Request<Body>| {
            let state = state.clone();
            let route = route.clone();
            let authority = authority.clone();
            let cleanup = cleanup.clone();
            let path = path.clone();
            let host = host.clone();
            async move {
                fn single<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
                    let mut values = headers.get_all(name).iter();
                    let value = values.next()?.to_str().ok()?;
                    if values.next().is_some() {
                        return None;
                    }
                    Some(value)
                }
                let headers = request.headers();
                let context = single(headers, "authorization")
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .zip(single(headers, "origin"))
                    .zip(single(headers, "labby-tailcat-generation"));
                let Some(((envelope, origin), generation)) = context else {
                    return StatusCode::UNAUTHORIZED.into_response();
                };
                let Ok(credential) = authority.authorize(envelope, origin, generation).await else {
                    return StatusCode::UNAUTHORIZED.into_response();
                };
                let wire = format!(
                    "Bearer {}{}_{}",
                    labby_primitives::product_credential::PRODUCT_CREDENTIAL_PREFIX,
                    credential.credential_id(),
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(credential.secret())
                );
                let Ok(mut bearer) = HeaderValue::from_str(&wire) else {
                    return StatusCode::UNAUTHORIZED.into_response();
                };
                bearer.set_sensitive(true);
                request.headers_mut().insert(header::AUTHORIZATION, bearer);
                request.headers_mut().insert(header::HOST, host);
                request.headers_mut().remove(header::COOKIE);
                // The exact browser origin was authenticated above. This inner
                // hop is native, like a desktop MCP client; its public HTTP
                // origin allowlist must not reinterpret the approved portal.
                request.headers_mut().remove(header::ORIGIN);
                request.extensions_mut().insert(cleanup);
                *request.uri_mut() = path;
                super::router::protected_mcp_route_entry(state, request, route).await
            }
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;
    #[cfg(unix)]
    #[tokio::test]
    async fn restricted_projection_refuses_unconfigured_or_disabled_oauth() {
        use base64::Engine as _;
        let (directory, _runtime, adapter, approved) =
            crate::dispatch::tailcat::testing::fixture().await;
        let authority = std::sync::Arc::new(crate::dispatch::tailcat::RequestAuthority {
            adapter,
            source: labby_primitives::product_credential::ProductCredential::parse(&format!(
                "lby_pc_v1_source_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
            ))
            .unwrap(),
            approved,
            key: std::sync::Arc::new(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap(),
            ),
        });
        let route = toml::from_str::<crate::config::ProtectedMcpRouteConfig>(
            r#"
name = "sandbox"
public_host = "labby.example"
public_path = "/sandbox"
scopes = ["lab"]
[target]
kind = "gateway_subset"
project_id = "bootstrap-default"
loadout = "sandbox"
"#,
        )
        .unwrap();
        let mut state = super::super::AppState::new();
        state.installation_id = Some("machine".into());
        state.access_credential_adapter = Some(authority.adapter.clone());
        assert!(restricted_router(state.clone(), route.clone(), authority.clone()).is_err());
        let auth_config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://labby.example").unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth.pem"),
            google: labby_auth::config::GoogleConfig {
                client_id: "fixture-client".into(),
                client_secret: "fixture-secret".into(),
                ..Default::default()
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap(),
            ),
            ..Default::default()
        };
        state.oauth_state = Some(std::sync::Arc::new(
            labby_auth::state::AuthState::new(auth_config)
                .await
                .unwrap(),
        ));
        let router = restricted_router(state.clone(), route.clone(), authority.clone()).unwrap();
        for (path, expected) in [
            ("/mcp", axum::http::StatusCode::UNAUTHORIZED),
            ("/v1/gateway", axum::http::StatusCode::NOT_FOUND),
            ("/health", axum::http::StatusCode::NOT_FOUND),
        ] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        state.web_ui_auth_disabled = true;
        assert!(restricted_router(state, route, authority).is_err());
    }
}
