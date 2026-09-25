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
use labby_runtime::authority::AuthoritySafeBoundary;
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

pub(crate) mod images;

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
const BASE_TEMPLATE_ID: ParamSpec = ParamSpec {
    name: "base_template_id",
    ty: "string",
    required: true,
    description: "Existing administrator-approved immutable base template",
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
const DEFINITION: ParamSpec = ParamSpec {
    name: "definition",
    ty: "object",
    required: true,
    description: "Bounded image definition using approved catalog identifiers",
};
const EXPECTED_REVISION: ParamSpec = ParamSpec {
    name: "expected_revision",
    ty: "integer",
    required: true,
    description: "Draft revision used for compare-and-swap",
};
const ENVIRONMENT: ParamSpec = ParamSpec {
    name: "environment",
    ty: "object[]",
    required: true,
    description: "Literal values or opaque secret references",
};
const REQUEST_ID: ParamSpec = ParamSpec {
    name: "request_id",
    ty: "string",
    required: true,
    description: "Caller-unique build intent identifier",
};
const BUILD_ID: ParamSpec = ParamSpec {
    name: "build_id",
    ty: "string",
    required: true,
    description: "Opaque image build identifier",
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
        "dev_containers.approved_templates.list",
        "List approved immutable templates available for authorized container creation",
        &[OWNER_KIND, OWNER_ID, CURSOR, LIMIT],
        false,
    ),
    action(
        "dev_containers.templates.list",
        "List image drafts in one authorized owner scope",
        &[OWNER_KIND, OWNER_ID, CURSOR, LIMIT],
        false,
    ),
    action(
        "dev_containers.templates.get",
        "Get one authorized image draft and its environment references",
        &[TEMPLATE_ID],
        false,
    ),
    action(
        "dev_containers.draft.create",
        "Create an image draft from an approved immutable base",
        &[
            TEMPLATE_ID,
            BASE_TEMPLATE_ID,
            OWNER_KIND,
            OWNER_ID,
            DEFINITION,
        ],
        false,
    ),
    action(
        "dev_containers.draft.update",
        "Replace a draft definition at an exact revision",
        &[TEMPLATE_ID, EXPECTED_REVISION, DEFINITION],
        false,
    ),
    action(
        "dev_containers.environment.replace",
        "Replace a draft environment using literals or opaque secret references",
        &[TEMPLATE_ID, EXPECTED_REVISION, ENVIRONMENT],
        false,
    ),
    action(
        "dev_containers.build",
        "Build and publish a new immutable image revision",
        &[TEMPLATE_ID, EXPECTED_REVISION, REQUEST_ID],
        false,
    ),
    action(
        "dev_containers.rebuild",
        "Rebuild and publish a new immutable image revision",
        &[TEMPLATE_ID, EXPECTED_REVISION, REQUEST_ID],
        false,
    ),
    action(
        "dev_containers.build.get",
        "Read bounded image build progress",
        &[BUILD_ID],
        false,
    ),
    action(
        "dev_containers.launch_reference",
        "Return the configured Incus endpoint, project and immutable image fingerprint",
        &[TEMPLATE_ID],
        false,
    ),
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
        "dev_containers.list"
        | "dev_containers.approved_templates.list"
        | "dev_containers.templates.list"
        | "dev_containers.templates.get"
        | "dev_containers.build.get"
        | "dev_containers.launch_reference" => Some(Capability::ScopeRead),
        "dev_containers.create" | "dev_containers.draft.create" => Some(Capability::ScopeCreate),
        "dev_containers.start"
        | "dev_containers.stop"
        | "dev_containers.reconcile"
        | "dev_containers.draft.update"
        | "dev_containers.environment.replace"
        | "dev_containers.build"
        | "dev_containers.rebuild" => Some(Capability::ScopeOperate),
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
        "dev_containers.approved_templates.list" => {
            list_approved_templates(&context, &store, action, &params).await
        }
        "dev_containers.list" => list(&context, &store, action, &params).await,
        "dev_containers.create" => create_instance(&context, &store, action, &params).await,
        action if images::is_image_action(action) => {
            images::dispatch(context, store, action, params).await
        }
        _ => lifecycle(&context, &store, action, &params).await,
    }
}

async fn list_approved_templates(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let kind = parse_owner_kind(params.get("owner_kind"))?;
    let owner_id = required_str(params, "owner_id")?;
    let owner = owner_scope(kind, owner_id).ok_or_else(|| invalid("owner_id"))?;
    authorize(context, store, action, owner, "approved-template-list")
        .await
        .map_err(store_error)?;
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
    let templates = store
        .list_approved_dev_container_templates(cursor.to_owned(), limit)
        .await
        .map_err(store_error)?;
    let next_cursor = templates.last().cloned();
    Ok(serde_json::json!({"templates": templates, "next_cursor": next_cursor}))
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
    let resolved_config = crate::config::resolved_dev_container_config();
    let (lease, created) = crate::access::authorize_and_create_approved_for_store(
        store,
        request,
        owner,
        instance_id.clone(),
        template_id,
        secrets,
        resolved_config.catalog.generation().to_owned(),
        resolved_config.catalog.digest().to_owned(),
        resolved_config.secret_values,
        context.identity.safe_fingerprint(),
        format!("create-{now}"),
        seconds(now)?,
    )
    .await
    .map_err(|error| ledger_error(&instance_id, &error))?;
    persist_observation(
        context,
        store,
        &lease,
        created.instance.owner(),
        required_capability(action, created.instance.owner().kind()).ok_or_else(denied)?,
        &instance_id,
        created.instance.lifecycle_nonce().as_str(),
        ObservedState::Starting,
        "create-starting",
    )
    .await?;
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
            launch_manifest_digest: created.launch_manifest.manifest_digest.clone(),
            profiles: created.launch_manifest.profiles.clone(),
            environment: created.environment.clone(),
        },
    )
    .await
    .map_err(|error| runtime_error(&instance_id, error))?;
    let handle = EngineHandle {
        instance_id: created.instance.id().clone(),
        lifecycle_nonce: created.instance.lifecycle_nonce().clone(),
    };
    let engine_state = inspect_authorized(
        context,
        store,
        &lease,
        created.instance.owner(),
        required_capability(action, created.instance.owner().kind()).ok_or_else(denied)?,
        &instance_id,
        &handle,
    )
    .await?;
    let observed = if engine_state == EngineState::Running {
        ObservedState::Running
    } else {
        ObservedState::Failed
    };
    persist_observation(
        context,
        store,
        &lease,
        created.instance.owner(),
        required_capability(action, created.instance.owner().kind()).ok_or_else(denied)?,
        &instance_id,
        created.instance.lifecycle_nonce().as_str(),
        observed,
        "create-observed",
    )
    .await?;
    if observed != ObservedState::Running {
        return Err(unavailable());
    }
    Ok(serde_json::json!({
        "instance_id": instance_id,
        "desired_state": "running",
        "observed_state": "running"
    }))
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
    let mut observed = record.observed_state;
    if let Some(next) = transitional_observation(intent, observed) {
        persist_observation(
            context,
            store,
            &lease,
            &owner,
            capability,
            instance_id,
            &record.lifecycle_nonce,
            next,
            "lifecycle-transition",
        )
        .await?;
        observed = next;
    }
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
    let engine_state = inspect_authorized(
        context,
        store,
        &lease,
        &owner,
        capability,
        instance_id,
        &handle,
    )
    .await?;
    let final_observed = match (intent, engine_state) {
        (DurableIntent::Running, EngineState::Running) => ObservedState::Running,
        (DurableIntent::Stopped, EngineState::Stopped) => ObservedState::Stopped,
        (DurableIntent::Deleted, EngineState::Missing) => ObservedState::Deleted,
        _ => ObservedState::Failed,
    };
    if final_observed != observed {
        persist_observation(
            context,
            store,
            &lease,
            &owner,
            capability,
            instance_id,
            &record.lifecycle_nonce,
            final_observed,
            "lifecycle-observed",
        )
        .await?;
    }
    Ok(serde_json::json!({
        "instance_id": instance_id,
        "recovery_action": format!("{result:?}").to_ascii_lowercase(),
        "observed_state": format!("{final_observed:?}").to_ascii_lowercase()
    }))
}

fn transitional_observation(intent: DurableIntent, prior: ObservedState) -> Option<ObservedState> {
    match (intent, prior) {
        (DurableIntent::Running, ObservedState::Pending | ObservedState::Stopped) => {
            Some(ObservedState::Starting)
        }
        (
            DurableIntent::Stopped | DurableIntent::Deleted,
            ObservedState::Pending | ObservedState::Starting | ObservedState::Running,
        ) => Some(ObservedState::Stopping),
        _ => None,
    }
}

async fn inspect_authorized(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    lease: &labby_runtime::authority::AuthorityLease,
    owner: &OwnerScope,
    capability: Capability,
    instance_id: &str,
    handle: &EngineHandle,
) -> Result<EngineState, ToolError> {
    let epochs = crate::access::refresh_authority_epochs(
        store,
        context.identity.clone(),
        owner.clone(),
        capability,
    )
    .await
    .map_err(store_error)?;
    lease
        .validate_at(
            AuthoritySafeBoundary::BeforeExternalEffect,
            now_millis()?,
            &epochs,
        )
        .map_err(|error| {
            runtime_error::<crate::access::DevContainerEngineError>(
                instance_id,
                RuntimeError::Authority(error),
            )
        })?;
    context
        .access_runtime
        .dev_container_runtime()
        .inspect(handle)
        .await
        .map_err(|error| runtime_error(instance_id, RuntimeError::Engine(error)))
}

#[allow(clippy::too_many_arguments)]
async fn persist_observation(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    lease: &labby_runtime::authority::AuthorityLease,
    owner: &OwnerScope,
    capability: Capability,
    instance_id: &str,
    lifecycle_nonce: &str,
    next: ObservedState,
    event: &str,
) -> Result<(), ToolError> {
    let epochs = crate::access::refresh_authority_epochs(
        store,
        context.identity.clone(),
        owner.clone(),
        capability,
    )
    .await
    .map_err(store_error)?;
    let now = now_millis()?;
    lease
        .validate_at(AuthoritySafeBoundary::BeforeCommit, now, &epochs)
        .map_err(|error| {
            runtime_error::<crate::access::DevContainerEngineError>(
                instance_id,
                RuntimeError::Authority(error),
            )
        })?;
    let next_name = format!("{next:?}").to_ascii_lowercase();
    crate::access::set_observed_for_store(
        store,
        instance_id.to_owned(),
        lifecycle_nonce.to_owned(),
        next,
        format!("observed-{event}-{instance_id}-{next_name}-{now}"),
        seconds(now)?,
    )
    .await
    .map_err(|error| ledger_error(instance_id, &error))
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
        vec![
            AuthoritySafeBoundary::BeforeExternalEffect,
            AuthoritySafeBoundary::BeforeCommit,
        ],
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
    use crate::access::DevContainerEngineError;
    use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
    use labby_auth::Authenticator;
    use labby_runtime::authority::AuthorityLeaseError;

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
            serde_json::json!({"instance_id":"missing"}),
        )
        .await
        .unwrap_err()
        .to_string();
        let existing = dispatch_unbound(
            "dev_containers.start",
            serde_json::json!({"instance_id":"known"}),
        )
        .await
        .unwrap_err()
        .to_string();
        assert_eq!(missing, existing);
    }

    #[tokio::test]
    async fn create_parameter_problems_are_caller_fixable() {
        let (_directory, context) = fixture().await;
        let base = serde_json::json!({
            "instance_id": "dc-1",
            "template_id": "tpl",
            "owner_kind": "personal",
            "owner_id": "owner",
        });
        let cases: [(&str, Value, &str, &str); 7] = [
            ("instance_id", Value::Null, "missing_param", "instance_id"),
            (
                "instance_id",
                Value::String("   ".into()),
                "invalid_param",
                "instance_id",
            ),
            ("template_id", Value::Null, "missing_param", "template_id"),
            (
                "owner_id",
                Value::Number(7.into()),
                "invalid_param",
                "owner_id",
            ),
            ("owner_kind", Value::Null, "missing_param", "owner_kind"),
            (
                "owner_kind",
                Value::String("bogus".into()),
                "invalid_param",
                "owner_kind",
            ),
            (
                "secret_references",
                serde_json::json!([1]),
                "invalid_param",
                "secret_references",
            ),
        ];
        for (field, value, kind, param) in cases {
            let mut params = base.clone();
            params[field] = value;
            let error = failure(&context, "dev_containers.create", params).await;
            assert_eq!(error.kind(), kind, "{field}");
            assert_eq!(param_of(&error), Some(param), "{field}");
        }
        let mut params = base.clone();
        params["owner_id"] = Value::String("a\u{0}b".into());
        let error = failure(&context, "dev_containers.create", params).await;
        assert_eq!(error.kind(), "invalid_param");
        assert_eq!(param_of(&error), Some("owner_id"));
    }

    #[tokio::test]
    async fn list_parameter_problems_are_caller_fixable() {
        let (_directory, context) = fixture().await;
        for (params, param) in [
            (serde_json::json!({"limit":"0"}), "limit"),
            (serde_json::json!({"limit":"101"}), "limit"),
            (serde_json::json!({"limit":"abc"}), "limit"),
            (serde_json::json!({"limit":true}), "limit"),
            (serde_json::json!({"cursor":5}), "cursor"),
        ] {
            let error = failure(&context, "dev_containers.list", params).await;
            assert_eq!(error.kind(), "invalid_param", "{param}");
            assert_eq!(param_of(&error), Some(param));
        }
    }

    #[tokio::test]
    async fn lifecycle_parameter_problems_are_caller_fixable() {
        let (_directory, context) = fixture().await;
        for action in [
            "dev_containers.start",
            "dev_containers.stop",
            "dev_containers.destroy",
            "dev_containers.reconcile",
        ] {
            let error = failure(&context, action, serde_json::json!({})).await;
            assert_eq!(error.kind(), "missing_param", "{action}");
            assert_eq!(param_of(&error), Some("instance_id"));
            let error = failure(&context, action, serde_json::json!({"instance_id":""})).await;
            assert_eq!(error.kind(), "invalid_param", "{action}");
            assert_eq!(param_of(&error), Some("instance_id"));
        }
    }

    #[tokio::test]
    async fn unknown_actions_are_rejected_before_store_access() {
        let (_directory, context) = fixture().await;
        let error = failure(
            &context,
            "dev_containers.bogus",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await;
        assert_eq!(error.kind(), "unknown_action");
    }

    #[tokio::test]
    async fn bound_lookup_and_authorization_failures_share_one_denial() {
        let (_directory, owner) = fixture().await;
        // A caller the store has never seen cannot be authorized for anything.
        let context = DevContainerDispatchContext {
            identity: browser("stranger"),
            ..owner
        };
        let absent = failure(
            &context,
            "dev_containers.start",
            serde_json::json!({"instance_id":"absent"}),
        )
        .await;
        assert_eq!(absent.kind(), "forbidden");
        let unauthorized_create = failure(
            &context,
            "dev_containers.create",
            serde_json::json!({
                "instance_id": "dc-1",
                "template_id": "tpl",
                "owner_kind": "team",
                "owner_id": "not-a-member",
            }),
        )
        .await;
        assert_eq!(unauthorized_create.kind(), "forbidden");
        assert_eq!(absent.to_string(), unauthorized_create.to_string());
        assert_eq!(absent.to_string(), denied().to_string());
    }

    #[test]
    fn runtime_errors_map_to_typed_kinds_without_leaking_causes() {
        type E = RuntimeError<DevContainerEngineError>;
        let cases: [(E, &str); 7] = [
            (E::Authority(AuthorityLeaseError::Expired), "forbidden"),
            (
                E::Authority(AuthorityLeaseError::AuthorityChanged),
                "forbidden",
            ),
            (
                E::Admission(DevContainerAdmissionError::QuotaExceeded),
                "quota_exceeded",
            ),
            (
                E::Admission(DevContainerAdmissionError::HostCapabilityDenied),
                "forbidden",
            ),
            (
                E::Admission(DevContainerAdmissionError::StaleLifecycleNonce),
                "conflict",
            ),
            (
                E::Admission(DevContainerAdmissionError::ImageDigestMismatch),
                "conflict",
            ),
            (
                E::Engine(DevContainerEngineError::Unconfigured),
                "service_unavailable",
            ),
        ];
        for (error, kind) in cases {
            let mapped = runtime_error("dc-1", error);
            assert_eq!(mapped.kind(), kind);
            let text = mapped.to_string();
            assert!(!text.contains("disabled"), "{text}");
            assert!(!text.contains("expired"), "{text}");
        }
        assert_eq!(
            runtime_error("dc-1", E::Authority(AuthorityLeaseError::Expired)).to_string(),
            denied().to_string()
        );
        assert!(matches!(
            runtime_error(
                "dc-1",
                E::Admission(DevContainerAdmissionError::InvalidLifecycleTransition)
            ),
            ToolError::Conflict { existing_id, .. } if existing_id == "dc-1"
        ));
    }

    #[test]
    fn ledger_and_store_failures_are_fixed_string_outages_or_denials() {
        for failure in [
            DevContainerStorageFailure::Locked,
            DevContainerStorageFailure::Corrupt,
            DevContainerStorageFailure::Unavailable,
        ] {
            let mapped = ledger_error("dc-1", &DevContainerLedgerError::Storage(failure));
            assert_eq!(mapped.kind(), "service_unavailable");
            assert!(!mapped.to_string().contains("sqlite"));
        }
        assert_eq!(
            ledger_error("dc-1", &DevContainerLedgerError::QuotaExhausted).kind(),
            "quota_exceeded"
        );
        assert_eq!(
            param_of(&ledger_error(
                "dc-1",
                &DevContainerLedgerError::TemplateUnavailable
            )),
            Some("template_id")
        );
        assert_eq!(
            ledger_error("dc-1", &DevContainerLedgerError::InvalidInput).kind(),
            "invalid_param"
        );
        assert_eq!(
            store_error(crate::access::AccessStoreError::NotAuthorized).to_string(),
            denied().to_string()
        );
        assert_eq!(
            store_error(crate::access::AccessStoreError::Locked).kind(),
            "service_unavailable"
        );
    }

    /// Engine stub whose `inspect` answer is fixed so reconcile outcomes are
    /// deterministic per test.
    struct FixedEngine {
        state: std::sync::Mutex<EngineState>,
        inspections: std::sync::atomic::AtomicUsize,
    }
    impl ContainerRuntime for FixedEngine {
        type Error = DevContainerEngineError;
        fn create<'a>(
            &'a self,
            _: EngineCreateRequest,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn inspect<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<EngineState, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                if self
                    .inspections
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    == 0
                {
                    Ok(EngineState::Running)
                } else {
                    Ok(*self.state.lock().unwrap())
                }
            })
        }
        fn start<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                *self.state.lock().unwrap() = EngineState::Running;
                Ok(())
            })
        }
        fn stop<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                *self.state.lock().unwrap() = EngineState::Stopped;
                Ok(())
            })
        }
        fn destroy<'a>(
            &'a self,
            _: &'a EngineHandle,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send + 'a>> {
            Box::pin(async move {
                *self.state.lock().unwrap() = EngineState::Missing;
                Ok(())
            })
        }
    }

    /// Bootstrap owner plus an approved template and a personal quota for
    /// the bootstrap principal, with the engine answer fixed to `state`.
    async fn engine_fixture(
        state: EngineState,
    ) -> (
        tempfile::TempDir,
        DevContainerDispatchContext,
        tokio::sync::OwnedMutexGuard<()>,
    ) {
        let config_guard = crate::config::dev_container_config_test_guard().await;
        let (directory, context) = fixture().await;
        crate::config::install_resolved_preferences(&crate::config::LabConfig::default());
        let resolved = crate::config::resolved_dev_container_config();
        let connection = rusqlite::Connection::open(directory.path().join("access.db")).unwrap();
        let image_digest = format!("sha256:{}", "a".repeat(64));
        let mut manifest = crate::access::DevContainerLaunchManifest {
            manifest_digest: String::new(),
            template_id: "tpl".to_owned(),
            source_build_id: "fixture-build".to_owned(),
            source_revision: 1,
            source_digest: format!("sha256:{}", "b".repeat(64)),
            image_digest: image_digest.clone(),
            catalog_generation: resolved.catalog.generation().to_owned(),
            catalog_digest: resolved.catalog.digest().to_owned(),
            network_mask: 0,
            profiles: Vec::new(),
            environment: Vec::new(),
        };
        manifest.manifest_digest = manifest.computed_digest().unwrap();
        connection.execute("INSERT INTO dev_container_templates(template_id,image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,host_capabilities_json,status,policy_epoch,created_at,updated_at) VALUES('tpl',?1,4,1000,1073741824,1073741824,3600,'[]','approved',1,1,1)", [&image_digest]).unwrap();
        connection.execute("INSERT INTO dev_container_template_drafts(template_id,owner_kind,owner_id,base_template_id,definition_json,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,revision,authority_fingerprint,created_at,updated_at) VALUES('fixture-template','personal','bootstrap-owner','tpl','{}',4,1000,1073741824,1073741824,3600,1,'fixture-authority',1,1)", []).unwrap();
        connection.execute("INSERT INTO dev_container_image_builds(build_id,request_id,actor_principal_id,identity_ref_json,ceiling_json,template_id,source_revision,source_digest,source_snapshot_json,lifecycle_nonce,builder_instance_name,request_kind,state,step,progress,output_image_digest,authority_fingerprint,created_at,updated_at,completed_at) VALUES('fixture-build','fixture-request','bootstrap-owner','{}','{}','fixture-template',1,?1,'{}','00000000000000000000000000000000','fixture-builder','build','succeeded','complete',100,?2,'fixture-authority',1,1,1)", rusqlite::params![manifest.source_digest, image_digest]).unwrap();
        connection.execute("INSERT INTO dev_container_launch_manifests(manifest_digest,template_id,source_build_id,source_revision,source_digest,image_digest,catalog_generation,catalog_digest,network_mask,profiles_json,environment_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'[]','[]',1)", rusqlite::params![manifest.manifest_digest,manifest.template_id,manifest.source_build_id,manifest.source_revision,manifest.source_digest,manifest.image_digest,manifest.catalog_generation,manifest.catalog_digest,manifest.network_mask]).unwrap();
        connection.execute("UPDATE dev_container_templates SET launch_manifest_digest=?1 WHERE template_id='tpl'", [&manifest.manifest_digest]).unwrap();
        connection.execute("INSERT INTO dev_container_owner_quotas(owner_kind,owner_id,max_active_instances,policy_epoch,updated_at) VALUES('personal','bootstrap-owner',4,1,1)", []).unwrap();
        drop(connection);
        let runtime = Arc::try_unwrap(context.access_runtime)
            .ok()
            .expect("fixture holds the only runtime handle")
            .with_dev_container_runtime(Arc::new(FixedEngine {
                state: std::sync::Mutex::new(state),
                inspections: std::sync::atomic::AtomicUsize::new(0),
            }));
        (
            directory,
            DevContainerDispatchContext {
                access_runtime: Arc::new(runtime),
                ..context
            },
            config_guard,
        )
    }

    fn open_ledger(directory: &tempfile::TempDir) -> rusqlite::Connection {
        rusqlite::Connection::open(directory.path().join("access.db")).unwrap()
    }

    fn ledger_states(directory: &tempfile::TempDir, instance_id: &str) -> (String, String, String) {
        open_ledger(directory)
            .query_row(
                "SELECT desired_state,observed_state,lifecycle_nonce FROM dev_container_instances WHERE instance_id=?1",
                [instance_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    async fn create_dc(context: &DevContainerDispatchContext, id: &str) {
        dispatch(
            context.clone(),
            "dev_containers.create",
            serde_json::json!({
                "instance_id": id,
                "template_id": "tpl",
                "owner_kind": "personal",
                "owner_id": "bootstrap-owner",
            }),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn lifecycle_writes_desired_state_inside_the_authorizing_transaction() {
        let (directory, context, _config_guard) = engine_fixture(EngineState::Running).await;
        create_dc(&context, "dc-1").await;
        let stopped = dispatch(
            context.clone(),
            "dev_containers.stop",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await
        .unwrap();
        assert_eq!(stopped["recovery_action"], "stop");
        assert_eq!(stopped["observed_state"], "stopped");
        let (desired, observed, _) = ledger_states(&directory, "dc-1");
        assert_eq!(desired, "stopped");
        assert_eq!(observed, "stopped");
    }

    #[tokio::test]
    async fn lifecycle_commits_verified_stop_start_and_destroy_observations() {
        let (directory, context, _config_guard) = engine_fixture(EngineState::Running).await;
        create_dc(&context, "dc-1").await;

        let stopped = dispatch(
            context.clone(),
            "dev_containers.stop",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await
        .unwrap();
        assert_eq!(stopped["recovery_action"], "stop");
        assert_eq!(stopped["observed_state"], "stopped");

        let started = dispatch(
            context.clone(),
            "dev_containers.start",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await
        .unwrap();
        assert_eq!(started["recovery_action"], "start");
        assert_eq!(started["observed_state"], "running");

        let destroyed = dispatch(
            context,
            "dev_containers.destroy",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await
        .unwrap();
        assert_eq!(destroyed["recovery_action"], "destroy");
        assert_eq!(destroyed["observed_state"], "deleted");
        let (desired, observed, _) = ledger_states(&directory, "dc-1");
        assert_eq!(desired, "deleted");
        assert_eq!(observed, "deleted");
    }

    #[tokio::test]
    async fn stale_lifecycle_nonce_is_refused_inside_the_write_transaction() {
        let (directory, context, _config_guard) = engine_fixture(EngineState::Running).await;
        create_dc(&context, "dc-1").await;
        // The row's lifecycle moved after the caller's read: the persisted
        // nonce no longer matches the one the lease was bound to, and the
        // record in the ledger is what the transaction compares against.
        // Exercise the store path directly with the stale nonce.
        let store = context.access_runtime.store().await.unwrap();
        let now = now_millis().unwrap();
        let request = authority_request(
            &context,
            "dev_containers.stop",
            OwnerScope::Personal(PrincipalId::new("bootstrap-owner").unwrap()),
            "dc-1",
            now,
        )
        .unwrap();
        let error = crate::access::authorize_and_set_dev_container_desired_state(
            &store,
            request,
            "dc-1".into(),
            "00000000000000000000000000000000".into(),
            DesiredState::Stopped,
            "actor".into(),
            seconds(now).unwrap(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            crate::access::AccessStoreError::NotAuthorized
        ));
        let (desired, _, _) = ledger_states(&directory, "dc-1");
        assert_eq!(desired, "running", "stale nonce must not mutate the row");
    }

    #[tokio::test]
    async fn mismatched_owner_is_refused_inside_the_write_transaction() {
        let (directory, context, _config_guard) = engine_fixture(EngineState::Running).await;
        create_dc(&context, "dc-1").await;
        let (_, _, nonce) = ledger_states(&directory, "dc-1");
        // Re-home the row after the caller's read: the lease is bound to the
        // personal owner, and the persisted owner is now a Team the caller has
        // no membership in, so neither the write transaction nor a fresh
        // dispatch may act on it.
        open_ledger(&directory)
            .execute(
                "UPDATE dev_container_instances SET owner_kind='team',owner_id='foreign-team' WHERE instance_id='dc-1'",
                [],
            )
            .unwrap();
        let store = context.access_runtime.store().await.unwrap();
        let now = now_millis().unwrap();
        let request = authority_request(
            &context,
            "dev_containers.stop",
            OwnerScope::Personal(PrincipalId::new("bootstrap-owner").unwrap()),
            "dc-1",
            now,
        )
        .unwrap();
        let error = crate::access::authorize_and_set_dev_container_desired_state(
            &store,
            request,
            "dc-1".into(),
            nonce,
            DesiredState::Stopped,
            "actor".into(),
            seconds(now).unwrap(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            crate::access::AccessStoreError::NotAuthorized
        ));
        let (desired, _, _) = ledger_states(&directory, "dc-1");
        assert_eq!(desired, "running", "owner mismatch must not mutate the row");
        // Through dispatch the same situation is the non-enumerating denial.
        let error = failure(
            &context,
            "dev_containers.stop",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await;
        assert_eq!(error.kind(), "forbidden");
    }

    #[tokio::test]
    async fn mark_failed_persists_the_failed_observation() {
        let (directory, context, _config_guard) = engine_fixture(EngineState::Missing).await;
        create_dc(&context, "dc-1").await;
        let result = dispatch(
            context.clone(),
            "dev_containers.reconcile",
            serde_json::json!({"instance_id":"dc-1"}),
        )
        .await
        .unwrap();
        assert_eq!(result["recovery_action"], "markfailed");
        let (desired, observed, _) = ledger_states(&directory, "dc-1");
        assert_eq!(desired, "running");
        assert_eq!(observed, "failed");
    }
}
