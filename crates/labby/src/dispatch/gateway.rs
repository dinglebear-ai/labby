//! Temporary compatibility shim: the gateway runtime moved into the
//! `labby-gateway` crate.
//!
//! Business logic now lives in `labby_gateway::gateway`. This module re-exports
//! it so existing `crate::dispatch::gateway::*` callers keep working during the
//! extraction. New runtime work should import `labby_gateway::gateway` directly.
//! The host-owned config-store implementation (which keeps `LabConfig` and the
//! `config.toml` render path in `lab`) lives in `config_store`.

pub use labby_gateway::gateway::*;

pub mod config_store;

/// Team-scoped gateway policy actions (`gateway.loadout.*`,
/// `gateway.protected_route.*`): every gateway action that is neither a
/// discovery built-in nor installation-scoped platform administration. The
/// authority class table in `crate::access` is the single source; this
/// predicate only names the complement so adapters never re-list actions.
#[must_use]
pub(crate) fn team_scoped_gateway_action(action: &str) -> bool {
    let bare = action.strip_prefix("gateway.").unwrap_or(action);
    action.starts_with("gateway.")
        && !matches!(bare, "help" | "schema")
        && !crate::access::gateway_transport_requires_admin(action)
}

/// Upstream names a team-scoped Loadout or protected-route mutation would
/// reference, read from the same request shapes the runtime dispatcher
/// deserializes (`loadout`, `patch`, and `route` payloads). The legacy
/// single `route.upstream` field and `route.target.upstreams` are treated
/// identically.
fn referenced_upstreams(action: &str, params: &serde_json::Value) -> Vec<String> {
    use labby_runtime::gateway_config::{ProtectedMcpRouteConfig, ProtectedMcpRouteTarget};
    let strings = |value: Option<&serde_json::Value>| {
        value
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let mut names = Vec::new();
    if action.starts_with("gateway.loadout.") {
        names.extend(strings(params.pointer("/loadout/upstreams")));
        names.extend(strings(params.pointer("/patch/upstreams")));
    } else if action.starts_with("gateway.protected_route.")
        && let Some(route) = params.get("route")
    {
        match serde_json::from_value::<ProtectedMcpRouteConfig>(route.clone()) {
            Ok(route) => {
                names.extend(route.upstream.clone());
                if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &route.target {
                    names.extend(target.upstreams.iter().cloned());
                }
            }
            Err(_) => {
                // Malformed routes are rejected by the runtime dispatcher's
                // strict deserializer; still surface every literal upstream
                // reference so an invalid payload cannot skip this gate.
                names.extend(
                    route
                        .get("upstream")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
                names.extend(strings(route.pointer("/target/upstreams")));
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Team-scoped Loadouts and protected routes may reference only upstreams
/// that carry an active host-custodied credential binding for that Team.
/// Shared by every surface adapter; a reference to any other upstream is a
/// caller-fixable `invalid_param` and never reveals whether the upstream
/// exists at installation scope.
pub(crate) async fn validate_team_scoped_upstream_references(
    store: &crate::access::AccessStore,
    team_id: &str,
    action: &str,
    params: &serde_json::Value,
) -> Result<(), crate::dispatch::error::ToolError> {
    use labby_runtime::gateway_authority::TeamCredentialStatus;
    if !team_scoped_gateway_action(action) {
        return Ok(());
    }
    for upstream in referenced_upstreams(action, params) {
        let binding = store
            .get_team_gateway_credential_binding(team_id.to_owned(), upstream.clone())
            .await
            .map_err(|error| {
                crate::dispatch::access_errors::map_store_error("gateway", error, || {
                    crate::dispatch::error::ToolError::Forbidden {
                        message: "Gateway operation is not authorized".into(),
                        required_scopes: Vec::new(),
                    }
                })
            })?;
        if !binding.is_some_and(|binding| binding.status == TeamCredentialStatus::Active) {
            tracing::warn!(
                service = "gateway",
                action,
                team_id,
                kind = "invalid_param",
                "team-scoped gateway policy referenced an upstream without an active Team credential binding"
            );
            return Err(crate::dispatch::error::ToolError::InvalidParam {
                message: "team-scoped gateway policy may only reference upstreams with an active Team credential binding".into(),
                param: "upstreams".into(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod team_scope_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn team_scoped_predicate_is_the_complement_of_platform_and_discovery() {
        assert!(team_scoped_gateway_action("gateway.loadout.list"));
        assert!(team_scoped_gateway_action("gateway.protected_route.add"));
        assert!(!team_scoped_gateway_action("gateway.add"));
        assert!(!team_scoped_gateway_action("gateway.oauth.clear"));
        assert!(!team_scoped_gateway_action("help"));
        assert!(!team_scoped_gateway_action("gateway.schema"));
        assert!(!team_scoped_gateway_action("skills.list"));
        // Surface policy and the domain class table agree on every action.
        for spec in ACTIONS {
            if matches!(
                spec.name,
                "help" | "schema" | "gateway.help" | "gateway.schema"
            ) {
                continue;
            }
            assert_eq!(
                spec.requires_admin,
                !team_scoped_gateway_action(spec.name),
                "{}",
                spec.name
            );
        }
    }

    #[test]
    fn referenced_upstreams_cover_loadouts_patches_and_both_route_shapes() {
        assert_eq!(
            referenced_upstreams(
                "gateway.loadout.add",
                &json!({"loadout": {"name": "l", "upstreams": ["b", "a", "a"]}})
            ),
            ["a", "b"]
        );
        assert_eq!(
            referenced_upstreams(
                "gateway.loadout.patch",
                &json!({"name": "l", "patch": {"upstreams": ["z"]}})
            ),
            ["z"]
        );
        let route = json!({"route": {
            "name": "r",
            "public_host": "mcp.example.com",
            "public_path": "/r",
            "upstream": "legacy",
            "target": {"kind": "gateway_subset", "upstreams": ["sub"]}
        }});
        assert_eq!(
            referenced_upstreams("gateway.protected_route.add", &route),
            ["legacy", "sub"]
        );
        let malformed = json!({"route": {"upstream": "only", "target": {"upstreams": ["x"]}}});
        assert_eq!(
            referenced_upstreams("gateway.protected_route.stage_add", &malformed),
            ["only", "x"]
        );
        assert!(referenced_upstreams("gateway.add", &json!({"spec": {"name": "n"}})).is_empty());
    }

    #[tokio::test]
    async fn team_policy_may_reference_only_upstreams_bound_to_that_team() {
        use crate::access::{AccessStore, BootstrapOwnerInput, PutTeamCredentialBinding};
        let directory = tempfile::Builder::new()
            .prefix("labby-gateway-team-scope-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner, "Local", "Default").unwrap())
            .await
            .unwrap();
        store
            .put_team_gateway_credential_binding(PutTeamCredentialBinding {
                binding_id: "binding-1".into(),
                team_id: "bootstrap-initial-team".into(),
                upstream_name: "bound".into(),
                custodian_principal_id: "bootstrap-owner".into(),
                rotated_at_millis: 1,
            })
            .await
            .unwrap();
        let loadout =
            |upstreams: &[&str]| json!({"loadout": {"name": "l", "upstreams": upstreams}});
        assert!(
            validate_team_scoped_upstream_references(
                &store,
                "bootstrap-initial-team",
                "gateway.loadout.add",
                &loadout(&["bound"]),
            )
            .await
            .is_ok()
        );
        let error = validate_team_scoped_upstream_references(
            &store,
            "bootstrap-initial-team",
            "gateway.loadout.add",
            &loadout(&["bound", "unbound"]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(!error.to_string().contains("unbound"));
        // Another Team gets the same answer for the same upstream name.
        let foreign = validate_team_scoped_upstream_references(
            &store,
            "other-team",
            "gateway.loadout.add",
            &loadout(&["bound"]),
        )
        .await
        .unwrap_err();
        assert_eq!(foreign.to_string(), error.to_string());
        // Platform actions and loadouts without upstream references pass.
        assert!(
            validate_team_scoped_upstream_references(
                &store,
                "other-team",
                "gateway.add",
                &json!({"spec": {"name": "bound"}}),
            )
            .await
            .is_ok()
        );
        store
            .revoke_team_gateway_credential_binding(
                "bootstrap-initial-team".into(),
                "bound".into(),
                2,
            )
            .await
            .unwrap();
        assert_eq!(
            validate_team_scoped_upstream_references(
                &store,
                "bootstrap-initial-team",
                "gateway.loadout.add",
                &loadout(&["bound"]),
            )
            .await
            .unwrap_err()
            .kind(),
            "invalid_param"
        );
    }
}
