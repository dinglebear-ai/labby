//! Dual-mode bearer/JWT/cookie auth middleware shipped as a [`tower::Layer`].
//!
//! Consumers integrate with `.layer(AuthLayer::new(...))` rather than wrapping
//! a free `authenticate_request` function in a closure-of-7-args. The
//! middleware writes an [`AuthContext`] into request extensions on success,
//! returns an [`AuthError`]-shaped 401 response on failure, and (for cookie
//! mode + browser GETs) optionally redirects to a configured login path so the
//! Google OAuth flow can establish a session.
//!
//! Precedence (matches the legacy lab middleware):
//!
//! 1. `Authorization: Bearer <token>` matches the static bearer (constant-time
//!    compare) -> grants `static_token_scopes`.
//! 2. `Authorization: Bearer <token>` validates as a JWT issued by the local
//!    auth state (audience + issuer enforced inside
//!    [`crate::jwt::SigningKeys::validate_access_token_with_issuer`]) ->
//!    grants the JWT-claim scopes.
//! 3. (Optional, when [`AuthLayer`] was constructed with
//!    `allow_session_cookie = true`.) Browser session cookie matches a row in
//!    the auth store, with CSRF enforced for non-GET/HEAD/OPTIONS.
//! 4. Otherwise, browser GET requests with `Accept: text/html` are redirected
//!    to the configured login path; everything else returns 401 with
//!    `WWW-Authenticate: Bearer resource_metadata=...`.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use labby_primitives::product_credential::{
    BoundAccessGrant, ProductCredentialGrant, ProductCredentialSelection,
    ProductCredentialVerificationError, ProductCredentialVerifier, select_product_credential,
};
use subtle::ConstantTimeEq;
use tower::{Layer, Service};

use crate::auth_context::{AuthContext, www_authenticate_value};
use crate::error::AuthError;
use crate::metadata::canonical_resource_url;
use crate::project_session::ProjectSessionState;
use crate::session;
use crate::state::AuthState;
use crate::types::ProjectSessionBinding;
use crate::{Authenticator, VerifiedIdentity};

/// Closure-erased actor-key derivation hook.
///
/// Consumers that have a notion of an opaque actor identifier (lab uses an
/// HMAC over the JWT subject for non-PII observability) build one and pass
/// it through [`AuthLayer::with_actor_key_deriver`]. Consumers without this
/// concept (e.g. syslog-mcp) leave it unset.
///
/// The closure receives the JWT `sub` (or `"static-bearer"` /
/// browser-session subject) and returns a per-request [`Arc<str>`] key.
pub type ActorKeyDeriver = dyn Fn(&str) -> Option<Arc<str>> + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProjectSessionRevalidationError {
    #[error("project session is no longer authorized")]
    Denied,
    #[error("project session authorization is unavailable")]
    Unavailable,
}

pub type ProjectSessionRevalidationFuture<'a> = Pin<
    Box<dyn Future<Output = Result<BoundAccessGrant, ProjectSessionRevalidationError>> + Send + 'a>,
>;

/// Consumer-injected policy lookup. It is called on every request carrying a
/// project-bound browser session; no cached session row is sufficient by
/// itself to authorize a request.
pub trait ProjectSessionRevalidator: Send + Sync {
    fn revalidate<'a>(
        &'a self,
        binding: &'a ProjectSessionBinding,
    ) -> ProjectSessionRevalidationFuture<'a>;
}

pub type ProductAccessGrantResolutionFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<BoundAccessGrant, ProductCredentialVerificationError>>
            + Send
            + 'a,
    >,
>;

/// Consumer-owned product authorization lookup performed only after the
/// credential verifier has proven the source credential.
pub trait ProductAccessGrantResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        grant: &'a ProductCredentialGrant,
    ) -> ProductAccessGrantResolutionFuture<'a>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequiredScopes(Vec<String>);

impl RequiredScopes {
    #[must_use]
    pub fn new(scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut scopes = scopes.into_iter().map(Into::into).collect::<Vec<_>>();
        scopes.sort();
        scopes.dedup();
        Self(scopes)
    }

    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

impl From<Vec<String>> for RequiredScopes {
    fn from(scopes: Vec<String>) -> Self {
        Self::new(scopes)
    }
}

/// Tower layer that authenticates inbound requests and writes
/// [`AuthContext`] into request extensions.
///
/// Construct via [`AuthLayer::new`] and customize with the chained
/// `with_*` helpers.
#[derive(Clone)]
pub struct AuthLayer {
    inner: Arc<AuthLayerInner>,
}

#[derive(Clone)]
struct AuthLayerInner {
    static_token: Option<Arc<str>>,
    auth_state: Option<Arc<AuthState>>,
    actor_key_deriver: Option<Arc<ActorKeyDeriver>>,
    resource_url: Option<Arc<str>>,
    protected_resource_metadata_url: Option<Arc<str>>,
    required_scopes: RequiredScopes,
    allow_session_cookie: bool,
    /// Scopes minted into the [`AuthContext`] when the static bearer or
    /// session-cookie path matches. For the static path this is the legacy
    /// `static_token_scopes` config; for the cookie path lab keeps the same
    /// list (browser-session subjects are admin-equivalent today).
    static_token_scopes: Vec<String>,
    /// Browser login path used for the GET+text/html unauthenticated
    /// redirect (when `allow_session_cookie` is `true`). Defaults to
    /// `/auth/login` per [`crate::config::DEFAULT_LOGIN_PATH`].
    login_path: String,
    /// Browser session cookie name. Read from
    /// [`crate::config::AuthConfig::session_cookie_name`] when an
    /// `auth_state` is supplied; otherwise this is unused.
    session_cookie_name: String,
    /// Optional consumer-owned error projection. Authentication decisions stay
    /// in this crate while products can preserve their public error contract.
    error_response_mapper: Option<Arc<dyn Fn(AuthError) -> Response + Send + Sync>>,
    project_session_revalidator: Option<Arc<dyn ProjectSessionRevalidator>>,
    product_credential_verifier: Option<Arc<dyn ProductCredentialVerifier>>,
    product_access_grant_resolver: Option<Arc<dyn ProductAccessGrantResolver>>,
    project_session_state: Option<Arc<ProjectSessionState>>,
}

impl AuthLayer {
    /// Build a bearer-only layer with neither a static token nor an auth
    /// state. Such a layer always rejects requests with 401 — useful only
    /// as a placeholder; real consumers immediately chain at least one of
    /// [`Self::with_static_token`] / [`Self::with_auth_state`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AuthLayerInner {
                static_token: None,
                auth_state: None,
                actor_key_deriver: None,
                resource_url: None,
                protected_resource_metadata_url: None,
                required_scopes: RequiredScopes::default(),
                allow_session_cookie: false,
                static_token_scopes: Vec::new(),
                login_path: crate::config::DEFAULT_LOGIN_PATH.to_string(),
                session_cookie_name: crate::config::DEFAULT_SESSION_COOKIE_NAME.to_string(),
                error_response_mapper: None,
                project_session_revalidator: None,
                product_credential_verifier: None,
                product_access_grant_resolver: None,
                project_session_state: None,
            }),
        }
    }

    /// Convenience constructor that pulls
    /// `static_token_scopes`, `login_path`, and `session_cookie_name`
    /// directly from the supplied [`AuthState`]'s config — typically the
    /// only call sites consumers need.
    #[must_use]
    pub fn from_state(auth_state: Arc<AuthState>) -> Self {
        let cfg = &auth_state.config;
        let static_token_scopes = cfg.static_token_scopes.clone();
        let login_path = cfg.login_path.clone();
        let session_cookie_name = cfg.session_cookie_name.clone();
        Self {
            inner: Arc::new(AuthLayerInner {
                static_token: None,
                auth_state: Some(auth_state),
                actor_key_deriver: None,
                resource_url: None,
                protected_resource_metadata_url: None,
                required_scopes: RequiredScopes::default(),
                allow_session_cookie: false,
                static_token_scopes,
                login_path,
                session_cookie_name,
                error_response_mapper: None,
                project_session_revalidator: None,
                product_credential_verifier: None,
                product_access_grant_resolver: None,
                project_session_state: None,
            }),
        }
    }

    fn with(mut self, mutate: impl FnOnce(&mut AuthLayerInner)) -> Self {
        let inner = Arc::make_mut(&mut self.inner);
        mutate(inner);
        self
    }

    #[must_use]
    pub fn with_static_token(self, token: Option<Arc<str>>) -> Self {
        self.with(|inner| inner.static_token = token)
    }

    #[must_use]
    pub fn with_auth_state(self, state: Option<Arc<AuthState>>) -> Self {
        self.with(|inner| {
            if let Some(state) = state.as_ref() {
                let cfg = &state.config;
                inner.static_token_scopes = cfg.static_token_scopes.clone();
                inner.login_path = cfg.login_path.clone();
                inner.session_cookie_name = cfg.session_cookie_name.clone();
            }
            inner.auth_state = state;
        })
    }

    #[must_use]
    pub fn with_actor_key_deriver(self, deriver: Option<Arc<ActorKeyDeriver>>) -> Self {
        self.with(|inner| inner.actor_key_deriver = deriver)
    }

    #[must_use]
    pub fn with_resource_url(self, resource_url: Option<Arc<str>>) -> Self {
        self.with(|inner| inner.resource_url = resource_url)
    }

    #[must_use]
    pub fn with_protected_resource_metadata_url(self, url: Option<Arc<str>>) -> Self {
        self.with(|inner| inner.protected_resource_metadata_url = url)
    }

    #[must_use]
    pub fn with_required_scopes(self, scopes: impl Into<RequiredScopes>) -> Self {
        self.with(|inner| inner.required_scopes = scopes.into())
    }

    #[must_use]
    pub fn with_allow_session_cookie(self, allow: bool) -> Self {
        self.with(|inner| inner.allow_session_cookie = allow)
    }

    /// Override the static-token scope list (defaults to the value pulled
    /// from `AuthConfig::static_token_scopes` via [`Self::from_state`] /
    /// [`Self::with_auth_state`]).
    #[must_use]
    pub fn with_static_token_scopes(self, scopes: Vec<String>) -> Self {
        self.with(|inner| inner.static_token_scopes = scopes)
    }

    /// Override the browser login path used for the GET+text/html
    /// unauthenticated redirect.
    #[must_use]
    pub fn with_login_path(self, path: impl Into<String>) -> Self {
        self.with(|inner| inner.login_path = path.into())
    }

    /// Override the session cookie name read from inbound requests.
    #[must_use]
    pub fn with_session_cookie_name(self, name: impl Into<String>) -> Self {
        self.with(|inner| inner.session_cookie_name = name.into())
    }

    /// Project authentication failures into a consumer-specific response
    /// envelope without duplicating authentication policy in the consumer.
    #[must_use]
    pub fn with_error_response_mapper(
        self,
        mapper: impl Fn(AuthError) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.with(|inner| inner.error_response_mapper = Some(Arc::new(mapper)))
    }

    #[must_use]
    pub fn with_project_session_revalidator(
        self,
        revalidator: Arc<dyn ProjectSessionRevalidator>,
    ) -> Self {
        self.with(|inner| inner.project_session_revalidator = Some(revalidator))
    }

    #[must_use]
    pub fn with_product_credential_verifier(
        self,
        verifier: Arc<dyn ProductCredentialVerifier>,
    ) -> Self {
        self.with(|inner| inner.product_credential_verifier = Some(verifier))
    }

    #[must_use]
    pub fn with_product_access_grant_resolver(
        self,
        resolver: Arc<dyn ProductAccessGrantResolver>,
    ) -> Self {
        self.with(|inner| inner.product_access_grant_resolver = Some(resolver))
    }

    #[must_use]
    pub fn with_project_session_state(self, state: Option<Arc<ProjectSessionState>>) -> Self {
        self.with(|inner| inner.project_session_state = state)
    }
}

impl Default for AuthLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            layer: self.inner.clone(),
        }
    }
}

/// Service half of [`AuthLayer`]. Forwards to `inner` after a successful
/// authentication; otherwise short-circuits with a 401 / redirect response.
#[derive(Clone)]
pub struct AuthService<S> {
    inner: S,
    layer: Arc<AuthLayerInner>,
}

impl<S> Service<Request<Body>> for AuthService<S>
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        // Per tower::Service contract, `call` may take a stale `self.inner`
        // because Service callers clone the service before calling. We follow
        // the standard tower middleware idiom: clone, then swap so the
        // freshly-readied service is the one we call.
        let clone = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, clone);
        let layer = self.layer.clone();
        Box::pin(authenticate_and_forward(layer, inner, request))
    }
}

async fn authenticate_and_forward<S>(
    layer: Arc<AuthLayerInner>,
    mut inner: S,
    request: Request<Body>,
) -> Result<Response, Infallible>
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Send,
    S::Future: Send,
{
    match authenticate(&layer, request).await {
        Ok(request) => inner.call(request).await,
        Err(response) => Ok(response),
    }
}

/// Core authentication routine. Returns the (possibly mutated) request on
/// success so the wrapping Service can forward it; returns a finished
/// [`Response`] on failure (401, redirect, etc.).
async fn authenticate(
    layer: &AuthLayerInner,
    mut request: Request<Body>,
) -> Result<Request<Body>, Response> {
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_bearer_token);

    // A project cookie and bearer token are two independent authorities. Do
    // not let header precedence silently combine them. Legacy Google sessions
    // retain the historical bearer-first behavior.
    if auth_header.is_some()
        && layer.allow_session_cookie
        && let Some(session_state) = layer.project_session_state.as_ref()
        && let Some(session_id) =
            session::read_cookie(request.headers(), &session_state.cookie_name)
    {
        match session_state.store.find_browser_session(&session_id).await {
            Ok(Some(_)) => {
                return Err(auth_error_response(
                    "project session cookie cannot be combined with bearer authorization",
                    layer,
                ));
            }
            Err(_) => return Err(project_session_unavailable_response(layer)),
            Ok(None) => {}
        }
    }
    if auth_header.is_some()
        && layer.allow_session_cookie
        && let Some(auth_state) = layer.auth_state.as_ref()
        && let Some(session_id) =
            session::read_cookie(request.headers(), &layer.session_cookie_name)
    {
        match auth_state.store.find_browser_session(&session_id).await {
            Ok(Some(session)) if session.project_binding.is_some() => {
                return Err(auth_error_response(
                    "project session cookie cannot be combined with bearer authorization",
                    layer,
                ));
            }
            Err(_) => return Err(project_session_unavailable_response(layer)),
            Ok(_) => {}
        }
    }

    if let Some(token) = auth_header {
        match select_product_credential(&token) {
            ProductCredentialSelection::Malformed(_) => {
                return Err(product_credential_denied_response(layer));
            }
            ProductCredentialSelection::Parsed(credential) => {
                let verifier = layer
                    .product_credential_verifier
                    .as_ref()
                    .ok_or_else(|| product_credential_denied_response(layer))?;
                let source_grant =
                    verifier
                        .verify(&credential)
                        .await
                        .map_err(|error| match error {
                            ProductCredentialVerificationError::Denied => {
                                product_credential_denied_response(layer)
                            }
                            ProductCredentialVerificationError::Unavailable => {
                                product_credential_unavailable_response(layer)
                            }
                        })?;
                let resolver = layer
                    .product_access_grant_resolver
                    .as_ref()
                    .ok_or_else(|| product_credential_denied_response(layer))?;
                let bound_grant =
                    resolver
                        .resolve(&source_grant)
                        .await
                        .map_err(|error| match error {
                            ProductCredentialVerificationError::Denied => {
                                product_credential_denied_response(layer)
                            }
                            ProductCredentialVerificationError::Unavailable => {
                                product_credential_unavailable_response(layer)
                            }
                        })?;
                if !source_grant_matches_bound(&source_grant, &bound_grant) {
                    return Err(product_credential_denied_response(layer));
                }
                let identity = VerifiedIdentity::local_credential_with_issuer(
                    Authenticator::ProductCredential,
                    bound_grant.issuer.clone(),
                    bound_grant.credential_id.clone(),
                )
                .map_err(|_| product_credential_denied_response(layer))?;
                let auth = AuthContext {
                    actor_key: derive_actor_key(
                        layer.actor_key_deriver.as_deref(),
                        &bound_grant.principal_id,
                    ),
                    sub: bound_grant.principal_id.clone(),
                    scopes: bound_grant.scopes.clone(),
                    issuer: bound_grant.issuer.clone(),
                    via_session: false,
                    csrf_token: None,
                    email: None,
                };
                if let Some(response) = insufficient_scope_response(layer, &auth.scopes) {
                    return Err(response);
                }
                request.extensions_mut().insert(identity);
                request.extensions_mut().insert(source_grant);
                request.extensions_mut().insert(bound_grant);
                request.extensions_mut().insert(auth);
                return Ok(request);
            }
            ProductCredentialSelection::NotProductCredential => {}
        }

        // 1. Static bearer match — skipped when the consumer has set
        //    `disable_static_token_with_oauth=true` and OAuth mode is active.
        let static_token_blocked = layer.auth_state.as_ref().is_some_and(|s| {
            s.config.disable_static_token_with_oauth
                && matches!(s.config.mode, crate::config::AuthMode::OAuth)
        });
        if !static_token_blocked
            && let Some(ref expected) = layer.static_token
            && tokens_equal(&token, expected.as_ref())
        {
            let sub = "static-bearer".to_string();
            let identity = VerifiedIdentity::local_credential(
                Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .expect("the configured static bearer slot has a stable non-empty identity");
            let actor_key = derive_actor_key(layer.actor_key_deriver.as_deref(), &sub);
            let auth = AuthContext {
                sub,
                actor_key,
                scopes: layer.static_token_scopes.clone(),
                issuer: "local".to_string(),
                via_session: false,
                csrf_token: None,
                email: None,
            };
            if let Some(response) = insufficient_scope_response(layer, &auth.scopes) {
                return Err(response);
            }
            request.extensions_mut().insert(identity);
            request.extensions_mut().insert(auth);
            return Ok(request);
        }

        // 2. JWT validation.
        if let Some(ref auth_state) = layer.auth_state {
            let Some(expected_issuer) = auth_state
                .config
                .public_url
                .as_ref()
                .map(|url| url.as_str().trim_end_matches('/').to_string())
            else {
                return Err(auth_error_response(
                    &format!(
                        "server misconfigured: {}_PUBLIC_URL required for JWT validation",
                        auth_state.config.env_prefix
                    ),
                    layer,
                ));
            };
            let expected_aud = layer
                .resource_url
                .as_deref()
                .map_or_else(|| canonical_resource_url(auth_state), str::to_string);
            match auth_state.signing_keys.validate_access_token_with_issuer(
                &token,
                &expected_aud,
                &expected_issuer,
            ) {
                Ok(claims) => {
                    let identity = match (
                        claims.identity_issuer.as_deref(),
                        claims.identity_credential_id.as_deref(),
                    ) {
                        // Access tokens minted before identity provenance was added remain
                        // valid for ordinary authenticated routes. Deliberately omit the
                        // VerifiedIdentity extension so identity-gated boundaries fail closed.
                        (None, None) => None,
                        _ => Some(
                            crate::verified_identity_from_access_claims(
                                &claims,
                                &auth_state.config,
                            )
                            .map_err(|_| {
                                auth_error_response("invalid authenticated identity", layer)
                            })?,
                        ),
                    };
                    let actor_key =
                        derive_actor_key(layer.actor_key_deriver.as_deref(), &claims.sub);
                    let auth = AuthContext {
                        actor_key,
                        sub: claims.sub,
                        scopes: claims
                            .scope
                            .split_whitespace()
                            .filter(|scope| !scope.is_empty())
                            .map(ToOwned::to_owned)
                            .collect(),
                        issuer: claims.iss,
                        via_session: false,
                        csrf_token: None,
                        email: None,
                    };
                    if let Some(response) = insufficient_scope_response(layer, &auth.scopes) {
                        return Err(response);
                    }
                    if let Some(identity) = identity {
                        request.extensions_mut().insert(identity);
                    }
                    request.extensions_mut().insert(auth);
                    return Ok(request);
                }
                Err(error) => {
                    // `jwt.rs` already emits one WARN naming the reason this
                    // token was refused. Repeating it here would triple the
                    // log volume of a single unauthenticated request — and
                    // this path runs *before* authn with no rate limiter, so
                    // an attacker could use the amplification to push genuine
                    // security events out of journald's rate-limit burst.
                    // Keep the extra context at debug.
                    tracing::debug!(error = %error, "bearer token rejected: JWT validation failed");
                }
            }
        }

        tracing::debug!("request rejected: bearer token matched no static token and no valid JWT");
        return Err(auth_error_response("invalid bearer token", layer));
    }

    // 3. Browser session cookie path.
    if layer.allow_session_cookie
        && let Some(session_state) = layer.project_session_state.as_ref()
        && let Some(session_id) =
            session::read_cookie(request.headers(), &session_state.cookie_name)
    {
        let session = session_state
            .store
            .find_browser_session(&session_id)
            .await
            .map_err(|_| project_session_unavailable_response(layer))?
            .ok_or_else(|| auth_error_response("invalid project session", layer))?;
        let binding = session
            .project_binding
            .as_ref()
            .ok_or_else(|| auth_error_response("invalid project session", layer))?;
        if !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        ) {
            let csrf = request
                .headers()
                .get(session::BROWSER_CSRF_HEADER_NAME)
                .and_then(|value| value.to_str().ok());
            if csrf != Some(session.csrf_token.as_str()) {
                return Err(csrf_error_response("missing or invalid csrf token"));
            }
        }
        let revalidator = layer
            .project_session_revalidator
            .as_ref()
            .ok_or_else(|| project_session_unavailable_response(layer))?;
        let grant = match revalidator.revalidate(binding).await {
            Ok(grant) => grant,
            Err(ProjectSessionRevalidationError::Denied) => {
                let _revocation_result = session_state
                    .store
                    .revoke_browser_session(&session.session_id)
                    .await;
                return Err(auth_error_response(
                    "project session is no longer authorized",
                    layer,
                ));
            }
            Err(ProjectSessionRevalidationError::Unavailable) => {
                return Err(project_session_unavailable_response(layer));
            }
        };
        if !binding_matches_grant(binding, &grant) {
            let _revocation_result = session_state
                .store
                .revoke_browser_session(&session.session_id)
                .await;
            return Err(auth_error_response(
                "project session authorization binding changed",
                layer,
            ));
        }
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::BrowserSession,
            binding.issuer.clone(),
            binding.source_credential_id.clone(),
        )
        .map_err(|_| auth_error_response("invalid authenticated identity", layer))?;
        let auth = AuthContext {
            actor_key: derive_actor_key(layer.actor_key_deriver.as_deref(), &binding.principal_id),
            sub: binding.principal_id.clone(),
            scopes: grant.scopes.clone(),
            issuer: binding.issuer.clone(),
            via_session: true,
            csrf_token: Some(session.csrf_token),
            email: None,
        };
        if let Some(response) = insufficient_scope_response(layer, &auth.scopes) {
            return Err(response);
        }
        request.extensions_mut().insert(identity);
        request.extensions_mut().insert(ProductCredentialGrant {
            issuer: binding.issuer.clone(),
            subject: binding.subject.clone(),
            credential_id: binding.source_credential_id.clone(),
            credential_generation: binding.source_credential_generation,
            scopes: binding.scopes.clone(),
            resource: binding.resource.clone(),
            audience: binding.audience.clone(),
            expires_at: binding.source_credential_expires_at,
        });
        request.extensions_mut().insert(grant);
        request.extensions_mut().insert(auth);
        return Ok(request);
    }

    if layer.allow_session_cookie
        && let Some(auth_state) = layer.auth_state.as_ref()
        && let Some(session_id) =
            session::read_cookie(request.headers(), &layer.session_cookie_name)
    {
        match auth_state.store.find_browser_session(&session_id).await {
            Ok(Some(session)) => {
                if !matches!(
                    *request.method(),
                    Method::GET | Method::HEAD | Method::OPTIONS
                ) {
                    let csrf = request
                        .headers()
                        .get(session::BROWSER_CSRF_HEADER_NAME)
                        .and_then(|value| value.to_str().ok());
                    if csrf != Some(session.csrf_token.as_str()) {
                        return Err(csrf_error_response("missing or invalid csrf token"));
                    }
                }

                if let Some(binding) = session.project_binding.as_ref() {
                    let revalidator = layer
                        .project_session_revalidator
                        .as_ref()
                        .ok_or_else(|| project_session_unavailable_response(layer))?;
                    let grant = match revalidator.revalidate(binding).await {
                        Ok(grant) => grant,
                        Err(ProjectSessionRevalidationError::Denied) => {
                            let _revocation_result = auth_state
                                .store
                                .revoke_browser_session(&session.session_id)
                                .await;
                            return Err(auth_error_response(
                                "project session is no longer authorized",
                                layer,
                            ));
                        }
                        Err(ProjectSessionRevalidationError::Unavailable) => {
                            return Err(project_session_unavailable_response(layer));
                        }
                    };
                    if !binding_matches_grant(binding, &grant) {
                        let _revocation_result = auth_state
                            .store
                            .revoke_browser_session(&session.session_id)
                            .await;
                        return Err(auth_error_response(
                            "project session authorization binding changed",
                            layer,
                        ));
                    }
                    let identity = VerifiedIdentity::local_credential_with_issuer(
                        Authenticator::BrowserSession,
                        binding.issuer.clone(),
                        binding.source_credential_id.clone(),
                    )
                    .map_err(|_| auth_error_response("invalid authenticated identity", layer))?;
                    let actor_key =
                        derive_actor_key(layer.actor_key_deriver.as_deref(), &binding.principal_id);
                    let auth = AuthContext {
                        actor_key,
                        sub: binding.principal_id.clone(),
                        scopes: grant.scopes.clone(),
                        issuer: binding.issuer.clone(),
                        via_session: true,
                        csrf_token: Some(session.csrf_token),
                        email: None,
                    };
                    if let Some(response) = insufficient_scope_response(layer, &auth.scopes) {
                        return Err(response);
                    }
                    request.extensions_mut().insert(identity);
                    request.extensions_mut().insert(ProductCredentialGrant {
                        issuer: binding.issuer.clone(),
                        subject: binding.subject.clone(),
                        credential_id: binding.source_credential_id.clone(),
                        credential_generation: binding.source_credential_generation,
                        scopes: binding.scopes.clone(),
                        resource: binding.resource.clone(),
                        audience: binding.audience.clone(),
                        expires_at: binding.source_credential_expires_at,
                    });
                    request.extensions_mut().insert(grant);
                    request.extensions_mut().insert(auth);
                    return Ok(request);
                }

                let identity = VerifiedIdentity::external(
                    Authenticator::BrowserSession,
                    crate::google::GOOGLE_ISSUER,
                    session.subject.clone(),
                )
                .map_err(|_| auth_error_response("invalid authenticated identity", layer))?;
                let actor_key =
                    derive_actor_key(layer.actor_key_deriver.as_deref(), &session.subject);
                let auth = AuthContext {
                    actor_key,
                    sub: session.subject,
                    scopes: layer.static_token_scopes.clone(),
                    issuer: "browser-session".to_string(),
                    via_session: true,
                    csrf_token: Some(session.csrf_token),
                    email: session.email,
                };
                if let Some(response) = insufficient_scope_response(layer, &auth.scopes) {
                    return Err(response);
                }
                request.extensions_mut().insert(identity);
                request.extensions_mut().insert(auth);
                return Ok(request);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(error = %error, "browser session lookup failed");
            }
        }
    }

    // 4. Browser GET → redirect to login_path.
    if layer.allow_session_cookie
        && layer.auth_state.is_some()
        && *request.method() == Method::GET
        && request
            .headers()
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|accept| accept.contains("text/html"))
    {
        let return_to = request
            .uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        let encoded = percent_encode_path(return_to);
        let login_url = format!("{}?return_to={encoded}", layer.login_path);
        return Err(Redirect::to(&login_url).into_response());
    }

    Err(auth_error_response(
        if layer.allow_session_cookie {
            "missing bearer token or session cookie"
        } else {
            "missing bearer token"
        },
        layer,
    ))
}

fn binding_matches_grant(binding: &ProjectSessionBinding, grant: &BoundAccessGrant) -> bool {
    binding == &ProjectSessionBinding::from(grant)
}

fn source_grant_matches_bound(source: &ProductCredentialGrant, bound: &BoundAccessGrant) -> bool {
    source.issuer == bound.issuer
        && source.subject == bound.subject
        && source.credential_id == bound.credential_id
        && source.credential_generation == bound.credential_generation
        && source.scopes == bound.scopes
        && source.resource == bound.resource
        && source.audience == bound.audience
        && source.expires_at == bound.expires_at
}

fn product_credential_denied_response(layer: &AuthLayerInner) -> Response {
    auth_error_response("invalid product credential", layer)
}

fn product_credential_unavailable_response(layer: &AuthLayerInner) -> Response {
    let error = AuthError::Server("product credential verification is unavailable".into());
    if let Some(mapper) = layer.error_response_mapper.as_ref() {
        mapper(error)
    } else {
        error.into_response()
    }
}

fn project_session_unavailable_response(layer: &AuthLayerInner) -> Response {
    let error = AuthError::Server("project session authorization is unavailable".into());
    if let Some(mapper) = layer.error_response_mapper.as_ref() {
        mapper(error)
    } else {
        error.into_response()
    }
}

/// Constant-time byte comparison for static-bearer matching (prevents
/// timing-based prefix leakage).
#[must_use]
pub fn tokens_equal(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// Parse a single `Authorization: Bearer <token>` header value, returning
/// `None` for malformed or non-Bearer schemes.
#[must_use]
pub fn parse_bearer_token(header_value: &str) -> Option<String> {
    let mut parts = header_value.split_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    if parts.next().is_some() || !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Some(token.to_string())
}

fn derive_actor_key(deriver: Option<&ActorKeyDeriver>, subject: &str) -> Option<Arc<str>> {
    deriver.and_then(|deriver| deriver(subject))
}

/// Build a 401 response wrapping [`AuthError::AuthFailed`] and decorate it
/// with `WWW-Authenticate` when a `resource_url` was supplied.
fn auth_error_response(message: &str, layer: &AuthLayerInner) -> Response {
    let error = AuthError::AuthFailed(message.to_string());
    let kind = error.kind();
    let mut response = if let Some(mapper) = layer.error_response_mapper.as_ref() {
        mapper(error)
    } else {
        error.into_response()
    };
    response
        .extensions_mut()
        .insert(crate::error::AuthErrorKind(kind));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    let challenge = layer
        .protected_resource_metadata_url
        .as_deref()
        .map(|url| format!("Bearer resource_metadata=\"{url}\""))
        .or_else(|| layer.resource_url.as_deref().map(www_authenticate_value));
    if let Some(challenge) = challenge {
        let www_auth = format!(
            "{}, scope=\"{}\"",
            challenge,
            challenge_scopes(layer).join(" ")
        );
        if let Ok(value) = HeaderValue::from_str(&www_auth) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
    }
    response
}

fn challenge_scopes(layer: &AuthLayerInner) -> &[String] {
    if !layer.required_scopes.as_slice().is_empty() {
        return layer.required_scopes.as_slice();
    }
    layer
        .auth_state
        .as_ref()
        .map_or(layer.static_token_scopes.as_slice(), |state| {
            state.config.scopes_supported.as_slice()
        })
}

fn insufficient_scope_response(layer: &AuthLayerInner, granted: &[String]) -> Option<Response> {
    let required = layer.required_scopes.as_slice();
    if required
        .iter()
        .all(|scope| granted.iter().any(|granted| granted == scope))
    {
        return None;
    }
    let metadata_url = layer
        .protected_resource_metadata_url
        .as_deref()
        .map(str::to_string)
        .or_else(|| layer.resource_url.as_deref().map(metadata_url_for_resource));
    let mut response = (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "kind": "insufficient_scope",
            "message": "authenticated principal lacks required scope",
        })),
    )
        .into_response();
    if let Some(metadata_url) = metadata_url {
        let challenge = format!(
            "Bearer error=\"insufficient_scope\", scope=\"{}\", resource_metadata=\"{}\"",
            required.join(" "),
            metadata_url
        );
        if let Ok(value) = HeaderValue::from_str(&challenge) {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
    }
    Some(response)
}

fn metadata_url_for_resource(resource: &str) -> String {
    crate::auth_context::protected_resource_metadata_url(resource)
}

fn csrf_error_response(message: &str) -> Response {
    AuthError::Validation(message.to_string()).into_response()
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        // Do NOT include `?` here — when return_to is used as a query-string
        // value a literal `?` would be interpreted as the start of a nested
        // query string by the redirect target.
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit(u32::from(b >> 4), 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit(u32::from(b & 0xf), 16)
                    .unwrap()
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::{get, post};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tower::ServiceExt;

    use crate::authorize::tests::{test_auth_config, test_auth_state, test_auth_state_with_config};
    use crate::{PrincipalLink, VerifiedIdentity};

    fn project_binding() -> ProjectSessionBinding {
        ProjectSessionBinding {
            installation_id: "install-1".into(),
            issuer: "https://issuer.example".into(),
            subject: "operator-1".into(),
            principal_id: "principal-1".into(),
            organization_id: "org-1".into(),
            project_id: "project-1".into(),
            loadout_id: "loadout-1".into(),
            loadout_generation: 2,
            assignment_generation: 3,
            catalog_generation: 4,
            route_id: "route-1".into(),
            route_generation: 5,
            membership_epoch: 6,
            organization_policy_epoch: 7,
            project_policy_epoch: 8,
            source_credential_id: "credential-1".into(),
            source_credential_generation: 9,
            scopes: vec!["lab:read".into()],
            resource: "lab://project-1".into(),
            audience: "labby".into(),
            source_credential_expires_at: u64::try_from(crate::util::now_unix() + 3_600).unwrap(),
        }
    }

    fn bound_grant(binding: &ProjectSessionBinding) -> BoundAccessGrant {
        BoundAccessGrant {
            installation_id: binding.installation_id.clone(),
            issuer: binding.issuer.clone(),
            subject: binding.subject.clone(),
            principal_id: binding.principal_id.clone(),
            organization_id: binding.organization_id.clone(),
            project_id: binding.project_id.clone(),
            loadout_id: binding.loadout_id.clone(),
            loadout_generation: binding.loadout_generation,
            assignment_generation: binding.assignment_generation,
            catalog_generation: binding.catalog_generation,
            route_id: binding.route_id.clone(),
            route_generation: binding.route_generation,
            membership_epoch: binding.membership_epoch,
            organization_policy_epoch: binding.organization_policy_epoch,
            project_policy_epoch: binding.project_policy_epoch,
            credential_id: binding.source_credential_id.clone(),
            credential_generation: binding.source_credential_generation,
            scopes: binding.scopes.clone(),
            resource: binding.resource.clone(),
            audience: binding.audience.clone(),
            expires_at: binding.source_credential_expires_at,
            requires_admin: false,
            destructive: false,
        }
    }

    struct CountingRevalidator {
        calls: Arc<AtomicUsize>,
        grant: BoundAccessGrant,
    }

    struct ToggleRevalidator {
        denied: Arc<AtomicBool>,
        grant: BoundAccessGrant,
    }

    impl ProjectSessionRevalidator for ToggleRevalidator {
        fn revalidate<'a>(
            &'a self,
            _: &'a ProjectSessionBinding,
        ) -> ProjectSessionRevalidationFuture<'a> {
            let denied = self.denied.load(Ordering::SeqCst);
            let grant = self.grant.clone();
            Box::pin(async move {
                if denied {
                    Err(ProjectSessionRevalidationError::Denied)
                } else {
                    Ok(grant)
                }
            })
        }
    }

    struct StubProductVerifier {
        result: Result<ProductCredentialGrant, ProductCredentialVerificationError>,
        calls: Arc<AtomicUsize>,
    }

    impl ProductCredentialVerifier for StubProductVerifier {
        fn verify<'a>(
            &'a self,
            _: &'a labby_primitives::product_credential::ProductCredential,
        ) -> labby_primitives::product_credential::ProductCredentialVerificationFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    struct StubProductResolver {
        result: Result<BoundAccessGrant, ProductCredentialVerificationError>,
    }

    impl ProductAccessGrantResolver for StubProductResolver {
        fn resolve<'a>(
            &'a self,
            _: &'a ProductCredentialGrant,
        ) -> ProductAccessGrantResolutionFuture<'a> {
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    fn product_source_grant() -> ProductCredentialGrant {
        ProductCredentialGrant {
            issuer: "https://issuer.example".into(),
            subject: "operator-1".into(),
            credential_id: "credential-1".into(),
            credential_generation: 9,
            scopes: vec!["lab:read".into()],
            resource: "lab://project-1".into(),
            audience: "labby".into(),
            expires_at: u64::try_from(crate::util::now_unix() + 3_600).unwrap(),
        }
    }

    fn product_bound_grant(source: &ProductCredentialGrant) -> BoundAccessGrant {
        BoundAccessGrant {
            installation_id: "install-1".into(),
            issuer: source.issuer.clone(),
            subject: source.subject.clone(),
            principal_id: "principal-1".into(),
            organization_id: "org-1".into(),
            project_id: "project-1".into(),
            loadout_id: "loadout-1".into(),
            loadout_generation: 2,
            assignment_generation: 3,
            catalog_generation: 4,
            route_id: "route-1".into(),
            route_generation: 5,
            membership_epoch: 6,
            organization_policy_epoch: 7,
            project_policy_epoch: 8,
            credential_id: source.credential_id.clone(),
            credential_generation: source.credential_generation,
            scopes: source.scopes.clone(),
            resource: source.resource.clone(),
            audience: source.audience.clone(),
            expires_at: source.expires_at,
            requires_admin: false,
            destructive: false,
        }
    }

    fn product_token() -> &'static str {
        "lby_pc_v1_credential-1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    }

    impl ProjectSessionRevalidator for CountingRevalidator {
        fn revalidate<'a>(
            &'a self,
            _: &'a ProjectSessionBinding,
        ) -> ProjectSessionRevalidationFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let grant = self.grant.clone();
            Box::pin(async move { Ok(grant) })
        }
    }

    fn echo_app(layer: AuthLayer) -> Router {
        Router::new()
            .route("/probe", get(|| async { "ok" }))
            .route_layer(layer)
    }

    fn principal_link_app(layer: AuthLayer) -> Router {
        Router::new()
            .route(
                "/probe",
                get(
                    |axum::Extension(identity): axum::Extension<VerifiedIdentity>| async move {
                        match identity.principal_link() {
                            PrincipalLink::External { issuer, subject } => {
                                format!("external|{issuer}|{subject}")
                            }
                            PrincipalLink::LocalCredential { credential_id } => {
                                format!("local|{credential_id}")
                            }
                        }
                    },
                ),
            )
            .route_layer(layer)
    }

    #[test]
    fn parse_bearer_token_accepts_valid_header() {
        assert_eq!(
            parse_bearer_token("Bearer abc.def").as_deref(),
            Some("abc.def")
        );
        assert_eq!(
            parse_bearer_token("bearer abc.def").as_deref(),
            Some("abc.def")
        );
    }

    #[test]
    fn parse_bearer_token_rejects_malformed() {
        assert_eq!(parse_bearer_token("Basic abc.def"), None);
        assert_eq!(parse_bearer_token("Bearer"), None);
        assert_eq!(parse_bearer_token("Bearer one two"), None);
        assert_eq!(parse_bearer_token(""), None);
    }

    #[test]
    fn tokens_equal_distinguishes_unequal_strings() {
        assert!(tokens_equal("abc", "abc"));
        assert!(!tokens_equal("abc", "abd"));
        assert!(!tokens_equal("abc", "abcd"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_bearer_token_returns_401_with_www_authenticate() {
        let layer = AuthLayer::new().with_resource_url(Some(Arc::<str>::from(
            "https://lab.example.com:9443/reverse-proxy/base/mcp?ignored=secret",
        )));
        let app = echo_app(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let www = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            www,
            concat!(
                "Bearer resource_metadata=\"https://lab.example.com:9443/",
                ".well-known/oauth-protected-resource\", scope=\"\""
            )
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_bearer_match_grants_configured_scopes() {
        let token: Arc<str> = Arc::<str>::from("super-secret");
        let layer = AuthLayer::new()
            .with_static_token(Some(token.clone()))
            .with_static_token_scopes(vec!["syslog:read".to_string(), "syslog:admin".to_string()]);
        let app = Router::new()
            .route(
                "/probe",
                get(
                    |axum::Extension(ctx): axum::Extension<AuthContext>| async move {
                        ctx.scopes.join(",")
                    },
                ),
            )
            .route_layer(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer super-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"syslog:read,syslog:admin");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_bearer_emits_an_explicit_local_credential_link() {
        let app = principal_link_app(
            AuthLayer::new().with_static_token(Some(Arc::<str>::from("super-secret"))),
        );

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer super-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"local|static-bearer:primary");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wrong_static_bearer_rejected() {
        let layer = AuthLayer::new().with_static_token(Some(Arc::<str>::from("super-secret")));
        let app = echo_app(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn jwt_validation_path_accepts_signed_token_and_writes_context() {
        let state = Arc::new(test_auth_state().await);
        let aud = canonical_resource_url(&state);
        let iss = state
            .config
            .public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .unwrap();
        let claims = crate::jwt::AccessClaims {
            iss: iss.clone(),
            sub: "user@example.com".to_string(),
            aud: aud.clone(),
            exp: (crate::util::now_unix() + 60) as usize,
            nbf: None,
            iat: crate::util::now_unix() as usize,
            jti: "j-1".to_string(),
            scope: "syslog:read syslog:admin".to_string(),
            azp: String::new(),
            identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
            identity_credential_id: None,
        };
        let token = state.signing_keys.issue_access_token(&claims).unwrap();
        let layer = AuthLayer::from_state(state);
        let app = Router::new()
            .route(
                "/probe",
                get(
                    |axum::Extension(ctx): axum::Extension<AuthContext>| async move {
                        format!("{}|{}", ctx.sub, ctx.scopes.join(","))
                    },
                ),
            )
            .route_layer(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"user@example.com|syslog:read,syslog:admin");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn legacy_jwt_without_identity_provenance_authenticates_without_verified_identity() {
        let state = Arc::new(test_auth_state().await);
        let issuer = state
            .config
            .public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .unwrap();
        let token = state
            .signing_keys
            .issue_access_token(&crate::jwt::AccessClaims {
                iss: issuer,
                sub: "ambiguous-subject".to_string(),
                aud: canonical_resource_url(&state),
                exp: (crate::util::now_unix() + 60) as usize,
                nbf: None,
                iat: crate::util::now_unix() as usize,
                jti: "missing-identity-provenance".to_string(),
                scope: "lab:read".to_string(),
                azp: String::new(),
                identity_issuer: None,
                identity_credential_id: None,
            })
            .unwrap();
        let auth_context_app = Router::new()
            .route(
                "/probe",
                get(|axum::Extension(ctx): axum::Extension<AuthContext>| async move { ctx.sub }),
            )
            .route_layer(AuthLayer::from_state(Arc::clone(&state)));
        let response = auth_context_app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"ambiguous-subject");

        let identity_response = principal_link_app(AuthLayer::from_state(state))
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            identity_response.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn jwt_with_both_identity_provenance_kinds_fails_closed() {
        let state = Arc::new(test_auth_state().await);
        let issuer = state
            .config
            .public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .unwrap();
        let token = state
            .signing_keys
            .issue_access_token(&crate::jwt::AccessClaims {
                iss: issuer,
                sub: "ambiguous-subject".to_string(),
                aud: canonical_resource_url(&state),
                exp: (crate::util::now_unix() + 60) as usize,
                nbf: None,
                iat: crate::util::now_unix() as usize,
                jti: "conflicting-identity-provenance".to_string(),
                scope: "lab:read".to_string(),
                azp: String::new(),
                identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
                identity_credential_id: Some("machine-client:ambiguous".to_string()),
            })
            .unwrap();
        let response = echo_app(AuthLayer::from_state(state))
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn browser_session_and_jwt_emit_the_same_provider_principal_link() {
        let state = Arc::new(test_auth_state().await);
        let subject = "google-subject-123";
        let session = session::create_browser_session(
            &state,
            subject.to_string(),
            Some("user@example.com".to_string()),
        )
        .await
        .unwrap();
        let cookie_name = state.config.session_cookie_name.clone();
        let browser_app = principal_link_app(
            AuthLayer::from_state(Arc::clone(&state)).with_allow_session_cookie(true),
        );

        let audience = canonical_resource_url(&state);
        let issuer = state
            .config
            .public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .unwrap();
        let token = state
            .signing_keys
            .issue_access_token(&crate::jwt::AccessClaims {
                iss: issuer,
                sub: subject.to_string(),
                aud: audience,
                exp: (crate::util::now_unix() + 60) as usize,
                nbf: None,
                iat: crate::util::now_unix() as usize,
                jti: "identity-link-test".to_string(),
                scope: "lab:read".to_string(),
                azp: String::new(),
                identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
                identity_credential_id: None,
            })
            .unwrap();
        let bearer_app = principal_link_app(AuthLayer::from_state(state));

        let browser_response = browser_app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(
                        header::COOKIE,
                        format!("{cookie_name}={}", session.session_id),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bearer_response = bearer_app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(browser_response.status(), StatusCode::OK);
        assert_eq!(bearer_response.status(), StatusCode::OK);
        let browser_body = axum::body::to_bytes(browser_response.into_body(), 1024)
            .await
            .unwrap();
        let bearer_body = axum::body::to_bytes(bearer_response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(browser_body, bearer_body);
        assert_eq!(
            &browser_body[..],
            b"external|https://accounts.google.com|google-subject-123",
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn google_and_enterprise_tokens_with_same_subject_keep_distinct_provider_links() {
        let base = test_auth_state().await;
        let mut config = (*base.config).clone();
        config.enterprise_issuers = vec![crate::config::EnterpriseIssuerConfig {
            issuer: "https://idp.example.com/oidc/tenant/".to_string(),
            jwks_uri: None,
            jwks: None,
            allowed_client_ids: Vec::new(),
        }];
        let state = Arc::new(AuthState::for_tests(
            config,
            base.store.clone(),
            (*base.signing_keys).clone(),
            (*base.google).clone(),
        ));
        let transport_issuer = state
            .config
            .public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .unwrap();
        let audience = canonical_resource_url(&state);
        let issue = |identity_issuer: &str, jti: &str| {
            state
                .signing_keys
                .issue_access_token(&crate::jwt::AccessClaims {
                    iss: transport_issuer.clone(),
                    sub: "shared-subject".to_string(),
                    aud: audience.clone(),
                    exp: (crate::util::now_unix() + 60) as usize,
                    nbf: None,
                    iat: crate::util::now_unix() as usize,
                    jti: jti.to_string(),
                    scope: "lab:read".to_string(),
                    azp: String::new(),
                    identity_issuer: Some(identity_issuer.to_string()),
                    identity_credential_id: None,
                })
                .unwrap()
        };
        let google = issue(crate::google::GOOGLE_ISSUER, "google-provider-link");
        let enterprise = issue(
            "https://idp.example.com/oidc/tenant/",
            "enterprise-provider-link",
        );

        let call = |token: String| {
            principal_link_app(AuthLayer::from_state(Arc::clone(&state))).oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        let google_response = call(google).await.unwrap();
        let enterprise_response = call(enterprise).await.unwrap();
        let google_body = axum::body::to_bytes(google_response.into_body(), 1024)
            .await
            .unwrap();
        let enterprise_body = axum::body::to_bytes(enterprise_response.into_body(), 1024)
            .await
            .unwrap();

        assert_eq!(
            &google_body[..],
            b"external|https://accounts.google.com|shared-subject"
        );
        assert_eq!(
            &enterprise_body[..],
            b"external|https://idp.example.com/oidc/tenant|shared-subject"
        );
        assert_ne!(google_body, enterprise_body);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn jwt_with_wrong_issuer_rejected() {
        let state = Arc::new(test_auth_state().await);
        let aud = canonical_resource_url(&state);
        let claims = crate::jwt::AccessClaims {
            iss: "https://attacker.example.com".to_string(),
            sub: "user@example.com".to_string(),
            aud,
            exp: (crate::util::now_unix() + 60) as usize,
            nbf: None,
            iat: crate::util::now_unix() as usize,
            jti: "j-1".to_string(),
            scope: "syslog:read".to_string(),
            azp: String::new(),
            identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
            identity_credential_id: None,
        };
        let token = state.signing_keys.issue_access_token(&claims).unwrap();
        let layer = AuthLayer::from_state(state)
            .with_resource_url(Some(Arc::<str>::from("https://lab.example.com")));
        let app = echo_app(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn jwt_with_wrong_audience_rejected() {
        let state = Arc::new(test_auth_state().await);
        let iss = state
            .config
            .public_url
            .as_ref()
            .map(|url| url.as_str().trim_end_matches('/').to_string())
            .unwrap();
        let claims = crate::jwt::AccessClaims {
            iss,
            sub: "user@example.com".to_string(),
            aud: "https://other.example.com/mcp".to_string(),
            exp: (crate::util::now_unix() + 60) as usize,
            nbf: None,
            iat: crate::util::now_unix() as usize,
            jti: "j-1".to_string(),
            scope: "syslog:read".to_string(),
            azp: String::new(),
            identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
            identity_credential_id: None,
        };
        let token = state.signing_keys.issue_access_token(&claims).unwrap();
        let layer = AuthLayer::from_state(state);
        let app = echo_app(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn html_get_with_session_cookie_enabled_redirects_to_login_path() {
        let state = Arc::new(test_auth_state().await);
        let layer = AuthLayer::from_state(state)
            .with_allow_session_cookie(true)
            .with_login_path("/auth/login");
        let app = echo_app(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe?x=1")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            location.starts_with("/auth/login?return_to="),
            "unexpected redirect Location: `{location}`"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn html_get_uses_configured_login_path_override() {
        let state = Arc::new(test_auth_state().await);
        let layer = AuthLayer::from_state(state)
            .with_allow_session_cookie(true)
            .with_login_path("/syslog/auth/login");
        let app = echo_app(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            location.starts_with("/syslog/auth/login?return_to="),
            "unexpected redirect Location: `{location}`"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_bearer_blocked_when_disable_static_token_with_oauth_is_set() {
        // When disable_static_token_with_oauth=true and mode=OAuth, the static
        // token must be rejected even though the token value matches.
        let mut config = test_auth_config();
        config.disable_static_token_with_oauth = true;
        let state = Arc::new(test_auth_state_with_config(config).await);

        let token: Arc<str> = Arc::from("super-secret");
        let layer = AuthLayer::from_state(state).with_static_token(Some(token.clone()));
        let app = echo_app(layer);

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, "Bearer super-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Must be 401 — static token blocked because OAuth is active.
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exact_resource_audience_including_port_and_path_is_enforced() {
        let state = Arc::new(test_auth_state().await);
        let issuer = state
            .config
            .public_url
            .as_ref()
            .unwrap()
            .as_str()
            .trim_end_matches('/')
            .to_string();
        let exact_resource = "https://proxy.example:53147/mcp";
        let token = state
            .signing_keys
            .issue_access_token(&crate::jwt::AccessClaims {
                iss: issuer,
                sub: "user@example.com".to_string(),
                aud: exact_resource.to_string(),
                exp: (crate::util::now_unix() + 60) as usize,
                nbf: None,
                iat: crate::util::now_unix() as usize,
                jti: "exact-resource".to_string(),
                scope: "mcp:read".to_string(),
                azp: String::new(),
                identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
                identity_credential_id: None,
            })
            .unwrap();

        for (configured_resource, expected) in [
            (exact_resource, StatusCode::OK),
            ("https://proxy.example:53148/mcp", StatusCode::UNAUTHORIZED),
            (
                "https://proxy.example:53147/other",
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let app = echo_app(
                AuthLayer::from_state(Arc::clone(&state))
                    .with_resource_url(Some(Arc::<str>::from(configured_resource)))
                    .with_required_scopes(vec!["mcp:read".to_string()]),
            );
            let response = app
                .oneshot(
                    HttpRequest::builder()
                        .uri("/probe")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                expected,
                "resource {configured_resource}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn insufficient_jwt_and_static_scopes_return_403_challenge() {
        let state = Arc::new(test_auth_state().await);
        let resource = "https://proxy.example:53147/mcp";
        let metadata = "https://proxy.example:53147/.well-known/oauth-protected-resource";
        let issuer = state
            .config
            .public_url
            .as_ref()
            .unwrap()
            .as_str()
            .trim_end_matches('/');
        let jwt = state
            .signing_keys
            .issue_access_token(&crate::jwt::AccessClaims {
                iss: issuer.to_string(),
                sub: "user@example.com".to_string(),
                aud: resource.to_string(),
                exp: (crate::util::now_unix() + 60) as usize,
                nbf: None,
                iat: crate::util::now_unix() as usize,
                jti: "insufficient-scope".to_string(),
                scope: "mcp:read".to_string(),
                azp: String::new(),
                identity_issuer: Some(crate::google::GOOGLE_ISSUER.to_string()),
                identity_credential_id: None,
            })
            .unwrap();

        let cases = [
            (
                AuthLayer::from_state(Arc::clone(&state)),
                format!("Bearer {jwt}"),
            ),
            (
                AuthLayer::new()
                    .with_static_token(Some(Arc::<str>::from("static-secret")))
                    .with_static_token_scopes(vec!["mcp:read".to_string()]),
                "Bearer static-secret".to_string(),
            ),
        ];
        for (base_layer, authorization) in cases {
            let app = echo_app(
                base_layer
                    .with_resource_url(Some(Arc::<str>::from(resource)))
                    .with_protected_resource_metadata_url(Some(Arc::<str>::from(metadata)))
                    .with_required_scopes(vec!["mcp:write".to_string()]),
            );
            let response = app
                .oneshot(
                    HttpRequest::builder()
                        .uri("/probe")
                        .header(header::AUTHORIZATION, authorization)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let challenge = response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .unwrap()
                .to_str()
                .unwrap();
            assert_eq!(
                challenge,
                concat!(
                    "Bearer error=\"insufficient_scope\", scope=\"mcp:write\", ",
                    "resource_metadata=\"https://proxy.example:53147/.well-known/oauth-protected-resource\""
                )
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn browser_session_is_rejected_when_required_scope_is_missing() {
        let state = Arc::new(test_auth_state().await);
        let session = session::create_browser_session(
            &state,
            "user@example.com".to_string(),
            Some("user@example.com".to_string()),
        )
        .await
        .unwrap();
        let cookie_name = state.config.session_cookie_name.clone();
        let app = echo_app(
            AuthLayer::from_state(state)
                .with_allow_session_cookie(true)
                .with_required_scopes(vec!["scope:not-granted".to_string()]),
        );
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(
                        header::COOKIE,
                        format!("{cookie_name}={}", session.session_id),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unauthenticated_challenge_uses_explicit_metadata_url_override() {
        let app = echo_app(
            AuthLayer::new()
                .with_resource_url(Some(Arc::<str>::from("https://proxy.example:53147/mcp")))
                .with_protected_resource_metadata_url(Some(Arc::<str>::from(
                    "https://proxy.example:53147/.well-known/oauth-protected-resource",
                )))
                .with_required_scopes(vec!["mcp:read".to_string(), "mcp:write".to_string()]),
        );
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .unwrap()
                .to_str()
                .unwrap(),
            concat!(
                "Bearer resource_metadata=\"https://proxy.example:53147/.well-known/oauth-protected-resource\", ",
                "scope=\"mcp:read mcp:write\""
            )
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_session_is_revalidated_on_every_request_and_exposes_bound_grant() {
        let state = Arc::new(test_auth_state().await);
        let binding = project_binding();
        let session = crate::types::BrowserSessionRow {
            session_id: "project-session".into(),
            subject: binding.subject.clone(),
            email: None,
            csrf_token: "csrf".into(),
            created_at: crate::util::now_unix(),
            expires_at: crate::util::now_unix() + 7_200,
            project_binding: Some(binding.clone()),
        };
        state.store.upsert_browser_session(session).await.unwrap();
        let persisted = state
            .store
            .find_browser_session("project-session")
            .await
            .unwrap()
            .unwrap();
        assert!(persisted.project_binding.as_ref() == Some(&binding));
        assert_eq!(
            persisted.expires_at,
            i64::try_from(binding.source_credential_expires_at).unwrap()
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let revalidator = Arc::new(CountingRevalidator {
            calls: Arc::clone(&calls),
            grant: bound_grant(&binding),
        });
        let app = Router::new()
            .route(
                "/probe",
                get(
                    |axum::Extension(grant): axum::Extension<BoundAccessGrant>| async move {
                        grant.project_id
                    },
                ),
            )
            .route_layer(
                AuthLayer::from_state(state)
                    .with_allow_session_cookie(true)
                    .with_project_session_revalidator(revalidator),
            );
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri("/probe")
                        .header(header::COOKIE, "lab_session=project-session")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_session_fails_closed_without_revalidator_and_rejects_mixed_authority() {
        let state = Arc::new(test_auth_state().await);
        let binding = project_binding();
        state
            .store
            .upsert_browser_session(crate::types::BrowserSessionRow {
                session_id: "project-session".into(),
                subject: binding.subject.clone(),
                email: None,
                csrf_token: "csrf".into(),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 60,
                project_binding: Some(binding),
            })
            .await
            .unwrap();
        let layer = AuthLayer::from_state(state)
            .with_allow_session_cookie(true)
            .with_static_token(Some(Arc::<str>::from("valid-static")));
        let unavailable = echo_app(layer.clone())
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::COOKIE, "lab_session=project-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unavailable.status(), StatusCode::BAD_GATEWAY);
        let mixed = echo_app(layer)
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::COOKIE, "lab_session=project-session")
                    .header(header::AUTHORIZATION, "Bearer valid-static")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mixed.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn access_revocation_remains_authoritative_when_auth_session_cleanup_fails() {
        let state = Arc::new(test_auth_state().await);
        let binding = project_binding();
        state
            .store
            .upsert_browser_session(crate::types::BrowserSessionRow {
                session_id: "cleanup-failure-session".into(),
                subject: binding.subject.clone(),
                email: None,
                csrf_token: "csrf".into(),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 60,
                project_binding: Some(binding.clone()),
            })
            .await
            .unwrap();
        state
            .store
            .execute_test_statement(
                "CREATE TRIGGER deny_project_session_delete
                 BEFORE DELETE ON browser_sessions
                 BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END;",
            )
            .await
            .unwrap();
        let denied = Arc::new(AtomicBool::new(true));
        let app = echo_app(
            AuthLayer::from_state(Arc::clone(&state))
                .with_allow_session_cookie(true)
                .with_project_session_revalidator(Arc::new(ToggleRevalidator {
                    denied,
                    grant: bound_grant(&binding),
                })),
        );

        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri("/probe")
                        .header(header::COOKIE, "lab_session=cleanup-failure-session")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        assert!(
            state
                .store
                .find_browser_session("cleanup-failure-session")
                .await
                .unwrap()
                .is_some(),
            "the injected auth.db cleanup failure must leave the row behind"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_revocation_and_policy_drift_fail_closed_without_cached_authority() {
        let state = Arc::new(test_auth_state().await);
        let binding = project_binding();
        state
            .store
            .upsert_browser_session(crate::types::BrowserSessionRow {
                session_id: "race-session".into(),
                subject: binding.subject.clone(),
                email: None,
                csrf_token: "csrf".into(),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 60,
                project_binding: Some(binding.clone()),
            })
            .await
            .unwrap();
        let denied = Arc::new(AtomicBool::new(false));
        let app = echo_app(
            AuthLayer::from_state(Arc::clone(&state))
                .with_allow_session_cookie(true)
                .with_project_session_revalidator(Arc::new(ToggleRevalidator {
                    denied: Arc::clone(&denied),
                    grant: bound_grant(&binding),
                })),
        );
        let allowed = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::COOKIE, "lab_session=race-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);

        denied.store(true, Ordering::SeqCst);
        let request = || {
            app.clone().oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::COOKIE, "lab_session=race-session")
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        let (first, second, third, fourth) =
            tokio::join!(request(), request(), request(), request());
        for response in <[_; 4]>::from((first, second, third, fourth)) {
            assert_eq!(response.unwrap().status(), StatusCode::UNAUTHORIZED);
        }

        let mut drifted = bound_grant(&binding);
        drifted.project_policy_epoch += 1;
        state
            .store
            .upsert_browser_session(crate::types::BrowserSessionRow {
                session_id: "drift-session".into(),
                subject: binding.subject.clone(),
                email: None,
                csrf_token: "csrf".into(),
                created_at: crate::util::now_unix(),
                expires_at: crate::util::now_unix() + 60,
                project_binding: Some(binding),
            })
            .await
            .unwrap();
        let drift_app = echo_app(
            AuthLayer::from_state(state)
                .with_allow_session_cookie(true)
                .with_project_session_revalidator(Arc::new(CountingRevalidator {
                    calls: Arc::new(AtomicUsize::new(0)),
                    grant: drifted,
                })),
        );
        let response = drift_app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::COOKIE, "lab_session=drift-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn csrf_is_rotated_per_session_and_old_or_mixed_authority_is_denied() {
        let state = Arc::new(test_auth_state().await);
        let binding = project_binding();
        let session_state =
            ProjectSessionState::from_store(state.store.clone(), "__Host-labby-session").unwrap();
        let first = session_state.create(&bound_grant(&binding)).await.unwrap();
        let second = session_state.create(&bound_grant(&binding)).await.unwrap();
        assert_ne!(first.session_id, second.session_id);
        assert_ne!(first.csrf_token, second.csrf_token);
        let layer = AuthLayer::from_state(state)
            .with_allow_session_cookie(true)
            .with_project_session_state(Some(Arc::new(session_state)))
            .with_project_session_revalidator(Arc::new(CountingRevalidator {
                calls: Arc::new(AtomicUsize::new(0)),
                grant: bound_grant(&binding),
            }));
        let app = Router::new()
            .route("/probe", post(|| async { "ok" }))
            .route_layer(layer);
        let stale_csrf = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/probe")
                    .header(
                        header::COOKIE,
                        format!("__Host-labby-session={}", second.session_id),
                    )
                    .header(session::BROWSER_CSRF_HEADER_NAME, first.csrf_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale_csrf.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let mixed = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/probe")
                    .header(
                        header::COOKIE,
                        format!("__Host-labby-session={}", second.session_id),
                    )
                    .header(header::AUTHORIZATION, "Bearer any")
                    .header(session::BROWSER_CSRF_HEADER_NAME, second.csrf_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mixed.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn product_credential_precedes_static_bearer_and_attaches_exact_bound_access() {
        let source = product_source_grant();
        let bound = product_bound_grant(&source);
        let calls = Arc::new(AtomicUsize::new(0));
        let layer = AuthLayer::new()
            .with_static_token(Some(Arc::<str>::from(product_token())))
            .with_product_credential_verifier(Arc::new(StubProductVerifier {
                result: Ok(source),
                calls: Arc::clone(&calls),
            }))
            .with_product_access_grant_resolver(Arc::new(StubProductResolver {
                result: Ok(bound),
            }));
        let app = Router::new().route("/probe", get(
            |axum::Extension(ctx): axum::Extension<AuthContext>,
             axum::Extension(grant): axum::Extension<BoundAccessGrant>,
             axum::Extension(identity): axum::Extension<VerifiedIdentity>| async move {
                assert_eq!(identity.authenticator(), Authenticator::ProductCredential);
                format!("{}|{}|{}", ctx.sub, grant.project_id, ctx.scopes.join(","))
            },
        )).route_layer(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/probe")
                    .header(header::AUTHORIZATION, format!("Bearer {}", product_token()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"principal-1|project-1|lab:read");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn malformed_unknown_and_unconfigured_product_credentials_fail_uniformly_without_fallback()
     {
        let calls = Arc::new(AtomicUsize::new(0));
        let denied_layer = AuthLayer::new()
            .with_static_token(Some(Arc::<str>::from(product_token())))
            .with_product_credential_verifier(Arc::new(StubProductVerifier {
                result: Err(ProductCredentialVerificationError::Denied),
                calls: Arc::clone(&calls),
            }));
        let cases = [
            (
                "lby_pc_v1_bad",
                AuthLayer::new().with_static_token(Some(Arc::<str>::from("lby_pc_v1_bad"))),
            ),
            (product_token(), denied_layer),
            (
                product_token(),
                AuthLayer::new().with_static_token(Some(Arc::<str>::from(product_token()))),
            ),
        ];
        let mut bodies = Vec::new();
        for (token, layer) in cases {
            let response = echo_app(layer)
                .oneshot(
                    HttpRequest::builder()
                        .uri("/probe")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            bodies.push(
                axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(bodies.windows(2).all(|pair| pair[0] == pair[1]));
    }
}
