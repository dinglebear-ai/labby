use super::*;
use labby_auth::ProductAccessGrantResolver as _;
use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};

type ApiFailure = (StatusCode, Json<Value>);

async fn authorize_google(
    state: &AppState,
    authority: &BrowserAuthority,
    auth: &AuthContext,
    identity: Option<&VerifiedIdentity>,
) -> Result<labby_auth::depot_delegation::BrowserDepotAuthorization, &'static str> {
    let identity = identity.ok_or("project_session_required")?;
    let live = authority
        .revalidate()
        .await
        .map_err(|_| "session_expired")?;
    if !auth.via_session
        || authority.identity_provider() != Some("google")
        || identity.authenticator() != Authenticator::BrowserSession
        || !matches!(identity.principal_link(),PrincipalLink::External{subject,..} if subject==&auth.sub)
    {
        return Err("browser_session_required");
    }
    let target = state
        .config
        .depot
        .publish
        .as_ref()
        .ok_or("publish_route_unconfigured")?;
    if target.route_id.is_empty() || target.project_id.is_empty() {
        return Err("publish_route_unconfigured");
    }
    let installation = state
        .installation_id
        .as_ref()
        .ok_or("project_access_unavailable")?;
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| "project_access_unavailable")?;
    let access = store
        .authorize_project(crate::access::AuthorizeProjectInput::new(
            identity.clone(),
            target.project_id.clone(),
            crate::access::Permission::ArtifactPublish,
        ))
        .await;
    let access = match access {
        Ok(access) => access,
        Err(_) => {
            return Err(pending_owner_approval(
                state,
                authority,
                auth,
                identity,
                &target.route_id,
                &target.project_id,
            )
            .await
            .unwrap_or("project_access_denied"));
        }
    };
    crate::api::services::owner_link::validate_route_target(
        state,
        &target.route_id,
        &target.project_id,
        &access.loadout_name,
        None,
    )
    .await?;
    // Bind both policy and session snapshots immediately before issuing authority.
    let current = store
        .authorize_project(crate::access::AuthorizeProjectInput::new(
            identity.clone(),
            target.project_id.clone(),
            crate::access::Permission::ArtifactPublish,
        ))
        .await
        .map_err(|_| "project_access_denied")?;
    if access != current {
        return Err("project_access_changed");
    }
    authority
        .revalidate()
        .await
        .map_err(|_| "session_expired")?;
    if !state.depot.publishing_configured() {
        return Err("depot_delegation_unavailable");
    }
    Ok(labby_auth::depot_delegation::BrowserDepotAuthorization {
        installation_id: installation.to_string(),
        principal_id: access.principal_id,
        organization_id: access.organization_id,
        project_id: access.project_id,
        membership_epoch: access.membership_epoch,
        organization_policy_epoch: access.organization_policy_epoch,
        project_policy_epoch: access.project_policy_epoch,
        expires_at: u64::try_from(live.expires_at()).map_err(|_| "session_expired")?,
    })
}

async fn pending_owner_approval(
    state: &AppState,
    authority: &BrowserAuthority,
    auth: &AuthContext,
    identity: &VerifiedIdentity,
    route_id: &str,
    project_id: &str,
) -> Option<&'static str> {
    let approval =
        crate::api::services::owner_link::current_approval(state, authority, auth, identity)
            .await
            .ok()?;
    if approval.route_id != route_id || approval.project_id != project_id {
        return None;
    }
    let store = state.access_runtime.store().await.ok()?;
    if store.owner_link_consumed(approval.clone()).await.ok()? {
        return None;
    }
    Some(if approval.expires_at > labby_auth::util::now_unix() {
        "owner_link_approval_pending"
    } else {
        "owner_link_approval_expired"
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillRequest {
    name: String,
    source: String,
}

async fn authorize(
    state: &AppState,
    authority: &BrowserAuthority,
    auth: &AuthContext,
    source: Option<&ProductCredentialGrant>,
    bound: Option<&BoundAccessGrant>,
) -> Result<BoundAccessGrant, &'static str> {
    let live = authority
        .revalidate()
        .await
        .map_err(|_| "session_expired")?;
    if !auth.via_session || !(live.has_scope("lab") || live.has_scope("lab:admin")) {
        return Err("write_permission_required");
    }
    let (Some(source), Some(bound)) = (source, bound) else {
        return Err("project_session_required");
    };
    if auth.sub != bound.principal_id {
        return Err("project_session_required");
    }
    let adapter = state
        .access_credential_adapter
        .as_ref()
        .ok_or("project_access_unavailable")?;
    let current = adapter
        .resolve(source)
        .await
        .map_err(|_| "project_access_denied")?;
    if &current != bound {
        return Err("project_access_changed");
    }
    require_publish_capability(state, &current).await?;
    if adapter
        .resolve(source)
        .await
        .map_err(|_| "project_access_denied")?
        != current
    {
        return Err("project_access_changed");
    }
    if !state.depot.publishing_configured() {
        return Err("depot_delegation_unavailable");
    }
    Ok(current)
}

#[cfg(feature = "gateway")]
async fn require_publish_capability(
    state: &AppState,
    grant: &BoundAccessGrant,
) -> Result<(), &'static str> {
    let manager = state
        .gateway_manager
        .as_ref()
        .ok_or("project_access_unavailable")?;
    let identity = VerifiedIdentity::local_credential_with_issuer(
        Authenticator::ProductCredential,
        grant.issuer.clone(),
        grant.credential_id.clone(),
    )
    .map_err(|_| "project_access_denied")?;
    let context = crate::access::project_runtime_loadout_context(
        &state.access_runtime,
        manager,
        identity,
        grant.project_id.clone(),
        crate::access::Permission::ArtifactPublish,
    )
    .await
    .map_err(|_| "project_access_denied")?;
    let access = context.access();
    let same_loadout = access.loadout_name == grant.loadout_id;
    if access.principal_id != grant.principal_id
        || access.organization_id != grant.organization_id
        || access.project_id != grant.project_id
        || !same_loadout
    {
        return Err("project_access_changed");
    }
    let route = manager
        .published_project_route_snapshot(&grant.route_id, &grant.project_id, &grant.loadout_id)
        .await
        .map_err(|_| "project_access_denied")?;
    if route.resource() != grant.resource
        || route.runtime_config_generation() != context.runtime_config_generation()
    {
        return Err("project_access_changed");
    }
    let loadout = route.effective_loadout();
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset_with_capabilities(
        route.route_name(),
        &loadout.upstreams,
        route.effective_service_names(),
        crate::mcp::route_scope::McpRouteCapabilityGates::from_loadout(loadout),
    );
    if !scope.allows_service(crate::dispatch::depot_publish::SERVICE) {
        return Err("publish_not_in_project_loadout");
    }
    Ok(())
}

#[cfg(not(feature = "gateway"))]
async fn require_publish_capability(
    _: &AppState,
    _: &BoundAccessGrant,
) -> Result<(), &'static str> {
    Err("project_access_unavailable")
}

pub(super) async fn availability(
    State(state): State<AppState>,
    authority: Option<Extension<BrowserAuthority>>,
    Extension(auth): Extension<AuthContext>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Json<Value> {
    let Some(Extension(authority)) = authority else {
        return Json(json!({"available":false,"reason":"browser_session_required"}));
    };
    if source.is_none() && bound.is_none() {
        return match authorize_google(&state, &authority, &auth, identity.as_ref().map(|v| &v.0))
            .await
        {
            Ok(grant) => Json(json!({"available":true,"projectId":grant.project_id})),
            Err(reason) => Json(json!({"available":false,"reason":reason})),
        };
    }
    match authorize(
        &state,
        &authority,
        &auth,
        source.as_ref().map(|v| &v.0),
        bound.as_ref().map(|v| &v.0),
    )
    .await
    {
        Ok(grant) => Json(json!({"available":true,"projectId":grant.project_id})),
        Err(reason) => Json(json!({"available":false,"reason":reason})),
    }
}

pub(super) async fn publish(
    State(state): State<AppState>,
    authority: Option<Extension<BrowserAuthority>>,
    Extension(auth): Extension<AuthContext>,
    source: Option<Extension<ProductCredentialGrant>>,
    bound: Option<Extension<BoundAccessGrant>>,
    identity: Option<Extension<VerifiedIdentity>>,
    headers: HeaderMap,
    Json(request): Json<SkillRequest>,
) -> Result<Json<Value>, ApiFailure> {
    let Some(Extension(authority)) = authority else {
        return Err(forbidden());
    };
    crate::api::services::require_session_csrf(
        "depot.publish_skill_archive",
        &headers,
        Some(&auth),
    )
    .map_err(|_| forbidden())?;
    let archive = crate::dispatch::depot_publish::skill_archive(&request.name, &request.source)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({"kind":"validation_failed","message":error.to_string()})),
            )
        })?;
    let response = if source.is_none() && bound.is_none() {
        authorize_google(&state, &authority, &auth, identity.as_ref().map(|v| &v.0))
            .await
            .map_err(|reason| {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"kind":"forbidden","message":reason})),
                )
            })?;
        state
            .depot
            .publish_skill_archive_for_browser_revalidated(
                "skill.tar.gz",
                archive,
                None,
                || async {
                    authorize_google(&state, &authority, &auth, identity.as_ref().map(|v| &v.0))
                        .await
                        .map_err(|_| DepotError::DelegationUnavailable)
                },
            )
            .await
            .map_err(map_error)?
    } else {
        let grant = authorize(
            &state,
            &authority,
            &auth,
            source.as_ref().map(|v| &v.0),
            bound.as_ref().map(|v| &v.0),
        )
        .await
        .map_err(|reason| {
            (
                StatusCode::FORBIDDEN,
                Json(json!({"kind":"forbidden","message":reason})),
            )
        })?;
        state
            .depot
            .publish_skill_archive("skill.tar.gz", archive, None, &grant)
            .await
            .map_err(map_error)?
    };
    let job = response
        .pointer("/result/job")
        .or_else(|| response.get("job"))
        .ok_or_else(|| map_error(DepotError::InvalidResponse))?;
    let id = job
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| map_error(DepotError::InvalidResponse))?;
    Ok(Json(
        json!({"jobId":id,"status":job.get("status").and_then(Value::as_str).unwrap_or("queued")}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composer_cannot_select_a_tenant_or_supply_a_grant() {
        for field in ["projectId", "tenantId", "grant", "token", "provider"] {
            let mut request = json!({"name":"review", "source":"# Review"});
            request[field] = json!("caller-controlled");
            assert!(serde_json::from_value::<SkillRequest>(request).is_err());
        }
    }

    #[tokio::test]
    async fn bearer_without_browser_authority_is_explicitly_unsupported() {
        let (_temp, _, mut auth, _) = super::super::tests::browser_context(&["lab"]).await;
        auth.via_session = false;
        let status = availability(
            State(AppState::new()),
            None,
            Extension(auth.clone()),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(
            status.0,
            json!({"available":false,"reason":"browser_session_required"})
        );
        let result = publish(
            State(AppState::new()),
            None,
            Extension(auth),
            None,
            None,
            None,
            HeaderMap::new(),
            Json(SkillRequest {
                name: "review".into(),
                source: "# Review".into(),
            }),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ordinary_browser_session_cannot_supply_delegated_project_authority() {
        let (_temp, authority, auth, _) =
            super::super::tests::browser_context(&["lab:read", "lab"]).await;
        assert_eq!(
            authorize(&AppState::new(), &authority, &auth, None, None)
                .await
                .err(),
            Some("project_session_required")
        );
    }

    #[tokio::test]
    async fn read_only_browser_cannot_publish() {
        let (_temp, authority, auth, _) = super::super::tests::browser_context(&["lab:read"]).await;
        assert_eq!(
            authorize(&AppState::new(), &authority, &auth, None, None)
                .await
                .err(),
            Some("write_permission_required")
        );
    }

    #[tokio::test]
    async fn publish_requires_session_csrf_before_any_upstream_work() {
        let (_temp, authority, auth, _) =
            super::super::tests::browser_context(&["lab:read", "lab"]).await;
        let result = publish(
            State(AppState::new()),
            Some(Extension(authority)),
            Extension(auth),
            None,
            None,
            None,
            HeaderMap::new(),
            Json(SkillRequest {
                name: "review".into(),
                source: "# Review".into(),
            }),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::FORBIDDEN);
    }
}

#[cfg(all(test, feature = "proxy-testkit"))]
#[path = "depot_publish_test.rs"]
mod project_test;
