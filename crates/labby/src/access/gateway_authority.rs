use std::time::{SystemTime, UNIX_EPOCH};

use labby_auth::{VerifiedIdentity, auth_context::AuthContext};
use labby_primitives::access::{
    ActionRef, Capability, InstallationId, OwnerScope, ResourceFamily, ResourceId, ResourceRef,
    TeamId,
};
use labby_runtime::authority::AuthoritySafeBoundary;

use super::{
    AccessRuntime, ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest, authorize_action,
};
use crate::dispatch::error::ToolError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GatewayAuthorityClass {
    Public,
    ScopedRead,
    ScopedManage,
    PlatformManage,
}

/// Gateway policy is team-manageable; host configuration and process/credential
/// lifecycle are installation authority. Unknown actions fail closed.
pub(crate) fn gateway_authority_class(action: &str) -> Option<GatewayAuthorityClass> {
    Some(match action {
        "help" | "schema" => GatewayAuthorityClass::Public,
        "gateway.loadout.list"
        | "gateway.loadout.list_state"
        | "gateway.loadout.get"
        | "gateway.protected_route.list"
        | "gateway.protected_route.list_state"
        | "gateway.protected_route.get" => GatewayAuthorityClass::ScopedRead,
        action
            if action.starts_with("gateway.loadout.")
                || action.starts_with("gateway.protected_route.") =>
        {
            GatewayAuthorityClass::ScopedManage
        }
        action if action.starts_with("gateway.") => GatewayAuthorityClass::PlatformManage,
        _ => return None,
    })
}

pub(crate) async fn authorize_gateway_action(
    runtime: &AccessRuntime,
    identity: VerifiedIdentity,
    auth: &AuthContext,
    installation_id: &str,
    team_id: Option<&str>,
    action: &str,
) -> Result<(), ToolError> {
    let class = gateway_authority_class(action).ok_or_else(denied)?;
    if class == GatewayAuthorityClass::Public {
        return Ok(());
    }
    let store = runtime.store().await.map_err(|_| denied())?;
    let (owner, capability, resource_id) = match class {
        GatewayAuthorityClass::ScopedRead | GatewayAuthorityClass::ScopedManage => {
            let team_id = team_id.ok_or_else(denied)?;
            let owner = OwnerScope::Team(TeamId::new(team_id).map_err(|_| denied())?);
            let capability = if class == GatewayAuthorityClass::ScopedRead {
                Capability::ScopeRead
            } else {
                Capability::ScopeManage
            };
            (owner, capability, team_id.to_owned())
        }
        GatewayAuthorityClass::PlatformManage => (
            OwnerScope::Installation(InstallationId::new(installation_id).map_err(|_| denied())?),
            Capability::PlatformManage,
            installation_id.to_owned(),
        ),
        GatewayAuthorityClass::Public => unreachable!(),
    };
    let action_ref = ActionRef::new("gateway", action).map_err(|_| denied())?;
    let resource = ResourceRef::new(
        owner,
        ResourceFamily::Gateway,
        ResourceId::new(resource_id).map_err(|_| denied())?,
    );
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| denied())?
            .as_millis(),
    )
    .map_err(|_| denied())?;
    authorize_action(
        &store,
        AuthorityRequest::new(
            identity,
            ActionAuthoritySpec::SCHEMA_VERSION,
            action_ref.clone(),
            resource,
            AuthorityCeiling::from_auth_context(auth),
            None,
            now,
            vec![AuthoritySafeBoundary::BeforeDispatch],
            vec![ActionAuthoritySpec::new(
                action_ref,
                ResourceFamily::Gateway,
                capability,
            )],
        ),
    )
    .await
    .map(|_| ())
    .map_err(|_| denied())
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Gateway operation is not authorized".into(),
        required_scopes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_policy_is_distinct_from_host_authority() {
        assert_eq!(
            gateway_authority_class("gateway.loadout.add"),
            Some(GatewayAuthorityClass::ScopedManage)
        );
        assert_eq!(
            gateway_authority_class("gateway.protected_route.get"),
            Some(GatewayAuthorityClass::ScopedRead)
        );
        for action in [
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.reload",
            "gateway.oauth.clear",
            "gateway.mcp.restart",
            "gateway.service_config.set",
        ] {
            assert_eq!(
                gateway_authority_class(action),
                Some(GatewayAuthorityClass::PlatformManage),
                "{action}"
            );
        }
        assert_eq!(
            gateway_authority_class("gateway.unknown"),
            Some(GatewayAuthorityClass::PlatformManage)
        );
        assert_eq!(gateway_authority_class("other.action"), None);
    }
}
