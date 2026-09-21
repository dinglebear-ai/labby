//! Authenticated, owner-scoped Agent surface shared by HTTP and MCP.

use crate::{
    access::{
        AccessStoreError, ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest,
        authorize_action, refresh_agent_authority_epochs, refresh_authority_epochs,
    },
    dispatch::{
        access_errors::map_store_error,
        agent_llm::{LlmAgentExecutor, current_harness_digest},
        agent_payloads::{AgentPayloadStore, DEFAULT_MODEL, inline_output},
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
    agent::{
        AgentDefinition, AgentRevision, AgentSessionBinding, AgentState, RunningRevocationPolicy,
    },
    digest::Sha256Digest,
};
use labby_runtime::{
    agent_runtime::{
        AGENT_MAX_RUNTIME_MILLIS, AgentAuthority, AgentExecutionOutput, AgentExecutionRequest,
        AgentExecutor, AgentResourceBounds, AgentRuntimeError, Cancellation, ExecutionGuard,
        execute_agent,
    },
    authority::{AuthorityEpochVector, AuthoritySafeBoundary},
    task_runtime::TaskScheduler,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::{Future, ready},
    sync::{LazyLock, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// Concurrent direct runs admitted per owner scope, mirroring the Task quota.
/// A saturated owner is rejected with `queue_saturated` instead of opening an
/// unbounded number of provider sessions.
const AGENT_RUN_PER_OWNER_LIMIT: usize = 4;
static AGENT_RUN_SCHEDULER: LazyLock<TaskScheduler> = LazyLock::new(|| {
    TaskScheduler::new(AGENT_RUN_PER_OWNER_LIMIT).expect("valid fixed Agent run quota")
});
/// Live in-process runs by session id, so `agents.session.cancel` can signal
/// the executor at its next safe boundary.
static AGENT_SESSION_CANCELLATIONS: LazyLock<Mutex<HashMap<String, Cancellation>>> =
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
        output_schema: None,
    }
}
/// An action that causes permanent, unrecoverable loss under the shared
/// destructive policy; surfaces derive confirmation from this flag alone.
const fn destructive_action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive: true,
        requires_admin: false,
        params,
        returns: "object",
        output_schema: None,
    }
}
pub const ACTIONS: &[ActionSpec] = &[
    action(
        "agents.create",
        "Create an Agent definition",
        &[
            param("agent_id"),
            param("owner_kind"),
            param("owner_id"),
            param("instructions"),
            optional_param("model"),
            optional_param("content_digest"),
            optional_param("repository_digest"),
            optional_param("image_digest"),
            optional_param("harness_digest"),
            optional_param("loadout_digest"),
            optional_param("catalog_generation"),
        ],
    ),
    action(
        "agents.list",
        "List caller-visible Agents",
        &[optional_param("cursor"), optional_param("limit")],
    ),
    action(
        "agents.get",
        "Get a caller-visible Agent",
        &[param("agent_id")],
    ),
    action(
        "agents.update",
        "Create the next immutable Agent revision",
        &[
            param("agent_id"),
            optional_param("content_digest"),
            optional_param("repository_digest"),
            optional_param("image_digest"),
            optional_param("harness_digest"),
            optional_param("loadout_digest"),
            optional_param("catalog_generation"),
            optional_param("instructions"),
            optional_param("model"),
        ],
    ),
    action("agents.suspend", "Suspend an Agent", &[param("agent_id")]),
    // Deleted definitions are filtered out of every read and have no restore
    // action, so deletion is permanent loss, not a reversible state change.
    destructive_action(
        "agents.delete",
        "Permanently delete an Agent definition",
        &[param("agent_id")],
    ),
    action(
        "agents.run",
        "Start a pinned Agent session",
        &[param("agent_id"), optional_param("input")],
    ),
    action(
        "agents.session.status",
        "Read Agent session status",
        &[param("agent_id"), param("session_id")],
    ),
    action(
        "agents.session.cancel",
        "Request cancellation of a running Agent session",
        &[param("agent_id"), param("session_id")],
    ),
];

/// Exact capability the shared evaluator demands for `action`. The generated
/// action catalog and the surface admin gate derive from this table; no
/// Agent action is platform-scoped, so none requires `lab:admin`.
pub(crate) fn required_capability(action: &str) -> Option<Capability> {
    Some(match action {
        "agents.create" => Capability::ScopeCreate,
        "agents.list" | "agents.get" | "agents.session.status" => Capability::ScopeRead,
        "agents.run" | "agents.session.cancel" => Capability::ScopeOperate,
        "agents.update" | "agents.suspend" => Capability::ScopeManage,
        "agents.delete" => Capability::ScopeDelete,
        _ => return None,
    })
}

#[derive(Clone)]
pub(crate) struct AgentDispatchContext {
    pub store: crate::access::AccessStore,
    pub identity: VerifiedIdentity,
    pub ceiling: AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: AgentDispatchContext,
    name: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("agents", ACTIONS));
    }
    if name == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    if !ACTIONS.iter().any(|a| a.name == name) {
        return Err(unknown(name));
    }
    let now = now()?;
    match name {
        "agents.create" => {
            reject_server_assigned(&params)?;
            let requested_owner = owner(&params)?;
            let requested_id = required(&params, "agent_id")?;
            // Authorize before any content-addressed payload write. The final
            // transactional put re-authorizes at commit, but this preflight
            // prevents denied callers and duplicate identifiers from leaving
            // orphaned immutable payloads behind.
            authorize(
                &context,
                name,
                &requested_owner,
                &requested_id,
                Capability::ScopeCreate,
                now,
            )
            .await?;
            // An identifier that is already taken must look exactly like an
            // authorization failure so `agents.create` cannot be used as an
            // existence oracle across owners.
            if context
                .store
                .get_agent_definition(requested_id.clone())
                .await
                .map_err(map)?
                .is_some()
            {
                return Err(denied());
            }
            required(&params, "instructions")?;
            let params = materialize_llm_payload(&context.store, params, None)?;
            let definition = definition(&params, None)?;
            let request = authority_request(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeCreate,
                now,
            )?;
            context
                .store
                .authorize_and_put_agent_definition(
                    request,
                    definition.clone(),
                    context.identity.safe_fingerprint(),
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map_put)?;
            Ok(render(&definition))
        }
        "agents.list" => {
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
                "authorized-list-probe",
                Capability::ScopeRead,
                now,
            )?;
            let page = context
                .store
                .list_authorized_agent_definitions(cursor.to_owned(), limit, request)
                .await
                .map_err(map)?;
            let next_cursor = page.last().map(|definition| definition.id.clone());
            let visible = page.iter().map(render).collect::<Vec<_>>();
            Ok(json!({"agents":visible,"next_cursor":next_cursor}))
        }
        "agents.get" => {
            let definition = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeRead,
                now,
            )
            .await?;
            Ok(render(&definition))
        }
        "agents.update" => {
            reject_server_assigned(&params)?;
            let prior = load(&context, &params).await?;
            // As with create, keep CAS writes behind a current authority
            // decision. The store still re-authorizes atomically at commit.
            authorize(
                &context,
                name,
                &prior.owner,
                &prior.id,
                Capability::ScopeManage,
                now,
            )
            .await?;
            let params = materialize_llm_payload(&context.store, params, Some(&prior))?;
            let definition = definition(&params, Some(&prior))?;
            let request = authority_request(
                &context,
                name,
                &prior.owner,
                &prior.id,
                Capability::ScopeManage,
                now,
            )?;
            context
                .store
                .authorize_and_put_agent_definition(
                    request,
                    definition.clone(),
                    context.identity.safe_fingerprint(),
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map_put)?;
            Ok(render(&definition))
        }
        "agents.suspend" | "agents.delete" => {
            let definition = load(&context, &params).await?;
            let capability = if name.ends_with("delete") {
                Capability::ScopeDelete
            } else {
                Capability::ScopeManage
            };
            let request = authority_request(
                &context,
                name,
                &definition.owner,
                &definition.id,
                capability,
                now,
            )?;
            let state = if name.ends_with("delete") {
                AgentState::Deleted
            } else {
                AgentState::Suspended
            };
            context
                .store
                .authorize_and_set_agent_definition_state(
                    request,
                    definition.id.clone(),
                    state,
                    context.identity.safe_fingerprint(),
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            Ok(json!({"agent_id":definition.id,"state":state_name(state)}))
        }
        "agents.run" => {
            let definition = load(&context, &params).await?;
            // Authorize before any recovery write, provider construction, or
            // caller-input validation so unauthorized callers cannot trigger
            // durable side effects or distinguish active definitions by error.
            let lease = authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeOperate,
                now,
            )
            .await?;
            context
                .store
                .recover_expired_agent_sessions(i64::try_from(now).map_err(|_| internal())?)
                .await
                .map_err(map)?;
            if definition.state != AgentState::Active {
                return Err(denied());
            }
            let executor = configured_executor(&context.store, optional_text(&params, "input")?)?;
            let epochs = refresh_authority_epochs(
                &context.store,
                context.identity.clone(),
                definition.owner.clone(),
                Capability::ScopeOperate,
            )
            .await
            .map_err(map)?;
            lease
                .validate_at(AuthoritySafeBoundary::BeforeCommit, now, &epochs)
                .map_err(|_| denied())?;
            // Admit against the owner's run quota only after every
            // caller-fixable validation, so a malformed request never depends
            // on load; the permit lives as long as the spawned run.
            let permit = AGENT_RUN_SCHEDULER
                .try_admit(&definition.owner)
                .map_err(|_| queue_saturated())?;
            let session_id = format!("{}-{}", definition.id, uuid::Uuid::new_v4());
            let lease_expires_at = lease.expires_at_millis();
            let authority_fingerprint = epochs.fingerprint().as_str().to_owned();
            let run_context = context.clone();
            let run_definition = definition.clone();
            let run_session_id = session_id.clone();
            let run_executor = executor;
            let owned = tokio::spawn(async move {
                let _permit = permit;
                let cancellation = Cancellation::new();
                register_agent_session_cancellation(run_session_id.clone(), cancellation.clone());
                let result = run_agent_session(
                    &run_context,
                    &run_definition,
                    &run_session_id,
                    &run_executor,
                    lease,
                    authority_fingerprint,
                    lease_expires_at,
                    cancellation,
                    now,
                )
                .await;
                unregister_agent_session_cancellation(&run_session_id);
                result
            });
            owned.await.map_err(|_| internal())?
        }
        "agents.session.status" => {
            context
                .store
                .recover_expired_agent_sessions(i64::try_from(now).map_err(|_| internal())?)
                .await
                .map_err(map)?;
            let definition = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeRead,
                now,
            )
            .await?;
            let session_id = required(&params, "session_id")?;
            let status = context
                .store
                .get_agent_session_status(definition.id.clone(), session_id.clone())
                .await
                .map_err(map)?
                .ok_or_else(denied)?;
            Ok(json!({"agent_id":definition.id,"session_id":session_id,"status":status}))
        }
        "agents.session.cancel" => {
            context
                .store
                .recover_expired_agent_sessions(i64::try_from(now).map_err(|_| internal())?)
                .await
                .map_err(map)?;
            let definition = load(&context, &params).await?;
            authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeOperate,
                now,
            )
            .await?;
            let session_id = required(&params, "session_id")?;
            let status = context
                .store
                .get_agent_session_status(definition.id.clone(), session_id.clone())
                .await
                .map_err(map)?
                .ok_or_else(denied)?;
            // The executor observes the signal at its next safe boundary and
            // settles the durable status itself; a session without a live
            // in-process owner keeps its durable status and gets no signal.
            let cancel_requested = signal_agent_session_cancellation(&session_id);
            let status = if cancel_requested {
                "cancelling".to_owned()
            } else {
                status
            };
            Ok(json!({
                "agent_id": definition.id,
                "session_id": session_id,
                "status": status,
                "cancel_requested": cancel_requested,
            }))
        }
        _ => Err(unknown(name)),
    }
}

/// Own one admitted direct run end to end: durable session row, fenced
/// execution, and terminal status.
#[allow(clippy::too_many_arguments)]
async fn run_agent_session(
    run_context: &AgentDispatchContext,
    run_definition: &AgentDefinition,
    run_session_id: &str,
    run_executor: &ConfiguredExecutor,
    lease: labby_runtime::authority::AuthorityLease,
    authority_fingerprint: String,
    lease_expires_at: u64,
    cancellation: Cancellation,
    now: u64,
) -> Result<Value, ToolError> {
    let run_session_id = run_session_id.to_owned();
    run_context
        .store
        .create_agent_session(
            run_session_id.clone(),
            run_definition.clone(),
            run_context.identity.safe_fingerprint(),
            authority_fingerprint,
            i64::try_from(lease_expires_at).map_err(|_| internal())?,
            i64::try_from(now).map_err(|_| internal())?,
        )
        .await
        .map_err(map)?;
    run_context
        .store
        .set_agent_session_status(
            run_definition.id.clone(),
            run_session_id.clone(),
            "admitted".into(),
            "running".into(),
        )
        .await
        .map_err(map)?;
    let request = AgentExecutionRequest {
        definition: run_definition.clone(),
        session: AgentSessionBinding {
            session_id: run_session_id.clone(),
            agent_id: run_definition.id.clone(),
            agent_version: run_definition.revision.version,
            principal: PrincipalId::new(lease.binding().principal_id())
                .map_err(|_| invalid("principal"))?,
            owner: run_definition.owner.clone(),
            catalog_generation: run_definition.revision.catalog_generation.clone(),
            authority_fingerprint: lease.epoch_fingerprint().as_str().into(),
            lease_expires_at: i64::try_from(lease_expires_at).map_err(|_| internal())?,
        },
        lease,
        bounds: AgentResourceBounds {
            max_runtime_millis: AGENT_MAX_RUNTIME_MILLIS,
            max_output_bytes: 16 * 1024 * 1024,
            max_external_effects: 1_000,
        },
    };
    let result = execute_agent(
        &LiveExecutionAuthority {
            store: run_context.store.clone(),
            identity: run_context.identity.clone(),
            owner: run_definition.owner.clone(),
            definition: run_definition.clone(),
        },
        run_executor,
        request,
        cancellation,
        now,
    )
    .await;
    let next = match result {
        Ok(_) => "completed",
        Err(AgentRuntimeError::Revoked | AgentRuntimeError::Lease(_)) => "revoked",
        Err(AgentRuntimeError::Cancelled) => "cancelled",
        Err(_) => "failed",
    };
    run_context
        .store
        .set_agent_session_status(
            run_definition.id.clone(),
            run_session_id.clone(),
            "running".into(),
            next.into(),
        )
        .await
        .map_err(map)?;
    match result {
        Ok(output) => {
            let (text, truncated) = inline_output(run_executor.output(&output.digest)?);
            Ok(json!({
                "agent_id":run_definition.id,
                "agent_version":run_definition.revision.version,
                "session_id":run_session_id,
                "status":next,
                "output_digest":output.digest,
                "output":text,
                "output_truncated":truncated,
                "authority_expires_at":lease_expires_at
            }))
        }
        Err(error) => Err(map_agent_runtime_error(&error)),
    }
}

fn register_agent_session_cancellation(session_id: String, cancellation: Cancellation) {
    AGENT_SESSION_CANCELLATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session_id, cancellation);
}

/// Signal the live run for `session_id`, if this process owns one.
fn signal_agent_session_cancellation(session_id: &str) -> bool {
    AGENT_SESSION_CANCELLATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_id)
        .is_some_and(|cancellation| {
            cancellation.cancel();
            true
        })
}

fn unregister_agent_session_cancellation(session_id: &str) {
    AGENT_SESSION_CANCELLATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id);
}

fn queue_saturated() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "queue_saturated".into(),
        message: format!(
            "this owner already has {AGENT_RUN_PER_OWNER_LIMIT} live Agent runs; retry after one completes"
        ),
    }
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

/// Live authority source shared by Agent sessions and Agent Task execution:
/// every safe-boundary check re-reads the durable epochs for the owner scope.
pub(crate) struct LiveExecutionAuthority {
    pub(crate) store: crate::access::AccessStore,
    pub(crate) identity: VerifiedIdentity,
    pub(crate) owner: OwnerScope,
    pub(crate) definition: AgentDefinition,
}
impl AgentAuthority for LiveExecutionAuthority {
    async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
        refresh_agent_authority_epochs(
            &self.store,
            self.identity.clone(),
            self.owner.clone(),
            self.definition.clone(),
        )
        .await
        .map_err(|error| match error {
            AccessStoreError::NotAuthorized
            | AccessStoreError::IdentityUnavailable
            | AccessStoreError::ProjectAccessUnavailable => AgentRuntimeError::Revoked,
            _ => AgentRuntimeError::AuthorityUnavailable,
        })
    }
}
/// Deterministic executor retained only for the live end-to-end test harness.
/// Product execution selects the shared OpenAI-compatible LLM executor instead.
///
/// The deterministic branch is a **test-only hook**: it is compiled in only
/// under the `proxy-testkit` cargo feature (test support, never a product
/// slice) and additionally requires `LABBY_E2E_DETERMINISTIC_EXECUTORS` at run
/// time so live end-to-end matrices can drive the lifecycle. Product builds
/// compile the branch out entirely; setting the variable there has no effect.
///
/// It stands in for a provider, so it honors the executor contract the same
/// way: the digest it returns keys bytes materialized in the output CAS that
/// `tasks.result` and `agents.run` re-read by digest. A synthetic digest with
/// no stored bytes settled Tasks `succeeded` with an `output_digest` that
/// `tasks.result` could only report as `unavailable`.
pub(crate) struct DisabledExecutor {
    payloads: AgentPayloadStore,
}

/// Fixed text every deterministic run materializes.
const DETERMINISTIC_OUTPUT: &str = "deterministic executor output";

impl DisabledExecutor {
    fn for_access_store(store: &crate::access::AccessStore) -> Self {
        Self {
            payloads: AgentPayloadStore::for_access_store(store),
        }
    }
}

impl AgentExecutor for DisabledExecutor {
    fn execute(
        &self,
        request: AgentExecutionRequest,
        _: ExecutionGuard<'_>,
    ) -> impl Future<Output = Result<AgentExecutionOutput, AgentRuntimeError>> + Send {
        ready(if deterministic_executor_enabled() {
            self.payloads
                .store_output(DETERMINISTIC_OUTPUT)
                .map(|digest| AgentExecutionOutput {
                    digest,
                    bytes: DETERMINISTIC_OUTPUT.len(),
                    external_effects: 0,
                })
                .map_err(|error| {
                    tracing::warn!(
                        agent_id = %request.definition.id,
                        session_id = %request.session.session_id,
                        kind = error.kind(),
                        "deterministic executor could not materialize its output"
                    );
                    AgentRuntimeError::ExecutorFailed
                })
        } else {
            Err(AgentRuntimeError::ExecutorFailed)
        })
    }
}

pub(crate) enum ConfiguredExecutor {
    Llm(LlmAgentExecutor),
    Deterministic(DisabledExecutor),
    Unavailable,
}

impl ConfiguredExecutor {
    /// Materialized output text for a completed run. Every executor that can
    /// complete stores its text before returning the digest, so a digest that
    /// fails to load is an error, never a silently absent field.
    pub(crate) fn output(&self, digest: &str) -> Result<String, ToolError> {
        match self {
            Self::Llm(executor) => executor.output(digest),
            Self::Deterministic(executor) => executor.payloads.load_output(digest),
            // Never completes a run, so it never has a digest to resolve.
            Self::Unavailable => Err(ToolError::Sdk {
                sdk_kind: "internal_error".into(),
                message: "no executor produced the requested Agent output".into(),
            }),
        }
    }
}

impl AgentExecutor for ConfiguredExecutor {
    async fn execute(
        &self,
        request: AgentExecutionRequest,
        guard: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        match self {
            Self::Llm(executor) => executor.execute(request, guard).await,
            Self::Deterministic(executor) => executor.execute(request, guard).await,
            Self::Unavailable => Err(AgentRuntimeError::ExecutorFailed),
        }
    }

    async fn cancel(&self, request: &AgentExecutionRequest) {
        if let Self::Llm(executor) = self {
            executor.cancel(request).await;
        }
    }
}

pub(crate) fn configured_executor(
    store: &crate::access::AccessStore,
    input: String,
) -> Result<ConfiguredExecutor, ToolError> {
    if deterministic_executor_enabled() {
        return Ok(ConfiguredExecutor::Deterministic(
            DisabledExecutor::for_access_store(store),
        ));
    }
    match LlmAgentExecutor::new(store, input) {
        Ok(executor) => Ok(ConfiguredExecutor::Llm(executor)),
        Err(error) if error.kind() == "invalid_param" => Err(error),
        Err(error) => {
            tracing::warn!(
                kind = error.kind(),
                "Agent LLM backend unavailable at execution admission"
            );
            Ok(ConfiguredExecutor::Unavailable)
        }
    }
}

pub(crate) fn configured_task_executor(
    store: &crate::access::AccessStore,
    input_digest: &str,
) -> ConfiguredExecutor {
    if deterministic_executor_enabled() {
        return ConfiguredExecutor::Deterministic(DisabledExecutor::for_access_store(store));
    }
    match LlmAgentExecutor::from_task(store, input_digest) {
        Ok(executor) => ConfiguredExecutor::Llm(executor),
        Err(error) => {
            tracing::warn!(
                kind = error.kind(),
                "Agent Task LLM backend unavailable for queued attempt"
            );
            ConfiguredExecutor::Unavailable
        }
    }
}

/// Process-wide switch for unit tests that drive the deterministic executor.
/// The crate forbids unsafe code and `std::env::set_var` is unsafe in edition
/// 2024, so tests pin the hook here instead of mutating the environment (the
/// same seam shape as `phoenix_openai::install_test_base_url`). It exists only
/// under `proxy-testkit`, so the release-profile tests that prove the branch
/// is compiled out cannot reach it.
#[cfg(all(test, feature = "proxy-testkit"))]
static TEST_DETERMINISTIC_EXECUTORS: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Select the deterministic executor for every later admission in this process.
#[cfg(all(test, feature = "proxy-testkit"))]
pub(crate) fn install_test_deterministic_executors() {
    TEST_DETERMINISTIC_EXECUTORS.get_or_init(|| ());
}

fn deterministic_executor_enabled() -> bool {
    #[cfg(all(test, feature = "proxy-testkit"))]
    if TEST_DETERMINISTIC_EXECUTORS.get().is_some() {
        return true;
    }
    cfg!(feature = "proxy-testkit")
        && std::env::var_os("LABBY_E2E_DETERMINISTIC_EXECUTORS").is_some()
}

async fn load(
    context: &AgentDispatchContext,
    params: &Value,
) -> Result<AgentDefinition, ToolError> {
    context
        .store
        .get_agent_definition(required(params, "agent_id")?)
        .await
        .map_err(map)?
        .ok_or_else(denied)
}
async fn authorize(
    context: &AgentDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: &str,
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
    context: &AgentDispatchContext,
    name: &str,
    owner: &OwnerScope,
    id: &str,
    capability: Capability,
    now: u64,
) -> Result<AuthorityRequest, ToolError> {
    let action = ActionRef::new("agents", name).map_err(|_| invalid("action"))?;
    // A run is fenced against its lease at every safe boundary, so the lease
    // issued for it must outlive the runtime bound; every other action keeps
    // the short request lease.
    let spec = ActionAuthoritySpec::new(action.clone(), ResourceFamily::Agent, capability);
    let (spec, safe_boundaries) = if name == "agents.run" {
        (
            spec.with_lease_lifetime_millis(AGENT_MAX_RUNTIME_MILLIS),
            execution_safe_boundaries(),
        )
    } else {
        (spec, request_safe_boundaries())
    };
    Ok(AuthorityRequest::new(
        context.identity.clone(),
        ActionAuthoritySpec::SCHEMA_VERSION,
        action.clone(),
        ResourceRef::new(
            owner.clone(),
            ResourceFamily::Agent,
            ResourceId::new(id).map_err(|_| invalid("agent_id"))?,
        ),
        context.ceiling.clone(),
        None,
        now,
        safe_boundaries,
        vec![spec],
    ))
}
/// Boundaries a request-scoped action revalidates at.
pub(crate) fn request_safe_boundaries() -> Vec<AuthoritySafeBoundary> {
    vec![
        AuthoritySafeBoundary::BeforeDispatch,
        AuthoritySafeBoundary::BeforeCommit,
    ]
}
/// Boundaries the Agent runtime and the LLM executor revalidate at: admission,
/// every provider effect, and the final commit. A lease that omits one of
/// them reads as revocation at that boundary.
pub(crate) fn execution_safe_boundaries() -> Vec<AuthoritySafeBoundary> {
    vec![
        AuthoritySafeBoundary::BeforeDispatch,
        AuthoritySafeBoundary::BeforeExternalEffect,
        AuthoritySafeBoundary::BeforeCommit,
    ]
}
fn materialize_llm_payload(
    store: &crate::access::AccessStore,
    params: Value,
    prior: Option<&AgentDefinition>,
) -> Result<Value, ToolError> {
    materialize_llm_payload_with(store, params, prior, current_harness_digest)
}

/// Materialize the LLM payload with an explicit harness-digest source so the
/// ordering against the content-addressed write can be tested without the
/// provider environment.
fn materialize_llm_payload_with(
    store: &crate::access::AccessStore,
    mut params: Value,
    prior: Option<&AgentDefinition>,
    harness: impl FnOnce() -> Result<String, ToolError>,
) -> Result<Value, ToolError> {
    let explicit_instructions = params
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let explicit_model = params
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if explicit_instructions.is_none() && explicit_model.is_none() {
        return Ok(params);
    }

    let payloads = AgentPayloadStore::for_access_store(store);
    let inherited = match prior {
        Some(definition) if explicit_instructions.is_none() || explicit_model.is_none() => {
            // A partial revision update must inherit from the exact pinned
            // payload. Missing/corrupt content is a hard failure; silently
            // defaulting would mutate an immutable revision's effective model.
            Some(payloads.load_agent(&definition.revision.content_digest)?)
        }
        _ => None,
    };
    let instructions = explicit_instructions
        .or_else(|| {
            inherited
                .as_ref()
                .map(|payload| payload.instructions.clone())
        })
        .ok_or_else(|| invalid("instructions"))?;
    let model = explicit_model
        .or_else(|| inherited.as_ref().map(|payload| payload.model.clone()))
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
    // Resolve the provider before the content-addressed write: a create that
    // fails for lack of a provider must not leave an immutable payload behind
    // that no revision references.
    let harness_digest = harness()?;
    let expected_content_digest = params.get("content_digest").and_then(Value::as_str);
    let content_digest =
        payloads.store_agent(Some(&model), &instructions, expected_content_digest)?;
    if params
        .get("harness_digest")
        .and_then(Value::as_str)
        .is_some_and(|supplied| supplied != harness_digest)
    {
        return Err(invalid("harness_digest"));
    }

    let object = params.as_object_mut().ok_or_else(|| invalid("params"))?;
    object.insert("content_digest".into(), Value::String(content_digest));
    object.insert("harness_digest".into(), Value::String(harness_digest));
    if prior.is_none() {
        for (key, label) in [
            ("repository_digest", "repository:none"),
            ("image_digest", "image:none"),
            ("loadout_digest", "loadout:none"),
        ] {
            object
                .entry(key.to_owned())
                .or_insert_with(|| Value::String(llm_default_digest(label)));
        }
        object
            .entry("catalog_generation".to_owned())
            .or_insert_with(|| Value::String("openai-compatible-v1".into()));
    }
    Ok(params)
}

fn llm_default_digest(label: &str) -> String {
    Sha256Digest::of(format!("labby:llm-agent:{label}:v1").as_bytes()).to_string()
}

fn definition(
    params: &Value,
    prior: Option<&AgentDefinition>,
) -> Result<AgentDefinition, ToolError> {
    let id = prior.map_or_else(|| required(params, "agent_id"), |v| Ok(v.id.clone()))?;
    let owner = prior.map_or_else(|| owner(params), |v| Ok(v.owner.clone()))?;
    let version = prior.map_or(1, |v| v.revision.version + 1);
    let digest = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                prior.map(|v| match key {
                    "content_digest" => v.revision.content_digest.clone(),
                    "repository_digest" => v.revision.repository_digest.clone(),
                    "image_digest" => v.revision.image_digest.clone(),
                    "harness_digest" => v.revision.harness_digest.clone(),
                    "loadout_digest" => v.revision.loadout_digest.clone(),
                    _ => v.revision.catalog_generation.clone(),
                })
            })
            .ok_or_else(|| invalid(key))
    };
    let value = AgentDefinition {
        id,
        owner,
        revision: AgentRevision {
            version,
            content_digest: digest("content_digest")?,
            repository_digest: digest("repository_digest")?,
            image_digest: digest("image_digest")?,
            harness_digest: digest("harness_digest")?,
            loadout_digest: digest("loadout_digest")?,
            catalog_generation: digest("catalog_generation")?,
            credential_references: params
                .get("credential_references")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .or_else(|| prior.map(|v| v.revision.credential_references.clone()))
                .unwrap_or_default(),
        },
        // A new revision never changes lifecycle state: updating a suspended
        // Agent must not silently reactivate it.
        state: prior.map_or(AgentState::Active, |v| v.state),
        required_capabilities: vec![Capability::ScopeOperate],
        // Epochs are server-assigned. `reject_server_assigned` has already
        // refused caller-supplied values, so they never reach the record.
        authority_epoch: prior.map_or(1, |v| v.authority_epoch),
        publication_epoch: prior.map_or(1, |v| v.publication_epoch.saturating_add(1)),
        revocation_policy: RunningRevocationPolicy::StopAtSafeBoundary,
    };
    value.validate().map_err(|_| invalid("definition"))?;
    Ok(value)
}
/// Parse the owner scope for a caller-created Agent. `installation` is not a
/// valid Agent owner per the authority matrix, so it is rejected as invalid
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
fn render(v: &AgentDefinition) -> Value {
    let (kind, id) = match &v.owner {
        OwnerScope::Installation(x) => ("installation", x.as_str()),
        OwnerScope::Team(x) => ("team", x.as_str()),
        OwnerScope::Project(x) => ("project", x.as_str()),
        OwnerScope::Personal(x) => ("personal", x.as_str()),
    };
    json!({"agent_id":v.id,"owner_kind":kind,"owner_id":id,"version":v.revision.version,"state":state_name(v.state),"catalog_generation":v.revision.catalog_generation,"authority_epoch":v.authority_epoch,"publication_epoch":v.publication_epoch})
}
fn state_name(v: AgentState) -> &'static str {
    match v {
        AgentState::Active => "active",
        AgentState::Suspended => "suspended",
        AgentState::Deleted => "deleted",
    }
}
fn required(v: &Value, k: &str) -> Result<String, ToolError> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| invalid(k))
}
/// Optional string parameter: absent and `null` mean "not supplied"; any
/// other non-string value is the caller's error, never silently ignored.
fn optional_text(v: &Value, k: &str) -> Result<String, ToolError> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(invalid(k)),
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
    ToolError::internal_message("Agent service unavailable")
}
fn map(error: AccessStoreError) -> ToolError {
    map_store_error("agents", error, denied)
}
/// Map a definition write failure. The store reports a lost compare-and-set on
/// `agent_id`/`version` (identifier already taken on create, or a stale
/// revision on update) as an `agent_version` integrity violation; that is an
/// authorization-shaped outcome for the caller and must not become an
/// enumerable outage.
fn map_put(error: AccessStoreError) -> ToolError {
    match error {
        AccessStoreError::IntegrityViolation {
            check: "agent_version",
        } => denied(),
        other => map(other),
    }
}
/// Refuse caller-supplied values for fields the server assigns.
pub(crate) fn reject_server_assigned(params: &Value) -> Result<(), ToolError> {
    for key in ["authority_epoch", "publication_epoch"] {
        if params.get(key).is_some() {
            return Err(invalid(key));
        }
    }
    Ok(())
}
/// Map a typed Agent runtime failure to the shared error envelope without
/// losing the reason. Shared by `agents.run` and Agent Task execution.
pub(crate) fn map_agent_runtime_error(error: &AgentRuntimeError) -> ToolError {
    match error {
        AgentRuntimeError::Revoked | AgentRuntimeError::Lease(_) => ToolError::Sdk {
            sdk_kind: "authority_changed".into(),
            message: "authority changed during execution".into(),
        },
        AgentRuntimeError::AuthorityUnavailable => ToolError::Sdk {
            sdk_kind: "source_unavailable".into(),
            message: "authority source is unavailable".into(),
        },
        AgentRuntimeError::ResourceLimit => ToolError::Sdk {
            sdk_kind: "quota_exceeded".into(),
            message: "Agent execution exceeded its resource bounds".into(),
        },
        AgentRuntimeError::Cancelled => ToolError::Sdk {
            sdk_kind: "cancelled".into(),
            message: "Agent execution was cancelled".into(),
        },
        AgentRuntimeError::ExecutorFailed => ToolError::Sdk {
            sdk_kind: "service_unavailable".into(),
            message: "Agent execution backend failed".into(),
        },
        // The session no longer matches the pinned definition, the lease is
        // not bound to this execution, or the granted capability cannot
        // dispatch it; the caller may not learn which part drifted.
        AgentRuntimeError::PinnedDefinitionMismatch
        | AgentRuntimeError::BindingMismatch
        | AgentRuntimeError::NotDispatchable => denied(),
        AgentRuntimeError::InvalidDefinition | AgentRuntimeError::InvalidBounds => {
            ToolError::internal_message("Agent execution request is invalid")
        }
    }
}
fn unknown(name: &str) -> ToolError {
    ToolError::UnknownAction {
        message: "unknown Agent action".into(),
        valid: ACTIONS.iter().map(|a| a.name.into()).collect(),
        hint: ACTIONS
            .iter()
            .find(|a| a.name.starts_with(name))
            .map(|a| a.name.into()),
    }
}
pub async fn dispatch_unbound(name: &str, params: Value) -> Result<Value, ToolError> {
    if name == "help" {
        return Ok(crate::dispatch::helpers::help_payload("agents", ACTIONS));
    }
    if name == "schema" {
        return crate::dispatch::helpers::action_schema(ACTIONS, &required(&params, "action")?);
    }
    Err(denied())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Store-backed fixtures shared by the Agent and Agent Task dispatch tests.
    use super::AgentDispatchContext;
    use crate::access::{AccessStore, AuthorityCeiling, BootstrapOwnerInput};
    use labby_auth::{Authenticator, VerifiedIdentity};
    use serde_json::{Value, json};

    /// Principal id the bootstrap flow assigns to the first (platform admin) owner.
    pub(crate) const BOOTSTRAP_PRINCIPAL: &str = "bootstrap-owner";

    pub(crate) fn secure_tempdir() -> tempfile::TempDir {
        let base = std::env::current_dir().expect("resolve the test working directory");
        let directory = tempfile::Builder::new()
            .prefix("labby-agent-dispatch-test-")
            .tempdir_in(base)
            .expect("create a fixture outside the symlinked macOS temporary directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("restrict fixture permissions");
        }
        directory
    }

    pub(crate) fn browser(subject: &str) -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            subject,
        )
        .unwrap()
    }

    /// Open a fresh access store with one bootstrapped owner (a platform admin
    /// whose personal owner scope is `personal/bootstrap-owner`).
    pub(crate) async fn fixture() -> (tempfile::TempDir, AccessStore, VerifiedIdentity) {
        // The harness digest is derived from the provider URL at create time;
        // no test connects to this address unless it drives execution.
        crate::dispatch::phoenix_openai::install_test_base_url("http://127.0.0.1:9/v1");
        let directory = secure_tempdir();
        let store = AccessStore::open(directory.path().join("access.db"))
            .await
            .unwrap();
        let owner = browser("owner-subject");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        (directory, store, owner)
    }

    pub(crate) fn agent_context(
        store: &AccessStore,
        identity: &VerifiedIdentity,
    ) -> AgentDispatchContext {
        AgentDispatchContext {
            store: store.clone(),
            identity: identity.clone(),
            ceiling: AuthorityCeiling::trusted_local(),
        }
    }

    pub(crate) fn digest(fill: char) -> String {
        format!(
            "sha256:{}",
            std::iter::repeat_n(fill, 64).collect::<String>()
        )
    }

    /// Valid `agents.create` parameters for a personal Agent owned by the
    /// bootstrap principal: exactly the caller-facing form.
    pub(crate) fn agent_params(agent_id: &str) -> Value {
        json!({
            "agent_id": agent_id,
            "owner_kind": "personal",
            "owner_id": BOOTSTRAP_PRINCIPAL,
            "instructions": "Summarize the input.",
            "model": "chatgpt-browser",
        })
    }

    /// Fully materialized revision parameters, as the dispatcher sees them
    /// after payload materialization, for tests that build definitions
    /// directly.
    pub(crate) fn agent_revision_params(agent_id: &str) -> Value {
        json!({
            "agent_id": agent_id,
            "owner_kind": "personal",
            "owner_id": BOOTSTRAP_PRINCIPAL,
            "content_digest": digest('a'),
            "repository_digest": digest('b'),
            "image_digest": digest('c'),
            "harness_digest": digest('d'),
            "loadout_digest": digest('e'),
            "catalog_generation": "catalog-1",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        BOOTSTRAP_PRINCIPAL, agent_context, agent_params, agent_revision_params, browser, digest,
        fixture,
    };
    use super::*;
    use labby_runtime::authority::AuthorityLeaseError;

    fn envelope(error: &ToolError) -> Value {
        serde_json::from_str(&error.to_string()).unwrap()
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
        assert_eq!(required_capability("agents.bogus"), None);
    }

    #[test]
    fn catalog_is_complete_and_unbound_denies() {
        assert_eq!(ACTIONS.len(), 9);
        let create = ACTIONS
            .iter()
            .find(|action| action.name == "agents.create")
            .unwrap();
        for required in ["agent_id", "owner_kind", "owner_id", "instructions"] {
            assert!(
                create
                    .params
                    .iter()
                    .any(|param| param.name == required && param.required)
            );
        }
        for optional in [
            "content_digest",
            "repository_digest",
            "image_digest",
            "harness_digest",
            "loadout_digest",
            "catalog_generation",
            "model",
        ] {
            assert!(
                create
                    .params
                    .iter()
                    .any(|param| param.name == optional && !param.required)
            );
        }
        let run = ACTIONS
            .iter()
            .find(|action| action.name == "agents.run")
            .unwrap();
        assert!(
            run.params
                .iter()
                .any(|param| param.name == "input" && !param.required)
        );
        assert!(ACTIONS.iter().all(|a| a.name.starts_with("agents.")));
    }
    /// Deletion filters the definition out of every read with no restore
    /// action, so it is permanent loss under the shared destructive policy.
    #[test]
    fn delete_is_marked_destructive() {
        let delete = ACTIONS
            .iter()
            .find(|action| action.name == "agents.delete")
            .unwrap();
        assert!(delete.destructive);
        assert!(
            !delete.requires_admin,
            "destructive and admin are separate axes"
        );
        // Suspension is reversible and cancellation stops work in flight;
        // neither loses data.
        for name in ["agents.suspend", "agents.session.cancel", "agents.run"] {
            let action = ACTIONS.iter().find(|action| action.name == name).unwrap();
            assert!(!action.destructive, "{name}");
        }
    }

    #[tokio::test]
    async fn context_free_is_fail_closed() {
        assert_eq!(
            dispatch_unbound("agents.list", json!({}))
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
    }

    #[test]
    fn runtime_errors_keep_their_typed_reason() {
        let table = [
            (AgentRuntimeError::Revoked, "authority_changed"),
            (
                AgentRuntimeError::Lease(AuthorityLeaseError::Expired),
                "authority_changed",
            ),
            (
                AgentRuntimeError::Lease(AuthorityLeaseError::AuthorityChanged),
                "authority_changed",
            ),
            (
                AgentRuntimeError::AuthorityUnavailable,
                "source_unavailable",
            ),
            (AgentRuntimeError::ResourceLimit, "quota_exceeded"),
            (AgentRuntimeError::Cancelled, "cancelled"),
            (AgentRuntimeError::ExecutorFailed, "service_unavailable"),
            (AgentRuntimeError::PinnedDefinitionMismatch, "forbidden"),
            (AgentRuntimeError::BindingMismatch, "forbidden"),
            (AgentRuntimeError::NotDispatchable, "forbidden"),
            (AgentRuntimeError::InvalidDefinition, "internal_error"),
            (AgentRuntimeError::InvalidBounds, "internal_error"),
        ];
        for (error, kind) in table {
            let mapped = map_agent_runtime_error(&error);
            assert_eq!(mapped.kind(), kind, "{error:?}");
        }
        assert_eq!(
            envelope(&map_agent_runtime_error(&AgentRuntimeError::Revoked))["message"],
            "authority changed during execution"
        );
        assert_eq!(
            envelope(&map_agent_runtime_error(&AgentRuntimeError::ExecutorFailed))["message"],
            "Agent execution backend failed"
        );
    }

    #[test]
    fn installation_owner_is_rejected_as_invalid_input() {
        let error = owner(&json!({"owner_kind":"installation","owner_id":"local"})).unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert_eq!(envelope(&error)["param"], "owner_kind");
        assert!(owner(&json!({"owner_kind":"personal","owner_id":"p1"})).is_ok());
    }

    #[test]
    fn caller_supplied_epochs_are_refused_and_server_assigned() {
        for key in ["authority_epoch", "publication_epoch"] {
            let mut params = agent_params("a1");
            params[key] = json!(99);
            let error = reject_server_assigned(&params).unwrap_err();
            assert_eq!(error.kind(), "invalid_param");
            assert_eq!(envelope(&error)["param"], key);
        }
        let created = definition(&agent_revision_params("a1"), None).unwrap();
        assert_eq!((created.authority_epoch, created.publication_epoch), (1, 1));
        let mut prior = created.clone();
        prior.state = AgentState::Suspended;
        prior.authority_epoch = 7;
        let next = definition(&json!({"agent_id":"a1"}), Some(&prior)).unwrap();
        assert_eq!(next.revision.version, 2);
        assert_eq!(
            next.state,
            AgentState::Suspended,
            "update must not reactivate"
        );
        assert_eq!((next.authority_epoch, next.publication_epoch), (7, 2));
    }

    #[tokio::test]
    async fn team_derived_project_agents_are_listed_and_disappear_on_suspension() {
        let (_dir, store, owner) = fixture().await;
        store.execute_test_statement(
            "INSERT INTO principals VALUES('team-reader','bootstrap-local','user','active',NULL,2,2);
             INSERT INTO principal_links VALUES('team-reader-link','team-reader','external','https://accounts.google.com','team-reader',NULL,'active',1,1,2,2);"
        ).await.unwrap();
        store
            .add_team_member(
                crate::access::AddTeamMemberInput::new(
                    owner.clone(),
                    "bootstrap-initial-team",
                    "team-reader",
                    crate::access::TeamRole::Member,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .assign_team_project(
                crate::access::AssignTeamProjectInput::new(
                    owner.clone(),
                    "bootstrap-initial-team",
                    "bootstrap-default",
                    crate::access::ProjectRole::Viewer,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut params = agent_params("project-agent");
        params["owner_kind"] = json!("project");
        params["owner_id"] = json!("bootstrap-default");
        dispatch(agent_context(&store, &owner), "agents.create", params)
            .await
            .unwrap();
        let reader = agent_context(&store, &browser("team-reader"));
        let listed = dispatch(reader.clone(), "agents.list", json!({}))
            .await
            .unwrap();
        assert_eq!(listed["agents"].as_array().unwrap().len(), 1);
        assert_eq!(listed["agents"][0]["agent_id"], "project-agent");
        store
            .suspend_team(owner, "bootstrap-initial-team".into())
            .await
            .unwrap();
        let listed = dispatch(reader, "agents.list", json!({})).await.unwrap();
        assert!(listed["agents"].as_array().unwrap().is_empty());
    }

    /// Bind exactly the parameters the shared catalog marks required, so the
    /// advertised schema is proven sufficient rather than merely necessary.
    fn required_only_params(action: &str, values: &[(&str, &str)]) -> Value {
        let spec = ACTIONS.iter().find(|a| a.name == action).unwrap();
        let mut params = serde_json::Map::new();
        for param in spec.params.iter().filter(|param| param.required) {
            let value = values
                .iter()
                .find(|(name, _)| *name == param.name)
                .unwrap_or_else(|| panic!("no test value for required param {}", param.name))
                .1;
            params.insert(param.name.to_owned(), json!(value));
        }
        Value::Object(params)
    }

    #[tokio::test]
    async fn create_with_exactly_the_required_params_succeeds() {
        let (_dir, store, owner) = fixture().await;
        let params = required_only_params(
            "agents.create",
            &[
                ("agent_id", "required-only-agent"),
                ("owner_kind", "personal"),
                ("owner_id", BOOTSTRAP_PRINCIPAL),
                ("instructions", "Summarize the input."),
            ],
        );
        let created = dispatch(agent_context(&store, &owner), "agents.create", params)
            .await
            .unwrap();
        assert_eq!(created["agent_id"], "required-only-agent");
        assert_eq!(created["version"], 1);
        let stored = store
            .get_agent_definition("required-only-agent".into())
            .await
            .unwrap()
            .unwrap();
        // The revision digests are server-derived from the materialized payload.
        assert!(stored.revision.content_digest.starts_with("sha256:"));
        assert!(stored.revision.harness_digest.starts_with("sha256:"));
    }

    #[tokio::test]
    async fn run_lease_covers_the_runtime_bound() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        let now = now().unwrap();
        let request = authority_request(
            &context,
            "agents.run",
            &OwnerScope::Personal(PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap()),
            "any-agent",
            Capability::ScopeOperate,
            now,
        )
        .unwrap();
        let lease = authorize_action(&store, request).await.unwrap();
        assert!(
            lease.expires_at_millis() - now >= AGENT_MAX_RUNTIME_MILLIS,
            "an agents.run lease must cover the runtime bound, got {} ms",
            lease.expires_at_millis() - now
        );
        // Every other Agent action keeps the short request lease.
        let read = authority_request(
            &context,
            "agents.get",
            &OwnerScope::Personal(PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap()),
            "any-agent",
            Capability::ScopeRead,
            now,
        )
        .unwrap();
        let read_lease = authorize_action(&store, read).await.unwrap();
        assert!(read_lease.expires_at_millis() - now < AGENT_MAX_RUNTIME_MILLIS);
    }

    #[tokio::test]
    async fn run_lease_declares_every_runtime_boundary() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        let now = now().unwrap();
        let personal = OwnerScope::Personal(PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap());
        let request = authority_request(
            &context,
            "agents.run",
            &personal,
            "any-agent",
            Capability::ScopeOperate,
            now,
        )
        .unwrap();
        let lease = authorize_action(&store, request).await.unwrap();
        let epochs = refresh_authority_epochs(&store, owner, personal, Capability::ScopeOperate)
            .await
            .unwrap();
        // The runtime revalidates at admission, before every provider effect,
        // and before commit; an undeclared boundary reads as revocation.
        for boundary in [
            AuthoritySafeBoundary::BeforeDispatch,
            AuthoritySafeBoundary::BeforeExternalEffect,
            AuthoritySafeBoundary::BeforeCommit,
        ] {
            assert_eq!(
                lease.validate_at(boundary, now, &epochs),
                Ok(()),
                "{boundary:?}"
            );
        }
    }

    #[tokio::test]
    async fn session_cancel_signals_the_live_run() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        dispatch(
            context.clone(),
            "agents.create",
            agent_params("cancel-agent"),
        )
        .await
        .unwrap();
        let definition = store
            .get_agent_definition("cancel-agent".into())
            .await
            .unwrap()
            .unwrap();
        let now = now().unwrap();
        store
            .create_agent_session(
                "cancel-agent-session-1".into(),
                definition,
                owner.safe_fingerprint(),
                "fingerprint".into(),
                i64::try_from(now + AGENT_MAX_RUNTIME_MILLIS).unwrap(),
                i64::try_from(now).unwrap(),
            )
            .await
            .unwrap();
        store
            .set_agent_session_status(
                "cancel-agent".into(),
                "cancel-agent-session-1".into(),
                "admitted".into(),
                "running".into(),
            )
            .await
            .unwrap();
        let cancel = json!({"agent_id":"cancel-agent","session_id":"cancel-agent-session-1"});
        // A live in-process run is signalled at its next safe boundary.
        let cancellation = Cancellation::new();
        register_agent_session_cancellation("cancel-agent-session-1".into(), cancellation.clone());
        let response = dispatch(context.clone(), "agents.session.cancel", cancel.clone())
            .await
            .unwrap();
        assert_eq!(response["status"], "cancelling");
        assert_eq!(response["cancel_requested"], true);
        assert!(cancellation.is_cancelled());
        unregister_agent_session_cancellation("cancel-agent-session-1");
        // A session with no live in-process owner reports its durable status
        // and signals nothing.
        let response = dispatch(context.clone(), "agents.session.cancel", cancel.clone())
            .await
            .unwrap();
        assert_eq!(response["status"], "running");
        assert_eq!(response["cancel_requested"], false);
        // Cancellation is an operate-scope action, never a read.
        let stranger = agent_context(&store, &browser("stranger-subject"));
        let error = dispatch(stranger, "agents.session.cancel", cancel)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
        // An unknown session is a non-enumerating denial.
        let error = dispatch(
            context,
            "agents.session.cancel",
            json!({"agent_id":"cancel-agent","session_id":"unknown"}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }

    /// A non-string `input` is a caller error, never silently treated as an
    /// empty run; `null` and absence still mean "no input".
    #[tokio::test]
    async fn run_rejects_non_string_input() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        dispatch(
            context.clone(),
            "agents.create",
            agent_params("typed-agent"),
        )
        .await
        .unwrap();
        for input in [json!(42), json!(["a"]), json!({"text":"a"}), json!(true)] {
            let error = dispatch(
                context.clone(),
                "agents.run",
                json!({"agent_id":"typed-agent","input":input}),
            )
            .await
            .unwrap_err();
            assert_eq!(error.kind(), "invalid_param", "{input}");
            assert_eq!(envelope(&error)["param"], "input", "{input}");
        }
        // Absent and null input are admitted (the run then fails against the
        // pinned dummy provider, never at parameter validation).
        for params in [
            json!({"agent_id":"typed-agent"}),
            json!({"agent_id":"typed-agent","input":null}),
        ] {
            let error = dispatch(context.clone(), "agents.run", params)
                .await
                .unwrap_err();
            assert_ne!(error.kind(), "invalid_param");
        }
    }

    #[tokio::test]
    async fn fifth_concurrent_run_for_owner_is_rejected() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        let mut params = agent_params("team-run-agent");
        params["owner_kind"] = json!("team");
        params["owner_id"] = json!("bootstrap-initial-team");
        dispatch(context.clone(), "agents.create", params)
            .await
            .unwrap();
        let team = OwnerScope::Team(TeamId::new("bootstrap-initial-team").unwrap());
        // Four live runs hold the owner's whole quota.
        let live = (0..AGENT_RUN_PER_OWNER_LIMIT)
            .map(|_| AGENT_RUN_SCHEDULER.try_admit(&team).unwrap())
            .collect::<Vec<_>>();
        let error = dispatch(
            context.clone(),
            "agents.run",
            json!({"agent_id":"team-run-agent"}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "queue_saturated");
        assert!(
            envelope(&error)["message"]
                .as_str()
                .unwrap()
                .contains("retry after one completes")
        );
        drop(live);
        // With capacity back the run is admitted and reaches the executor,
        // which fails against the pinned dummy provider instead of at admission.
        let error = dispatch(context, "agents.run", json!({"agent_id":"team-run-agent"}))
            .await
            .unwrap_err();
        assert_ne!(error.kind(), "queue_saturated");
    }

    #[tokio::test]
    async fn live_execution_rejects_a_suspended_pinned_definition() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        dispatch(
            context.clone(),
            "agents.create",
            agent_params("running-agent"),
        )
        .await
        .unwrap();
        let definition = store
            .get_agent_definition("running-agent".into())
            .await
            .unwrap()
            .unwrap();
        let authority = LiveExecutionAuthority {
            store: store.clone(),
            identity: owner,
            owner: definition.owner.clone(),
            definition,
        };
        assert!(authority.current_epochs().await.is_ok());
        dispatch(
            context,
            "agents.suspend",
            json!({"agent_id":"running-agent"}),
        )
        .await
        .unwrap();
        assert_eq!(
            authority.current_epochs().await.unwrap_err(),
            AgentRuntimeError::Revoked
        );
    }

    #[tokio::test]
    async fn create_refuses_caller_epochs_and_nothing_is_stored() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        let mut params = agent_params("epoch-agent");
        params["authority_epoch"] = json!(42);
        let error = dispatch(context.clone(), "agents.create", params)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(
            store
                .get_agent_definition("epoch-agent".into())
                .await
                .unwrap()
                .is_none()
        );
        let created = dispatch(context, "agents.create", agent_params("epoch-agent"))
            .await
            .unwrap();
        assert_eq!(created["authority_epoch"], 1);
        assert_eq!(created["publication_epoch"], 1);
    }

    #[tokio::test]
    async fn partial_llm_revision_update_fails_when_inherited_payload_is_missing() {
        let (_directory, store, _owner) = fixture().await;
        let prior = definition(&agent_revision_params("legacy-agent"), None).unwrap();
        let error = materialize_llm_payload(
            &store,
            json!({"agent_id":"legacy-agent","instructions":"new instructions"}),
            Some(&prior),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "unavailable");
    }

    /// Resolving the provider must come before the content-addressed write:
    /// a create that fails because the provider is unconfigured must not
    /// leave an immutable payload behind that no revision references.
    #[tokio::test]
    async fn create_without_provider_url_leaves_no_orphaned_payload() {
        let (_directory, store, _owner) = fixture().await;
        let error = materialize_llm_payload_with(&store, agent_params("orphan"), None, || {
            Err(ToolError::Sdk {
                sdk_kind: "unavailable".into(),
                message: "provider unconfigured".into(),
            })
        })
        .unwrap_err();
        assert_eq!(error.kind(), "unavailable");
        assert!(
            !store.storage_dir().join("agent-payloads").exists(),
            "a failed create must not materialize an orphaned Agent payload"
        );
    }

    #[tokio::test]
    async fn unauthorized_run_is_denied_before_input_validation() {
        let (_dir, store, owner) = fixture().await;
        dispatch(
            agent_context(&store, &owner),
            "agents.create",
            agent_params("private-agent"),
        )
        .await
        .unwrap();
        let stranger = agent_context(&store, &browser("stranger-subject"));
        let error = dispatch(
            stranger,
            "agents.run",
            json!({
                "agent_id":"private-agent",
                "input": "x".repeat(crate::dispatch::agent_payloads::MAX_TASK_INPUT_BYTES + 1),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
        assert_eq!(envelope(&error)["message"], "access denied");
    }

    #[tokio::test]
    async fn update_preserves_suspended_state() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        dispatch(
            context.clone(),
            "agents.create",
            agent_params("suspended-agent"),
        )
        .await
        .unwrap();
        dispatch(
            context.clone(),
            "agents.suspend",
            json!({"agent_id":"suspended-agent"}),
        )
        .await
        .unwrap();
        let updated = dispatch(
            context.clone(),
            "agents.update",
            json!({"agent_id":"suspended-agent","content_digest":digest('f')}),
        )
        .await
        .unwrap();
        assert_eq!(updated["version"], 2);
        assert_eq!(updated["state"], "suspended");
        let stored = store
            .get_agent_definition("suspended-agent".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, AgentState::Suspended);
        assert_eq!(stored.revision.content_digest, digest('f'));
        // A suspended Agent still cannot run after the revision bump.
        let error = dispatch(context, "agents.run", json!({"agent_id":"suspended-agent"}))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
    }

    #[tokio::test]
    async fn taken_identifier_is_indistinguishable_from_denial() {
        let (_dir, store, owner) = fixture().await;
        let context = agent_context(&store, &owner);
        dispatch(context.clone(), "agents.create", agent_params("taken"))
            .await
            .unwrap();
        let duplicate = dispatch(context.clone(), "agents.create", agent_params("taken"))
            .await
            .unwrap_err();
        // A stranger with no principal is denied for a free identifier.
        let stranger = agent_context(&store, &browser("stranger-subject"));
        let unauthorized = dispatch(stranger, "agents.create", agent_params("free"))
            .await
            .unwrap_err();
        assert_eq!(duplicate.kind(), "forbidden");
        assert_eq!(envelope(&duplicate), envelope(&unauthorized));
        assert_eq!(envelope(&duplicate)["message"], "access denied");
        // The lost compare-and-set from the store maps the same way.
        let raced = map_put(AccessStoreError::IntegrityViolation {
            check: "agent_version",
        });
        assert_eq!(envelope(&raced), envelope(&duplicate));
        assert_eq!(
            map_put(AccessStoreError::Corrupt).kind(),
            "service_unavailable"
        );
        // The original record is untouched.
        let stored = store
            .get_agent_definition("taken".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.revision.version, 1);
        assert_eq!(
            stored.owner.to_owned(),
            OwnerScope::Personal(PrincipalId::new(BOOTSTRAP_PRINCIPAL).unwrap())
        );
    }

    #[tokio::test]
    async fn store_denials_collapse_to_one_non_enumerating_error() {
        for error in [
            AccessStoreError::NotAuthorized,
            AccessStoreError::IdentityUnavailable,
            AccessStoreError::ProjectAccessUnavailable,
            AccessStoreError::TeamUnavailable,
        ] {
            let mapped = map(error);
            assert_eq!(mapped.kind(), "forbidden");
            assert_eq!(envelope(&mapped)["message"], "access denied");
        }
        assert_eq!(map(AccessStoreError::Locked).kind(), "service_unavailable");
    }
}
