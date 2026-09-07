//! Authenticated, owner-scoped Agent surface shared by HTTP and MCP.

use crate::{
    access::{
        AccessStoreError, ActionAuthoritySpec, AuthorityCeiling, AuthorityRequest,
        authorize_action, refresh_authority_epochs,
    },
    dispatch::{access_errors::map_store_error, error::ToolError},
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
};
use labby_runtime::{
    agent_runtime::{
        AgentAuthority, AgentExecutionOutput, AgentExecutionRequest, AgentExecutor,
        AgentResourceBounds, AgentRuntimeError, Cancellation, ExecutionGuard, execute_agent,
    },
    authority::{AuthorityEpochVector, AuthoritySafeBoundary},
};
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
        "agents.create",
        "Create an Agent definition",
        &[
            param("agent_id"),
            param("owner_kind"),
            param("owner_id"),
            param("content_digest"),
            param("repository_digest"),
            param("image_digest"),
            param("harness_digest"),
            param("loadout_digest"),
            param("catalog_generation"),
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
        &[param("agent_id")],
    ),
    action("agents.suspend", "Suspend an Agent", &[param("agent_id")]),
    action("agents.delete", "Delete an Agent", &[param("agent_id")]),
    action(
        "agents.run",
        "Start a pinned Agent session",
        &[param("agent_id")],
    ),
    action(
        "agents.session.status",
        "Read Agent session status",
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
        "agents.run" => Capability::ScopeOperate,
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
            let definition = definition(&params, None)?;
            let request = authority_request(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeCreate,
                now,
            )?;
            // An identifier that is already taken must look exactly like an
            // authorization failure so `agents.create` cannot be used as an
            // existence oracle across owners.
            if context
                .store
                .get_agent_definition(definition.id.clone())
                .await
                .map_err(map)?
                .is_some()
            {
                return Err(denied());
            }
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
            if definition.state != AgentState::Active {
                return Err(denied());
            }
            let lease = authorize(
                &context,
                name,
                &definition.owner,
                &definition.id,
                Capability::ScopeOperate,
                now,
            )
            .await?;
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
            let session_id = format!("{}-{now}", definition.id);
            let lease_expires_at = lease.expires_at_millis();
            context
                .store
                .create_agent_session(
                    session_id.clone(),
                    definition.clone(),
                    context.identity.safe_fingerprint(),
                    epochs.fingerprint().as_str().to_owned(),
                    i64::try_from(lease.expires_at_millis()).map_err(|_| internal())?,
                    i64::try_from(now).map_err(|_| internal())?,
                )
                .await
                .map_err(map)?;
            context
                .store
                .set_agent_session_status(
                    definition.id.clone(),
                    session_id.clone(),
                    "admitted".into(),
                    "running".into(),
                )
                .await
                .map_err(map)?;
            let request = AgentExecutionRequest {
                definition: definition.clone(),
                session: AgentSessionBinding {
                    session_id: session_id.clone(),
                    agent_id: definition.id.clone(),
                    agent_version: definition.revision.version,
                    // The runtime rejects a session whose principal or epoch fingerprint
                    // differs from the lease binding; both come from the lease itself.
                    principal: PrincipalId::new(lease.binding().principal_id())
                        .map_err(|_| invalid("principal"))?,
                    owner: definition.owner.clone(),
                    catalog_generation: definition.revision.catalog_generation.clone(),
                    authority_fingerprint: lease.epoch_fingerprint().as_str().into(),
                    lease_expires_at: i64::try_from(lease_expires_at).map_err(|_| internal())?,
                },
                lease,
                bounds: AgentResourceBounds {
                    max_runtime_millis: 300_000,
                    max_output_bytes: 16 * 1024 * 1024,
                    max_external_effects: 1_000,
                },
            };
            let result = execute_agent(
                &LiveExecutionAuthority {
                    store: context.store.clone(),
                    identity: context.identity.clone(),
                    owner: definition.owner.clone(),
                },
                &DisabledExecutor,
                request,
                Cancellation::new(),
                now,
            )
            .await;
            let next = match result {
                Ok(_) => "completed",
                Err(AgentRuntimeError::Revoked | AgentRuntimeError::Lease(_)) => "revoked",
                Err(AgentRuntimeError::Cancelled) => "cancelled",
                Err(_) => "failed",
            };
            context
                .store
                .set_agent_session_status(
                    definition.id.clone(),
                    session_id.clone(),
                    "running".into(),
                    next.into(),
                )
                .await
                .map_err(map)?;
            match result {
                Ok(output) => Ok(
                    json!({"agent_id":definition.id,"agent_version":definition.revision.version,"session_id":session_id,"status":next,"output_digest":output.digest,"authority_expires_at":lease_expires_at}),
                ),
                Err(error) => Err(map_agent_runtime_error(&error)),
            }
        }
        "agents.session.status" => {
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
        _ => Err(unknown(name)),
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
}
impl AgentAuthority for LiveExecutionAuthority {
    async fn current_epochs(&self) -> Result<AuthorityEpochVector, AgentRuntimeError> {
        refresh_authority_epochs(
            &self.store,
            self.identity.clone(),
            self.owner.clone(),
            Capability::ScopeOperate,
        )
        .await
        .map_err(|_| AgentRuntimeError::AuthorityUnavailable)
    }
}
/// Placeholder executor shared by `agents.run` and `tasks.queue` until a real
/// execution backend is wired. Product builds always fail with
/// [`AgentRuntimeError::ExecutorFailed`].
///
/// The deterministic branch is a **test-only hook**: it is compiled in only
/// under the `proxy-testkit` cargo feature (test support, never a product
/// slice) and additionally requires `LABBY_E2E_DETERMINISTIC_EXECUTORS` at run
/// time so live end-to-end matrices can drive the lifecycle. Product builds
/// compile the branch out entirely; setting the variable there has no effect.
pub(crate) struct DisabledExecutor;
impl AgentExecutor for DisabledExecutor {
    async fn execute(
        &self,
        _: AgentExecutionRequest,
        _: ExecutionGuard<'_>,
    ) -> Result<AgentExecutionOutput, AgentRuntimeError> {
        if cfg!(feature = "proxy-testkit")
            && std::env::var_os("LABBY_E2E_DETERMINISTIC_EXECUTORS").is_some()
        {
            Ok(AgentExecutionOutput {
                digest: format!("sha256:{}", "0".repeat(64)),
                bytes: 0,
                external_effects: 0,
            })
        } else {
            Err(AgentRuntimeError::ExecutorFailed)
        }
    }
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
        vec![
            AuthoritySafeBoundary::BeforeDispatch,
            AuthoritySafeBoundary::BeforeCommit,
        ],
        vec![ActionAuthoritySpec::new(
            action,
            ResourceFamily::Agent,
            capability,
        )],
    ))
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
            message: "Agent execution backend is not configured".into(),
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
    /// bootstrap principal.
    pub(crate) fn agent_params(agent_id: &str) -> Value {
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
        BOOTSTRAP_PRINCIPAL, agent_context, agent_params, browser, digest, fixture,
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
        assert_eq!(ACTIONS.len(), 8);
        let create = ACTIONS
            .iter()
            .find(|action| action.name == "agents.create")
            .unwrap();
        for required in [
            "content_digest",
            "repository_digest",
            "image_digest",
            "harness_digest",
            "loadout_digest",
            "catalog_generation",
        ] {
            assert!(
                create
                    .params
                    .iter()
                    .any(|param| param.name == required && param.required)
            );
        }
        assert!(ACTIONS.iter().all(|a| a.name.starts_with("agents.")));
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
            "Agent execution backend is not configured"
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
        let created = definition(&agent_params("a1"), None).unwrap();
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
