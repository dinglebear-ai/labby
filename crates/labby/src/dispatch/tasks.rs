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
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    Err(denied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{AccessStore, AddTeamMemberInput, TeamRole};
    use crate::dispatch::agents::{
        self,
        test_support::{
            BOOTSTRAP_PRINCIPAL, agent_context, agent_params, browser, digest, fixture,
        },
    };
    use labby_runtime::{agent_runtime::AgentRuntimeError, authority::AuthorityLeaseError};

    /// Team the bootstrap flow creates with the bootstrap owner as its owner.
    const BOOTSTRAP_TEAM: &str = "bootstrap-initial-team";
    /// Second principal linked to `browser("member-subject")`.
    const MEMBER_PRINCIPAL: &str = "member-1";

    fn envelope(error: &ToolError) -> Value {
        serde_json::from_str(&error.to_string()).unwrap()
    }
    fn task_context(store: &AccessStore, identity: &VerifiedIdentity) -> TaskDispatchContext {
        TaskDispatchContext {
            store: store.clone(),
            identity: identity.clone(),
            ceiling: AuthorityCeiling::trusted_local(),
        }
    }
    fn task_params(task_id: &str, agent_id: &str) -> Value {
        json!({
            "task_id": task_id,
            "idempotency_key": format!("{task_id}-key"),
            "owner_kind": "personal",
            "owner_id": BOOTSTRAP_PRINCIPAL,
            "agent_id": agent_id,
            "input_digest": digest('1'),
        })
    }
    async fn wait_for_task_terminal(
        store: &AccessStore,
        task_id: &str,
    ) -> crate::access::TaskRecord {
        for _ in 0..200 {
            let record = store
                .get_agent_task(task_id.to_owned())
                .await
                .unwrap()
                .expect("task exists");
            if record.state.terminal() {
                return record;
            }
            tokio::task::yield_now().await;
        }
        panic!("task `{task_id}` did not settle");
    }

    async fn create_agent(store: &AccessStore, owner: &VerifiedIdentity, agent_id: &str) {
        agents::dispatch(
            agent_context(store, owner),
            "agents.create",
            agent_params(agent_id),
        )
        .await
        .unwrap();
    }
    /// Add a second principal to the bootstrap team as a team admin (not the
    /// creator of anything) and return its identity.
    async fn add_team_admin(store: &AccessStore, owner: &VerifiedIdentity) -> VerifiedIdentity {
        store
            .execute_test_statement(
                "INSERT INTO principals VALUES('member-1','bootstrap-local','user','active','Member',10,10);
                 INSERT INTO principal_links VALUES('member-link','member-1','external','https://accounts.google.com','member-subject',NULL,'active',1,1,10,10);",
            )
            .await
            .unwrap();
        store
            .add_team_member(
                AddTeamMemberInput::new(
                    owner.clone(),
                    BOOTSTRAP_TEAM,
                    MEMBER_PRINCIPAL,
                    TeamRole::Admin,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        browser("member-subject")
    }
    fn assert_summary_shape(value: &Value) {
        let object = value.as_object().unwrap();
        assert!(!object.contains_key("output_digest"), "{value}");
        assert!(!object.contains_key("error_code"), "{value}");
        for key in [
            "task_id",
            "owner_kind",
            "owner_id",
            "agent_id",
            "agent_version",
            "state",
            "attempt",
        ] {
            assert!(object.contains_key(key), "missing {key}: {value}");
        }
    }

    #[test]
    fn every_action_has_a_capability_and_none_is_platform_scoped() {
        for spec in ACTIONS {
            let capability = required_capability(spec.name);
            assert!(capability.is_some(), "{}", spec.name);
            assert_eq!(
                spec.requires_admin,
                capability.is_some_and(Capability::is_platform),
                "{}",
                spec.name
            );
        }
        assert_eq!(required_capability("tasks.bogus"), None);
    }

    #[test]
    fn catalog_is_complete() {
        assert_eq!(ACTIONS.len(), 6);
        assert!(ACTIONS.iter().all(|a| a.name.starts_with("tasks.")));
        let create = ACTIONS
            .iter()
            .find(|action| action.name == "tasks.create")
            .unwrap();
        assert!(
            create
                .params
                .iter()
                .any(|param| param.name == "input_digest" && param.required)
        );
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

    // B-C2: summary renderers never carry result material.
    #[test]
    fn summary_render_omits_result_material_and_result_render_includes_it() {
        let record = crate::access::TaskRecord {
            intent: TaskIntent {
                id: "t-1".into(),
                idempotency_key: "k-1".into(),
                owner: OwnerScope::Personal(PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap()),
                project: None,
                creator: PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap(),
                agent_id: "a-1".into(),
                agent_version: 1,
                agent_revision_digest: digest('a'),
                input_digest: digest('1'),
                catalog_generation: "catalog-1".into(),
                authority_fingerprint: "fp".into(),
            },
            state: TaskState::Succeeded,
            attempt: 1,
            output_digest: Some(digest('9')),
            error_code: Some("secret-reason".into()),
        };
        let summary = render_summary(&record);
        assert_summary_shape(&summary);
        assert!(!summary.to_string().contains("secret-reason"));
        assert!(!summary.to_string().contains(&digest('9')));
        let result = render_result(&record);
        assert_eq!(result["output_digest"], digest('9'));
        assert_eq!(result["error_code"], "secret-reason");
        assert_eq!(result["state"], "succeeded");
    }

    // B-C2: a team admin who did not create the Task sees the summary through
    // `tasks.get`/`tasks.list` and is denied by `tasks.result`; the creator
    // receives the full result only once the Task is terminal.
    #[tokio::test]
    async fn result_material_is_creator_only_and_terminal_only() {
        let (_dir, store, owner) = fixture().await;
        let admin = add_team_admin(&store, &owner).await;
        let mut agent = agent_params("team-agent");
        agent["owner_kind"] = json!("team");
        agent["owner_id"] = json!(BOOTSTRAP_TEAM);
        agents::dispatch(agent_context(&store, &owner), "agents.create", agent)
            .await
            .unwrap();
        let mut task = task_params("team-task", "team-agent");
        task["owner_kind"] = json!("team");
        task["owner_id"] = json!(BOOTSTRAP_TEAM);
        let created = dispatch(task_context(&store, &owner), "tasks.create", task)
            .await
            .unwrap();
        assert_eq!(created["task_id"], "team-task");
        let stored = store
            .get_agent_task("team-task".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.intent.creator.as_str(), BOOTSTRAP_PRINCIPAL);

        // The admin can read the summary but never the result.
        let admin_context = task_context(&store, &admin);
        let got = dispatch(
            admin_context.clone(),
            "tasks.get",
            json!({"task_id":"team-task"}),
        )
        .await
        .unwrap();
        assert_summary_shape(&got);
        assert_eq!(got["state"], "created");
        let listed = dispatch(admin_context.clone(), "tasks.list", json!({}))
            .await
            .unwrap();
        let listed = listed["tasks"].as_array().unwrap();
        assert_eq!(listed.len(), 1);
        assert_summary_shape(&listed[0]);
        let admin_denied = dispatch(
            admin_context.clone(),
            "tasks.result",
            json!({"task_id":"team-task"}),
        )
        .await
        .unwrap_err();
        assert_eq!(admin_denied.kind(), "forbidden");
        assert_eq!(envelope(&admin_denied)["message"], "access denied");

        // The creator is denied while the Task is not terminal.
        let owner_context = task_context(&store, &owner);
        let early = dispatch(
            owner_context.clone(),
            "tasks.result",
            json!({"task_id":"team-task"}),
        )
        .await
        .unwrap_err();
        assert_eq!(early.kind(), "forbidden");

        // Queueing is durable and service-owned: the call returns as soon as
        // the Task is queued, while the detached attempt owns execution and
        // terminal settlement even if the request future is dropped.
        let queued = dispatch(
            owner_context.clone(),
            "tasks.queue",
            json!({"task_id":"team-task"}),
        )
        .await
        .unwrap();
        assert_eq!(queued["state"], "queued");
        let settled = wait_for_task_terminal(&store, "team-task").await;
        assert!(settled.state.terminal(), "{:?}", settled.state);

        let result = dispatch(
            owner_context.clone(),
            "tasks.result",
            json!({"task_id":"team-task"}),
        )
        .await
        .unwrap();
        let object = result.as_object().unwrap();
        assert!(object.contains_key("output_digest"), "{result}");
        assert!(object.contains_key("error_code"), "{result}");
        if settled.state == TaskState::Failed {
            assert_eq!(result["error_code"], "execution_failed");
        }
        // Terminal state does not widen the audience.
        let still_denied = dispatch(
            admin_context.clone(),
            "tasks.result",
            json!({"task_id":"team-task"}),
        )
        .await
        .unwrap_err();
        assert_eq!(still_denied.kind(), "forbidden");
        let got = dispatch(admin_context, "tasks.get", json!({"task_id":"team-task"}))
            .await
            .unwrap();
        assert_summary_shape(&got);
        let got = dispatch(owner_context, "tasks.get", json!({"task_id":"team-task"}))
            .await
            .unwrap();
        assert_summary_shape(&got);
    }

    // B-I11: runtime failures keep their typed reason.
    #[test]
    fn runtime_errors_keep_their_typed_reason() {
        let table = [
            (
                TaskRuntimeError::Agent(AgentRuntimeError::Revoked),
                "authority_changed",
            ),
            (
                TaskRuntimeError::Agent(AgentRuntimeError::Lease(AuthorityLeaseError::Expired)),
                "authority_changed",
            ),
            (
                TaskRuntimeError::Agent(AgentRuntimeError::AuthorityUnavailable),
                "source_unavailable",
            ),
            (
                TaskRuntimeError::Agent(AgentRuntimeError::ResourceLimit),
                "quota_exceeded",
            ),
            (
                TaskRuntimeError::Agent(AgentRuntimeError::Cancelled),
                "cancelled",
            ),
            (
                TaskRuntimeError::Agent(AgentRuntimeError::ExecutorFailed),
                "service_unavailable",
            ),
            (
                TaskRuntimeError::Agent(AgentRuntimeError::PinnedDefinitionMismatch),
                "forbidden",
            ),
            (TaskRuntimeError::FencedConflict, "conflict"),
            (TaskRuntimeError::Unavailable, "service_unavailable"),
            (TaskRuntimeError::InvalidLease, "internal_error"),
            (TaskRuntimeError::InvalidQuota, "internal_error"),
        ];
        for (error, kind) in table {
            let mapped = map_task_runtime_error("t-1", &error);
            assert_eq!(mapped.kind(), kind, "{error:?}");
        }
        let fenced = envelope(&map_task_runtime_error(
            "t-1",
            &TaskRuntimeError::FencedConflict,
        ));
        assert_eq!(fenced["existing_id"], "t-1");
        assert_eq!(
            envelope(&map_task_runtime_error(
                "t-1",
                &TaskRuntimeError::Agent(AgentRuntimeError::ExecutorFailed)
            ))["message"],
            "Agent execution backend is not configured"
        );
    }

    // B-I11: store denials collapse through the shared mapper.
    #[test]
    fn store_denials_collapse_to_one_non_enumerating_error() {
        for error in [
            AccessStoreError::NotAuthorized,
            AccessStoreError::IdentityUnavailable,
            AccessStoreError::ProjectAccessUnavailable,
            AccessStoreError::TeamUnavailable,
            AccessStoreError::ForeignKeyViolation,
        ] {
            let mapped = map(error);
            assert_eq!(mapped.kind(), "forbidden");
            assert_eq!(envelope(&mapped)["message"], "access denied");
        }
        assert_eq!(map(AccessStoreError::Locked).kind(), "service_unavailable");
        assert_eq!(map(AccessStoreError::Corrupt).kind(), "service_unavailable");
    }

    // B-I12(a): installation is not a Task owner.
    #[test]
    fn installation_owner_is_rejected_as_invalid_input() {
        let error = owner(&json!({"owner_kind":"installation","owner_id":"local"})).unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert_eq!(envelope(&error)["param"], "owner_kind");
        assert!(owner(&json!({"owner_kind":"personal","owner_id":"p1"})).is_ok());
        assert!(owner(&json!({"owner_kind":"team","owner_id":"t1"})).is_ok());
        assert!(owner(&json!({"owner_kind":"project","owner_id":"pr1"})).is_ok());
    }
    #[tokio::test]
    async fn installation_owned_task_create_is_invalid_and_nothing_is_stored() {
        let (_dir, store, owner) = fixture().await;
        create_agent(&store, &owner, "agent-1").await;
        let mut params = task_params("install-task", "agent-1");
        params["owner_kind"] = json!("installation");
        params["owner_id"] = json!("local");
        let error = dispatch(task_context(&store, &owner), "tasks.create", params)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert_eq!(envelope(&error)["param"], "owner_kind");
        assert!(
            store
                .get_agent_task("install-task".into())
                .await
                .unwrap()
                .is_none()
        );
    }

    // B-I12(b): caller-supplied epochs are refused before anything is stored.
    #[tokio::test]
    async fn create_refuses_caller_epochs_and_nothing_is_stored() {
        let (_dir, store, owner) = fixture().await;
        create_agent(&store, &owner, "agent-1").await;
        let context = task_context(&store, &owner);
        for key in ["authority_epoch", "publication_epoch"] {
            let mut params = task_params("epoch-task", "agent-1");
            params[key] = json!(99);
            let error = dispatch(context.clone(), "tasks.create", params)
                .await
                .unwrap_err();
            assert_eq!(error.kind(), "invalid_param");
            assert_eq!(envelope(&error)["param"], key);
        }
        assert!(
            store
                .get_agent_task("epoch-task".into())
                .await
                .unwrap()
                .is_none()
        );
        let created = dispatch(
            context,
            "tasks.create",
            task_params("epoch-task", "agent-1"),
        )
        .await
        .unwrap();
        assert_eq!(created["task_id"], "epoch-task");
        assert_eq!(created["state"], "created");
    }

    // B-I12(c): a taken identifier is indistinguishable from a denial.
    #[tokio::test]
    async fn taken_identifier_is_indistinguishable_from_denial() {
        let (_dir, store, owner) = fixture().await;
        create_agent(&store, &owner, "agent-1").await;
        let context = task_context(&store, &owner);
        dispatch(
            context.clone(),
            "tasks.create",
            task_params("taken", "agent-1"),
        )
        .await
        .unwrap();
        // Same identifier, different intent content.
        let mut reused = task_params("taken", "agent-1");
        reused["idempotency_key"] = json!("another-key");
        let duplicate = dispatch(context.clone(), "tasks.create", reused)
            .await
            .unwrap_err();
        // A stranger with no principal is denied for a free identifier.
        let stranger = task_context(&store, &browser("stranger-subject"));
        let unauthorized = dispatch(stranger, "tasks.create", task_params("free", "agent-1"))
            .await
            .unwrap_err();
        assert_eq!(duplicate.kind(), "forbidden");
        assert_eq!(envelope(&duplicate), envelope(&unauthorized));
        assert_eq!(envelope(&duplicate)["message"], "access denied");
        // Same identifier and key, different input: still a denial.
        let mut drifted = task_params("taken", "agent-1");
        drifted["input_digest"] = json!(digest('2'));
        let drifted = dispatch(context.clone(), "tasks.create", drifted)
            .await
            .unwrap_err();
        assert_eq!(envelope(&drifted), envelope(&unauthorized));
        // An exact replay keeps the store's idempotent-create contract.
        let replay = dispatch(context, "tasks.create", task_params("taken", "agent-1"))
            .await
            .unwrap();
        assert_eq!(replay["task_id"], "taken");
        // The store's own duplicate signals map the same way.
        for raced in [
            AccessStoreError::IntegrityViolation {
                check: "task_idempotency",
            },
            AccessStoreError::Unavailable(
                "UNIQUE constraint failed: agent_tasks.task_id".to_owned(),
            ),
        ] {
            assert_eq!(envelope(&map_create(raced)), envelope(&duplicate));
        }
        assert_eq!(
            map_create(AccessStoreError::Corrupt).kind(),
            "service_unavailable"
        );
        assert_eq!(
            map_create(AccessStoreError::Unavailable("disk gone".into())).kind(),
            "service_unavailable"
        );
        // The original record is untouched.
        let stored = store.get_agent_task("taken".into()).await.unwrap().unwrap();
        assert_eq!(stored.intent.idempotency_key, "taken-key");
        assert_eq!(stored.intent.input_digest, digest('1'));
        assert_eq!(stored.state, TaskState::Created);
    }

    // B-I12(d): the pinned Agent revision is revalidated before execution.
    #[test]
    fn pinned_revision_check_rejects_every_drift() {
        use labby_primitives::agent::{AgentRevision, RunningRevocationPolicy};
        let created = AgentDefinition {
            id: "a1".into(),
            owner: OwnerScope::Personal(PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap()),
            revision: AgentRevision {
                version: 1,
                content_digest: digest('a'),
                repository_digest: digest('b'),
                image_digest: digest('c'),
                harness_digest: digest('d'),
                loadout_digest: digest('e'),
                catalog_generation: "catalog-1".into(),
                credential_references: Vec::new(),
            },
            state: AgentState::Active,
            required_capabilities: vec![Capability::ScopeOperate],
            authority_epoch: 1,
            publication_epoch: 1,
            revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
        };
        let intent = TaskIntent {
            id: "t-1".into(),
            idempotency_key: "k-1".into(),
            owner: created.owner.clone(),
            project: None,
            creator: PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap(),
            agent_id: created.id.clone(),
            agent_version: created.revision.version,
            agent_revision_digest: created.revision.content_digest.clone(),
            input_digest: digest('1'),
            catalog_generation: created.revision.catalog_generation.clone(),
            authority_fingerprint: "fp".into(),
        };
        assert!(pinned_revision_matches(&created, &intent));
        let mut bumped = created.clone();
        bumped.revision.version += 1;
        assert!(!pinned_revision_matches(&bumped, &intent));
        let mut rewritten = created.clone();
        rewritten.revision.content_digest = digest('f');
        assert!(!pinned_revision_matches(&rewritten, &intent));
        let mut suspended = created.clone();
        suspended.state = AgentState::Suspended;
        assert!(!pinned_revision_matches(&suspended, &intent));
        let mut moved = created;
        moved.owner = OwnerScope::Team(TeamId::new("other-team").unwrap());
        assert!(!pinned_revision_matches(&moved, &intent));
    }
    #[tokio::test]
    async fn queue_denies_when_pinned_agent_revision_drifted() {
        let (_dir, store, owner) = fixture().await;
        create_agent(&store, &owner, "agent-1").await;
        let context = task_context(&store, &owner);
        dispatch(
            context.clone(),
            "tasks.create",
            task_params("pinned", "agent-1"),
        )
        .await
        .unwrap();
        // A new Agent revision invalidates the pin.
        agents::dispatch(
            agent_context(&store, &owner),
            "agents.update",
            json!({"agent_id":"agent-1","content_digest":digest('f')}),
        )
        .await
        .unwrap();
        let error = dispatch(context.clone(), "tasks.queue", json!({"task_id":"pinned"}))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
        assert_eq!(envelope(&error)["message"], "access denied");
        let stored = store
            .get_agent_task("pinned".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, TaskState::Created, "no lease was acquired");
        assert_eq!(stored.attempt, 0);
        // The authoritative execution-time check denies for a drifted record
        // even when the caller already holds an authority lease.
        let lease = authorize(
            &context,
            "tasks.queue",
            &stored.intent.owner,
            stored.intent.id.clone(),
            Capability::ScopeOperate,
            now().unwrap(),
        )
        .await
        .unwrap();
        let error = execute_queued(
            &context,
            &stored,
            lease,
            Cancellation::new(),
            now().unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }
    #[tokio::test]
    async fn queue_denies_when_pinned_agent_is_suspended() {
        let (_dir, store, owner) = fixture().await;
        create_agent(&store, &owner, "agent-1").await;
        let context = task_context(&store, &owner);
        dispatch(
            context.clone(),
            "tasks.create",
            task_params("suspended-pin", "agent-1"),
        )
        .await
        .unwrap();
        agents::dispatch(
            agent_context(&store, &owner),
            "agents.suspend",
            json!({"agent_id":"agent-1"}),
        )
        .await
        .unwrap();
        let error = dispatch(context, "tasks.queue", json!({"task_id":"suspended-pin"}))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
        let stored = store
            .get_agent_task("suspended-pin".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, TaskState::Created);
    }

    // Release-profile guard: without `proxy-testkit` the deterministic branch
    // of the shared placeholder executor is compiled out, so
    // `LABBY_E2E_DETERMINISTIC_EXECUTORS` cannot turn a product build into a
    // fake-success backend. The crate forbids `unsafe`, so the variable cannot
    // be set from inside the test; run the suite with it exported to exercise
    // the guard in both states.
    #[cfg(not(feature = "proxy-testkit"))]
    #[tokio::test]
    async fn disabled_executor_fails_closed_without_testkit() {
        let (_dir, store, owner) = fixture().await;
        create_agent(&store, &owner, "agent-1").await;
        let context = task_context(&store, &owner);
        dispatch(
            context.clone(),
            "tasks.create",
            task_params("closed", "agent-1"),
        )
        .await
        .unwrap();
        let error = dispatch(context, "tasks.queue", json!({"task_id":"closed"}))
            .await
            .unwrap_err();
        assert_eq!(
            error.kind(),
            "service_unavailable",
            "deterministic hook must be inert (env set: {})",
            std::env::var_os("LABBY_E2E_DETERMINISTIC_EXECUTORS").is_some()
        );
        assert_eq!(
            envelope(&error)["message"],
            "Agent execution backend is not configured"
        );
        let stored = store
            .get_agent_task("closed".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, TaskState::Failed);
        assert_eq!(stored.error_code.as_deref(), Some("execution_failed"));
        assert_eq!(stored.output_digest, None);
    }
}
