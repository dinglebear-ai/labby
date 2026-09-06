use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
};
use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::{AuthContext, Authenticator, PrincipalLink, VerifiedIdentity};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{
    route_registry::{RouteAuth, RouteDescriptor, RouteGroup},
    state::AppState,
};
use crate::dispatch::depot::admin::{AdminError, Mutation};
use crate::dispatch::depot::discovery::{self, DiscoveryError, DiscoveryRequest};
use crate::dispatch::depot::{DepotError, error_body};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationRequest {
    operation: String,
    #[serde(default)]
    params: Value,
    destructive_intent: Option<DestructiveIntent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DestructiveIntent {
    confirmed: bool,
    idempotency_key: String,
}

pub fn routes(_state: AppState) -> RouteGroup {
    let mut routes = descriptors().into_iter();
    RouteGroup::empty()
        .route(routes.next().expect("status descriptor"), get(status))
        .route(routes.next().expect("session descriptor"), get(session))
        .route(
            routes.next().expect("operations descriptor"),
            get(operations),
        )
        .route(routes.next().expect("call descriptor"), post(call))
        .route(routes.next().expect("discover descriptor"), post(discover))
        .route(routes.next().expect("detail descriptor"), post(detail))
        .route(routes.next().expect("providers descriptor"), get(providers))
        .route(
            routes.next().expect("upsert descriptor"),
            post(upsert_provider),
        )
        .route(
            routes.next().expect("probe descriptor"),
            post(probe_provider),
        )
        .route(
            routes.next().expect("remove descriptor"),
            delete(remove_provider),
        )
        .route(
            routes.next().expect("outcome descriptor"),
            get(provider_operation),
        )
}

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![
        RouteDescriptor::new("GET", "/status", "status", "depot", RouteAuth::V1).private_no_store(),
        RouteDescriptor::new("GET", "/session", "session", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new("GET", "/operations", "operations", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new("POST", "/operations", "call", "depot", RouteAuth::V1)
            .private_no_store()
            .side_effects("bounded canonical Depot operation"),
        RouteDescriptor::new("POST", "/discover", "discover_v2", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new(
            "POST",
            "/artifacts/detail",
            "detail_v2",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store(),
        RouteDescriptor::new("GET", "/providers", "providers", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new(
            "POST",
            "/providers",
            "providers_upsert",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store()
        .side_effects("durable provider configuration mutation"),
        RouteDescriptor::new(
            "POST",
            "/providers/probe",
            "providers_probe",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store()
        .side_effects("bounded provider diagnostic"),
        RouteDescriptor::new(
            "DELETE",
            "/providers/{provider_id}",
            "providers_remove",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store()
        .side_effects("durable provider removal"),
        RouteDescriptor::new(
            "GET",
            "/provider-operations/{operation_id}",
            "provider_operation",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store(),
    ]
}

async fn discover(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Json(request): Json<DiscoveryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    discovery::discover(
        &state.depot_manager,
        &authority,
        &request,
        tokio::time::Instant::now(),
    )
    .await
    .and_then(|response| {
        serde_json::to_value(response).map_err(|_| DiscoveryError::InvalidProvider)
    })
    .map(Json)
    .map_err(map_discovery_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailRequest {
    provider_id: String,
    artifact_id: String,
}

async fn detail(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Json(request): Json<DetailRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    discovery::detail(
        &state.depot_manager,
        &authority,
        &request.provider_id,
        &request.artifact_id,
        tokio::time::Instant::now(),
    )
    .await
    .and_then(|response| {
        serde_json::to_value(response).map_err(|_| DiscoveryError::InvalidProvider)
    })
    .map(Json)
    .map_err(map_discovery_error)
}

async fn providers(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let grant = authority.revalidate().await.map_err(|_| forbidden())?;
    if !grant.has_scope("lab:read") {
        return Err(forbidden());
    }
    let value = if grant.has_scope("lab:admin") {
        let version = state
            .depot_admin
            .as_ref()
            .ok_or_else(unavailable)?
            .current_version()
            .await
            .map_err(map_admin_error)?;
        serde_json::to_value(state.depot_manager.admin_status(&version))
    } else {
        serde_json::to_value(state.depot_manager.status())
    };
    value.map(Json).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"kind":"internal","message":"provider status unavailable"})),
        )
    })
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UpsertRequest {
    #[serde(flatten)]
    mutation: Mutation,
    expected_version: String,
    operation_id: String,
    proof: Option<String>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoveRequest {
    expected_version: String,
    operation_id: String,
    proof: String,
}

async fn upsert_provider(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    Json(request): Json<UpsertRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin_mutation(&authority, &auth, &headers, "providers.upsert").await?;
    let admin = state.depot_admin.as_ref().ok_or_else(unavailable)?;
    let credential = match request.mutation.credential {
        crate::dispatch::depot::admin::CredentialChange::Retain => "retain",
        crate::dispatch::depot::admin::CredentialChange::Replace(_) => "replace",
        crate::dispatch::depot::admin::CredentialChange::Clear => "clear",
    };
    let payload = json!({"id":request.mutation.id,"name":request.mutation.name,"endpoint":request.mutation.endpoint,"enabled":request.mutation.enabled,"authMode":request.mutation.auth_mode,"credential":credential});
    admin
        .upsert(
            &authority,
            request.proof,
            &request.expected_version,
            &request.operation_id,
            &request.mutation,
            &payload,
        )
        .await
        .map(|outcome| Json(json!(outcome)))
        .map_err(map_admin_error)
}

async fn probe_provider(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    Json(mutation): Json<Mutation>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin_mutation(&authority, &auth, &headers, "providers.probe").await?;
    let health = state
        .depot_admin
        .as_ref()
        .ok_or_else(unavailable)?
        .probe(&mutation)
        .await
        .map_err(map_admin_error)?;
    Ok(Json(
        json!({"providerId":mutation.id,"state":health.state,"observedAt":health.observed_at.unwrap_or_default()}),
    ))
}

async fn remove_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Extension(authority): Extension<BrowserAuthority>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    Json(request): Json<RemoveRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin_mutation(&authority, &auth, &headers, "providers.remove").await?;
    let admin = state.depot_admin.as_ref().ok_or_else(unavailable)?;
    let payload = json!({"providerId":provider_id});
    admin
        .remove(
            &authority,
            request.proof,
            &provider_id,
            &request.expected_version,
            &request.operation_id,
            &payload,
        )
        .await
        .map(|outcome| Json(json!(outcome)))
        .map_err(map_admin_error)
}

async fn provider_operation(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
    Extension(authority): Extension<BrowserAuthority>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let grant = authority.revalidate().await.map_err(|_| forbidden())?;
    if !grant.has_scope("lab:read") || !grant.has_scope("lab:admin") {
        return Err(forbidden());
    }
    state
        .depot_admin
        .as_ref()
        .ok_or_else(unavailable)?
        .operation(&operation_id)
        .await
        .map(|outcome| Json(json!(outcome)))
        .map_err(map_admin_error)
}

async fn require_admin_mutation(
    authority: &BrowserAuthority,
    auth: &AuthContext,
    headers: &HeaderMap,
    action: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let grant = authority.revalidate().await.map_err(|_| forbidden())?;
    if !grant.has_scope("lab:admin") {
        return Err(forbidden());
    }
    crate::api::services::require_session_csrf(action, headers, Some(auth)).map_err(|error| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"kind":error.kind(),"message":error.to_string()})),
        )
    })
}

fn unavailable() -> (StatusCode, Json<Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"kind":"unavailable","message":"provider administration is unavailable"})),
    )
}

fn map_admin_error(error: AdminError) -> (StatusCode, Json<Value>) {
    let (status, kind) = match error {
        AdminError::Invalid => (StatusCode::BAD_REQUEST, "validation_failed"),
        AdminError::FreshAuth => (StatusCode::UNAUTHORIZED, "reauthentication_required"),
        AdminError::Stale => (StatusCode::CONFLICT, "conflict"),
        AdminError::Recovery => (StatusCode::SERVICE_UNAVAILABLE, "recovery_required"),
    };
    (
        status,
        Json(json!({"kind":kind,"message":error.to_string()})),
    )
}

fn map_discovery_error(error: DiscoveryError) -> (StatusCode, Json<Value>) {
    let (status, kind, message) = match error {
        DiscoveryError::InvalidQuery
        | DiscoveryError::InvalidLimit
        | DiscoveryError::InvalidProvider => (
            StatusCode::BAD_REQUEST,
            "validation_failed",
            error.to_string(),
        ),
        DiscoveryError::CursorExpired => {
            (StatusCode::CONFLICT, "cursor_expired", error.to_string())
        }
        DiscoveryError::ProviderUnavailable => {
            (StatusCode::NOT_FOUND, "not_found", error.to_string())
        }
        DiscoveryError::Capacity => (
            StatusCode::SERVICE_UNAVAILABLE,
            "capacity",
            error.to_string(),
        ),
        DiscoveryError::ResponseTooLarge => (
            StatusCode::BAD_GATEWAY,
            "upstream_invalid",
            error.to_string(),
        ),
    };
    (
        status,
        Json(json!({"kind":kind,"message":message,"recovery":{"action":"restart_discovery"}})),
    )
}

async fn status(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_read(&authority).await?;
    let actor = actor(auth, identity)?;
    Ok(Json(
        json!({"depot": state.depot.status_for_actor(&actor).await}),
    ))
}

fn actor(
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let Some(Extension(auth)) = auth else {
        return Err(forbidden());
    };
    let Some(Extension(identity)) = identity else {
        return Err(forbidden());
    };
    let durable_browser_actor = auth.via_session
        && identity.authenticator() == Authenticator::BrowserSession
        && matches!(identity.principal_link(), PrincipalLink::External { subject, .. } if subject == &auth.sub);
    durable_browser_actor
        .then(|| identity.safe_fingerprint().to_string())
        .ok_or_else(forbidden)
}

fn forbidden() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":"verified_identity_required"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request};
    use labby_auth::{
        Authenticator,
        browser_authority::{BrowserPolicy, PermissionState, PolicyFuture},
        sqlite::SqliteStore,
        types::BrowserSessionRow,
        util::now_unix,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tempfile::TempDir;
    use tower::ServiceExt;

    struct Policy {
        scopes: Vec<String>,
    }

    impl BrowserPolicy for Policy {
        fn current<'a>(&'a self, _: &'a BrowserSessionRow) -> PolicyFuture<'a> {
            Box::pin(async move {
                Ok(PermissionState {
                    epoch: "1".to_owned(),
                    scopes: self.scopes.clone(),
                })
            })
        }
    }

    async fn browser_context(
        scopes: &[&str],
    ) -> (TempDir, BrowserAuthority, AuthContext, VerifiedIdentity) {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(temp.path().join("auth.db"))
            .await
            .unwrap();
        let now = now_unix();
        let row = BrowserSessionRow {
            session_id: "depot-route-session".to_owned(),
            subject: "depot-route-subject".to_owned(),
            email: Some("operator@example.test".to_owned()),
            csrf_token: "depot-route-csrf".to_owned(),
            created_at: now,
            expires_at: now + 3600,
            project_binding: None,
        };
        store.upsert_browser_session(row.clone()).await.unwrap();
        let authority = BrowserAuthority::verify(
            store,
            &row.session_id,
            "depot-route-test",
            Arc::new(Policy {
                scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            }),
        )
        .await
        .unwrap();
        let auth = AuthContext {
            sub: row.subject.clone(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            issuer: "browser-session".to_owned(),
            via_session: true,
            csrf_token: Some(row.csrf_token),
            email: row.email,
        };
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            &row.subject,
        )
        .unwrap();
        (temp, authority, auth, identity)
    }

    async fn upstream() -> (url::Url, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let app = Router::new().fallback(move |request: Request<Body>| {
            let observed = Arc::clone(&observed);
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                if request.uri().path() == "/api/operations" {
                    Json(json!({"operations":[
                        {"name":"depot.system.status","annotations":{"readOnlyHint":true,"destructiveHint":false}},
                        {"name":"depot.sources.refresh","annotations":{"readOnlyHint":false,"destructiveHint":false}},
                        {"name":"depot.tokens.revoke","annotations":{"readOnlyHint":false,"destructiveHint":true}}
                    ]}))
                } else {
                    Json(json!({"result":{"ok":true}}))
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (
            url::Url::parse(&format!("http://{address}/")).unwrap(),
            calls,
        )
    }

    fn operation_router(
        state: AppState,
        authority: BrowserAuthority,
        auth: AuthContext,
        identity: VerifiedIdentity,
    ) -> Router {
        routes(state.clone())
            .router
            .with_state(state)
            .layer(Extension(identity))
            .layer(Extension(auth))
            .layer(Extension(authority))
    }

    fn operation_request(operation: &str, csrf: Option<&str>) -> Request<Body> {
        let mut request = Request::builder()
            .method("POST")
            .uri("/operations")
            .header("content-type", "application/json");
        if let Some(csrf) = csrf {
            request = request.header(labby_auth::session::BROWSER_CSRF_HEADER_NAME, csrf);
        }
        request
            .body(Body::from(
                json!({"operation":operation,"params":{}}).to_string(),
            ))
            .unwrap()
    }

    fn destructive_operation_request(operation: &str, key: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/operations")
            .header("content-type", "application/json")
            .header(
                labby_auth::session::BROWSER_CSRF_HEADER_NAME,
                "depot-route-csrf",
            )
            .body(Body::from(
                json!({
                    "operation": operation,
                    "params": {"tokenId":"token-1"},
                    "destructiveIntent":{"confirmed":true,"idempotencyKey":key}
                })
                .to_string(),
            ))
            .unwrap()
    }

    #[test]
    fn depot_rejects_web_ui_auth_disabled_identity() {
        let identity =
            VerifiedIdentity::local_credential(Authenticator::StaticBearer, "web-ui-dev:local")
                .unwrap();

        let (status, body) = actor(None, Some(Extension(identity))).unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0, json!({"error":"verified_identity_required"}));
    }

    #[test]
    fn depot_accepts_only_durable_browser_identity() {
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "subject-1",
        )
        .unwrap();
        let auth = AuthContext {
            sub: "subject-1".into(),
            actor_key: None,
            scopes: vec![],
            issuer: "browser-session".into(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        assert_eq!(
            actor(Some(Extension(auth)), Some(Extension(identity)))
                .unwrap()
                .len(),
            12
        );
    }

    #[test]
    fn depot_rejects_static_bearer_and_non_session_oauth() {
        for authenticator in [Authenticator::StaticBearer, Authenticator::OauthBearer] {
            let identity = VerifiedIdentity::local_credential(authenticator, "credential").unwrap();
            let auth = AuthContext {
                sub: "subject-1".into(),
                actor_key: None,
                scopes: vec![],
                issuer: "local".into(),
                via_session: false,
                csrf_token: None,
                email: None,
            };
            assert!(actor(Some(Extension(auth)), Some(Extension(identity))).is_err());
        }
    }

    #[test]
    fn v2_federation_routes_are_literal_private_browser_contracts() {
        let routes = descriptors();
        for (method, path) in [
            ("POST", "/discover"),
            ("POST", "/artifacts/detail"),
            ("GET", "/providers"),
            ("POST", "/providers"),
            ("POST", "/providers/probe"),
            ("DELETE", "/providers/{provider_id}"),
            ("GET", "/provider-operations/{operation_id}"),
        ] {
            let route = routes
                .iter()
                .find(|route| route.method == method && route.path == path)
                .unwrap();
            assert_eq!(route.auth, RouteAuth::V1);
            assert_eq!(route.cache_posture, "private, no-store");
        }
    }

    #[tokio::test]
    async fn read_operation_reaches_depot_without_admin_or_csrf() {
        let (base_url, calls) = upstream().await;
        let (_temp, authority, auth, identity) = browser_context(&["lab:read"]).await;
        let mut state = AppState::new();
        state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
            base_url,
            "read-token",
        ));
        state
            .depot
            .operations(&identity.safe_fingerprint())
            .await
            .unwrap();

        let response = operation_router(state, authority, auth, identity)
            .oneshot(operation_request("depot.system.status", None))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn current_read_revocation_rejects_every_direct_depot_read_without_upstream_calls() {
        for (method, uri, body) in [
            ("GET", "/status", ""),
            ("GET", "/session", ""),
            ("GET", "/operations", ""),
            (
                "POST",
                "/operations",
                r#"{"operation":"depot.system.status","params":{}}"#,
            ),
        ] {
            let (base_url, calls) = upstream().await;
            let (_temp, authority, auth, identity) = browser_context(&[]).await;
            let mut state = AppState::new();
            state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
                base_url,
                "read-token",
            ));
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();

            let response = operation_router(state, authority, auth, identity)
                .oneshot(request)
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
            assert_eq!(calls.load(Ordering::SeqCst), 0, "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn operation_execution_requires_a_current_actor_catalog_without_refetching() {
        let (base_url, calls) = upstream().await;
        let (_temp, authority, auth, identity) = browser_context(&["lab:read", "lab:admin"]).await;
        let mut state = AppState::new();
        state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
            base_url,
            "write-token",
        ));

        let response = operation_router(state, authority, auth, identity)
            .oneshot(operation_request(
                "depot.sources.refresh",
                Some("depot-route-csrf"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn mutation_without_admin_is_rejected_before_depot() {
        let (base_url, calls) = upstream().await;
        let (_temp, authority, auth, identity) = browser_context(&["lab:read"]).await;
        let mut state = AppState::new();
        state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
            base_url,
            "write-token",
        ));
        state
            .depot
            .operations(&identity.safe_fingerprint())
            .await
            .unwrap();

        let response = operation_router(state, authority, auth, identity)
            .oneshot(operation_request(
                "depot.sources.refresh",
                Some("depot-route-csrf"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mutation_without_valid_csrf_is_rejected_before_depot() {
        for csrf in [None, Some("wrong-csrf")] {
            let (base_url, calls) = upstream().await;
            let (_temp, authority, auth, identity) =
                browser_context(&["lab:read", "lab:admin"]).await;
            let mut state = AppState::new();
            state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
                base_url,
                "write-token",
            ));
            state
                .depot
                .operations(&identity.safe_fingerprint())
                .await
                .unwrap();

            let response = operation_router(state, authority, auth, identity)
                .oneshot(operation_request("depot.sources.refresh", csrf))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn mutation_with_admin_and_csrf_reaches_depot() {
        let (base_url, calls) = upstream().await;
        let (_temp, authority, auth, identity) = browser_context(&["lab:read", "lab:admin"]).await;
        let mut state = AppState::new();
        state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
            base_url,
            "write-token",
        ));
        state
            .depot
            .operations(&identity.safe_fingerprint())
            .await
            .unwrap();

        let response = operation_router(state, authority, auth, identity)
            .oneshot(operation_request(
                "depot.sources.refresh",
                Some("depot-route-csrf"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn destructive_execution_requires_intent_and_replays_the_bound_success() {
        let (base_url, calls) = upstream().await;
        let (_temp, authority, auth, identity) = browser_context(&["lab:read", "lab:admin"]).await;
        let mut state = AppState::new();
        state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
            base_url,
            "write-token",
        ));
        state
            .depot
            .operations(&identity.safe_fingerprint())
            .await
            .unwrap();
        let router = operation_router(state, authority, auth, identity);

        let missing = router
            .clone()
            .oneshot(operation_request(
                "depot.tokens.revoke",
                Some("depot-route-csrf"),
            ))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        for _ in 0..2 {
            let response = router
                .clone()
                .oneshot(destructive_operation_request(
                    "depot.tokens.revoke",
                    "revoke-token-1",
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

async fn session(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_read(&authority).await?;
    let actor = actor(auth, identity)?;
    state
        .depot
        .session(&actor)
        .await
        .map(Json)
        .map_err(map_error)
}

async fn operations(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_read(&authority).await?;
    let actor = actor(auth, identity)?;
    state
        .depot
        .operations(&actor)
        .await
        .map(Json)
        .map_err(map_error)
}

async fn call(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Extension(auth): Extension<AuthContext>,
    identity: Option<Extension<VerifiedIdentity>>,
    headers: HeaderMap,
    Json(request): Json<OperationRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let actor = actor(Some(Extension(auth.clone())), identity)?;
    require_read(&authority).await?;
    let policy = state
        .depot
        .operation_policy(&request.operation, &actor)
        .await
        .map_err(map_error)?;
    if !policy.read_only {
        require_admin_mutation(&authority, &auth, &headers, &request.operation).await?;
    }
    let idempotency_key = if policy.destructive {
        let intent = request
            .destructive_intent
            .as_ref()
            .filter(|intent| intent.confirmed)
            .ok_or_else(|| map_error(DepotError::DestructiveIntentRequired))?;
        Some(intent.idempotency_key.as_str())
    } else {
        None
    };
    state
        .depot
        .call(
            &request.operation,
            request.params,
            &actor,
            policy,
            idempotency_key,
        )
        .await
        .map(Json)
        .map_err(map_error)
}

async fn require_read(authority: &BrowserAuthority) -> Result<(), (StatusCode, Json<Value>)> {
    let grant = authority.revalidate().await.map_err(|_| forbidden())?;
    grant
        .has_scope("lab:read")
        .then_some(())
        .ok_or_else(forbidden)
}

fn map_error(error: DepotError) -> (StatusCode, Json<Value>) {
    let status = match &error {
        DepotError::Disabled | DepotError::Unconfigured => StatusCode::SERVICE_UNAVAILABLE,
        DepotError::UnsupportedOperation => StatusCode::BAD_REQUEST,
        DepotError::InvalidCatalog => StatusCode::BAD_GATEWAY,
        DepotError::DestructiveIntentRequired => StatusCode::UNPROCESSABLE_ENTITY,
        DepotError::IdempotencyConflict => StatusCode::CONFLICT,
        DepotError::OutcomeIndeterminate => StatusCode::CONFLICT,
        DepotError::Upstream(status, _) => *status,
        DepotError::ResponseTooLarge => StatusCode::BAD_GATEWAY,
        DepotError::QueueTimeout => StatusCode::SERVICE_UNAVAILABLE,
        DepotError::Unavailable(_) | DepotError::InvalidResponse => StatusCode::BAD_GATEWAY,
    };
    (status, Json(error_body(&error)))
}
