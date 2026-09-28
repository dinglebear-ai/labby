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
#[path = "depot_publish.rs"]
mod publishing;
#[path = "depot_read_access.rs"]
mod read_access;
use read_access::ReadAccess;

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
        .route(
            routes.next().expect("publish availability descriptor"),
            get(publishing::availability),
        )
        .route(
            routes.next().expect("publish descriptor"),
            post(publishing::publish),
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
        RouteDescriptor::new(
            "GET",
            "/publish",
            "publish_availability",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store(),
        RouteDescriptor::new("POST", "/publish", "publish_skill", "depot", RouteAuth::V1)
            .private_no_store()
            .side_effects("delegated Skill archive publication"),
    ]
}

async fn discover(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(request): Json<DiscoveryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let auth = auth.as_ref().map(|value| &value.0);
    let identity = identity.as_ref().map(|value| &value.0);
    let access = ReadAccess::begin(&state, &authority, auth, identity).await?;
    let result = discovery::discover_with_access_epoch(
        &state.depot_manager,
        &authority,
        &request,
        tokio::time::Instant::now(),
        access.epoch().as_deref(),
    )
    .await
    .and_then(|response| {
        serde_json::to_value(response).map_err(|_| DiscoveryError::InvalidProvider)
    })
    .map(Json)
    .map_err(map_discovery_error);
    access
        .finish(&state, &authority, auth, identity, result)
        .await
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
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(request): Json<DetailRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let auth = auth.as_ref().map(|value| &value.0);
    let identity = identity.as_ref().map(|value| &value.0);
    let access = ReadAccess::begin(&state, &authority, auth, identity).await?;
    let result = discovery::detail(
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
    .map_err(map_discovery_error);
    access
        .finish(&state, &authority, auth, identity, result)
        .await
}

async fn providers(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let auth = auth.as_ref().map(|value| &value.0);
    let identity = identity.as_ref().map(|value| &value.0);
    let access = ReadAccess::begin(&state, &authority, auth, identity).await?;
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
    let result = value.map(Json).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"kind":"internal","message":"provider status unavailable"})),
        )
    });
    access
        .finish(&state, &authority, auth, identity, result)
        .await
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
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let auth = auth.as_ref().map(|value| &value.0);
    let identity = identity.as_ref().map(|value| &value.0);
    let access = ReadAccess::begin(&state, &authority, auth, identity).await?;
    let grant = authority.revalidate().await.map_err(|_| forbidden())?;
    if !grant.has_scope("lab:read") || !grant.has_scope("lab:admin") {
        return Err(forbidden());
    }
    let result = state
        .depot_admin
        .as_ref()
        .ok_or_else(unavailable)?
        .operation(&operation_id)
        .await
        .map(|outcome| Json(json!(outcome)))
        .map_err(map_admin_error);
    access
        .finish(&state, &authority, auth, identity, result)
        .await
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

async fn admin_depot_authorization(
    state: &AppState,
    authority: &BrowserAuthority,
    auth: &AuthContext,
    identity: Option<&VerifiedIdentity>,
) -> Result<labby_auth::depot_delegation::BrowserDepotAuthorization, (StatusCode, Json<Value>)> {
    let identity = identity.ok_or_else(forbidden)?;
    if !auth.via_session
        || identity.authenticator() != Authenticator::BrowserSession
        || !matches!(identity.principal_link(), PrincipalLink::External { subject, .. } if subject == &auth.sub)
    {
        return Err(forbidden());
    }
    let target = state
        .config
        .depot
        .publish
        .as_ref()
        .ok_or_else(|| map_error(DepotError::DelegationUnavailable))?;
    if target.project_id.trim().is_empty() {
        return Err(map_error(DepotError::DelegationUnavailable));
    }
    let installation = state
        .installation_id
        .as_ref()
        .ok_or_else(|| map_error(DepotError::DelegationUnavailable))?;
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| forbidden())?;
    let access = store
        .authorize_project(crate::access::AuthorizeProjectInput::new(
            identity.clone(),
            target.project_id.clone(),
            crate::access::Permission::ProjectManage,
        ))
        .await
        .map_err(|_| forbidden())?;
    let current = store
        .authorize_project(crate::access::AuthorizeProjectInput::new(
            identity.clone(),
            target.project_id.clone(),
            crate::access::Permission::ProjectManage,
        ))
        .await
        .map_err(|_| forbidden())?;
    if access != current || !state.depot.publishing_configured() {
        return Err(map_error(DepotError::DelegationUnavailable));
    }
    let live = authority.revalidate().await.map_err(|_| forbidden())?;
    if !live.has_scope("lab:admin") {
        return Err(forbidden());
    }
    Ok(labby_auth::depot_delegation::BrowserDepotAuthorization {
        installation_id: installation.to_string(),
        principal_id: access.principal_id,
        organization_id: access.organization_id,
        project_id: access.project_id,
        membership_epoch: access.membership_epoch,
        organization_policy_epoch: access.organization_policy_epoch,
        project_policy_epoch: access.project_policy_epoch,
        expires_at: u64::try_from(live.expires_at()).map_err(|_| forbidden())?,
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
        DiscoveryError::InvalidKind
        | DiscoveryError::InvalidQuery
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
    let access = ReadAccess::begin(
        &state,
        &authority,
        auth.as_ref().map(|v| &v.0),
        identity.as_ref().map(|v| &v.0),
    )
    .await?;
    let actor = actor(auth.clone(), identity.clone())?;
    let result = Ok(Json(
        json!({"depot": state.depot.status_for_actor(&actor).await, "authority_projection": crate::dispatch::depot::authority_projection::projection_readiness()}),
    ));
    access
        .finish(
            &state,
            &authority,
            auth.as_ref().map(|v| &v.0),
            identity.as_ref().map(|v| &v.0),
            result,
        )
        .await
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
    use crate::access::{AccessRuntime, AccessStore, ActivateProofInput, ConsumeBootstrapInput};
    use axum::{Router, body::Body, http::Request};
    use labby_auth::{
        Authenticator,
        browser_authority::{BrowserPolicy, PermissionState, PolicyFuture},
        depot_delegation::DepotDelegationTarget,
        jwt::SigningKeys,
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

    const DEPOT_OPERATIONS_GOLDEN: &str = include_str!(
        "../../../../../docs/contracts/fixtures/depot-control-plane/operations-v1.json"
    );

    fn catalog_operation(
        name: &str,
        required_scope: &str,
        authorized: bool,
        read_only: bool,
        destructive: bool,
    ) -> Value {
        let fixture: Value = serde_json::from_str(DEPOT_OPERATIONS_GOLDEN).unwrap();
        let mut definition = fixture["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|definition| definition["name"] == name)
            .unwrap()
            .clone();
        let object = definition.as_object_mut().unwrap();
        object.insert("requiredScope".into(), json!(required_scope));
        object.insert("authorized".into(), json!(authorized));
        object.insert("transportAvailable".into(), json!(true));
        object.insert(
            "annotations".into(),
            json!({"readOnlyHint":read_only,"destructiveHint":destructive}),
        );
        definition
    }

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

    pub(super) async fn browser_context(
        scopes: &[&str],
    ) -> (TempDir, BrowserAuthority, AuthContext, VerifiedIdentity) {
        browser_context_for_subject(scopes, "depot-route-subject").await
    }

    pub(super) async fn browser_context_for_subject(
        scopes: &[&str],
        subject: &str,
    ) -> (TempDir, BrowserAuthority, AuthContext, VerifiedIdentity) {
        let temp = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(temp.path().join("auth.db"))
            .await
            .unwrap();
        let now = now_unix();
        let row = BrowserSessionRow {
            session_id: "depot-route-session".to_owned(),
            subject: subject.to_owned(),
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

    async fn admin_access_runtime(temp: &TempDir, subject: &str) -> Arc<AccessRuntime> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db = temp.path().join("access.db");
        let store = AccessStore::open(db.clone()).await.unwrap();
        let now = now_unix();
        store
            .activate_bootstrap_proof(ActivateProofInput {
                proof_id: "proof-1".into(),
                prepare_id: "prepare-1".into(),
                installation_id: "installation-1".into(),
                installation_generation: 1,
                proof_digest: [1; 32],
                manifest_digest: [2; 32],
                request_digest: [3; 32],
                idempotency_digest: [4; 32],
                credential_id: "credential-1".into(),
                credential_digest: [5; 32],
                proof_generation: 1,
                created_at: now,
                expires_at: now + 60,
            })
            .await
            .unwrap();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap();
        store
            .consume_bootstrap_proof(ConsumeBootstrapInput {
                proof_id: "proof-1".into(),
                proof_digest: [1; 32],
                request_digest: [3; 32],
                idempotency_digest: [4; 32],
                organization_name: "Local".into(),
                project_name: "Default".into(),
                canonical_issuer: "https://accounts.google.com".into(),
                subject: subject.into(),
                identity_fingerprint: identity.safe_fingerprint(),
                loadout_id: "production".into(),
                loadout_generation: 1,
                catalog_generation: 1,
                loadout_policy_fingerprint: [6; 32],
                route_id: "team".into(),
                route_generation: 1,
                resource: "https://labby.example/mcp/team".into(),
                audience: "https://labby.example/mcp/team".into(),
                scopes_json: r#"["lab","lab:read"]"#.into(),
                now,
                credential_expires_at: now + 3600,
            })
            .await
            .unwrap();
        drop(store);
        Arc::new(AccessRuntime::initialize(db).await)
    }

    async fn upstream() -> (url::Url, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let app = Router::new().fallback(move |request: Request<Body>| {
            let observed = Arc::clone(&observed);
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                if matches!(request.uri().path(), "/api/operations" | "/api/operations/catalog") {
                    Json(json!({"operations":[
                        catalog_operation("depot.system.status", "read", true, true, false),
                        catalog_operation("depot.tokens.create", "write", false, false, false),
                        catalog_operation("depot.tokens.revoke", "write", false, false, true),
                        catalog_operation("depot.maintenance.upstream", "operator", false, true, false)
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

    async fn incompatible_upstream() -> (url::Url, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut definition = catalog_operation("depot.system.status", "read", true, true, false);
        definition["inputSchema"] = json!({
            "type":"object",
            "properties":{"forged":{"type":"string"}}
        });
        let app = Router::new().fallback(move |_request: Request<Body>| {
            let observed = Arc::clone(&observed);
            let definition = definition.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Json(json!({"operations":[definition]}))
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
                "depot.tokens.create",
                Some("depot-route-csrf"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn incompatible_catalog_is_rejected_before_operation_execution() {
        let (base_url, calls) = incompatible_upstream().await;
        let (_temp, authority, auth, identity) = browser_context(&["lab:read"]).await;
        let mut state = AppState::new();
        state.depot = Arc::new(crate::dispatch::depot::DepotClient::for_test(
            base_url,
            "read-token",
        ));

        assert!(matches!(
            state.depot.operations(&identity.safe_fingerprint()).await,
            Err(DepotError::InvalidCatalog)
        ));
        let response = operation_router(state, authority, auth, identity)
            .oneshot(operation_request("depot.system.status", None))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
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
                "depot.tokens.create",
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
                .oneshot(operation_request("depot.tokens.create", csrf))
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn administration_executes_write_and_operator_operations_with_browser_delegation() {
        let (base_url, calls) = upstream().await;
        let access_dir = tempfile::tempdir().unwrap();
        let runtime = admin_access_runtime(&access_dir, "operator-1").await;
        let (_session_dir, authority, auth, identity) =
            browser_context_for_subject(&["lab:read", "lab:admin"], "operator-1").await;
        let keys = Arc::new(
            SigningKeys::load_or_create(&access_dir.path().join("delegation.der")).unwrap(),
        );
        let client = crate::dispatch::depot::DepotClient::for_test(base_url, "read-service-token")
            .with_test_delegation(
                keys,
                DepotDelegationTarget {
                    issuer: "https://labby.example".into(),
                    audience: "https://depot.example".into(),
                    deployment_id: "test-depot".into(),
                    account_id: "test-account".into(),
                    tenant_id: "test-tenant".into(),
                    team_id: None,
                },
            );
        let mut state = AppState::new().with_access_runtime(runtime);
        state.depot = Arc::new(client);
        state.installation_id = Some(Arc::from("installation-1"));
        Arc::make_mut(&mut state.config).depot.publish =
            Some(crate::config::depot::DepotPublishTarget {
                route_id: "team".into(),
                project_id: "bootstrap-default".into(),
            });
        state
            .depot
            .operations(&identity.safe_fingerprint())
            .await
            .unwrap();
        let router = operation_router(state, authority, auth, identity);

        for operation in ["depot.tokens.create", "depot.maintenance.upstream"] {
            let response = router
                .clone()
                .oneshot(operation_request(operation, Some("depot-route-csrf")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{operation}");
        }
        let destructive = router
            .clone()
            .oneshot(destructive_operation_request(
                "depot.tokens.revoke",
                "delegated-revoke-1",
            ))
            .await
            .unwrap();
        assert_eq!(destructive.status(), StatusCode::OK);
        let retry = router
            .oneshot(destructive_operation_request(
                "depot.tokens.revoke",
                "delegated-revoke-1",
            ))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn mutation_with_admin_and_csrf_still_cannot_fall_back_to_static_token() {
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
                "depot.tokens.create",
                Some("depot-route-csrf"),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn destructive_execution_requires_intent_and_no_delegation_stays_retryable() {
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

        let response = router
            .clone()
            .oneshot(destructive_operation_request(
                "depot.tokens.revoke",
                "revoke-token-1",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let retry = router
            .oneshot(destructive_operation_request(
                "depot.tokens.revoke",
                "revoke-token-1",
            ))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

async fn session(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let access = ReadAccess::begin(
        &state,
        &authority,
        auth.as_ref().map(|v| &v.0),
        identity.as_ref().map(|v| &v.0),
    )
    .await?;
    let actor = actor(auth.clone(), identity.clone())?;
    let result = state
        .depot
        .session(&actor)
        .await
        .map(Json)
        .map_err(map_error);
    access
        .finish(
            &state,
            &authority,
            auth.as_ref().map(|v| &v.0),
            identity.as_ref().map(|v| &v.0),
            result,
        )
        .await
}

async fn operations(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let access = ReadAccess::begin(
        &state,
        &authority,
        auth.as_ref().map(|v| &v.0),
        identity.as_ref().map(|v| &v.0),
    )
    .await?;
    let actor = actor(auth.clone(), identity.clone())?;
    let result = state
        .depot
        .operations(&actor)
        .await
        .map(Json)
        .map_err(map_error);
    access
        .finish(
            &state,
            &authority,
            auth.as_ref().map(|v| &v.0),
            identity.as_ref().map(|v| &v.0),
            result,
        )
        .await
}

async fn call(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Extension(auth): Extension<AuthContext>,
    identity: Option<Extension<VerifiedIdentity>>,
    headers: HeaderMap,
    Json(request): Json<OperationRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let access = ReadAccess::begin(
        &state,
        &authority,
        Some(&auth),
        identity.as_ref().map(|v| &v.0),
    )
    .await?;
    let actor = actor(Some(Extension(auth.clone())), identity.clone())?;
    let policy = state
        .depot
        .operation_policy(&request.operation, &actor)
        .await
        .map_err(map_error)?;
    // Derived from the shared policy so a destructive read-scoped catalog
    // entry is gated exactly like a write.
    if policy.requires_delegation() {
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
    let result = if policy.requires_delegation() {
        let authorization = admin_depot_authorization(
            &state,
            &authority,
            &auth,
            identity.as_ref().map(|value| &value.0),
        )
        .await?;
        state
            .depot
            .call_with_browser_authorization(
                &request.operation,
                request.params,
                &actor,
                policy,
                idempotency_key,
                &authorization,
            )
            .await
    } else {
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
    }
    .map(Json)
    .map_err(map_error);
    access
        .finish(
            &state,
            &authority,
            Some(&auth),
            identity.as_ref().map(|v| &v.0),
            result,
        )
        .await
}

fn map_error(error: DepotError) -> (StatusCode, Json<Value>) {
    let status = match &error {
        DepotError::Disabled | DepotError::Unconfigured | DepotError::DelegationUnavailable => {
            StatusCode::SERVICE_UNAVAILABLE
        }
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
