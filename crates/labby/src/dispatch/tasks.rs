//! Authenticated, owner-scoped Agent Task surface shared by HTTP and MCP.

use crate::access::{ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest, authorize_action};
use crate::dispatch::error::ToolError;
use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerScope, PrincipalId, ProjectId, ResourceFamily,
        ResourceId, ResourceRef, TeamId,
    },
    action::{ActionSpec, ParamSpec},
    task::{TaskIntent, TaskState},
};
use labby_runtime::authority::AuthoritySafeBoundary;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const fn param(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
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
pub const ACTIONS: &[ActionSpec] = &[
    action(
        "tasks.create",
        "Create an immutable Agent Task intent",
        &[
            param("task_id"),
            param("idempotency_key"),
            param("owner_kind"),
            param("owner_id"),
            param("agent_id"),
        ],
    ),
    action("tasks.list", "List caller-visible Agent Tasks", &[]),
    action(
        "tasks.get",
        "Get a caller-visible Agent Task",
        &[param("task_id")],
    ),
    action("tasks.queue", "Queue an Agent Task", &[param("task_id")]),
    action("tasks.cancel", "Cancel an Agent Task", &[param("task_id")]),
    action(
        "tasks.result",
        "Read an Agent Task result",
        &[param("task_id")],
    ),
];

#[derive(Clone)]
pub(crate) struct TaskDispatchContext {
    pub store: crate::access::AccessStore,
    pub identity: VerifiedIdentity,
    pub ceiling: AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: TaskDispatchContext,
    name: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("tasks", ACTIONS));
    }
    if name == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    if !ACTIONS.iter().any(|a| a.name == name) {
        return Err(unknown(name));
    }
    let now = now()?;
    match name {
        "tasks.create" => {
            let owner = owner(&params)?;
            authorize(
                &context,
                name,
                &owner,
                required(&params, "task_id")?,
                Capability::ScopeCreate,
                now,
            )
            .await?;
            let agent = context
                .store
                .get_agent_definition(required(&params, "agent_id")?)
                .await
                .map_err(map)?
                .ok_or_else(denied)?;
            if agent.owner != owner || agent.state != labby_primitives::agent::AgentState::Active {
                return Err(denied());
            }
            let intent = TaskIntent {
                id: required(&params, "task_id")?,
                idempotency_key: required(&params, "idempotency_key")?,
                owner,
                project: params
                    .get("project_id")
                    .and_then(Value::as_str)
                    .map(|v| ProjectId::new(v.to_owned()).map_err(|_| invalid("project_id")))
                    .transpose()?,
                creator: PrincipalId::new(context.identity.safe_fingerprint())
                    .map_err(|_| invalid("principal"))?,
                agent_id: agent.id,
                agent_version: agent.revision.version,
                agent_revision_digest: agent.revision.content_digest,
                input_digest: required(&params, "input_digest")?,
                catalog_generation: agent.revision.catalog_generation,
                authority_fingerprint: context.identity.safe_fingerprint(),
            };
            let id = context
                .store
                .create_agent_task(intent, i64::try_from(now).map_err(|_| internal())?)
                .await
                .map_err(map)?;
            Ok(json!({"task_id":id,"state":"created"}))
        }
        "tasks.list" => {
            let mut tasks = Vec::new();
            for record in context.store.list_agent_tasks().await.map_err(map)? {
                if authorize(
                    &context,
                    name,
                    &record.intent.owner,
                    record.intent.id.clone(),
                    Capability::ScopeRead,
                    now,
                )
                .await
                .is_ok()
                {
                    tasks.push(render(&record));
                }
            }
            Ok(json!({"tasks":tasks}))
        }
        "tasks.get" | "tasks.result" => {
            let record = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &record.intent.owner,
                record.intent.id.clone(),
                Capability::ScopeRead,
                now,
            )
            .await?;
            if name == "tasks.result" {
                // Task outputs default to creator-only. Team administration is
                // not a secret-output grant; a future broader policy must be
                // captured explicitly in the durable intent.
                if !record.state.terminal()
                    || record.intent.creator.as_str() != context.identity.safe_fingerprint()
                {
                    return Err(denied());
                }
            }
            Ok(render(&record))
        }
        "tasks.queue" | "tasks.cancel" => {
            let record = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &record.intent.owner,
                record.intent.id.clone(),
                Capability::ScopeOperate,
                now,
            )
            .await?;
            let next = if name == "tasks.queue" {
                TaskState::Queued
            } else {
                TaskState::Cancelling
            };
            context
                .store
                .transition_agent_task(
                    record.intent.id.clone(),
                    record.state,
                    next,
                    context.identity.safe_fingerprint(),
                    record.attempt,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            Ok(json!({"task_id":record.intent.id,"state":next.wire()}))
        }
        _ => Err(unknown(name)),
    }
}

async fn load(
    context: &TaskDispatchContext,
    params: &Value,
) -> Result<crate::access::TaskRecord, ToolError> {
    context
        .store
        .get_agent_task(required(params, "task_id")?)
        .await
        .map_err(map)?
        .ok_or_else(denied)
}
async fn authorize(
    context: &TaskDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: String,
    capability: Capability,
    now: u64,
) -> Result<(), ToolError> {
    let action = ActionRef::new("tasks", name).map_err(|_| invalid("action"))?;
    authorize_action(
        &context.store,
        AuthorityRequest::new(
            context.identity.clone(),
            ActionAuthoritySpec::SCHEMA_VERSION,
            action.clone(),
            ResourceRef::new(
                owner.clone(),
                ResourceFamily::Task,
                ResourceId::new(id).map_err(|_| invalid("task_id"))?,
            ),
            context.ceiling.clone(),
            None,
            now,
            vec![
                AuthoritySafeBoundary::BeforeDispatch,
                AuthoritySafeBoundary::BeforeCommit,
            ],
            vec![ActionAuthoritySpec::new(
                action,
                ResourceFamily::Task,
                capability,
            )],
        ),
    )
    .await
    .map(|_| ())
    .map_err(map)
}
fn owner(params: &Value) -> Result<OwnerScope, ToolError> {
    let id = required(params, "owner_id")?;
    match required(params, "owner_kind")?.as_str() {
        "installation" => Ok(OwnerScope::Installation(
            InstallationId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        "team" => Ok(OwnerScope::Team(
            TeamId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        "project" => Ok(OwnerScope::Project(
            ProjectId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        "personal" => Ok(OwnerScope::Personal(
            PrincipalId::new(id).map_err(|_| invalid("owner_id"))?,
        )),
        _ => Err(invalid("owner_kind")),
    }
}
fn render(v: &crate::access::TaskRecord) -> Value {
    let (kind, id) = match &v.intent.owner {
        OwnerScope::Installation(x) => ("installation", x.as_str()),
        OwnerScope::Team(x) => ("team", x.as_str()),
        OwnerScope::Project(x) => ("project", x.as_str()),
        OwnerScope::Personal(x) => ("personal", x.as_str()),
    };
    json!({"task_id":v.intent.id,"owner_kind":kind,"owner_id":id,"agent_id":v.intent.agent_id,"agent_version":v.intent.agent_version,"state":v.state.wire(),"attempt":v.attempt,"output_digest":v.output_digest,"error_code":v.error_code})
}
fn required(v: &Value, k: &str) -> Result<String, ToolError> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(k))
}
fn now() -> Result<u64, ToolError> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| internal())?
            .as_millis(),
    )
    .map_err(|_| internal())
}
fn invalid(p: &str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid parameter `{p}`"),
        param: p.into(),
    }
}
fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "access denied".into(),
        required_scopes: vec![],
    }
}
fn internal() -> ToolError {
    ToolError::internal_message("Task service unavailable")
}
fn map(e: crate::access::AccessStoreError) -> ToolError {
    match e {
        crate::access::AccessStoreError::NotAuthorized
        | crate::access::AccessStoreError::IdentityUnavailable
        | crate::access::AccessStoreError::ProjectAccessUnavailable
        | crate::access::AccessStoreError::TeamUnavailable => denied(),
        _ => internal(),
    }
}
fn unknown(name: &str) -> ToolError {
    ToolError::UnknownAction {
        message: "unknown Task action".into(),
        valid: ACTIONS.iter().map(|a| a.name.into()).collect(),
        hint: ACTIONS
            .iter()
            .find(|a| a.name.starts_with(name))
            .map(|a| a.name.into()),
    }
}
pub async fn dispatch_unbound(_: &str, _: Value) -> Result<Value, ToolError> {
    Err(denied())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_complete() {
        assert_eq!(ACTIONS.len(), 6);
        assert!(ACTIONS.iter().all(|a| a.name.starts_with("tasks.")));
    }
    #[tokio::test]
    async fn unbound_is_non_enumerating() {
        assert_eq!(
            dispatch_unbound("tasks.get", json!({"task_id":"guessed"}))
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
    }
}
