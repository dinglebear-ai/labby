//! Authentication, challenge mapping, and credential binding for protected MCP routes.

use crate::api::error::ApiError;
use crate::api::router_middleware::derive_actor_key;
use crate::config::ProtectedMcpRouteEffectiveTarget;
use crate::dispatch::error::ToolError;
use axum::{
    body::Body,
    http::{HeaderValue, Request, header},
    response::IntoResponse,
};
use labby_primitives::product_credential::{
    BoundAccessGrant, ProductCredentialSelection, select_product_credential,
};

pub(in crate::api::router) fn auth_error_response_with_challenge(
    message: &str,
    metadata_url: &str,
    scopes: &[String],
) -> axum::response::Response {
    let err = ToolError::Sdk {
        sdk_kind: "auth_failed".into(),
        message: message.into(),
    };
    let mut response = ApiError::new(err).into_response();
    let scope = scopes
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let metadata_url = quoted_challenge_value(metadata_url);
    let scope = quoted_challenge_value(&scope);
    let www_auth = format!("Bearer resource_metadata=\"{metadata_url}\", scope=\"{scope}\"");
    let value = HeaderValue::from_str(&www_auth)
        .expect("Bearer challenge serializer emits header-safe ASCII");
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, value);
    response
}

pub(in crate::api::router) fn quoted_challenge_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'\\' => encoded.push_str("\\\\"),
            b'"' => encoded.push_str("\\\""),
            0x20..=0x7e => encoded.push(char::from(byte)),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

pub(super) fn route_resource_metadata_url(
    route: &crate::config::ProtectedMcpRouteConfig,
) -> String {
    format!(
        "https://{}/.well-known/oauth-protected-resource{}",
        route.public_host,
        route.public_path.trim_end_matches('/')
    )
}

pub(super) struct AuthenticatedProtectedRoute {
    pub(super) identity: Option<labby_auth::VerifiedIdentity>,
    pub(super) transport: Option<crate::mcp::bound_access::TransportCredentialBinding>,
    pub(super) product_bound: Option<BoundAccessGrant>,
}

#[cfg(feature = "gateway")]
pub(super) async fn authenticate_protected_route_request(
    request: &mut Request<Body>,
    route: &crate::config::ProtectedMcpRouteConfig,
    auth_state: Option<&labby_auth::state::AuthState>,
    product_adapter: Option<&crate::access::AccessCredentialAdapter>,
    actor_key_deriver: Option<&crate::observability::activity::ActorKeyDeriver>,
) -> Result<AuthenticatedProtectedRoute, axum::response::Response> {
    let resource = route.public_resource();
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(labby_auth::parse_bearer_token);
    let Some(token) = auth_header else {
        tracing::warn!(
            route = %route.name,
            resource = %resource,
            method = %request.method(),
            path = %request.uri().path(),
            "protected MCP route auth failed: missing bearer token"
        );
        return Err(auth_error_response_with_challenge(
            "missing bearer token",
            &route_resource_metadata_url(route),
            &route.scopes,
        ));
    };
    match select_product_credential(&token) {
        ProductCredentialSelection::Malformed(_) => {
            return Err(auth_error_response_with_challenge(
                "invalid bearer token",
                &route_resource_metadata_url(route),
                &route.scopes,
            ));
        }
        ProductCredentialSelection::Parsed(credential) => {
            let Some(adapter) = product_adapter else {
                return Err(auth_error_response_with_challenge(
                    "invalid bearer token",
                    &route_resource_metadata_url(route),
                    &route.scopes,
                ));
            };
            let effective_target = route.effective_target();
            let (project_id, loadout_id) = match &effective_target {
                ProtectedMcpRouteEffectiveTarget::GatewaySubset(target) => {
                    (target.project_id.as_deref(), target.loadout.as_deref())
                }
                _ => {
                    return Err(auth_error_response_with_challenge(
                        "invalid bearer token",
                        &route_resource_metadata_url(route),
                        &route.scopes,
                    ));
                }
            };
            let verified = adapter
                .bind_protected_route(
                    &credential,
                    crate::access::ProtectedCredentialRequirements {
                        route_id: &route.name,
                        resource: &resource,
                        project_id,
                        loadout_id,
                        scopes: &route.scopes,
                    },
                )
                .await
                .map_err(|_| {
                    auth_error_response_with_challenge(
                        "invalid bearer token",
                        &route_resource_metadata_url(route),
                        &route.scopes,
                    )
                })?;
            let source = verified.source;
            let bound = verified.bound;
            let identity = labby_auth::VerifiedIdentity::local_credential_with_issuer(
                labby_auth::Authenticator::ProductCredential,
                bound.issuer.clone(),
                bound.credential_id.clone(),
            )
            .map_err(|_| {
                auth_error_response_with_challenge(
                    "invalid authenticated identity",
                    &route_resource_metadata_url(route),
                    &route.scopes,
                )
            })?;
            let auth = labby_auth::AuthContext {
                actor_key: derive_actor_key(actor_key_deriver, &bound.principal_id),
                sub: bound.principal_id.clone(),
                scopes: bound.scopes.clone(),
                issuer: bound.issuer.clone(),
                via_session: false,
                csrf_token: None,
                email: None,
            };
            let transport = crate::mcp::bound_access::validated_product_transport_binding(
                &bound.issuer,
                &bound.credential_id,
                bound.credential_generation,
                bound.expires_at,
                std::time::SystemTime::now(),
            )
            .map_err(|_| {
                auth_error_response_with_challenge(
                    "invalid bearer token",
                    &route_resource_metadata_url(route),
                    &route.scopes,
                )
            })?;
            request.extensions_mut().insert(identity.clone());
            request.extensions_mut().insert(source);
            request.extensions_mut().insert(bound.clone());
            request.extensions_mut().insert(auth);
            return Ok(AuthenticatedProtectedRoute {
                identity: Some(identity),
                transport: Some(transport),
                product_bound: Some(bound),
            });
        }
        ProductCredentialSelection::NotProductCredential => {}
    }
    let Some(auth_state) = auth_state else {
        tracing::error!(
            route = %route.name,
            resource = %resource,
            "protected MCP route auth failed: oauth auth state missing"
        );
        return Err(auth_error_response_with_challenge(
            "oauth auth state is not configured",
            &route_resource_metadata_url(route),
            &route.scopes,
        ));
    };
    let Some(expected_issuer) = auth_state
        .config
        .public_url
        .as_ref()
        .map(|url| url.as_str().trim_end_matches('/').to_string())
    else {
        tracing::error!(
            route = %route.name,
            resource = %resource,
            "protected MCP route auth failed: LABBY_PUBLIC_URL missing"
        );
        return Err(auth_error_response_with_challenge(
            "server misconfigured: LABBY_PUBLIC_URL required for JWT validation",
            &route_resource_metadata_url(route),
            &route.scopes,
        ));
    };
    let claims = auth_state
        .signing_keys
        .validate_access_token_with_issuer(&token, &resource, &expected_issuer)
        .map_err(|error| {
            tracing::warn!(
                error = %error,
                route = %route.name,
                resource = %resource,
                method = %request.method(),
                path = %request.uri().path(),
                "protected MCP route auth failed: JWT validation failed"
            );
            auth_error_response_with_challenge(
                "invalid bearer token",
                &route_resource_metadata_url(route),
                &route.scopes,
            )
        })?;
    let required_scopes = route.scopes.iter().map(String::as_str).collect::<Vec<_>>();
    let granted = claims.scope.split_whitespace().collect::<Vec<_>>();
    let is_lab_admin = granted.iter().any(|s| *s == "lab:admin");
    if !is_lab_admin
        && !required_scopes
            .iter()
            .all(|required| granted.iter().any(|scope| scope == required))
    {
        tracing::warn!(
            route = %route.name,
            resource = %resource,
            subject_id = %labby_auth::util::fingerprint(&claims.sub),
            required_scopes = ?required_scopes,
            granted_scopes = ?granted,
            "protected MCP route auth failed: insufficient scope"
        );
        let mut response = ApiError::new(ToolError::Sdk {
            sdk_kind: "forbidden".into(),
            message: "insufficient OAuth scope for protected MCP route".into(),
        })
        .into_response();
        let scope = required_scopes.join(" ");
        let scope = quoted_challenge_value(&scope);
        let metadata_url = quoted_challenge_value(&route_resource_metadata_url(route));
        let challenge = format!(
            "Bearer error=\"insufficient_scope\", scope=\"{scope}\", resource_metadata=\"{}\"",
            metadata_url
        );
        let value = HeaderValue::from_str(&challenge)
            .expect("Bearer challenge serializer emits header-safe ASCII");
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
        return Err(response);
    }
    let subject_id = labby_auth::util::fingerprint(&claims.sub);
    let issuer = claims.iss.clone();
    let granted_scopes = granted.iter().map(|scope| (*scope).to_string()).collect();
    request
        .extensions_mut()
        .insert(crate::api::oauth::AuthContext {
            actor_key: derive_actor_key(actor_key_deriver, &claims.sub),
            sub: claims.sub.clone(),
            scopes: granted_scopes,
            issuer: claims.iss.clone(),
            via_session: false,
            csrf_token: None,
            email: None,
        });
    tracing::info!(
        route = %route.name,
        resource = %resource,
        subject_id = %subject_id,
        issuer = %issuer,
        granted_scopes = ?granted,
        "protected MCP route bearer and scope validation accepted"
    );
    let requires_project_binding = matches!(
        route.effective_target(),
        ProtectedMcpRouteEffectiveTarget::GatewaySubset(target)
            if target.project_id.is_some()
    );
    let identity = requires_project_binding
        .then(|| labby_auth::verified_identity_from_access_claims(&claims, &auth_state.config))
        .transpose()
        .map_err(|_| {
            auth_error_response_with_challenge(
                "invalid authenticated identity",
                &route_resource_metadata_url(route),
                &route.scopes,
            )
        })?;
    let transport = requires_project_binding
        .then(|| {
            crate::mcp::bound_access::validate_transport_credential_binding(
                &claims.iss,
                &claims.jti,
                claims.exp,
                std::time::SystemTime::now(),
            )
        })
        .transpose()
        .map_err(|_| {
            auth_error_response_with_challenge(
                "invalid bearer token",
                &route_resource_metadata_url(route),
                &route.scopes,
            )
        })?;
    Ok(AuthenticatedProtectedRoute {
        identity,
        transport,
        product_bound: None,
    })
}
