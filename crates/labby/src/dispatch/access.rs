//! Shared multi-user administration dispatch.

use std::time::{SystemTime, UNIX_EPOCH};

use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerScope, ResourceFamily, ResourceId, ResourceRef,
        TeamId,
    },
    action::{ActionSpec, ParamSpec},
};
use labby_runtime::authority::AuthoritySafeBoundary;
use serde_json::{Value, json};

use crate::{
    access::{
        AcceptTeamInvitationInput, ActionAuthoritySpec, AddTeamMemberInput, AssignTeamProjectInput,
        AuthorityCeiling, AuthorityRequest, CreateTeamInput, CreateTeamInvitationInput,
        PlatformAdministratorInput, ProjectRole, TeamMembershipInput, TeamRole, authorize_action,
    },
    dispatch::error::ToolError,
};

/// Shared action catalog. The Team Gateway credential actions exist only with
/// the `gateway` feature, so their specs are compiled out together with their
/// dispatch arms instead of advertising actions that can never execute.
macro_rules! access_actions {
    ($($gateway_only:expr),* $(,)?) => {
        &[
            admin_action(
                "access.team.create",
                "Create a team",
                &[string("team_id"), string("name")],
            ),
            action("access.team.list", "List the caller's teams", &[]),
            action(
                "access.team.member.add",
                "Add a team member",
                &[string("team_id"), string("principal_id"), string("role")],
            ),
            action(
                "access.team.member.role.set",
                "Change a team member role",
                &[string("team_id"), string("principal_id"), string("role")],
            ),
            action(
                "access.team.member.suspend",
                "Suspend a team member",
                &[string("team_id"), string("principal_id")],
            ),
            action(
                "access.team.member.remove",
                "Remove a team member",
                &[string("team_id"), string("principal_id")],
            ),
            action(
                "access.team.suspend",
                "Suspend a team",
                &[string("team_id")],
            ),
            action(
                "access.team.activate",
                "Activate a team",
                &[string("team_id")],
            ),
            action(
                "access.team_invitation.create",
                "Create a team invitation",
                &[
                    string("team_id"),
                    string("principal_id"),
                    string("role"),
                    string("token"),
                    integer("ttl_seconds"),
                ],
            ),
            action(
                "access.team_invitation.accept",
                "Accept a team invitation",
                &[string("token")],
            ),
            action(
                "access.team_project.assign",
                "Assign a team to a project",
                &[string("team_id"), string("project_id"), string("role")],
            ),
            action(
                "access.project.effective.list",
                "List effective project roles",
                &[],
            ),
            admin_action(
                "access.platform_admin.grant",
                "Grant platform administrator authority",
                &[string("principal_id")],
            ),
            admin_action(
                "access.platform_admin.revoke",
                "Revoke platform administrator authority",
                &[string("principal_id")],
            ),
            $($gateway_only,)*
        ]
    };
}

#[cfg(feature = "gateway")]
pub const ACTIONS: &[ActionSpec] = access_actions!(
    action(
        "access.gateway_credential.list",
        "List redacted Team Gateway credential bindings",
        &[string("team_id")],
    ),
    action(
        "access.gateway_credential.bind",
        "Bind a host-custodied credential to a Team upstream",
        &[
            string("team_id"),
            string("upstream_name"),
            string("binding_id"),
        ],
    ),
    action(
        "access.gateway_credential.revoke",
        "Revoke a Team Gateway credential binding",
        &[string("team_id"), string("upstream_name")],
    ),
);

#[cfg(not(feature = "gateway"))]
pub const ACTIONS: &[ActionSpec] = access_actions!();

/// Exact capability the shared evaluator demands for `action`, or `None` for
/// the two caller-membership projections (`access.team.list`,
/// `access.project.effective.list`) whose visibility is filtered inside the
/// store. Surface policy (`requires_admin`) and the generated action catalog
/// derive from this single table; `requires_admin` is true exactly when the
/// capability is platform-scoped.
pub(crate) fn required_capability(action: &str) -> Option<Capability> {
    Some(match action {
        "access.team.list" | "access.project.effective.list" => return None,
        "access.team.create" | "access.platform_admin.grant" | "access.platform_admin.revoke" => {
            Capability::PlatformManage
        }
        "access.team_invitation.accept" => Capability::ScopeOperate,
        "access.gateway_credential.list" => Capability::ScopeRead,
        "access.gateway_credential.bind"
        | "access.gateway_credential.revoke"
        | "access.team_project.assign" => Capability::ScopeManage,
        "access.team.member.add"
        | "access.team.member.role.set"
        | "access.team.member.suspend"
        | "access.team.member.remove"
        | "access.team.suspend"
        | "access.team.activate"
        | "access.team_invitation.create" => Capability::MembershipManage,
        _ => return None,
    })
}

const fn string(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
        required: true,
        description: "",
    }
}
const fn integer(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "integer",
        required: true,
        description: "",
    }
}
const fn action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive: false,
        requires_admin: false,
        params,
        returns: "object",
    }
}
/// Installation-scoped administration: the shared evaluator demands
/// `platform.manage`, so the transport ceiling must carry `lab:admin`.
const fn admin_action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive: false,
        requires_admin: true,
        params,
        returns: "object",
    }
}

#[derive(Clone)]
pub(crate) struct AccessDispatchContext {
    pub(crate) store: crate::access::AccessStore,
    pub(crate) identity: VerifiedIdentity,
    pub(crate) ceiling: AuthorityCeiling,
    pub(crate) installation_id: String,
    #[cfg(feature = "gateway")]
    pub(crate) gateway_manager:
        Option<std::sync::Arc<labby_gateway::gateway::manager::GatewayManager>>,
}

pub(crate) async fn dispatch(
    context: AccessDispatchContext,
    action_name: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if action_name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("access", ACTIONS));
    }
    if action_name == "schema" {
        return crate::dispatch::helpers::action_schema(
            ACTIONS,
            &required_string(&params, "action")?,
        );
    }
    if !ACTIONS.iter().any(|spec| spec.name == action_name) {
        return Err(unknown_action(action_name));
    }

    // Filtered list operations derive visibility inside AccessStore. Every mutation is
    // additionally checked through the exact evaluator.
    if action_name != "access.team.list" && action_name != "access.project.effective.list" {
        authorize_administration(&context, action_name, &params).await?;
    }

    let result = match action_name {
        "access.team.create" => {
            let input = CreateTeamInput::new(
                context.identity,
                required_string(&params, "team_id")?,
                required_string(&params, "name")?,
            )
            .map_err(map_access_error)?;
            let value = context
                .store
                .create_team(input)
                .await
                .map_err(map_access_error)?;
            team_json(value)
        }
        "access.team.list" => {
            let values = context
                .store
                .list_teams(context.identity)
                .await
                .map_err(map_access_error)?;
            json!({"teams": values.into_iter().map(team_json).collect::<Vec<_>>()})
        }
        "access.team.member.add" | "access.team.member.role.set" => {
            let input = AddTeamMemberInput::new(
                context.identity,
                required_string(&params, "team_id")?,
                required_string(&params, "principal_id")?,
                team_role(&params)?,
            )
            .map_err(map_access_error)?;
            if action_name.ends_with("role.set") {
                context
                    .store
                    .set_team_member_role(input)
                    .await
                    .map_err(map_access_error)?;
                json!({"ok": true})
            } else {
                membership_json(
                    context
                        .store
                        .add_team_member(input)
                        .await
                        .map_err(map_access_error)?,
                )
            }
        }
        "access.team.member.suspend" | "access.team.member.remove" => {
            let input = TeamMembershipInput::new(
                context.identity,
                required_string(&params, "team_id")?,
                required_string(&params, "principal_id")?,
            )
            .map_err(map_access_error)?;
            if action_name.ends_with("remove") {
                context.store.remove_team_member(input).await
            } else {
                context.store.suspend_team_member(input).await
            }
            .map_err(map_access_error)?;
            json!({"ok": true})
        }
        "access.team.suspend" | "access.team.activate" => {
            let team_id = required_string(&params, "team_id")?;
            if action_name.ends_with("suspend") {
                context.store.suspend_team(context.identity, team_id).await
            } else {
                context.store.activate_team(context.identity, team_id).await
            }
            .map_err(map_access_error)?;
            json!({"ok": true})
        }
        "access.team_invitation.create" => {
            let input = CreateTeamInvitationInput::new(
                context.identity,
                required_string(&params, "team_id")?,
                required_string(&params, "principal_id")?,
                team_role(&params)?,
                token(&params)?,
                required_i64(&params, "ttl_seconds")?,
            )
            .map_err(map_access_error)?;
            let value = context
                .store
                .create_team_invitation(input)
                .await
                .map_err(map_access_error)?;
            json!({"team_id": value.team_id, "role": team_role_name(value.role), "status": value.status, "team_membership_epoch": value.team_membership_epoch, "expires_at": value.expires_at})
        }
        "access.team_invitation.accept" => membership_json(
            context
                .store
                .accept_team_invitation(
                    AcceptTeamInvitationInput::new(context.identity, token(&params)?)
                        .map_err(map_access_error)?,
                )
                .await
                .map_err(map_access_error)?,
        ),
        "access.team_project.assign" => {
            let input = AssignTeamProjectInput::new(
                context.identity,
                required_string(&params, "team_id")?,
                required_string(&params, "project_id")?,
                project_role(&params)?,
            )
            .map_err(map_access_error)?;
            let value = context
                .store
                .assign_team_project(input)
                .await
                .map_err(map_access_error)?;
            json!({"team_id": value.team_id, "project_id": value.project_id, "role": project_role_name(value.role), "assignment_epoch": value.assignment_epoch})
        }
        "access.project.effective.list" => {
            let values = context
                .store
                .list_effective_projects(context.identity)
                .await
                .map_err(map_access_error)?;
            json!({"projects": values.into_iter().map(|value| json!({"project_id": value.project_id, "role": project_role_name(value.role), "direct": value.direct, "team_derived": value.team_derived, "global_revision": value.global_revision})).collect::<Vec<_>>()})
        }
        #[cfg(feature = "gateway")]
        "access.gateway_credential.list" => {
            let values = context
                .store
                .list_team_gateway_credential_bindings(required_string(&params, "team_id")?)
                .await
                .map_err(map_access_error)?;
            json!({"bindings": values})
        }
        #[cfg(feature = "gateway")]
        "access.gateway_credential.bind" => {
            let now = now_millis()?;
            let custodian = context
                .store
                .resolve_file_stash_principal(context.identity.clone())
                .await
                .map_err(map_access_error)?;
            let request = administration_authority_request(&context, action_name, &params).await?;
            let value = context
                .store
                .put_team_gateway_credential_binding_authorized(
                    request,
                    crate::access::PutTeamCredentialBinding {
                        binding_id: required_string(&params, "binding_id")?,
                        team_id: required_string(&params, "team_id")?,
                        upstream_name: required_string(&params, "upstream_name")?,
                        custodian_principal_id: custodian.as_str().to_owned(),
                        rotated_at_millis: now,
                    },
                )
                .await
                .map_err(map_access_error)?;
            invalidate_team_gateway_credential(
                &context,
                &value.team_id,
                &value.upstream_name,
                "team credential rotated",
            )
            .await;
            serde_json::to_value(value).map_err(|_| unavailable())?
        }
        #[cfg(feature = "gateway")]
        "access.gateway_credential.revoke" => {
            let request = administration_authority_request(&context, action_name, &params).await?;
            let binding = context
                .store
                .revoke_team_gateway_credential_binding_authorized(
                    request,
                    required_string(&params, "team_id")?,
                    required_string(&params, "upstream_name")?,
                    now_millis()?,
                )
                .await
                .map_err(|error| match error {
                    crate::access::AccessStoreError::TeamCredentialBindingUnavailable => {
                        ToolError::Sdk {
                            sdk_kind: "not_found".to_owned(),
                            message: "no Team Gateway credential binding exists for that upstream"
                                .to_owned(),
                        }
                    }
                    other => map_access_error(other),
                })?;
            invalidate_team_gateway_credential(
                &context,
                &binding.team_id,
                &binding.upstream_name,
                "team credential revoked",
            )
            .await;
            json!({"binding": binding})
        }
        "access.platform_admin.grant" | "access.platform_admin.revoke" => {
            let input = PlatformAdministratorInput::new(
                context.identity,
                required_string(&params, "principal_id")?,
            )
            .map_err(map_access_error)?;
            if action_name.ends_with("grant") {
                context.store.grant_platform_administrator(input).await
            } else {
                context.store.revoke_platform_administrator(input).await
            }
            .map_err(map_access_error)?;
            json!({"ok": true})
        }
        _ => return Err(unknown_action(action_name)),
    };
    Ok(result)
}

#[cfg(feature = "gateway")]
async fn invalidate_team_gateway_credential(
    context: &AccessDispatchContext,
    team_id: &str,
    upstream: &str,
    reason: &'static str,
) {
    let Some(manager) = &context.gateway_manager else {
        tracing::warn!(
            service = "access",
            team_id,
            upstream,
            reason,
            "team gateway credential invalidation skipped: no gateway manager is wired"
        );
        return;
    };
    match manager.current_pool().await {
        Some(pool) => {
            pool.invalidate_oauth_subject_sessions(upstream, &format!("team:{team_id}"), reason)
                .await;
        }
        None => tracing::warn!(
            service = "access",
            team_id,
            upstream,
            reason,
            "team gateway credential invalidation skipped: upstream pool is not loaded"
        ),
    }
}

async fn authorize_administration(
    context: &AccessDispatchContext,
    action: &str,
    params: &Value,
) -> Result<(), ToolError> {
    let request = administration_authority_request(context, action, params).await?;
    authorize_action(&context.store, request)
        .await
        .map_err(map_access_error)?;
    Ok(())
}

async fn administration_authority_request(
    context: &AccessDispatchContext,
    action: &str,
    params: &Value,
) -> Result<AuthorityRequest, ToolError> {
    // One capability table drives the evaluator, the surface admin gate, and
    // the generated catalog; an action without a row cannot be authorized.
    let capability = required_capability(action).ok_or_else(|| unknown_action(action))?;
    let (owner, family, id) = if action == "access.team_invitation.accept" {
        (
            crate::access::resolve_personal_owner(&context.store, context.identity.clone())
                .await
                .map_err(map_access_error)?,
            ResourceFamily::Platform,
            "team-invitation".to_owned(),
        )
    } else if capability.is_platform() {
        (
            OwnerScope::Installation(
                InstallationId::new(context.installation_id.clone())
                    .map_err(|_| invalid("installation_id"))?,
            ),
            ResourceFamily::Platform,
            context.installation_id.clone(),
        )
    } else {
        let team_id = required_string(params, "team_id")?;
        (
            OwnerScope::Team(TeamId::new(team_id.clone()).map_err(|_| invalid("team_id"))?),
            ResourceFamily::Platform,
            team_id,
        )
    };
    let action_ref = ActionRef::new("access", action).map_err(|_| invalid("action"))?;
    let resource = ResourceRef::new(
        owner,
        family,
        ResourceId::new(id).map_err(|_| invalid("resource_id"))?,
    );
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ToolError::internal_message("system clock unavailable"))?
            .as_millis(),
    )
    .map_err(|_| ToolError::internal_message("system clock unavailable"))?;
    Ok(AuthorityRequest::new(
        context.identity.clone(),
        ActionAuthoritySpec::SCHEMA_VERSION,
        action_ref.clone(),
        resource,
        context.ceiling.clone(),
        None,
        now,
        vec![AuthoritySafeBoundary::BeforeDispatch],
        vec![ActionAuthoritySpec::new(action_ref, family, capability)],
    ))
}

fn now_millis() -> Result<u64, ToolError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_millis(),
    )
    .map_err(|_| unavailable())
}

fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".to_owned(),
        message: "access administration is unavailable".to_owned(),
    }
}

fn required_string(params: &Value, name: &str) -> Result<String, ToolError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| ToolError::MissingParam {
            message: format!("missing required parameter `{name}`"),
            param: name.to_owned(),
        })
}
fn required_i64(params: &Value, name: &str) -> Result<i64, ToolError> {
    params
        .get(name)
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid(name))
}
fn token(params: &Value) -> Result<[u8; 32], ToolError> {
    let encoded = required_string(params, "token")?;
    let bytes = hex::decode(encoded).map_err(|_| invalid("token"))?;
    bytes.try_into().map_err(|_| invalid("token"))
}
fn team_role(params: &Value) -> Result<TeamRole, ToolError> {
    match required_string(params, "role")?.as_str() {
        "owner" => Ok(TeamRole::Owner),
        "admin" => Ok(TeamRole::Admin),
        "member" => Ok(TeamRole::Member),
        _ => Err(invalid("role")),
    }
}
fn project_role(params: &Value) -> Result<ProjectRole, ToolError> {
    match required_string(params, "role")?.as_str() {
        "owner" => Ok(ProjectRole::Owner),
        "admin" => Ok(ProjectRole::Admin),
        "member" => Ok(ProjectRole::Member),
        "viewer" => Ok(ProjectRole::Viewer),
        _ => Err(invalid("role")),
    }
}
fn team_role_name(role: TeamRole) -> &'static str {
    match role {
        TeamRole::Owner => "owner",
        TeamRole::Admin => "admin",
        TeamRole::Member => "member",
    }
}
fn project_role_name(role: ProjectRole) -> &'static str {
    match role {
        ProjectRole::Owner => "owner",
        ProjectRole::Admin => "admin",
        ProjectRole::Member => "member",
        ProjectRole::Viewer => "viewer",
    }
}
fn team_json(value: crate::access::TeamSnapshot) -> Value {
    json!({"team_id": value.team_id, "name": value.name, "status": value.status, "role": value.role.map(team_role_name), "policy_epoch": value.policy_epoch, "membership_epoch": value.membership_epoch, "global_revision": value.global_revision})
}
fn membership_json(value: crate::access::TeamMembershipSnapshot) -> Value {
    json!({"team_id": value.team_id, "principal_id": value.principal_id, "role": team_role_name(value.role), "status": value.status, "membership_epoch": value.membership_epoch})
}
fn invalid(param: &str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid parameter `{param}`"),
        param: param.to_owned(),
    }
}
fn unknown_action(action: &str) -> ToolError {
    ToolError::UnknownAction {
        message: "unknown access action".to_owned(),
        valid: ACTIONS.iter().map(|spec| spec.name.to_owned()).collect(),
        hint: ACTIONS
            .iter()
            .find(|spec| spec.name.starts_with(action))
            .map(|spec| spec.name.to_owned()),
    }
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "access denied".to_owned(),
        required_scopes: Vec::new(),
    }
}
/// Every store failure goes through the shared map: denial-class failures
/// collapse to one non-enumerating `forbidden`, input problems become
/// `invalid_param`, and lifecycle/integrity failures become a fixed-string
/// `service_unavailable` whose typed cause is logged server-side only.
fn map_access_error(error: crate::access::AccessStoreError) -> ToolError {
    crate::dispatch::access_errors::map_store_error("access", error, denied)
}

/// Registry fallback. Real API/MCP adapters must supply server-owned state and identity.
pub async fn dispatch_unbound(action: &str, params: Value) -> Result<Value, ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload("access", ACTIONS));
    }
    if action == "schema" {
        return crate::dispatch::helpers::action_schema(
            ACTIONS,
            &required_string(&params, "action")?,
        );
    }
    Err(ToolError::Forbidden {
        message: "access administration requires host-established identity".to_owned(),
        required_scopes: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_registers_only_canonical_access_actions() {
        assert_eq!(
            ACTIONS.len(),
            if cfg!(feature = "gateway") { 17 } else { 14 }
        );
        assert!(ACTIONS.iter().all(|spec| spec.name.starts_with("access.")));
        assert!(ACTIONS.iter().all(|spec| !spec.destructive));
        assert_eq!(
            ACTIONS
                .iter()
                .any(|spec| spec.name.starts_with("access.gateway_credential.")),
            cfg!(feature = "gateway")
        );
    }

    #[test]
    fn requires_admin_is_exactly_the_platform_capability_axis() {
        for spec in ACTIONS {
            let capability = required_capability(spec.name);
            assert_eq!(
                spec.requires_admin,
                capability.is_some_and(Capability::is_platform),
                "{}",
                spec.name
            );
        }
        assert_eq!(required_capability("access.team.list"), None);
        assert_eq!(required_capability("access.bogus"), None);
        assert_eq!(
            required_capability("access.team.create"),
            Some(Capability::PlatformManage)
        );
    }

    #[test]
    fn tokens_are_exact_opaque_32_byte_values() {
        let valid = json!({"token": hex::encode([7_u8; 32])});
        assert_eq!(token(&valid).unwrap(), [7_u8; 32]);
        assert!(token(&json!({"token": "07"})).is_err());
        assert!(token(&json!({"token": "not-hex"})).is_err());
    }

    #[test]
    fn inaccessible_targets_collapse_to_one_non_enumerating_error() {
        for error in [
            crate::access::AccessStoreError::NotAuthorized,
            crate::access::AccessStoreError::IdentityUnavailable,
            crate::access::AccessStoreError::ProjectAccessUnavailable,
            crate::access::AccessStoreError::TeamUnavailable,
        ] {
            let mapped = map_access_error(error);
            assert_eq!(mapped.kind(), "forbidden");
            let envelope: Value = serde_json::from_str(&mapped.to_string()).unwrap();
            assert_eq!(envelope["message"], "access denied");
            assert_eq!(envelope["required_scopes"], json!([]));
        }
        // Lifecycle and integrity failures are outages with fixed messages.
        for error in [
            crate::access::AccessStoreError::MalformedVocabulary,
            crate::access::AccessStoreError::Unavailable("sqlite: /secret/access.db".into()),
            crate::access::AccessStoreError::Locked,
        ] {
            let mapped = map_access_error(error);
            assert_eq!(mapped.kind(), "service_unavailable");
            assert!(!mapped.to_string().contains("/secret"));
        }
        assert_eq!(
            map_access_error(crate::access::AccessStoreError::InvalidTeamInput).kind(),
            "invalid_param"
        );
    }

    #[tokio::test]
    async fn context_free_registry_fallback_never_executes_authority() {
        let error = dispatch_unbound("access.team.create", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }
}