//! Shared Dev Container dispatch orchestration.
//!
//! Every surface reaches this module through [`dispatch`] with a
//! host-established identity: the authenticated HTTP adapter
//! (`crate::api::services::dev_containers`) and the MCP tool binding
//! (`crate::mcp::call_tool`). The registry's context-free entry,
//! [`dispatch_unbound`], serves only `help`/`schema` and denies every other
//! action. Runtime effects are delegated to the surface-neutral, pluggable
//! engine contract in `labby_runtime::dev_container_runtime`.

use labby_auth::VerifiedIdentity;
use labby_primitives::{
    access::{
        ActionRef, Capability, InstallationId, OwnerKind, OwnerScope, PrincipalId, ProjectId,
        ResourceFamily, ResourceId, ResourceRef, TeamId,
    },
    action::{ActionSpec, ParamSpec},
    dev_container::{DesiredState, DevContainerId, LifecycleNonce, ObservedState},
};
use labby_runtime::dev_container::DevContainerAdmissionError;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[allow(unused_imports)]
pub(crate) use labby_runtime::dev_container_runtime::{
    ContainerRuntime, DurableIntent, EngineCreateRequest, EngineHandle, EngineState,
    RecoveryAction, RuntimeError, create, reconcile,
};

use crate::access::{DevContainerLedgerError, DevContainerStorageFailure};
use crate::dispatch::access_errors::{map_runtime_error, map_store_error};
use crate::dispatch::error::ToolError;

const SERVICE: &str = "dev_containers";

const INSTANCE_ID: ParamSpec = ParamSpec {
    name: "instance_id",
    ty: "string",
    required: true,
    description: "Opaque Dev Container identifier",
};
const TEMPLATE_ID: ParamSpec = ParamSpec {
    name: "template_id",
    ty: "string",
    required: true,
    description: "Administrator-approved template identifier",
};
const OWNER_KIND: ParamSpec = ParamSpec {
    name: "owner_kind",
    ty: "installation|team|project|personal",
    required: true,
    description: "Single durable owner scope",
};
const OWNER_ID: ParamSpec = ParamSpec {
    name: "owner_id",
    ty: "string",
    required: true,
    description: "Owner identifier resolved against caller authority",
};
const SECRET_REFS: ParamSpec = ParamSpec {
    name: "secret_references",
    ty: "string[]",
    required: false,
    description: "Opaque secret references; secret material is never accepted",
};
const CURSOR: ParamSpec = ParamSpec {
    name: "cursor",
    ty: "string",
    required: false,
    description: "Exclusive instance identifier cursor",
};
const LIMIT: ParamSpec = ParamSpec {
    name: "limit",
    ty: "string",
    required: false,
    description: "Page size from 1 through 100",
};

const fn action(
    name: &'static str,
    description: &'static str,
    params: &'static [ParamSpec],
    destructive: bool,
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

pub(crate) const ACTIONS: &[ActionSpec] = &[
    action(
        "dev_containers.create",
        "Create from an approved template",
        &[INSTANCE_ID, TEMPLATE_ID, OWNER_KIND, OWNER_ID, SECRET_REFS],
        false,
    ),
    action(
        "dev_containers.list",
        "List Dev Containers visible to the caller",
        &[CURSOR, LIMIT],
        false,
    ),
    action(
        "dev_containers.start",
        "Request a stopped Dev Container start",
        &[INSTANCE_ID],
        false,
    ),
    action(
        "dev_containers.stop",
        "Request a Dev Container stop",
        &[INSTANCE_ID],
        false,
    ),
    action(
        "dev_containers.destroy",
        "Permanently destroy a Dev Container",
        &[INSTANCE_ID],
        true,
    ),
    action(
        "dev_containers.reconcile",
        "Reconcile durable intent with the runtime",
        &[INSTANCE_ID],
        false,
    ),
];

/// Exact evaluator input used before resolving a lease. Unknown actions deny.
pub(crate) fn required_capability(action: &str, _owner: OwnerKind) -> Option<Capability> {
    match action {
        "dev_containers.list" => Some(Capability::ScopeRead),
        "dev_containers.create" => Some(Capability::ScopeCreate),
        "dev_containers.start" | "dev_containers.stop" | "dev_containers.reconcile" => {
            Some(Capability::ScopeOperate)
        }
        "dev_containers.destroy" => Some(Capability::ScopeDelete),
        _ => None,
    }
}

/// Context-free registry entry. It carries no host-established identity or
/// authority epochs, so it can only answer `help`/`schema`; every other
/// action is refused without revealing whether the named instance exists.
/// The bound surfaces (HTTP and MCP) call [`dispatch`] instead.
pub(crate) async fn dispatch_unbound(action: &str, params: Value) -> Result<Value, ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload(SERVICE, ACTIONS));
    }
    if action == "schema" {
        let requested = params
            .get("action")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| missing("action"))?;
        return crate::dispatch::helpers::action_schema(ACTIONS, requested);
    }
    Err(denied())
}

#[derive(Clone)]
pub(crate) struct DevContainerDispatchContext {
    pub access_runtime: Arc<crate::access::AccessRuntime>,
    pub identity: VerifiedIdentity,
    pub ceiling: crate::access::AuthorityCeiling,
}

pub(crate) async fn dispatch(
    context: DevContainerDispatchContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if action == "help" {
        return Ok(crate::dispatch::helpers::help_payload(SERVICE, ACTIONS));
    }
    if action == "schema" {
        return crate::dispatch::helpers::action_schema(
            ACTIONS,
            params
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    }
    if !ACTIONS.iter().any(|spec| spec.name == action) {
        return Err(unknown_action(action));
    }
    let store = context
        .access_runtime
        .store()
        .await
        .map_err(|error| map_runtime_error(SERVICE, error))?;
    match action {
        "dev_containers.list" => list(&context, &store, action, &params).await,
        "dev_containers.create" => create_instance(&context, &store, action, &params).await,
        _ => lifecycle(&context, &store, action, &params).await,
    }
}

async fn list(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let limit = match params.get("limit") {
        None | Some(Value::Null) => Some(100),
        Some(Value::String(value)) => value.parse::<usize>().ok(),
        Some(Value::Number(value)) => value.as_u64().and_then(|value| usize::try_from(value).ok()),
        Some(_) => None,
    }
    .filter(|value| (1..=100).contains(value))
    .ok_or_else(|| invalid("limit"))?;
    let cursor = match params.get("cursor") {
        None | Some(Value::Null) => "",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => return Err(invalid("cursor")),
    };
    // The probe owner is a placeholder resource; the store re-evaluates every
    // returned record against the caller's real authority inside one
    // transaction, so the placeholder never grants visibility by itself.
    let probe_owner = OwnerScope::Installation(
        InstallationId::new("authorized-list-probe").map_err(|_| unavailable())?,
    );
    let now = now_millis()?;
    let request = authority_request(context, action, probe_owner, "authorized-list-probe", now)
        .map_err(store_error)?;
    let inventory = store
        .list_authorized_dev_containers(cursor.to_owned(), limit, request)
        .await
        .map_err(store_error)?;
    let next_cursor = inventory.last().map(|record| record.instance_id.clone());
    let visible = inventory.iter().map(record_json).collect::<Vec<_>>();
    Ok(serde_json::json!({"instances":visible,"next_cursor":next_cursor}))
}

async fn create_instance(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let instance_id = required_str(params, "instance_id")?.to_owned();
    let template_id = required_str(params, "template_id")?.to_owned();
    let owner_id = required_str(params, "owner_id")?;
    let kind = parse_owner_kind(params.get("owner_kind"))?;
    let owner = owner_scope(kind, owner_id).ok_or_else(|| invalid("owner_id"))?;
    let secrets = match params.get("secret_references") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("secret_references"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(invalid("secret_references")),
    };
    let now = now_millis()?;
    let request = authority_request(context, action, owner.clone(), &instance_id, now)
        .map_err(store_error)?;
    let (lease, created) = crate::access::authorize_and_create_approved_for_store(
        store,
        request,
        owner,
        instance_id.clone(),
        template_id,
        secrets,
        context.identity.safe_fingerprint(),
        format!("create-{now}"),
        seconds(now)?,
    )
    .await
    .map_err(|error| ledger_error(&instance_id, &error))?;
    // The durable admission write above is an asynchronous boundary. Fetch
    // current epochs immediately before the engine call so a membership or
    // policy revocation cannot reuse the earlier authorization snapshot.
    let epochs = crate::access::refresh_authority_epochs(
        store,
        context.identity.clone(),
        created.instance.owner().clone(),
        required_capability(action, created.instance.owner().kind()).ok_or_else(denied)?,
    )
    .await
    .map_err(store_error)?;
    create(
        context.access_runtime.dev_container_runtime().as_ref(),
        &lease,
        &epochs,
        now_millis()?,
        &created.template,
        EngineCreateRequest {
            handle: EngineHandle {
                instance_id: created.instance.id().clone(),
                lifecycle_nonce: created.instance.lifecycle_nonce().clone(),
            },
            image_digest: created.instance.image().as_str().into(),
            cpu_millis: created.resources.cpu_millis,
            memory_bytes: created.resources.memory_bytes,
            disk_bytes: created.resources.disk_bytes,
            lifetime_seconds: created.resources.lifetime_seconds,
            host_capabilities: BTreeSet::new(),
        },
    )
    .await
    .map_err(|error| runtime_error(&instance_id, error))?;
    Ok(
        serde_json::json!({"instance_id":instance_id,"desired_state":"running","observed_state":"pending"}),
    )
}

async fn lifecycle(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let instance_id = required_str(params, "instance_id")?;
    let desired = match action {
        "dev_containers.start" => Some(DesiredState::Running),
        "dev_containers.stop" => Some(DesiredState::Stopped),
        "dev_containers.destroy" => Some(DesiredState::Deleted),
        "dev_containers.reconcile" => None,
        _ => return Err(unknown_action(action)),
    };
    // Lookup failures and "not yours" collapse to the same denial.
    let record = lookup_record(store, instance_id)
        .await?
        .ok_or_else(denied)?;
    let owner = stored_owner_scope(&record)?;
    let capability = required_capability(action, owner.kind()).ok_or_else(denied)?;
    let now = now_millis()?;
    let lease = match desired {
        // Authorization and the desired-state write share one immediate
        // transaction: the persisted owner and lifecycle nonce are re-read
        // under the write lock and compared with the lease binding, so a row
        // whose owner or lifecycle moved since the read above is refused
        // instead of being mutated under a stale authorization.
        Some(desired) => crate::access::authorize_and_set_dev_container_desired_state(
            store,
            authority_request(context, action, owner.clone(), instance_id, now)
                .map_err(store_error)?,
            record.instance_id.clone(),
            record.lifecycle_nonce.clone(),
            desired,
            format!("{}:{now}", context.identity.safe_fingerprint()),
            seconds(now)?,
        )
        .await
        .map_err(store_error)?,
        None => {
            authorize(context, store, action, owner.clone(), instance_id)
                .await
                .map_err(store_error)?
                .0
        }
    };
    let intent = match desired.unwrap_or(record.desired_state) {
        DesiredState::Running => DurableIntent::Running,
        DesiredState::Stopped => DurableIntent::Stopped,
        DesiredState::Deleted => DurableIntent::Deleted,
    };
    let handle = EngineHandle {
        instance_id: DevContainerId::new(record.instance_id.clone())
            .map_err(|_| stored_vocabulary_error(instance_id, "instance_id"))?,
        lifecycle_nonce: LifecycleNonce::new(record.lifecycle_nonce.clone())
            .map_err(|_| stored_vocabulary_error(instance_id, "lifecycle_nonce"))?,
    };
    // Refresh after every asynchronous engine boundary, including inspection.
    let result = reconcile(
        context.access_runtime.dev_container_runtime().as_ref(),
        &lease,
        || async {
            let epochs = crate::access::refresh_authority_epochs(
                store,
                context.identity.clone(),
                owner.clone(),
                capability,
            )
            .await
            .map_err(|error| {
                let mapped = store_error(error);
                if mapped.kind() == "forbidden" {
                    RuntimeError::Authority(
                        labby_runtime::authority::AuthorityLeaseError::AuthorityChanged,
                    )
                } else {
                    RuntimeError::AuthorityUnavailable
                }
            })?;
            let now = now_millis().map_err(|_| RuntimeError::AuthorityUnavailable)?;
            Ok((epochs, now))
        },
        &handle,
        intent,
    )
    .await
    .map_err(|error| runtime_error(instance_id, error))?;
    if result == RecoveryAction::MarkFailed {
        // The engine no longer knows the instance: persist the Failed
        // observation so the durable ledger stops describing it as live.
        tracing::warn!(
            service = SERVICE,
            instance_id,
            recovery_action = "mark_failed",
            "Dev Container is missing from the engine; recording the Failed observation"
        );
        crate::access::set_observed_for_store(
            store,
            record.instance_id.clone(),
            record.lifecycle_nonce.clone(),
            ObservedState::Failed,
            format!("observed-failed-{}-{now}", record.instance_id),
            seconds(now)?,
        )
        .await
        .map_err(|error| ledger_error(instance_id, &error))?;
    }
    Ok(
        serde_json::json!({"instance_id":instance_id,"recovery_action":format!("{result:?}").to_ascii_lowercase()}),
    )
}

/// Resolve one durable record by identifier.
///
/// Storage failures are outages; an absent row is `Ok(None)` so the caller
/// can collapse it into the non-enumerating denial.
async fn lookup_record(
    store: &crate::access::AccessStore,
    instance_id: &str,
) -> Result<Option<crate::access::RecoveryRecord>, ToolError> {
    crate::access::lookup_dev_container_for_store(store, instance_id.to_owned())
        .await
        .map_err(|error| ledger_error(instance_id, &error))
}

async fn authorize(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    owner: OwnerScope,
    id: &str,
) -> Result<
    (
        labby_runtime::authority::AuthorityLease,
        labby_runtime::authority::AuthorityEpochVector,
    ),
    crate::access::AccessStoreError,
> {
    let capability = required_capability(action, owner.kind())
        .ok_or(crate::access::AccessStoreError::NotAuthorized)?;
    let now = now_millis().map_err(|_| crate::access::AccessStoreError::NotAuthorized)?;
    let lease = crate::access::authorize_action(
        store,
        authority_request(context, action, owner.clone(), id, now)?,
    )
    .await?;
    let epochs =
        crate::access::refresh_authority_epochs(store, context.identity.clone(), owner, capability)
            .await?;
    Ok((lease, epochs))
}

fn authority_request(
    context: &DevContainerDispatchContext,
    action: &str,
    owner: OwnerScope,
    id: &str,
    now: u64,
) -> Result<crate::access::AuthorityRequest, crate::access::AccessStoreError> {
    let capability = required_capability(action, owner.kind())
        .ok_or(crate::access::AccessStoreError::NotAuthorized)?;
    let action_ref = ActionRef::new(SERVICE, action)
        .map_err(|_| crate::access::AccessStoreError::MalformedVocabulary)?;
    let resource = ResourceRef::new(
        owner.clone(),
        ResourceFamily::DevContainer,
        ResourceId::new(id).map_err(|_| crate::access::AccessStoreError::MalformedVocabulary)?,
    );
    Ok(crate::access::AuthorityRequest::new(
        context.identity.clone(),
        crate::access::ActionAuthoritySpec::SCHEMA_VERSION,
        action_ref.clone(),
        resource,
        context.ceiling.clone(),
        None,
        now,
        vec![labby_runtime::authority::AuthoritySafeBoundary::BeforeExternalEffect],
        vec![crate::access::ActionAuthoritySpec::new(
            action_ref,
            ResourceFamily::DevContainer,
            capability,
        )],
    ))
}

/// Build an owner scope from caller- or store-supplied vocabulary. `None`
/// carries no detail; callers decide whether a failure is caller-fixable
/// (request parameters) or a stored-vocabulary integrity problem.
fn owner_scope(kind: OwnerKind, id: &str) -> Option<OwnerScope> {
    Some(match kind {
        OwnerKind::Installation => OwnerScope::Installation(InstallationId::new(id).ok()?),
        OwnerKind::Team => OwnerScope::Team(TeamId::new(id).ok()?),
        OwnerKind::Project => OwnerScope::Project(ProjectId::new(id).ok()?),
        OwnerKind::Personal => OwnerScope::Personal(PrincipalId::new(id).ok()?),
    })
}

fn stored_owner_scope(record: &crate::access::RecoveryRecord) -> Result<OwnerScope, ToolError> {
    owner_scope(record.owner_kind, &record.owner_id)
        .ok_or_else(|| stored_vocabulary_error(&record.instance_id, "owner_id"))
}

fn record_json(record: &crate::access::RecoveryRecord) -> Value {
    serde_json::json!({"instance_id":record.instance_id,"owner_kind":format!("{:?}",record.owner_kind).to_ascii_lowercase(),"owner_id":record.owner_id,"desired_state":format!("{:?}",record.desired_state).to_ascii_lowercase(),"observed_state":format!("{:?}",record.observed_state).to_ascii_lowercase()})
}

fn required_str<'a>(params: &'a Value, name: &'static str) -> Result<&'a str, ToolError> {
    match params.get(name) {
        None | Some(Value::Null) => Err(missing(name)),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.as_str()),
        Some(_) => Err(invalid(name)),
    }
}

fn parse_owner_kind(value: Option<&Value>) -> Result<OwnerKind, ToolError> {
    match value {
        None | Some(Value::Null) => Err(missing("owner_kind")),
        Some(Value::String(value)) => match value.as_str() {
            "installation" => Ok(OwnerKind::Installation),
            "team" => Ok(OwnerKind::Team),
            "project" => Ok(OwnerKind::Project),
            "personal" => Ok(OwnerKind::Personal),
            _ => Err(invalid("owner_kind")),
        },
        Some(_) => Err(invalid("owner_kind")),
    }
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

fn seconds(now_millis: u64) -> Result<i64, ToolError> {
    i64::try_from(now_millis / 1000).map_err(|_| unavailable())
}

fn store_error(error: crate::access::AccessStoreError) -> ToolError {
    map_store_error(SERVICE, error, denied)
}

/// Map a typed durable-ledger failure surfaced by the `*_for_store` helpers.
///
/// Caller-fixable input problems stay `invalid_param`; templates are not
/// enumerable, so an unknown template reads as an invalid `template_id`;
/// quota exhaustion is the shared `quota_exceeded` kind; every storage
/// failure is logged with its typed cause and instance id, and returned as a
/// fixed-string outage.
fn ledger_error(instance_id: &str, error: &DevContainerLedgerError) -> ToolError {
    match error {
        DevContainerLedgerError::InvalidInput => invalid("params"),
        DevContainerLedgerError::TemplateUnavailable => invalid("template_id"),
        DevContainerLedgerError::QuotaExhausted => ToolError::Sdk {
            sdk_kind: "quota_exceeded".into(),
            message: "Dev Container quota is exhausted for this owner or template".into(),
        },
        DevContainerLedgerError::Storage(DevContainerStorageFailure::NotAuthorized) => denied(),
        DevContainerLedgerError::Storage(failure) => {
            match failure {
                DevContainerStorageFailure::Corrupt
                | DevContainerStorageFailure::IntegrityViolation
                | DevContainerStorageFailure::ForeignKeyViolation => tracing::error!(
                    service = SERVICE,
                    instance_id,
                    cause = %failure,
                    kind = "service_unavailable",
                    "Dev Container ledger integrity failure; operator action required"
                ),
                _ => tracing::warn!(
                    service = SERVICE,
                    instance_id,
                    cause = %failure,
                    kind = "service_unavailable",
                    "Dev Container ledger operation failed"
                ),
            }
            unavailable()
        }
    }
}

/// Map a typed engine/authority failure to the shared envelope. Engine causes
/// are logged with the instance id and never echoed to the caller.
fn runtime_error<E: std::error::Error>(instance_id: &str, error: RuntimeError<E>) -> ToolError {
    match error {
        RuntimeError::Authority(cause) => {
            tracing::warn!(
                service = SERVICE,
                instance_id,
                cause = %cause,
                kind = "forbidden",
                "Dev Container authority lease rejected before external effect"
            );
            denied()
        }
        RuntimeError::AuthorityUnavailable => {
            tracing::warn!(
                service = SERVICE,
                instance_id,
                kind = "service_unavailable",
                "Dev Container authority refresh failed before external effect"
            );
            unavailable()
        }
        RuntimeError::Admission(cause) => match cause {
            DevContainerAdmissionError::QuotaExceeded => ToolError::Sdk {
                sdk_kind: "quota_exceeded".into(),
                message: "Dev Container resource request exceeds the approved template quota"
                    .into(),
            },
            DevContainerAdmissionError::HostCapabilityDenied => ToolError::Forbidden {
                message: "Dev Container requested a host capability not approved by its template"
                    .into(),
                required_scopes: Vec::new(),
            },
            DevContainerAdmissionError::StaleLifecycleNonce => ToolError::Conflict {
                message: "Dev Container lifecycle changed since the request was issued".into(),
                existing_id: instance_id.to_owned(),
            },
            DevContainerAdmissionError::InvalidLifecycleTransition => ToolError::Conflict {
                message: "Dev Container lifecycle transition is invalid".into(),
                existing_id: instance_id.to_owned(),
            },
            // The durable record's image no longer matches the approved
            // template; the template changed underneath the instance.
            DevContainerAdmissionError::ImageDigestMismatch => ToolError::Conflict {
                message: "Dev Container image digest is not the approved template image".into(),
                existing_id: instance_id.to_owned(),
            },
        },
        RuntimeError::Engine(cause) => {
            tracing::warn!(
                service = SERVICE,
                instance_id,
                cause = %cause,
                kind = "service_unavailable",
                "Dev Container engine operation failed"
            );
            unavailable()
        }
    }
}

fn stored_vocabulary_error(instance_id: &str, field: &'static str) -> ToolError {
    tracing::error!(
        service = SERVICE,
        instance_id,
        field,
        kind = "service_unavailable",
        "Dev Container record holds malformed vocabulary; operator action required"
    );
    unavailable()
}

fn unknown_action(action: &str) -> ToolError {
    let mut valid = ACTIONS
        .iter()
        .map(|spec| spec.name.to_owned())
        .collect::<Vec<_>>();
    valid.push("help".into());
    valid.push("schema".into());
    ToolError::UnknownAction {
        message: format!("unknown action: `{action}`"),
        valid,
        hint: None,
    }
}

fn missing(param: &'static str) -> ToolError {
    ToolError::MissingParam {
        message: format!("missing required parameter `{param}`"),
        param: param.into(),
    }
}

fn invalid(param: &'static str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("invalid parameter `{param}`"),
        param: param.into(),
    }
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Dev Container operation is not authorized".into(),
        required_scopes: Vec::new(),
    }
}

fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: "Dev Container runtime is unavailable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
    use labby_auth::Authenticator;
    use labby_runtime::authority::AuthorityLeaseError;
    use labby_runtime::dev_container_runtime::DisabledRuntimeError;

    fn secure_tempdir() -> tempfile::TempDir {
        let directory = tempfile::Builder::new()
            .prefix("labby-dev-containers-")
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

    async fn fixture() -> (tempfile::TempDir, DevContainerDispatchContext) {
        let directory = secure_tempdir();
        let path = directory.path().join("access.db");
        let store = AccessStore::open(path.clone()).await.unwrap();
        let owner = browser("owner");
        store
            .bootstrap_owner(BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap())
            .await
            .unwrap();
        drop(store);
        let runtime = AccessRuntime::initialize(path).await;
        (
            directory,
            DevContainerDispatchContext {
                access_runtime: Arc::new(runtime),
                identity: owner,
                ceiling: crate::access::AuthorityCeiling::trusted_local(),
            },
        )
    }

    fn param_of(error: &ToolError) -> Option<&str> {
        match error {
            ToolError::MissingParam { param, .. } | ToolError::InvalidParam { param, .. } => {
                Some(param.as_str())
            }
            _ => None,
        }
    }

    async fn failure(
        context: &DevContainerDispatchContext,
        action: &str,
        params: Value,
    ) -> ToolError {
        dispatch(context.clone(), action, params)
            .await
            .expect_err("dispatch must fail")
    }

    #[test]
    fn actions_have_exact_fail_closed_capabilities_and_no_raw_host_authority() {
        assert_eq!(
            required_capability("dev_containers.destroy", OwnerKind::Team),
            Some(Capability::ScopeDelete)
        );
        assert_eq!(
            required_capability("dev_containers.unknown", OwnerKind::Team),
            None
        );
        assert_eq!(
            required_capability("dev_containers.create", OwnerKind::Installation),
            Some(Capability::ScopeCreate)
        );
        assert!(
            ACTIONS
                .iter()
                .flat_map(|action| action.params)
                .all(|param| !matches!(
                    param.name,
                    "image" | "privileged" | "host_network" | "devices" | "mounts"
                ))
        );
    }

    #[tokio::test]
    async fn unbound_denial_does_not_enumerate_resources() {
        let missing = dispatch_unbound(
            "dev_containers.start",
            s