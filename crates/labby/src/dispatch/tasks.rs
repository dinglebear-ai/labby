//! Authenticated, owner-scoped Agent Task surface shared by HTTP and MCP.

use crate::{
    access::{
        AccessStoreError, ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest,
        authorize_action, refresh_authority_epochs,
    },
    dispatch::{
        access_errors::map_store_error,
        agents::{
            DisabledExecutor, LiveExecutionAuthority, map_agent_runtime_error,
            reject_server_assigned,
        },
        error::ToolError,
    },
};
use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerScope, PrincipalId, ProjectId, ResourceFamily,
        ResourceId, ResourceRef, TeamId,
    },
    action::{ActionSpec, ParamSpec},
    agent::{AgentDefinition, AgentSessionBinding, AgentState},
    task::{TaskIntent, TaskSettlement, TaskState},
};
use labby_runtime::{
    agent_runtime::{
        AgentExecutionOutput, AgentExecutionRequest, AgentResourceBounds, AgentRuntimeError,
        Cancellation,
    },
    authority::AuthoritySafeBoundary,
    task_runtime::{ScheduledTask, TaskLedger, TaskRuntimeError, TaskScheduler, execute_task},
};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

static TASK_SCHEDULER: LazyLock<TaskScheduler> =
    LazyLock::new(|| TaskScheduler::new(4).expect("valid fixed task quota"));
static TASK_CANCELLATIONS: LazyLock<Mutex<HashMap<String, (u32, Cancellation)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const fn param(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
        required: true,
        description: "",
    }
}
const fn optional_param(name: &'static str) -> ParamSpec {
    ParamSpec {
        name,
        ty: "string",
        required: false,
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
            param("input_digest"),
        ],
    ),
    action(
        "tasks.list",
        "List caller-visible Agent Tasks",
        &[optional_param("cursor"), optional_param("limit")],
    ),
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

/// Exact capability the shared evaluator demands for `action`. The generated
/// action catalog and the surface admin gate derive from this table; no Task
/// action is platform-scoped, so none requires `lab:admin`.
pub(crate) fn required_capability(action: &str) -> Option<Capability> {
    Some(match action {
        "tasks.create" => Capability::ScopeCreate,
        "tasks.list" | "tasks.get" | "tasks.result" => Capability::ScopeRead,
        "tasks.queue" | "tasks.cancel" => Capability::ScopeOperate,
        _ => return None,
    })
}

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
            reject_server_assigned(&params)?;
            let owner = owner(&params)?;
            let task_id = required(&params, "task_id")?;
            let request = authority_request(
                &context,
                name,
                &owner,
                task_id.clone(),
                Capability::ScopeCreate,
                now,
            )?;
            let agent = context
                .store
                .get_agent_definition(required(&params, "agent_id")?)
                .await
                .map_err(map)?
                .ok_or_else(denied)?;
            if agent.owner != owner || agent.state != AgentState::Active {
                return Err(denied());
            }
            let intent = TaskIntent {
                id: task_id.clone(),
                idempotency_key: required(&params, "idempotency_key")?,
                owner,
                project: params
                    .get("project_id")
                    .and_then(Value::as_str)
                    .map(|v| ProjectId::new(v.to_owned()).map_err(|_| invalid("project_id")))
                    .transpose()?,
                creator: PrincipalId::new("pending-authority-binding")
                    .map_err(|_| invalid("principal"))?,
                agent_id: agent.id,
                agent_version: agent.revision.version,
                agent_revision_digest: agent.revision.content_digest,
                input_digest: required(&params, "input_digest")?,
                catalog_generation: agent.revision.catalog_generation,
                authority_fingerprint: context.identity.safe_fingerprint(),
            };
            // An identifier that is already taken must look exactly like an
            // authorization failure so `tasks.create` cannot be used as an
            // existence oracle across owners. The only exception is an exact
            // idempotent replay of the same intent, which the store still
            // re-authorizes before it returns the existing identifier.
            if let Some(existing) = context.store.get_agent_task(task_id).await.map_err(map)?
                && !is_idempotent_replay(&existing, &intent)
            {
                return Err(denied());
            }
            let id = context
                .store
                .authorize_and_create_agent_task(
                    request,
                    intent,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map_create)?;
            Ok(json!({"task_id":id,"state":"created"}))
        }
        "tasks.list" => {
            let limit = page_limit(&params)?;
            let cursor = params
                .get("cursor")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let probe_owner = OwnerScope::Installation(
                InstallationId::new("authorized-list-probe").map_err(|_| internal())?,
            );
            let request = authority_request(
                &context,
                name,
                &probe_owner,
                "authorized-list-probe".to_owned(),
                Capability::ScopeRead,
                now,
            )?;
            let page = context
                .store
                .list_authorized_agent_tasks(cursor.to_owned(), limit, request)
                .await
                .map_err(map)?;
            let next_cursor = page.last().map(|record| record.intent.id.clone());
            let tasks = page.iter().map(render_summary).collect::<Vec<_>>();
            Ok(json!({"tasks":tasks,"next_cursor":next_cursor}))
        }
        "tasks.get" => {
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
            Ok(render_summary(&record))
        }
        "tasks.result" => {
            let record = load(&context, &params).await?;
            let authority_lease = authorize(
                &context,
                name,
                &record.intent.owner,
                record.intent.id.clone(),
                Capability::ScopeRead,
                now,
            )
            .await?;
            // Task outputs default to creator-only. Team administration is
            // not a secret-output grant; a future broader policy must be
            // captured explicitly in the durable intent.
            if !record.state.terminal()
                || record.intent.creator.as_str() != authority_lease.binding().principal_id()
            {
                return Err(denied());
            }
            Ok(render_result(&record))
        }
        "tasks.queue" | "tasks.cancel" => {
            let record = load(&context, &params).await?;
            if name == "tasks.queue" {
                // Refuse to move a Task out of `created` when its pinned Agent
                // revision has already drifted; the authoritative re-check
                // happens again inside `execute_queued` before the lease.
                pinned_definition(&context, &record).await?;
            }
            let request = authority_request(
                &context,
                name,
                &record.intent.owner,
                record.intent.id.clone(),
                Capability::ScopeOperate,
                now,
            )?;
            let next = if name == "tasks.queue" {
                TaskState::Queued
            } else {
                TaskState::Cancelling
            };
            let authority_lease = context
                .store
                .authorize_and_transition_agent_task(
                    request,
                    record.intent.id.clone(),
                    record.state,
                    next,
                    context.identity.safe_fingerprint(),
                    record.attempt,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            let response_state = if name == "tasks.queue" {
                let attempt = record.attempt.saturating_add(1);
                let cancellation = Cancellation::new();
                register_task_cancellation(record.intent.id.clone(), attempt, cancellation.clone());
                let owned_context = context.clone();
                let owned_record = record.clone();
                tokio::spawn(async move {
                    let task_id = owned_record.intent.id.clone();
                    let result = execute_queued(
                        &owned_context,
                        &owned_record,
                        authority_lease,
                        cancellation,
                        now,
                    )
                    .await;
                    unregister_task_cancellation(&task_id, attempt);
                    if let Err(error) = result {
                        tracing::warn!(
                            task_id = %task_id,
                            kind = error.kind(),
                            "detached Agent Task attempt did not complete normally"
                        );
                        // A failure before lease acquisition otherwise leaves
                        // a durable queued row with no owner. Expire only when
                        // this attempt still owns the queued state; concurrent
                        // cancellation or execution wins through the state fence.
                        if let Ok(Some(current)) =
                            owned_context.store.get_agent_task(task_id.clone()).await
                            && current.state == TaskState::Queued
                        {
                            drop(
                                owned_context
                                    .store
                                    .transition_agent_task(
                                        task_id,
                                        TaskState::Queued,
                                        TaskState::Expired,
                                        owned_context.identity.safe_fingerprint(),
                                        current.attempt,
                                        i64::try_from(now).unwrap_or(i64::MAX),
                                    )
                                    .await,
                            );
                        }
                    }
                });
                "queued"
            } else {
                let live_owner = signal_task_cancellation(&record.intent.id);
                if record.state == TaskState::Running && live_owner {
                    "cancelling"
                } else {
                    context
                        .store
                        .transition_agent_task(
                            record.intent.id.clone(),
                            TaskState::Cancelling,
                            TaskState::Cancelled,
                            context.identity.safe_fingerprint(),
                            record.attempt,
                            i64::try_from(now).map_err(|_| internal())?,
                        )
                        .await
                        .map_err(map)?;
                    "cancelled"
                }
            };
            Ok(json!({"task_id":record.intent.id,"state":response_state}))
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
) -> Result<labby_runtime::authority::AuthorityLease, ToolError> {
    authorize_action(
        &context.store,
        authority_request(context, name, owner, id, capability, now)?,
    )
    .await
    .map_err(map)
}
fn authority_request(
    context: &TaskDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: String,
    capability: Capability,
    now: u64,
) -> Result<AuthorityRequest, ToolError> {
    let action = ActionRef::new("tasks", name).map_err(|_| invalid("action"))?;
    Ok(AuthorityRequest::new(
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
    ))
}

/// Load the Agent definition a Task intent was pinned to and refuse to hand it
/// out when the live definition no longer matches the pinned revision
/// (version or content digest) or is no longer active. The denial is the
/// shared non-enumerating error so the caller cannot learn which part drifted.
async fn pinned_definition(
    context: &TaskDispatchContext,
    record: &crate::access::TaskRecord,
) -> Result<AgentDefinition, ToolError> {
    let definition = context
        .store
        .get_agent_definition(record.intent.agent_id.clone())
        .await
        .map_err(map)?
        .ok_or_else(denied)?;
    if !pinned_revision_matches(&definition, &record.intent) {
        return Err(denied());
    }
    Ok(definition)
}
fn pinned_revision_matches(definition: &AgentDefinition, intent: &TaskIntent) -> bool {
    definition.state == AgentState::Active
        && definition.owner == intent.owner
        && definition.revision.version == intent.agent_version
        && definition.revision.content_digest == intent.agent_revision_digest
}
/// An exact replay of an existing intent (same owner, idempotency key, Agent
/// pin, and input) is the store's idempotent-create contract. Anything else
/// that reuses the identifier is treated as taken.
fn is_idempotent_replay(existing: &crate::access::TaskRecord, intent: &TaskIntent) -> bool {
    existing.intent.owner == intent.owner
        && existing.intent.idempotency_key == intent.idempotency_key
        && existing.intent.agent_id == intent.agent_id
        && existing.intent.agent_revision_digest == intent.agent_revision_digest
        && existing.intent.input_digest == intent.input_digest
}

fn register_task_cancellation(task_id: String, attempt: u32, cancellation: Cancellation) {
    TASK_CANCELLATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(task_id, (attempt, cancellation));
}

fn signal_task_cancellation(task_id: &str) -> bool {
    if let Some((_attempt, cancellation)) = TASK_CANCELLATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(task_id)
        .cloned()
    {
        cancellation.cancel();
        true
    } else {
        false
    }
}

fn unregister_task_cancellation(task_id: &str, attempt: u32) {
    let mut cancellations = TASK_CANCELLATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cancellations
        .get(task_id)
        .is_some_and(|(registered, _)| *registered == attempt)
    {
        cancellations.remove(task_id);
    }
}

async fn execute_queued(
    context: &TaskDispatchContext,
    record: &crate::access::TaskRecord,
    lease: labby_runtime::authority::AuthorityLease,
    cancellation: Cancellation,
    now: u64,
) -> Result<(), ToolError> {
    use sha2::{Digest as _, Sha256};
    // Revalidate the pinned Agent revision at execution time, before any lease
    // is acquired or an executor is admitted.
    let definition = pinned_definition(context, record).await?;
    let epochs = refresh_authority_epochs(
        &context.store,
        context.identity.clone(),
        record.intent.owner.clone(),
        Capability::ScopeOperate,
    )
    .await
    .map_err(map)?;
    // Durable transition and execution are separate boundaries: the lease
    // must still be current against freshly read epochs before any executor
    // is admitted.
    lease
        .validate_at(AuthoritySafeBoundary::BeforeCommit, now, &epochs)
        .map_err(|_| map_agent_runtime_error(&AgentRuntimeError::Revoked))?;
    let attempt = record.attempt.saturating_add(1);
    let fence = hex::encode(Sha256::digest(format!(
        "{}:{attempt}:{now}",
        record.intent.id
    )));
    let expires = now.saturating_add(30_000);
    let request = AgentExecutionRequest {
        definition: definition.clone(),
        session: AgentSessionBinding {
            session_id: format!("task-{}-{attempt}", record.intent.id),
            agent_id: definition.id.clone(),
            agent_version: definition.revision.version,
            // The runtime rejects a session whose principal or epoch fingerprint
            // differs from the lease binding; both come from the lease itself.
            principal: PrincipalId::new(lease.binding().principal_id())
                .map_err(|_| invalid("principal"))?,
            owner: record.intent.owner.clone(),
            catalog_generation: definition.revision.catalog_generation.clone(),
            authority_fingerprint: lease.epoch_fingerprint().as_str().into(),
            lease_expires_at: i64::try_from(lease.expires_at_millis()).map_err(|_| internal())?,
        },
        lease,
        bounds: AgentResourceBounds {
            max_runtime_millis: 300_000,
            max_output_bytes: 16 * 1024 * 1024,
            max_external_effects: 1_000,
        },
    };
    let task = ScheduledTask {
        task_id: record.intent.id.clone(),
        owner: record.intent.owner.clone(),
        attempt,
        fencing_token: fence,
        lease_expires_at: expires,
        agent_request: request,
    };
    let ledger = StoreLedger {
        store: context.store.clone(),
        actor: context.identity.safe_fingerprint(),
        now,
    };
    let _ = ledger.recover_expired(now).await.map_err(|_| internal())?;
    execute_task(
        &TASK_SCHEDULER,
        &ledger,
        &LiveExecutionAuthority {
            store: context.store.clone(),
            identity: context.identity.clone(),
            owner: record.intent.owner.clone(),
            definition,
        },
        &DisabledExecutor,
        task,
        cancellation,
        now,
    )
    .await
    .map(drop)
    .map_err(|error| map_task_runtime_error(&record.intent.id, &error))
}

struct StoreLedger {
    store: crate::access::AccessStore,
    actor: String,
    now: u64,
}
impl TaskLedger for StoreLedger {
    async fn acquire(&self, task: &ScheduledTask) -> Result<(), TaskRuntimeError> {
        let now = i64::try_from(self.now).map_err(|_| TaskRuntimeError::Unavailable)?;
        self.store
            .acquire_agent_task_lease(
                task.task_id.clone(),
                task.attempt,
                task.fencing_token.clone(),
                i64::try_from(task.lease_expires_at).map_err(|_| TaskRuntimeError::Unavailable)?,
                now,
            )
            .await
            .map_err(|_| TaskRuntimeError::FencedConflict)?;
        self.store
            .settle_agent_task(
                task.task_id.clone(),
                TaskState::Queued,
                TaskState::Running,
                self.actor.clone(),
                task.attempt,
                task.fencing_token.clone(),
                TaskSettlement {
                    state: TaskState::Running,
                    output_digest: None,
                    error_code: None,
                    settled_at: now,
                },
                now,
            )
            .await
            .map_err(|_| TaskRuntimeError::FencedConflict)
    }
    async fn settle(
        &self,
        task: &ScheduledTask,
        state: TaskState,
        output: Option<&AgentExecutionOutput>,
        reason: Option<&str>,
    ) -> Result<(), TaskRuntimeError> {
        let now = i64::try_from(self.now).map_err(|_| TaskRuntimeError::Unavailable)?;
        let from = if state == TaskState::Cancelled {
            TaskState::Cancelling
        } else {
            TaskState::Running
        };
        self.store
            .settle_agent_task(
                task.task_id.clone(),
                from,
                state,
                self.actor.clone(),
                task.attempt,
                task.fencing_token.clone(),
                TaskSettlement {
                    state,
                    output_digest: output.map(|v| v.digest.clone()),
                    error_code: reason.map(str::to_owned),
                    settled_at: now,
                },
                now,
            )
            .await
            .map_err(|_| TaskRuntimeError::FencedConflict)
    }
    async fn recover_expired(&self, now: u64) -> Result<usize, TaskRuntimeError> {
        self.store
            .recover_expired_agent_tasks(
                i64::try_from(now).map_err(|_| TaskRuntimeError::Unavailable)?,
            )
            .await
            .map_err(|_| TaskRuntimeError::Unavailable)
    }
}
/// Parse the owner scope for a caller-created Task. `installation` is not a
/// valid Task owner per the authority matrix, so it is rejected as invalid
/// input rather than being authorized against platform-administrator authority.
fn owner(params: &Value) -> Result<OwnerScope, ToolError> {
    let id = required(params, "owner_id")?;
    match required(params, "owner_kind")?.as_str() {
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
fn owner_wire(owner: &OwnerScope) -> (&'static str, &str) {
    match owner {
        OwnerScope::Installation(x) => ("installation", x.as_str()),
        OwnerScope::Team(x) => ("team", x.as_str()),
        OwnerScope::Project(x) => ("project", x.as_str()),
        OwnerScope::Personal(x) => ("personal", x.as_str()),
    }
}
/// Caller-visible Task summary. Deliberately omits `output_digest` and
/// `error_code`: those are result material and are only released through
/// `tasks.result` to the Task creator once the Task is terminal.
fn render_summary(v: &crate::access::TaskRecord) -> Value {
    let (kind, id) = owner_wire(&v.intent.owner);
    json!({"task_id":v.intent.id,"owner_kind":kind,"owner_id":id,"agent_id":v.intent.agent_id,"agent_version":v.intent.agent_version,"state":v.state.wire(),"attempt":v.attempt})
}
/// Full Task result. Only `tasks.result` may render this, and only for the
/// creator of a terminal Task.
fn render_result(v: &crate::access::TaskRecord) -> Value {
    let mut value = render_summary(v);
    value["output_digest"] = json!(v.output_digest);
    value["error_code"] = json!(v.error_code);
    value
}
fn required(v: &Value, k: &str) -> Result<String, ToolError> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(k))
}
fn page_limit(params: &Value) -> Result<usize, ToolError> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(100),
        Some(Value::String(value)) => value
            .parse::<usize>()
            .ok()
            .filter(|value| (1..=100).contains(value))
            .ok_or_else(|| invalid("limit")),
        _ => Err(invalid("limit")),
    }
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
fn map(error: AccessStoreError) -> ToolError {
    map_store_error("tasks", error, denied)
}
/// Map a Task create failure. A reused identifier that reaches the store is an
/// authorization-shaped outcome for the caller and must not become an
/// enumerable outage:
///
/// - the store reports a reused `idempotency_key` with different intent
///   content as a `task_idempotency` integrity violation;
/// - a lost race on the `task_id` primary key is reported by the store as the
///   typed `task_id` integrity violation (`access::task::map_task_insert_error`).
/// A taken Task identifier must be indistinguishable from a denial, so an
/// existing row never becomes an existence oracle for a caller who cannot see
/// it. The store reports a raced insert either as a typed integrity violation
/// or, when the collision surfaces from SQLite before the typed mapping, as an
/// `Unavailable` carrying the constraint text; both collapse to one denial.
fn map_create(error: AccessStoreError) -> ToolError {
    match error {
        AccessStoreError::IntegrityViolation {
            check: "task_idempotency" | "task_id",
        } => denied(),
        AccessStoreError::Unavailable(ref reason)
            if reason.contains("agent_tasks.task_id")
                || reason.contains("agent_tasks.idempotency_key") =>
        {
            denied()
        }
        other => map(other),
    }
}
/// Map a typed Task runtime failure to the shared error envelope without
/// losing the reason. Agent-runtime causes reuse the Agent mapping so both
/// surfaces agree on `authority_changed`, `source_unavailable`,
/// `quota_exceeded`, `cancelled`, and `service_unavailable`.
fn map_task_runtime_error(task_id: &str, error: &TaskRuntimeError) -> ToolError {
    match error {
        TaskRuntimeError::Agent(inner) => map_agent_runtime_error(inner),
        TaskRuntimeError::FencedConflict => ToolError::Conflict {
            message: "Agent Task attempt was fenced by a newer lease or settlement".into(),
            existing_id: task_id.to_owned(),
        },
        TaskRuntimeError::Unavailable => ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "Agent Task runtime is unavailable".into(),
        },
        TaskRuntimeError::InvalidLease | TaskRuntimeError::InvalidQuota => {
            ToolError::internal_message("Agent Task execution request is invalid")
        }
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
pub async fn dispatch_unbound(name: &str, params: Value) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("tasks", ACTIONS));
    }
    if name == "schema" {