//! Team-scoped Project lifecycle semantics shared by the authenticated
//! surfaces: the HTTP adapter at `/v1/projects` and the caller-bound MCP
//! `projects` tool. Both adapters supply a host-established identity and the
//! transport authority ceiling; the registry's context-free entry
//! ([`dispatch_unbound`]) answers only `help`/`schema` and denies everything
//! else without revealing whether a Team or Project exists.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    access::{
        AccessStore, ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest,
        ManageTeamProjectInput, authorize_action,
    },
    dispatch::{access_errors::map_store_error, error::ToolError},
};
use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{ActionRef, Capability, OwnerScope, ResourceFamily, ResourceId, ResourceRef, TeamId},
    action::{ActionSpec, ParamSpec},
};
use labby_runtime::authority::AuthoritySafeBoundary;
use serde_json::Value;

const SERVICE: &str = "projects";

const TEAM_ID: ParamSpec = ParamSpec {
    name: "team_id",
    ty: "string",
    required: true,
    description: "Owning Team identifier",
};
const PROJECT_ID: ParamSpec = ParamSpec {
    name: "project_id",
    ty: "string",
    required: true,
    description: "Project identifier",
};
const NAME: ParamSpec = ParamSpec {
    name: "name",
    ty: "string",
    required: true,
    description: "Human-readable Project name",
};

const fn action(
    name: &'static str,
    description: &'static str,
    destructive: bool,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive,
        requires_admin: false,
        params,
        returns: "object",
    }
}

pub const ACTIONS: &[ActionSpec] = &[
    action(
        "projects.list",
        "List Projects visible to the caller through Team membership",
        false,
        &[],
    ),
    action(
        "projects.create",
        "Create a Project owned by a Team the caller manages",
        false,
        &[TEAM_ID, PROJECT_ID, NAME],
    ),
    action(
        "projects.get",
        "Get one Team-owned Project",
        false,
        &[TEAM_ID, PROJECT_ID],
    ),
    action(
        "projects.update",
        "Rename a Team-owned Project",
        false,
        &[TEAM_ID, PROJECT_ID, NAME],
    ),
    // Archiving is reversible through `projects.activate`, so it is a state
    // mutation rather than a destructive (hard-to-recover) action.
    action(
        "projects.archive",
        "Archive a Team-owned Project (reversible with projects.activate)",
        false,
        &[TEAM_ID, PROJECT_ID],
    ),
    action(
        "projects.activate",
        "Reactivate an archived Team-owned Project",
        false,
        &[TEAM_ID, PROJECT_ID],
    ),
];

/// Exact capability the shared evaluator demands over the owning Team, or
/// `None` for the membership projection (`projects.list`) whose visibility is
/// filtered inside the store. The store re-checks Team role inside its own
/// transaction; this table only decides the ceiling/evaluator gate.
pub(crate) fn required_capability(action: &str) -> Option<Capability> {
    Some(match action {
        "projects.get" => Capability::ScopeRead,
        "projects.create" => Capability::ScopeCreate,
        "projects.update" | "projects.archive" | "projects.activate" => Capability::ScopeManage,
        _ => return None,
    })
}

#[derive(Clone)]
pub(crate) struct ProjectDispatchContext {
    pub(crate) store: AccessStore,
    pub(crate) identity: VerifiedIdentity,
    pub(crate) ceiling: AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: ProjectDispatchContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload(SERVICE, ACTIONS));
    }
    if action == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    if !ACTIONS.iter().any(|spec| spec.name == action) {
        return Err(unknown_action(action));
    }
    if action == "projects.list" {
        return serde_json::to_value(
            context
                .store
                .list_managed_projects(context.identity)
                .await
                .map_err(map)?,
        )
        .map_err(|_| unavailable());
    }
    let team_id = required(&params, "team_id")?;
    let project_id = required(&params, "project_id")?;
    authorize(&context, action, &team_id, &project_id).await?;
    let input = |name: Option<String>| {
        ManageTeamProjectInput::new(
            context.identity.clone(),
            team_id.clone(),
            project_id.clone(),
            name,
        )
        .map_err(map)
    };
    let result = match action {
        "projects.create" => {
            context
                .store
                .create_managed_project(input(Some(required(&params, "name")?))?)
                .await
        }
        "projects.get" => context.store.get_managed_project(input(None)?).await,
        "projects.update" => {
            context
                .store
                .update_managed_project(input(Some(required(&params, "name")?))?, false)
                .await
        }
        "projects.archive" => {
            context
                .store
                .update_managed_project(input(None)?, true)
                .await
        }
        "projects.activate" => context.store.activate_managed_project(input(None)?).await,
        _ => return Err(unknown_action(action)),
    }
    .map_err(map)?;
    serde_json::to_value(result).map_err(|_| unavailable())
}

/// Evaluate the transport ceiling and Team capability for a Project action
/// before the store's own role check runs inside its transaction.
async fn authorize(
    context: &ProjectDispatchContext,
    action: &str,
    team_id: &str,
    project_id: &str,
) -> Result<(), ToolError> {
    let capability = required_capability(action).ok_or_else(|| unknown_action(action))?;
    let action_ref = ActionRef::new(SERVICE, action).map_err(|_| invalid("action"))?;
    let resource = ResourceRef::new(
        OwnerScope::Team(TeamId::new(team_id).map_err(|_| invalid("team_id"))?),
        ResourceFamily::Project,
        ResourceId::new(project_id).map_err(|_| invalid("project_id"))?,
    );
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| unavailable())?
            .as_millis(),
    )
    .map_err(|_| unavailable())?;
    authorize_action(
        &context.store,
        AuthorityRequest::new(
            context.identity.clone(),
            ActionAuthoritySpec::SCHEMA_VERSION,
            action_ref.clone(),
            resource,
            context.ceiling.clone(),
            None,
            now,
            vec![AuthoritySafeBoundary::BeforeDispatch],
            vec![ActionAuthoritySpec::new(
                action_ref,
                ResourceFamily::Project,
                capability,
            )],
        ),
    )
    .await
    .map(|_| ())
    .map_err(map)
}

fn required(value: &Value, key: &'static str) -> Result<String, ToolError> {
    match value.get(key) {
        None | Some(Value::Null) => Err(ToolError::MissingParam {
            message: format!("missing required parameter `{key}`"),
            param: key.to_owned(),
        }),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        Some(_) => Err(invalid(key)),
    }
}
fn invalid(param: &str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid parameter `{param}`"),
        param: param.to_owned(),
    }
}
fn unknown_action(action: &str) -> ToolError {
    let mut valid = ACTIONS
        .iter()
        .map(|spec| spec.name.to_owned())
        .collect::<Vec<_>>();
    valid.push("help".to_owned());
    valid.push("schema".to_owned());
    ToolError::UnknownAction {
        message: format!("unknown action: `{action}`"),
        valid,
        hint: None,
    }
}
fn map(error: crate::access::AccessStoreError) -> ToolError {
    map_store_error(SERVICE, error, denied)
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "access denied".to_owned(),
        required_scopes: Vec::new(),
    }
}
fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".to_owned(),
        message: "Project service unavailable".to_owned(),
    }
}

/// Registry fallback. Real API/MCP adapters supply server-owned state and a
/// host-established identity; without them nothing can be authorized.
pub async fn dispatch_unbound(action: &str, params: Value) -> Result<Value, ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload(SERVICE, ACTIONS));
    }
    if action == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    Err(denied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::BootstrapOwnerInput;
    use labby_auth::Authenticator;
    use serde_json::json;

    fn secure_tempdir() -> tempfile::TempDir {
        let directory = tempfile::Builder::new()
            .prefix("labby-projects-dispatch-")
            .tempdir_in(std::env::current_dir().expect("test working directory"))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory
    }

    fn browser(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    async fn fixture() -> (tempfile::TempDir, ProjectDispatchContext) {
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = browser("owner");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (
            directory,
            ProjectDispatchContext {
                store,
                identity: owner,
                ceiling: AuthorityCeiling::trusted_local(),
            },
        )
    }

    #[test]
    fn catalog_is_complete_and_nothing_is_destructive() {
        assert_eq!(ACTIONS.len(), 6);
        assert!(
            ACTIONS
                .iter()
                .all(|spec| spec.name.starts_with("projects."))
        );
        assert!(ACTIONS.iter().all(|spec| !spec.requires_admin));
        let destructive = ACTIONS
            .iter()
            .filter(|spec| spec.destructive)
            .map(|spec| spec.name)
            .collect::<Vec<_>>();
        // Archive is reversible through `projects.activate`, so no Project
        // action can cause permanent loss.
        assert!(destructive.is_empty(), "{destructive:?}");
        for spec in ACTIONS {
            assert_eq!(
                spec.requires_admin,
                required_capability(spec.name).is_some_and(Capability::is_platform),
                "{}",
                spec.name
            );
        }
        assert_eq!(required_capability("projects.list"), None);
    }

    #[tokio::test]
    async fn unbound_answers_help_and_schema_but_denies_everything_else() {
        assert!(dispatch_unbound("help", json!({})).await.is_ok());
        assert!(
            dispatch_unbound("schema", json!({"action":"projects.get"}))
                .await
                .is_ok()
        );
        let error = dispatch_unbound(
            "projects.get",
            json!({"team_id":"guess","project_id":"guess"}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }

    #[tokio::test]
    async fn owner_manages_projects_and_parameter_problems_are_caller_fixable() {
        let (_directory, context) = fixture().await;
        let created = dispatch(
            context.clone(),
            "projects.create",
            json!({"team_id":"bootstrap-initial-team","project_id":"p1","name":"One"}),
        )
        .await
        .unwrap();
        assert_eq!(created["project_id"], "p1");
        assert_eq!(created["status"], "active");
        let renamed = dispatch(
            context.clone(),
            "projects.update",
            json!({"team_id":"bootstrap-initial-team","project_id":"p1","name":"Two"}),
        )
        .await
        .unwrap();
        assert_eq!(renamed["name"], "Two");
        let listed = dispatch(context.clone(), "projects.list", json!({}))
            .await
            .unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|project| project["project_id"] == "p1")
        );
        let archived = dispatch(
            context.clone(),
            "projects.archive",
            json!({"team_id":"bootstrap-initial-team","project_id":"p1"}),
        )
        .await
        .unwrap();
        assert_eq!(archived["status"], "disabled");
        // Archived Projects leave the membership listing and come back on
        // activation with a newer policy epoch.
        let listed = dispatch(context.clone(), "projects.list", json!({}))
            .await
            .unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .all(|project| project["project_id"] != "p1")
        );
        let activated = dispatch(
            context.clone(),
            "projects.activate",
            json!({"team_id":"bootstrap-initial-team","project_id":"p1"}),
        )
        .await
        .unwrap();
        assert_eq!(activated["status"], "active");
        assert_eq!(activated["name"], "Two");
        assert!(activated["policy_epoch"].as_u64() > archived["policy_epoch"].as_u64());
        let listed = dispatch(context.clone(), "projects.list", json!({}))
            .await
            .unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|project| project["project_id"] == "p1")
        );
        // Activating an already-active Project is the same non-enumerating
        // denial as an absent one.
        let again = dispatch(
            context.clone(),
            "projects.activate",
            json!({"team_id":"bootstrap-initial-team","project_id":"p1"}),
        )
        .await
        .unwrap_err();
        assert_eq!(again.kind(), "forbidden");

        let missing = dispatch(context.clone(), "projects.get", json!({"team_id":"t"}))
            .await
            .unwrap_err();
        assert_eq!(missing.kind(), "missing_param");
        let invalid = dispatch(
            context.clone(),
            "projects.get",
            json!({"team_id":"t","project_id":7}),
        )
        .await
        .unwrap_err();
        assert_eq!(invalid.kind(), "invalid_param");
        let unknown = dispatch(context, "projects.bogus", json!({}))
            .await
            .unwrap_err();
        assert_eq!(unknown.kind(), "unknown_action");
    }

    #[tokio::test]
    async fn strangers_and_read_only_ceilings_are_denied_without_enumeration() {
        let (_directory, owner) = fixture().await;
        let stranger = ProjectDispatchContext {
            identity: browser("stranger"),
            ..owner.clone()
        };
        let absent = dispatch(
            stranger.clone(),
            "projects.get",
            json!({"team_id":"bootstrap-initial-team","project_id":"absent"}),
        )
        .await
        .unwrap_err();
        let foreign_team = dispatch(
            stranger,
            "projects.create",
            json!({"team_id":"other-team","project_id":"p2","name":"Two"}),
        )
        .await
        .unwrap_err();
        assert_eq!(absent.kind(), "forbidden");
        assert_eq!(absent.to_string(), foreign_team.to_string());

        // A `lab:read`-only transport ceiling cannot mint scope.create even for
        // the bootstrap owner.
        let read_only = ProjectDispatchContext {
            ceiling: AuthorityCeiling::from_auth_context(&labby_auth::AuthContext {
                sub: "owner".into(),
                actor_key: None,
                scopes: vec!["lab:read".into()],
                issuer: "test".into(),
                via_session: false,
                csrf_token: None,
                email: None,
            }),
            ..owner
        };
        let error = dispatch(
            read_only,
            "projects.create",
            json!({"team_id":"bootstrap-initial-team","project_id":"p3","name":"Three"}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }
}
