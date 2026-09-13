//! Bounded draft, environment, and immutable image build actions.

use labby_primitives::access::OwnerScope;
use labby_primitives::dev_container::ImageDigest;
use labby_runtime::dev_container_image_runtime::{
    ApprovedProvisionCatalog, ContainerImageRuntime, ImageBuildEffect, ImageBuildHandle,
    ImageBuildOperation, ImageBuildRequest, ProvisionCommand,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    DevContainerDispatchContext, ToolError, authority_request, denied, invalid, now_millis,
    owner_scope, parse_owner_kind, required_str, store_error,
};

const MAX_DEFINITION_BYTES: usize = 64 * 1024;
const MAX_ENVIRONMENT_ENTRIES: usize = 128;
const MAX_PROVISION_COMMANDS: usize = 48;

pub(super) fn is_image_action(action: &str) -> bool {
    matches!(
        action,
        "dev_containers.templates.list"
            | "dev_containers.templates.get"
            | "dev_containers.draft.create"
            | "dev_containers.draft.update"
            | "dev_containers.environment.replace"
            | "dev_containers.build"
            | "dev_containers.rebuild"
            | "dev_containers.build.get"
            | "dev_containers.launch_reference"
    )
}

/// Startup waits only recorded effects, inventories any owner-labelled output,
/// and removes the exact managed builder. A completed publication is committed
/// only after restoring the trusted identity reference and reauthorizing it;
/// otherwise that exact image is discarded before the build becomes failed.
pub(crate) async fn recover_interrupted(
    store: crate::access::AccessStore,
    runtime: std::sync::Arc<crate::access::DynDevContainerImageRuntime>,
) {
    let mut after_build_id = String::new();
    loop {
        let builds = match store
            .dev_container_build_active(after_build_id.clone())
            .await
        {
            Ok(builds) => builds,
            Err(error) => {
                tracing::warn!(error = %error, "Dev Container image build recovery inventory failed");
                return;
            }
        };
        let page_len = builds.len();
        for build in builds {
            after_build_id.clone_from(&build.build_id);
            recover_one(&store, runtime.as_ref(), build).await;
        }
        if page_len < 100 {
            return;
        }
    }
}

async fn recover_one<
    R: ContainerImageRuntime<Error = crate::access::DevContainerEngineError> + ?Sized,
>(
    store: &crate::access::AccessStore,
    runtime: &R,
    build: crate::access::DevContainerImageBuild,
) {
    let handle = ImageBuildHandle {
        build_id: build.build_id.clone(),
        lifecycle_nonce: build.lifecycle_nonce.clone(),
        builder_instance_name: build.builder_instance_name.clone(),
    };
    let prior_operation_id = build.engine_operation_id.clone();
    if let Some(operation_id) = &prior_operation_id {
        let operation = ImageBuildOperation {
            effect: recovery_effect(&build),
            operation_id: operation_id.clone(),
        };
        if let Err(error) = runtime.wait(&operation).await {
            tracing::warn!(build_id = %build.build_id, error = %error, "interrupted image operation remains uncertain");
            return;
        }
        if operation.effect == ImageBuildEffect::Discard {
            fail_recovered_build(
                store,
                &build.build_id,
                "authority_changed",
                "uncommitted published output was discarded after reauthorization failed",
            )
            .await;
            return;
        }
    }

    let published = match runtime.published_image(&handle).await {
        Ok(published) => published,
        Err(error) => {
            tracing::warn!(build_id = %build.build_id, error = %error, "interrupted image output inspection failed");
            return;
        }
    };
    let now = recovery_now();
    if build.step == "cleanup" && build.progress >= 99 {
        if let Some(image) = published {
            discard_orphan(store, runtime, &build, &handle, &image, None, now).await;
        } else {
            fail_recovered_build(
                store,
                &build.build_id,
                "authority_changed",
                "uncommitted published output was discarded after reauthorization failed",
            )
            .await;
        }
        return;
    }
    if let Err(error) = store
        .dev_container_build_prepare_recovery_effect(
            build.build_id.clone(),
            prior_operation_id,
            95,
            now,
        )
        .await
    {
        tracing::warn!(build_id = %build.build_id, error = %error, "interrupted image cleanup intent could not be persisted");
        return;
    }
    let cleanup = match runtime.cleanup(&handle).await {
        Ok(operation) => operation,
        Err(error) => {
            tracing::warn!(build_id = %build.build_id, error = %error, "interrupted image builder cleanup submission remains uncertain");
            return;
        }
    };
    if let Err(error) = store
        .dev_container_build_record_recovery_cleanup(
            build.build_id.clone(),
            95,
            cleanup.operation_id.clone(),
            now,
        )
        .await
    {
        tracing::warn!(build_id = %build.build_id, error = %error, "interrupted image cleanup operation could not be recorded");
        return;
    }
    if let Err(error) = runtime.wait(&cleanup).await {
        tracing::warn!(build_id = %build.build_id, error = %error, "interrupted image builder cleanup remains uncertain");
        return;
    }

    let published = match runtime.published_image(&handle).await {
        Ok(published) => published,
        Err(error) => {
            tracing::warn!(build_id = %build.build_id, error = %error, "post-cleanup image output inspection failed");
            return;
        }
    };
    let Some(image) = published else {
        fail_recovered_build(
            store,
            &build.build_id,
            "restart_interrupted",
            "interrupted image build produced no published output after verified cleanup",
        )
        .await;
        return;
    };
    let source = verified_source_snapshot(&build);
    let identity =
        serde_json::from_str::<crate::access::DurableIdentityReference>(&build.identity_ref_json)
            .ok()
            .and_then(|reference| reference.restore().ok());
    if let (Some(source), Some(identity)) = (source, identity) {
        let context = DevContainerDispatchContext {
            access_runtime: std::sync::Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            identity,
            ceiling: crate::access::AuthorityCeiling::durable_execution(),
        };
        if let Ok(owner) = draft_owner(&source.draft) {
            let millis = u64::try_from(now).unwrap_or(0).saturating_mul(1_000);
            if let Ok(request) = authority_request(
                &context,
                "dev_containers.build",
                owner,
                &build.template_id,
                millis,
            ) {
                if let Err(error) = store
                    .dev_container_build_clear_operation(
                        build.build_id.clone(),
                        cleanup.operation_id.clone(),
                        "cleanup".to_owned(),
                        95,
                        now,
                    )
                    .await
                {
                    tracing::warn!(build_id = %build.build_id, error = %error, "verified cleanup observation could not be committed");
                    return;
                }
                let catalog = crate::config::resolved_dev_container_build_catalog();
                if catalog_matches(&source, &catalog)
                    && store
                        .dev_container_build_succeed(
                            build.build_id.clone(),
                            image.as_str().to_owned(),
                            catalog.generation().to_owned(),
                            catalog.digest().to_owned(),
                            now,
                            request,
                        )
                        .await
                        .is_ok()
                {
                    return;
                }
                discard_orphan(store, runtime, &build, &handle, &image, None, now).await;
                return;
            }
        }
    }
    discard_orphan(
        store,
        runtime,
        &build,
        &handle,
        &image,
        Some(cleanup.operation_id),
        now,
    )
    .await;
}

fn recovery_effect(build: &crate::access::DevContainerImageBuild) -> ImageBuildEffect {
    match build.step.as_str() {
        "launching" => ImageBuildEffect::Launch,
        "provisioning" => ImageBuildEffect::Provision,
        "publishing" if build.progress < 80 => ImageBuildEffect::Stop,
        "publishing" => ImageBuildEffect::Publish,
        "cleanup" if build.progress >= 99 => ImageBuildEffect::Discard,
        _ => ImageBuildEffect::Cleanup,
    }
}

fn verified_source_snapshot(
    build: &crate::access::DevContainerImageBuild,
) -> Option<crate::access::DevContainerBuildSource> {
    let expected = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(build.source_snapshot_json.as_bytes()))
    );
    if expected != build.source_digest {
        return None;
    }
    serde_json::from_str(&build.source_snapshot_json).ok()
}

async fn discard_orphan<
    R: ContainerImageRuntime<Error = crate::access::DevContainerEngineError> + ?Sized,
>(
    store: &crate::access::AccessStore,
    runtime: &R,
    build: &crate::access::DevContainerImageBuild,
    handle: &ImageBuildHandle,
    image: &ImageDigest,
    expected_operation_id: Option<String>,
    now: i64,
) {
    if let Err(error) = store
        .dev_container_build_prepare_recovery_effect(
            build.build_id.clone(),
            expected_operation_id,
            99,
            now,
        )
        .await
    {
        tracing::warn!(build_id = %build.build_id, error = %error, "image discard intent could not be persisted");
        return;
    }
    let operation = match runtime.discard_image(handle, image).await {
        Ok(operation) => operation,
        Err(error) => {
            tracing::warn!(build_id = %build.build_id, error = %error, "uncommitted image output discard remains uncertain");
            return;
        }
    };
    if let Err(error) = store
        .dev_container_build_record_recovery_cleanup(
            build.build_id.clone(),
            99,
            operation.operation_id.clone(),
            now,
        )
        .await
    {
        tracing::warn!(build_id = %build.build_id, error = %error, "image discard operation could not be recorded");
        return;
    }
    if let Err(error) = runtime.wait(&operation).await {
        tracing::warn!(build_id = %build.build_id, error = %error, "uncommitted image output discard remains uncertain");
        return;
    }
    fail_recovered_build(
        store,
        &build.build_id,
        "authority_changed",
        "uncommitted published output was discarded after reauthorization failed",
    )
    .await;
}

async fn fail_recovered_build(
    store: &crate::access::AccessStore,
    build_id: &str,
    error_kind: &str,
    summary: &str,
) {
    if let Err(error) = store
        .dev_container_build_fail(
            build_id.to_owned(),
            error_kind.to_owned(),
            summary.to_owned(),
            recovery_now(),
        )
        .await
    {
        tracing::warn!(build_id, error = %error, "recovered image build terminal state could not be committed");
    }
}

fn recovery_now() -> i64 {
    i64::try_from(now_millis().unwrap_or(0) / 1_000).unwrap_or(0)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TemplateDefinition {
    display_name: String,
    #[serde(default)]
    toolchains: Vec<CatalogSelection>,
    #[serde(default)]
    agents: Vec<CatalogSelection>,
    #[serde(default)]
    packages: Vec<PackageSelection>,
    #[serde(default)]
    network: NetworkSelection,
    #[serde(default)]
    loadout_ids: Vec<String>,
    #[serde(default)]
    repository_ids: Vec<String>,
    #[serde(default)]
    dotfiles_reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogSelection {
    id: String,
    version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct PackageSelection {
    registry: String,
    name: String,
    version: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct NetworkSelection {
    tailnet: bool,
    web: bool,
    lan: bool,
    nested_docker: bool,
}

pub(super) async fn dispatch(
    context: DevContainerDispatchContext,
    store: crate::access::AccessStore,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    match action {
        "dev_containers.templates.list" => list(&context, &store, action, &params).await,
        "dev_containers.templates.get" => get(&context, &store, action, &params).await,
        "dev_containers.draft.create" => create(&context, &store, action, &params).await,
        "dev_containers.draft.update" => update(&context, &store, action, &params).await,
        "dev_containers.environment.replace" => {
            replace_environment(&context, &store, action, &params).await
        }
        "dev_containers.build" | "dev_containers.rebuild" => {
            enqueue_build(context, store, action, &params).await
        }
        "dev_containers.build.get" => build_get(&context, &store, action, &params).await,
        "dev_containers.launch_reference" => {
            launch_reference(&context, &store, action, &params).await
        }
        _ => Err(denied()),
    }
}

async fn list(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let kind = parse_owner_kind(params.get("owner_kind"))?;
    let owner_id = required_str(params, "owner_id")?;
    let owner = owner_scope(kind, owner_id).ok_or_else(|| invalid("owner_id"))?;
    super::authorize(context, store, action, owner, "template-list")
        .await
        .map_err(store_error)?;
    let cursor = params
        .get("cursor")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(invalid("limit"));
    }
    let drafts = store
        .dev_container_draft_page(
            crate::access::owner_kind_wire(kind).to_owned(),
            owner_id.to_owned(),
            cursor.to_owned(),
            limit,
        )
        .await
        .map_err(store_error)?;
    let next_cursor = drafts.last().map(|draft| draft.template_id.clone());
    Ok(json!({"templates": drafts, "next_cursor": next_cursor}))
}

async fn get(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let template_id = required_str(params, "template_id")?;
    let source = authorized_source(context, store, action, template_id).await?;
    let definition = serde_json::from_str::<Value>(&source.draft.definition_json)
        .map_err(|_| super::unavailable())?;
    Ok(
        json!({"template": source.draft, "definition": definition, "environment": source.environment}),
    )
}

async fn create(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let template_id = bounded_id(params, "template_id", 256)?;
    let base_template_id = bounded_id(params, "base_template_id", 256)?;
    let kind = parse_owner_kind(params.get("owner_kind"))?;
    let owner_id = bounded_id(params, "owner_id", 256)?;
    let owner = owner_scope(kind, &owner_id).ok_or_else(|| invalid("owner_id"))?;
    let definition = definition(params)?;
    let now = now_millis()?;
    let request =
        authority_request(context, action, owner, &template_id, now).map_err(store_error)?;
    let draft = store
        .dev_container_draft_create(
            crate::access::CreateTemplateDraft {
                template_id,
                owner_kind: crate::access::owner_kind_wire(kind).to_owned(),
                owner_id,
                base_template_id,
                definition_json: serde_json::to_string(&definition)
                    .map_err(|_| super::unavailable())?,
                requested_max_active_instances: positive_i64(params, "max_active_instances")?,
                requested_cpu_millis: positive_i64(params, "cpu_millis")?,
                requested_memory_bytes: positive_i64(params, "memory_bytes")?,
                requested_disk_bytes: positive_i64(params, "disk_bytes")?,
                requested_max_lifetime_seconds: positive_i64(params, "max_lifetime_seconds")?,
                authority_fingerprint: context.identity.safe_fingerprint(),
                now: seconds(now)?,
            },
            request,
        )
        .await
        .map_err(store_error)?;
    Ok(json!({"template": draft, "definition": definition}))
}

async fn update(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let template_id = required_str(params, "template_id")?;
    let source = authorized_source(context, store, action, template_id).await?;
    let definition = definition(params)?;
    let revision = revision(params)?;
    if revision != source.draft.revision {
        return Err(denied());
    }
    let now = now_millis()?;
    let owner = draft_owner(&source.draft)?;
    let draft = store
        .dev_container_draft_update(
            template_id.to_owned(),
            revision,
            serde_json::to_string(&definition).map_err(|_| super::unavailable())?,
            context.identity.safe_fingerprint(),
            seconds(now)?,
            authority_request(context, action, owner, template_id, now).map_err(store_error)?,
        )
        .await
        .map_err(store_error)?;
    Ok(json!({"template":draft,"definition":definition}))
}

async fn replace_environment(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let template_id = required_str(params, "template_id")?;
    let source = authorized_source(context, store, action, template_id).await?;
    let revision = revision(params)?;
    if revision != source.draft.revision {
        return Err(denied());
    }
    let entries = serde_json::from_value::<Vec<crate::access::TemplateEnvironmentEntry>>(
        params
            .get("environment")
            .cloned()
            .ok_or_else(|| invalid("environment"))?,
    )
    .map_err(|_| invalid("environment"))?;
    validate_environment(&entries)?;
    let now = now_millis()?;
    let owner = draft_owner(&source.draft)?;
    let draft = store
        .dev_container_environment_replace(
            template_id.to_owned(),
            revision,
            entries.clone(),
            context.identity.safe_fingerprint(),
            seconds(now)?,
            authority_request(context, action, owner, template_id, now).map_err(store_error)?,
        )
        .await
        .map_err(store_error)?;
    Ok(json!({"template":draft,"environment":entries}))
}

async fn enqueue_build(
    context: DevContainerDispatchContext,
    store: crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let template_id = bounded_id(params, "template_id", 256)?;
    let request_id = bounded_id(params, "request_id", 256)?;
    let revision = revision(params)?;
    let mut source = authorized_source(&context, &store, action, &template_id).await?;
    if source.draft.revision != revision {
        return Err(denied());
    }
    let definition: TemplateDefinition =
        serde_json::from_str(&source.draft.definition_json).map_err(|_| super::unavailable())?;
    let catalog = crate::config::resolved_dev_container_build_catalog();
    let (commands, runtime_profiles) = provisioning_commands(&definition, &catalog)?;
    let environment = launch_environment(&source.environment, &catalog)?;
    source.build_catalog = Some(crate::access::DevContainerBuildCatalogSnapshot {
        generation: catalog.generation().to_owned(),
        digest: catalog.digest().to_owned(),
        builder_profiles: catalog.builder_profiles(),
        runtime_network_mask: definition.network.mask(),
        runtime_profiles,
        environment,
    });
    let snapshot_json = serde_json::to_string(&source).map_err(|_| super::unavailable())?;
    if snapshot_json.len() > MAX_DEFINITION_BYTES {
        return Err(invalid("definition"));
    }
    let source_digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(snapshot_json.as_bytes()))
    );
    let nonce = hex::encode(Sha256::digest(
        format!(
            "{}\0{request_id}\0{source_digest}",
            context.identity.safe_fingerprint()
        )
        .as_bytes(),
    ));
    let build_id = format!("build-{}", &nonce[..32]);
    let handle = ImageBuildHandle {
        build_id: build_id.clone(),
        lifecycle_nonce: nonce,
        builder_instance_name: String::new(),
    };
    let handle = ImageBuildHandle {
        builder_instance_name: ImageBuildHandle::deterministic_name(
            &handle.build_id,
            &handle.lifecycle_nonce,
        ),
        ..handle
    };
    let now = now_millis()?;
    let owner = draft_owner(&source.draft)?;
    let request =
        authority_request(&context, action, owner, &template_id, now).map_err(store_error)?;
    let build = store
        .dev_container_build_enqueue(
            crate::access::EnqueueImageBuild {
                build_id: build_id.clone(),
                request_id,
                template_id: template_id.clone(),
                expected_revision: revision,
                base_image_digest: source.base_image_digest.clone(),
                source_digest,
                source_snapshot_json: snapshot_json,
                lifecycle_nonce: handle.lifecycle_nonce.clone(),
                builder_instance_name: handle.builder_instance_name.clone(),
                request_kind: action.rsplit('.').next().unwrap_or("build").to_owned(),
                identity_ref_json: serde_json::to_string(
                    &crate::access::DurableIdentityReference::capture(&context.identity),
                )
                .map_err(|_| super::unavailable())?,
                ceiling_json: "[\"scope:read\",\"scope:create\",\"scope:operate\"]".to_owned(),
                authority_fingerprint: context.identity.safe_fingerprint(),
                now: seconds(now)?,
            },
            request,
        )
        .await
        .map_err(store_error)?;
    if build.state == "queued" {
        let task_build = build.clone();
        tokio::spawn(async move {
            if let Err(error) =
                run_build(context, store.clone(), task_build.clone(), source, commands).await
            {
                match store
                    .dev_container_build_get(task_build.build_id.clone())
                    .await
                {
                    Ok(Some(current))
                        if current.step == "queued" && current.engine_operation_id.is_none() =>
                    {
                        if let Err(store_error) = store
                            .dev_container_build_fail(
                                task_build.build_id.clone(),
                                error.kind().to_owned(),
                                "image build failed before any engine effect was prepared"
                                    .to_owned(),
                                recovery_now(),
                            )
                            .await
                        {
                            tracing::warn!(build_id = %task_build.build_id, error = %store_error, "unstarted image build failure could not be committed");
                        }
                    }
                    Ok(Some(_)) => {
                        tracing::warn!(build_id = %task_build.build_id, error = %error, "image build stopped with recoverable engine state")
                    }
                    Ok(None) => {
                        tracing::warn!(build_id = %task_build.build_id, error = %error, "image build row disappeared after worker failure")
                    }
                    Err(store_error) => {
                        tracing::warn!(build_id = %task_build.build_id, error = %store_error, "image build state could not be inspected after worker failure")
                    }
                }
            }
        });
    }
    Ok(json!({"build":build}))
}

async fn run_build(
    context: DevContainerDispatchContext,
    store: crate::access::AccessStore,
    build: crate::access::DevContainerImageBuild,
    source: crate::access::DevContainerBuildSource,
    commands: Vec<ProvisionCommand>,
) -> Result<(), ToolError> {
    run_build_inner(context, store, build, source, commands).await
}

async fn run_build_inner(
    context: DevContainerDispatchContext,
    store: crate::access::AccessStore,
    build: crate::access::DevContainerImageBuild,
    source: crate::access::DevContainerBuildSource,
    commands: Vec<ProvisionCommand>,
) -> Result<(), ToolError> {
    let runtime = context.access_runtime.dev_container_image_runtime();
    let handle = ImageBuildHandle {
        build_id: build.build_id.clone(),
        lifecycle_nonce: build.lifecycle_nonce.clone(),
        builder_instance_name: build.builder_instance_name.clone(),
    };
    let base_image =
        ImageDigest::new(source.base_image_digest.clone()).map_err(|_| super::unavailable())?;
    prepare_effect(&context, &store, &source, &build, "queued", "launching", 5).await?;
    let launch = runtime
        .launch(ImageBuildRequest {
            handle: handle.clone(),
            base_image,
            cpu_millis: u32::try_from(source.draft.cpu_millis).map_err(|_| super::unavailable())?,
            memory_bytes: u64::try_from(source.draft.memory_bytes)
                .map_err(|_| super::unavailable())?,
            disk_bytes: u64::try_from(source.draft.disk_bytes).map_err(|_| super::unavailable())?,
            profiles: source
                .build_catalog
                .as_ref()
                .ok_or_else(super::unavailable)?
                .builder_profiles
                .clone(),
        })
        .await
        .map_err(engine_error)?;
    record_and_wait(&store, &build, "launching", 5, &launch, runtime.as_ref()).await?;
    for (index, command) in commands.iter().enumerate() {
        let expected_step = if index == 0 {
            "launching"
        } else {
            "provisioning"
        };
        let progress = i64::try_from(10 + index).unwrap_or(60);
        prepare_effect(
            &context,
            &store,
            &source,
            &build,
            expected_step,
            "provisioning",
            progress,
        )
        .await?;
        let operation = runtime
            .provision(&handle, command)
            .await
            .map_err(engine_error)?;
        record_and_wait(
            &store,
            &build,
            "provisioning",
            progress,
            &operation,
            runtime.as_ref(),
        )
        .await?;
    }
    prepare_effect(
        &context,
        &store,
        &source,
        &build,
        if commands.is_empty() {
            "launching"
        } else {
            "provisioning"
        },
        "publishing",
        70,
    )
    .await?;
    let stop = runtime.stop(&handle).await.map_err(engine_error)?;
    record_and_wait(&store, &build, "publishing", 70, &stop, runtime.as_ref()).await?;
    prepare_effect(
        &context,
        &store,
        &source,
        &build,
        "publishing",
        "publishing",
        85,
    )
    .await?;
    let publish = runtime.publish(&handle).await.map_err(engine_error)?;
    let digest = record_and_wait(&store, &build, "publishing", 85, &publish, runtime.as_ref())
        .await?
        .image_digest
        .ok_or_else(super::unavailable)?;
    prepare_effect(
        &context,
        &store,
        &source,
        &build,
        "publishing",
        "cleanup",
        95,
    )
    .await?;
    let cleanup = runtime.cleanup(&handle).await.map_err(engine_error)?;
    record_and_wait(&store, &build, "cleanup", 95, &cleanup, runtime.as_ref()).await?;
    let now = now_millis()?;
    let catalog = crate::config::resolved_dev_container_build_catalog();
    let result = if catalog_matches(&source, &catalog) {
        store
            .dev_container_build_succeed(
                build.build_id.clone(),
                digest.as_str().to_owned(),
                catalog.generation().to_owned(),
                catalog.digest().to_owned(),
                seconds(now)?,
                authority_request(
                    &context,
                    "dev_containers.build",
                    draft_owner(&source.draft)?,
                    &source.draft.template_id,
                    now,
                )
                .map_err(store_error)?,
            )
            .await
            .map_err(store_error)
    } else {
        Err(policy_changed())
    };
    if let Err(error) = result {
        discard_orphan(
            &store,
            runtime.as_ref(),
            &build,
            &handle,
            &digest,
            None,
            seconds(now)?,
        )
        .await;
        return Err(error);
    }
    Ok(())
}

async fn prepare_effect(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    source: &crate::access::DevContainerBuildSource,
    build: &crate::access::DevContainerImageBuild,
    expected_step: &str,
    next_step: &str,
    progress: i64,
) -> Result<(), ToolError> {
    let catalog = crate::config::resolved_dev_container_build_catalog();
    if !catalog_matches(source, &catalog) {
        return Err(policy_changed());
    }
    let now = now_millis()?;
    store
        .dev_container_build_prepare_effect(
            build.build_id.clone(),
            expected_step.to_owned(),
            next_step.to_owned(),
            progress,
            catalog.generation().to_owned(),
            catalog.digest().to_owned(),
            seconds(now)?,
            authority_request(
                context,
                "dev_containers.build",
                draft_owner(&source.draft)?,
                &source.draft.template_id,
                now,
            )
            .map_err(store_error)?,
        )
        .await
        .map_err(store_error)
}

async fn record_and_wait<
    R: ContainerImageRuntime<Error = crate::access::DevContainerEngineError> + ?Sized,
>(
    store: &crate::access::AccessStore,
    build: &crate::access::DevContainerImageBuild,
    prepared_step: &str,
    progress: i64,
    operation: &ImageBuildOperation,
    runtime: &R,
) -> Result<labby_runtime::dev_container_image_runtime::ImageBuildOperationResult, ToolError> {
    let now = now_millis()?;
    store
        .dev_container_build_record_operation(
            build.build_id.clone(),
            prepared_step.to_owned(),
            progress,
            operation.operation_id.clone(),
            seconds(now)?,
        )
        .await
        .map_err(store_error)?;
    let result = runtime.wait(operation).await.map_err(engine_error)?;
    let now = now_millis()?;
    store
        .dev_container_build_clear_operation(
            build.build_id.clone(),
            operation.operation_id.clone(),
            prepared_step.to_owned(),
            progress,
            seconds(now)?,
        )
        .await
        .map_err(store_error)?;
    Ok(result)
}

async fn build_get(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let id = required_str(params, "build_id")?;
    let build = store
        .dev_container_build_get(id.to_owned())
        .await
        .map_err(store_error)?
        .ok_or_else(denied)?;
    let source = authorized_source(context, store, action, &build.template_id).await?;
    drop(source);
    Ok(json!({"build":build}))
}

async fn launch_reference(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    params: &Value,
) -> Result<Value, ToolError> {
    let id = required_str(params, "template_id")?;
    drop(authorized_source(context, store, action, id).await?);
    let digest = store
        .dev_container_published_image_get(id.to_owned())
        .await
        .map_err(store_error)?
        .ok_or_else(denied)?;
    let endpoint =
        std::env::var("LABBY_DEV_CONTAINER_INCUS_URL").map_err(|_| super::unavailable())?;
    let project =
        std::env::var("LABBY_DEV_CONTAINER_INCUS_PROJECT").map_err(|_| super::unavailable())?;
    Ok(
        json!({"distribution":{"kind":"incus_project","endpoint":endpoint,"project":project,"image_digest":digest},"aliases":[]}),
    )
}

async fn authorized_source(
    context: &DevContainerDispatchContext,
    store: &crate::access::AccessStore,
    action: &str,
    id: &str,
) -> Result<crate::access::DevContainerBuildSource, ToolError> {
    let source = store
        .dev_container_build_source(id.to_owned())
        .await
        .map_err(store_error)?
        .ok_or_else(denied)?;
    super::authorize(context, store, action, draft_owner(&source.draft)?, id)
        .await
        .map_err(store_error)?;
    Ok(source)
}
fn draft_owner(draft: &crate::access::DevContainerTemplateDraft) -> Result<OwnerScope, ToolError> {
    let kind = match draft.owner_kind.as_str() {
        "installation" => labby_primitives::access::OwnerKind::Installation,
        "team" => labby_primitives::access::OwnerKind::Team,
        "project" => labby_primitives::access::OwnerKind::Project,
        "personal" => labby_primitives::access::OwnerKind::Personal,
        _ => return Err(super::unavailable()),
    };
    owner_scope(kind, &draft.owner_id).ok_or_else(super::unavailable)
}
fn definition(params: &Value) -> Result<TemplateDefinition, ToolError> {
    let value = params
        .get("definition")
        .cloned()
        .ok_or_else(|| invalid("definition"))?;
    if value.to_string().len() > MAX_DEFINITION_BYTES {
        return Err(invalid("definition"));
    }
    let definition = serde_json::from_value(value).map_err(|_| invalid("definition"))?;
    validate_definition(&definition)?;
    Ok(definition)
}
fn validate_definition(value: &TemplateDefinition) -> Result<(), ToolError> {
    if value.display_name.trim().is_empty()
        || value.display_name.len() > 200
        || value.toolchains.len() > 32
        || value.agents.len() > 32
        || value.packages.len() > 128
        || value.loadout_ids.len() > 32
        || value.repository_ids.len() > 32
    {
        return Err(invalid("definition"));
    }
    for item in value.toolchains.iter().chain(&value.agents) {
        validate_token(&item.id, 128)?;
        validate_token(&item.version, 64)?;
    }
    for item in &value.packages {
        validate_token(&item.registry, 16)?;
        validate_token(&item.name, 128)?;
        validate_token(&item.version, 64)?;
    }
    for item in value.loadout_ids.iter().chain(&value.repository_ids) {
        validate_token(item, 256)?;
    }
    if value
        .dotfiles_reference
        .as_ref()
        .is_some_and(|item| validate_token(item, 256).is_err())
    {
        return Err(invalid("definition.dotfiles_reference"));
    }
    Ok(())
}
fn provisioning_commands(
    value: &TemplateDefinition,
    catalog: &ApprovedProvisionCatalog,
) -> Result<
    (
        Vec<ProvisionCommand>,
        Vec<labby_runtime::dev_container_runtime::IncusProfileReference>,
    ),
    ToolError,
> {
    if value.network.nested_docker {
        return Err(ToolError::Sdk {
            sdk_kind: "policy_unsupported".into(),
            message: "Nested Docker is not supported by the restricted Incus adapter".into(),
        });
    }
    if !value.loadout_ids.is_empty()
        || !value.repository_ids.is_empty()
        || value.dotfiles_reference.is_some()
    {
        return Err(ToolError::Sdk{sdk_kind:"policy_unsupported".into(),message:"Loadouts, repositories, and dotfiles require an immutable operator-approved import catalog".into()});
    }
    let mut commands = Vec::new();
    let mut toolchains = value.toolchains.clone();
    toolchains.sort();
    reject_duplicates(&toolchains)?;
    for selection in toolchains {
        commands.extend(
            catalog
                .resolve_toolchain(&selection.id, &selection.version)
                .ok_or_else(unsupported_catalog_selection)?,
        );
    }
    let mut agents = value.agents.clone();
    agents.sort();
    reject_duplicates(&agents)?;
    for selection in agents {
        commands.extend(
            catalog
                .resolve_agent(&selection.id, &selection.version)
                .ok_or_else(unsupported_catalog_selection)?,
        );
    }
    let mut packages = value.packages.clone();
    packages.sort();
    reject_duplicates(&packages)?;
    for selection in packages {
        commands.extend(
            catalog
                .resolve_package(&selection.registry, &selection.name, &selection.version)
                .ok_or_else(unsupported_catalog_selection)?,
        );
    }
    if commands.len() > MAX_PROVISION_COMMANDS {
        return Err(ToolError::Sdk {
            sdk_kind: "policy_unsupported".into(),
            message: "The resolved build plan exceeds the bounded provisioning command limit"
                .into(),
        });
    }
    let profiles = catalog
        .resolve_network(value.network.mask())
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "policy_unsupported".into(),
            message:
                "The requested network combination has no operator-approved runtime profile mapping"
                    .into(),
        })?;
    Ok((commands, profiles))
}

fn launch_environment(
    entries: &[crate::access::TemplateEnvironmentEntry],
    catalog: &ApprovedProvisionCatalog,
) -> Result<Vec<crate::access::DevContainerLaunchEnvironmentEntry>, ToolError> {
    entries
        .iter()
        .map(|entry| match entry.value_kind.as_str() {
            "literal" => Ok(crate::access::DevContainerLaunchEnvironmentEntry::Literal {
                name: entry.name.clone(),
                value: entry.literal_value.clone().ok_or_else(super::unavailable)?,
            }),
            "secret_reference" => {
                let reference = entry
                    .secret_reference
                    .clone()
                    .ok_or_else(super::unavailable)?;
                let source_env = catalog
                    .resolve_environment_secret(&reference, &entry.name)
                    .ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "policy_unsupported".into(),
                        message: "The environment secret reference and target name are not operator-approved".into(),
                    })?;
                Ok(crate::access::DevContainerLaunchEnvironmentEntry::SecretReference {
                    name: entry.name.clone(),
                    reference,
                    source_env: source_env.to_owned(),
                })
            }
            _ => Err(super::unavailable()),
        })
        .collect()
}

fn reject_duplicates<T: Ord>(values: &[T]) -> Result<(), ToolError> {
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        Err(invalid("definition"))
    } else {
        Ok(())
    }
}

fn unsupported_catalog_selection() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "policy_unsupported".into(),
        message: "The exact selected version is absent from the operator-approved build catalog"
            .into(),
    }
}

impl NetworkSelection {
    fn mask(&self) -> u8 {
        u8::from(self.tailnet)
            | (u8::from(self.web) << 1)
            | (u8::from(self.lan) << 2)
            | (u8::from(self.nested_docker) << 3)
    }
}

fn catalog_matches(
    source: &crate::access::DevContainerBuildSource,
    catalog: &ApprovedProvisionCatalog,
) -> bool {
    let Some(snapshot) = &source.build_catalog else {
        return false;
    };
    snapshot.generation == catalog.generation() && snapshot.digest == catalog.digest()
}

fn policy_changed() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "policy_changed".into(),
        message: "The operator-approved Dev Container build catalog changed".into(),
    }
}
fn validate_environment(
    entries: &[crate::access::TemplateEnvironmentEntry],
) -> Result<(), ToolError> {
    if entries.len() > MAX_ENVIRONMENT_ENTRIES {
        return Err(invalid("environment"));
    }
    let mut names = std::collections::BTreeSet::new();
    for entry in entries {
        if !names.insert(entry.name.as_str())
            || entry.name.len() > 128
            || !entry.name.bytes().enumerate().all(|(i, b)| {
                b == b'_' || b.is_ascii_alphanumeric() && (i > 0 || !b.is_ascii_digit())
            })
        {
            return Err(invalid("environment.name"));
        }
        match (
            entry.value_kind.as_str(),
            &entry.literal_value,
            &entry.secret_reference,
        ) {
            ("literal", Some(value), None) if value.len() <= 4096 => {}
            ("secret_reference", None, Some(value)) if !value.is_empty() && value.len() <= 256 => {}
            _ => return Err(invalid("environment")),
        }
    }
    Ok(())
}
fn validate_token(value: &str, max: usize) -> Result<(), ToolError> {
    if value.trim().is_empty() || value.len() > max || value.bytes().any(|b| b.is_ascii_control()) {
        Err(invalid("definition"))
    } else {
        Ok(())
    }
}
fn bounded_id(params: &Value, name: &'static str, max: usize) -> Result<String, ToolError> {
    let value = required_str(params, name)?;
    if value.len() > max {
        return Err(invalid(name));
    }
    Ok(value.to_owned())
}
fn revision(params: &Value) -> Result<i64, ToolError> {
    params
        .get("expected_revision")
        .and_then(Value::as_i64)
        .filter(|v| *v > 0)
        .ok_or_else(|| invalid("expected_revision"))
}
fn positive_i64(params: &Value, name: &'static str) -> Result<Option<i64>, ToolError> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .filter(|v| *v > 0)
            .map(Some)
            .ok_or_else(|| invalid(name)),
    }
}
fn seconds(now: u64) -> Result<i64, ToolError> {
    i64::try_from(now / 1000).map_err(|_| super::unavailable())
}
fn engine_error(error: crate::access::DevContainerEngineError) -> ToolError {
    tracing::warn!(error=%error,"Dev Container image engine effect failed");
    super::unavailable()
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_runtime::dev_container_image_runtime::ImageBuildOperationResult;

    use super::*;

    #[derive(Clone, Copy)]
    enum BuildBehavior {
        Complete,
        SubmitUncertain,
        InspectUncertain,
        WaitUncertain,
        RevokeAfterLaunch,
        NarrowBaseAfterLaunch,
    }

    struct TestImageRuntime {
        path: std::path::PathBuf,
        behavior: BuildBehavior,
        launch_observation: Mutex<Option<(String, i64, Option<String>)>>,
        waits: AtomicUsize,
        stops: AtomicUsize,
        cleanups: AtomicUsize,
    }

    impl TestImageRuntime {
        fn new(path: std::path::PathBuf, behavior: BuildBehavior) -> Self {
            Self {
                path,
                behavior,
                launch_observation: Mutex::new(None),
                waits: AtomicUsize::new(0),
                stops: AtomicUsize::new(0),
                cleanups: AtomicUsize::new(0),
            }
        }

        fn operation(effect: ImageBuildEffect, id: &str) -> ImageBuildOperation {
            ImageBuildOperation {
                effect,
                operation_id: id.to_owned(),
            }
        }
    }

    impl ContainerImageRuntime for TestImageRuntime {
        type Error = crate::access::DevContainerEngineError;

        fn launch<'a>(
            &'a self,
            request: ImageBuildRequest,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                let observation = rusqlite::Connection::open(&self.path)
                    .unwrap()
                    .query_row(
                        "SELECT step,progress,engine_operation_id FROM dev_container_image_builds WHERE build_id=?1",
                        [&request.handle.build_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .unwrap();
                *self.launch_observation.lock().unwrap() = Some(observation);
                if matches!(
                    self.behavior,
                    BuildBehavior::SubmitUncertain | BuildBehavior::InspectUncertain
                ) {
                    Err(crate::access::DevContainerEngineError::Unconfigured)
                } else {
                    Ok(Self::operation(ImageBuildEffect::Launch, "launch-op"))
                }
            })
        }

        fn provision<'a>(
            &'a self,
            _: &'a ImageBuildHandle,
            _: &'a ProvisionCommand,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>
        {
            Box::pin(async { unreachable!("unsupported catalog entries must not provision") })
        }

        fn stop<'a>(
            &'a self,
            _: &'a ImageBuildHandle,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                self.stops.fetch_add(1, Ordering::SeqCst);
                Ok(Self::operation(ImageBuildEffect::Stop, "stop-op"))
            })
        }

        fn publish<'a>(
            &'a self,
            _: &'a ImageBuildHandle,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>
        {
            Box::pin(async { Ok(Self::operation(ImageBuildEffect::Publish, "publish-op")) })
        }

        fn cleanup<'a>(
            &'a self,
            _: &'a ImageBuildHandle,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                self.cleanups.fetch_add(1, Ordering::SeqCst);
                Ok(Self::operation(ImageBuildEffect::Cleanup, "cleanup-op"))
            })
        }

        fn published_image<'a>(
            &'a self,
            _: &'a ImageBuildHandle,
        ) -> Pin<Box<dyn Future<Output = Result<Option<ImageDigest>, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                if matches!(self.behavior, BuildBehavior::InspectUncertain) {
                    Err(crate::access::DevContainerEngineError::Unconfigured)
                } else {
                    Ok(None)
                }
            })
        }

        fn discard_image<'a>(
            &'a self,
            _: &'a ImageBuildHandle,
            _: &'a ImageDigest,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperation, Self::Error>> + Send + 'a>>
        {
            Box::pin(async { Ok(Self::operation(ImageBuildEffect::Discard, "discard-op")) })
        }

        fn wait<'a>(
            &'a self,
            operation: &'a ImageBuildOperation,
        ) -> Pin<Box<dyn Future<Output = Result<ImageBuildOperationResult, Self::Error>> + Send + 'a>>
        {
            Box::pin(async move {
                self.waits.fetch_add(1, Ordering::SeqCst);
                if operation.effect == ImageBuildEffect::Launch {
                    match self.behavior {
                        BuildBehavior::WaitUncertain => {
                            return Err(crate::access::DevContainerEngineError::Unconfigured);
                        }
                        BuildBehavior::RevokeAfterLaunch => {
                            rusqlite::Connection::open(&self.path)
                                .unwrap()
                                .execute(
                                    "UPDATE principals SET status='disabled',updated_at=updated_at+1 WHERE principal_id='bootstrap-owner'",
                                    [],
                                )
                                .unwrap();
                        }
                        BuildBehavior::NarrowBaseAfterLaunch => {
                            rusqlite::Connection::open(&self.path)
                                .unwrap()
                                .execute(
                                    "UPDATE dev_container_templates SET cpu_millis=1,policy_epoch=policy_epoch+1 WHERE template_id='base'",
                                    [],
                                )
                                .unwrap();
                        }
                        BuildBehavior::Complete
                        | BuildBehavior::SubmitUncertain
                        | BuildBehavior::InspectUncertain => {}
                    }
                }
                Ok(ImageBuildOperationResult {
                    image_digest: (operation.effect == ImageBuildEffect::Publish)
                        .then(|| ImageDigest::new(format!("sha256:{}", "b".repeat(64))).unwrap()),
                })
            })
        }
    }

    async fn image_fixture(
        behavior: BuildBehavior,
    ) -> (
        tempfile::TempDir,
        DevContainerDispatchContext,
        crate::access::AccessStore,
        Arc<TestImageRuntime>,
        tokio::sync::OwnedMutexGuard<()>,
    ) {
        let config_guard = crate::config::dev_container_config_test_guard().await;
        let config = crate::config::LabConfig {
            dev_containers: toml::from_str(
                r#"
catalog_generation = "image-tests-v1"

[[builder_profiles]]
name = "labby-builder"
content_digest = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[runtime_network_profiles]]

[[runtime_network_profiles.profiles]]
name = "labby-runtime-isolated"
content_digest = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#,
            )
            .unwrap(),
            ..Default::default()
        };
        crate::config::install_resolved_preferences(&config);
        let directory = crate::access::test_support::secure_tempdir();
        let path = directory.path().join("access.db");
        let store = crate::access::AccessStore::open(path.clone())
            .await
            .unwrap();
        let owner = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "image-owner",
        )
        .unwrap();
        store
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO dev_container_templates(template_id,image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,host_capabilities_json,status,policy_epoch,created_at,updated_at) VALUES('base',?1,4,1000,1073741824,2147483648,3600,'[]','approved',1,1,1)",
                [format!("sha256:{}", "a".repeat(64))],
            )
            .unwrap();
        drop(store);
        let engine = Arc::new(TestImageRuntime::new(path.clone(), behavior));
        let access_runtime = crate::access::AccessRuntime::initialize(path)
            .await
            .with_dev_container_image_runtime(engine.clone());
        let context = DevContainerDispatchContext {
            access_runtime: Arc::new(access_runtime),
            identity: owner,
            ceiling: crate::access::AuthorityCeiling::trusted_local(),
        };
        let store = context.access_runtime.store().await.unwrap();
        dispatch(
            context.clone(),
            store.clone(),
            "dev_containers.draft.create",
            json!({
                "template_id":"derived",
                "base_template_id":"base",
                "owner_kind":"personal",
                "owner_id":"bootstrap-owner",
                "definition": {
                    "display_name":"Derived",
                    "toolchains":[],
                    "agents":[],
                    "packages":[],
                    "network":{},
                    "loadout_ids":[],
                    "repository_ids":[]
                }
            }),
        )
        .await
        .unwrap();
        (directory, context, store, engine, config_guard)
    }

    async fn enqueue_fixture_build(
        context: &DevContainerDispatchContext,
        store: &crate::access::AccessStore,
        request_id: &str,
    ) -> String {
        let response = dispatch(
            context.clone(),
            store.clone(),
            "dev_containers.build",
            json!({
                "template_id":"derived",
                "request_id":request_id,
                "expected_revision":1
            }),
        )
        .await
        .unwrap();
        response["build"]["build_id"].as_str().unwrap().to_owned()
    }

    async fn wait_for_build(
        store: &crate::access::AccessStore,
        build_id: &str,
        predicate: impl Fn(&crate::access::DevContainerImageBuild) -> bool,
    ) -> crate::access::DevContainerImageBuild {
        for _ in 0..100 {
            if let Some(build) = store
                .dev_container_build_get(build_id.to_owned())
                .await
                .unwrap()
            {
                if predicate(&build) {
                    return build;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for image build {build_id}")
    }

    fn definition(network: NetworkSelection) -> TemplateDefinition {
        TemplateDefinition {
            display_name: "Rust image".to_owned(),
            toolchains: Vec::new(),
            agents: Vec::new(),
            packages: Vec::new(),
            network,
            loadout_ids: Vec::new(),
            repository_ids: Vec::new(),
            dotfiles_reference: None,
        }
    }

    fn approved_catalog(generation: &str) -> ApprovedProvisionCatalog {
        use labby_runtime::dev_container_image_runtime::{
            ApprovedBuildExecutable, ApprovedEnvironmentSecret, ApprovedProvisionRecipe,
            ApprovedProvisionStep, ApprovedRuntimeNetworkProfile, ProvisionRecipeKey,
        };
        use labby_runtime::dev_container_runtime::IncusProfileReference;

        let profile = |name: &str, byte: char| IncusProfileReference {
            name: name.to_owned(),
            content_digest: format!("sha256:{}", byte.to_string().repeat(64)),
        };

        ApprovedProvisionCatalog::new(
            generation.to_owned(),
            vec![
                ApprovedProvisionRecipe {
                    key: ProvisionRecipeKey::Toolchain {
                        id: "node".to_owned(),
                        version: "22.1.0".to_owned(),
                    },
                    steps: vec![ApprovedProvisionStep {
                        executable: ApprovedBuildExecutable::AptGet,
                        arguments: vec![
                            "install".to_owned(),
                            "-y".to_owned(),
                            "nodejs=22.1.0".to_owned(),
                        ],
                    }],
                },
                ApprovedProvisionRecipe {
                    key: ProvisionRecipeKey::Toolchain {
                        id: "rust".to_owned(),
                        version: "1.97.1".to_owned(),
                    },
                    steps: vec![ApprovedProvisionStep {
                        executable: ApprovedBuildExecutable::AptGet,
                        arguments: vec![
                            "install".to_owned(),
                            "-y".to_owned(),
                            "rustc=1.97.1".to_owned(),
                        ],
                    }],
                },
            ],
            vec![profile("labby-builder", 'a')],
            vec![ApprovedRuntimeNetworkProfile {
                mask: 0b0010,
                profiles: vec![profile("labby-runtime-web", 'b')],
            }],
            vec![ApprovedEnvironmentSecret {
                reference: "secret/team/github".to_owned(),
                source_env: "LABBY_GITHUB_TOKEN".to_owned(),
                allowed_target_names: vec!["TOKEN".to_owned()],
            }],
        )
        .expect("approved fixture catalog")
    }

    #[test]
    fn build_plan_uses_only_exact_catalog_entries_and_profile_mapping() {
        let catalog = approved_catalog("catalog-1");
        let mut requested = definition(NetworkSelection {
            web: true,
            ..NetworkSelection::default()
        });
        requested.toolchains = vec![
            CatalogSelection {
                id: "rust".to_owned(),
                version: "1.97.1".to_owned(),
            },
            CatalogSelection {
                id: "node".to_owned(),
                version: "22.1.0".to_owned(),
            },
        ];

        let (commands, profiles) =
            provisioning_commands(&requested, &catalog).expect("resolved build plan");
        assert_eq!(
            commands
                .iter()
                .map(ProvisionCommand::argv)
                .collect::<Vec<_>>(),
            vec![
                vec!["/usr/bin/apt-get", "install", "-y", "nodejs=22.1.0"],
                vec!["/usr/bin/apt-get", "install", "-y", "rustc=1.97.1"],
            ]
        );
        assert_eq!(profiles[0].name, "labby-runtime-web");

        requested.toolchains[0].version = "latest".to_owned();
        assert_eq!(
            provisioning_commands(&requested, &catalog)
                .unwrap_err()
                .kind(),
            "policy_unsupported"
        );
    }

    #[tokio::test]
    async fn catalog_generation_and_digest_are_a_build_effect_fence() {
        let (_directory, _context, store, _engine, _config_guard) =
            image_fixture(BuildBehavior::Complete).await;
        let mut source = store
            .dev_container_build_source("derived".to_owned())
            .await
            .unwrap()
            .unwrap();
        let original = approved_catalog("catalog-1");
        source.build_catalog = Some(crate::access::DevContainerBuildCatalogSnapshot {
            generation: original.generation().to_owned(),
            digest: original.digest().to_owned(),
            builder_profiles: original.builder_profiles(),
            runtime_network_mask: 0b0010,
            runtime_profiles: original.resolve_network(0b0010).unwrap(),
            environment: Vec::new(),
        });
        assert!(catalog_matches(&source, &original));

        let changed_generation = approved_catalog("catalog-2");
        assert!(!catalog_matches(&source, &changed_generation));

        let changed_recipe = ApprovedProvisionCatalog::new(
            "catalog-1".to_owned(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        assert!(!catalog_matches(&source, &changed_recipe));
    }

    #[test]
    fn network_modes_require_explicit_operator_profiles() {
        let catalog = ApprovedProvisionCatalog::deny_all();
        for network in [
            NetworkSelection {
                web: true,
                ..NetworkSelection::default()
            },
            NetworkSelection {
                lan: true,
                ..NetworkSelection::default()
            },
            NetworkSelection {
                nested_docker: true,
                ..NetworkSelection::default()
            },
            NetworkSelection {
                tailnet: true,
                ..NetworkSelection::default()
            },
        ] {
            let error = provisioning_commands(&definition(network), &catalog).unwrap_err();
            assert_eq!(error.kind(), "policy_unsupported");
        }
    }

    #[test]
    fn environment_requires_one_bounded_literal_or_opaque_reference() {
        let accepted = vec![
            crate::access::TemplateEnvironmentEntry {
                name: "MODE".into(),
                value_kind: "literal".into(),
                literal_value: Some("test".into()),
                secret_reference: None,
            },
            crate::access::TemplateEnvironmentEntry {
                name: "TOKEN".into(),
                value_kind: "secret_reference".into(),
                literal_value: None,
                secret_reference: Some("secret/team/github".into()),
            },
        ];
        assert!(validate_environment(&accepted).is_ok());
        let mut ambiguous = accepted;
        ambiguous[1].literal_value = Some("raw-secret".into());
        assert!(validate_environment(&ambiguous).is_err());
    }

    #[test]
    fn opaque_environment_reference_requires_the_exact_approved_target() {
        let catalog = approved_catalog("catalog-1");
        let approved = crate::access::TemplateEnvironmentEntry {
            name: "TOKEN".to_owned(),
            value_kind: "secret_reference".to_owned(),
            literal_value: None,
            secret_reference: Some("secret/team/github".to_owned()),
        };
        assert_eq!(
            launch_environment(&[approved.clone()], &catalog).unwrap(),
            vec![
                crate::access::DevContainerLaunchEnvironmentEntry::SecretReference {
                    name: "TOKEN".to_owned(),
                    reference: "secret/team/github".to_owned(),
                    source_env: "LABBY_GITHUB_TOKEN".to_owned(),
                }
            ]
        );
        let mut wrong_target = approved;
        wrong_target.name = "OTHER_TOKEN".to_owned();
        assert_eq!(
            launch_environment(&[wrong_target], &catalog)
                .unwrap_err()
                .kind(),
            "policy_unsupported"
        );
    }

    #[tokio::test]
    async fn literal_environment_is_snapshotted_into_the_launch_manifest_plan() {
        let (_directory, context, store, _engine, _config_guard) =
            image_fixture(BuildBehavior::SubmitUncertain).await;
        dispatch(
            context.clone(),
            store.clone(),
            "dev_containers.environment.replace",
            json!({
                "template_id":"derived",
                "expected_revision":1,
                "environment":[{
                    "name":"MODE",
                    "value_kind":"literal",
                    "literal_value":"test"
                }]
            }),
        )
        .await
        .unwrap();
        let response = dispatch(
            context,
            store.clone(),
            "dev_containers.build",
            json!({
                "template_id":"derived",
                "request_id":"environment-unsupported",
                "expected_revision":2
            }),
        )
        .await
        .unwrap();
        let build_id = response["build"]["build_id"].as_str().unwrap();
        let build = wait_for_build(&store, build_id, |build| build.step == "launching").await;
        let source: crate::access::DevContainerBuildSource =
            serde_json::from_str(&build.source_snapshot_json).unwrap();
        assert_eq!(
            source.build_catalog.unwrap().environment,
            vec![crate::access::DevContainerLaunchEnvironmentEntry::Literal {
                name: "MODE".to_owned(),
                value: "test".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn terminal_success_cannot_be_published_twice() {
        let (directory, context, store, _engine, _config_guard) =
            image_fixture(BuildBehavior::Complete).await;
        let build_id = enqueue_fixture_build(&context, &store, "complete-once").await;
        let completed = wait_for_build(&store, &build_id, |build| build.state == "succeeded").await;
        let source = store
            .dev_container_build_source("derived".to_owned())
            .await
            .unwrap()
            .unwrap();
        let now = now_millis().unwrap();
        let duplicate = store
            .dev_container_build_succeed(
                build_id.clone(),
                completed.output_image_digest.clone().unwrap(),
                crate::config::resolved_dev_container_build_catalog()
                    .generation()
                    .to_owned(),
                crate::config::resolved_dev_container_build_catalog()
                    .digest()
                    .to_owned(),
                seconds(now).unwrap(),
                authority_request(
                    &context,
                    "dev_containers.build",
                    draft_owner(&source.draft).unwrap(),
                    "derived",
                    now,
                )
                .unwrap(),
            )
            .await;
        assert!(matches!(
            duplicate,
            Err(crate::access::AccessStoreError::NotAuthorized)
        ));
        let connection = rusqlite::Connection::open(directory.path().join("access.db")).unwrap();
        let publication_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM dev_container_published_images WHERE template_id='derived'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(publication_count, 1);
        let manifest_binding: (i64, String) = connection
            .query_row(
                "SELECT count(*),t.launch_manifest_digest FROM dev_container_launch_manifests m JOIN dev_container_templates t ON t.template_id=m.template_id WHERE m.template_id='derived' AND m.source_build_id=?1",
                [&build_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(manifest_binding.0, 1);
        assert!(manifest_binding.1.starts_with("sha256:"));
    }

    #[test]
    fn caller_supplied_packages_are_not_an_operator_catalog() {
        for registry in ["apt", "npm", "pypi", "cargo"] {
            let mut requested = definition(NetworkSelection::default());
            requested.packages.push(PackageSelection {
                registry: registry.to_owned(),
                name: "caller-choice".to_owned(),
                version: "1.0.0".to_owned(),
            });
            let error = provisioning_commands(&requested, &ApprovedProvisionCatalog::deny_all())
                .unwrap_err();
            assert_eq!(error.kind(), "policy_unsupported", "registry={registry}");
        }
    }

    #[tokio::test]
    async fn launch_intent_is_durable_before_an_uncertain_submission() {
        let (_directory, context, store, engine, _config_guard) =
            image_fixture(BuildBehavior::SubmitUncertain).await;
        let build_id = enqueue_fixture_build(&context, &store, "submit-uncertain").await;
        let build = wait_for_build(&store, &build_id, |build| {
            build.step == "launching" && engine.launch_observation.lock().unwrap().is_some()
        })
        .await;
        assert_eq!(build.progress, 5);
        assert_eq!(build.engine_operation_id, None);
        assert_eq!(
            engine.launch_observation.lock().unwrap().clone(),
            Some(("launching".to_owned(), 5, None))
        );
        assert_eq!(build.state, "building");
    }

    #[tokio::test]
    async fn recovery_keeps_a_failed_wait_active_and_does_not_guess_cleanup() {
        let (_directory, context, store, engine, _config_guard) =
            image_fixture(BuildBehavior::WaitUncertain).await;
        let build_id = enqueue_fixture_build(&context, &store, "wait-uncertain").await;
        let before = wait_for_build(&store, &build_id, |build| {
            build.engine_operation_id.as_deref() == Some("launch-op")
        })
        .await;
        let runtime: Arc<crate::access::DynDevContainerImageRuntime> = engine.clone();
        recover_interrupted(store.clone(), runtime).await;
        let after = store
            .dev_container_build_get(build_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.state, "building");
        assert_eq!(after.step, before.step);
        assert_eq!(after.engine_operation_id, before.engine_operation_id);
        assert_eq!(engine.cleanups.load(Ordering::SeqCst), 0);
        assert!(engine.waits.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn recovery_keeps_a_failed_output_inspection_active() {
        let (_directory, context, store, engine, _config_guard) =
            image_fixture(BuildBehavior::InspectUncertain).await;
        let build_id = enqueue_fixture_build(&context, &store, "inspect-uncertain").await;
        wait_for_build(&store, &build_id, |build| {
            build.step == "launching" && engine.launch_observation.lock().unwrap().is_some()
        })
        .await;
        let runtime: Arc<crate::access::DynDevContainerImageRuntime> = engine.clone();
        recover_interrupted(store.clone(), runtime).await;
        let after = store
            .dev_container_build_get(build_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.state, "building");
        assert_eq!(after.step, "launching");
        assert_eq!(after.engine_operation_id, None);
        assert_eq!(engine.cleanups.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn revocation_blocks_the_next_effect_but_recovery_can_remove_the_builder() {
        let (_directory, context, store, engine, _config_guard) =
            image_fixture(BuildBehavior::RevokeAfterLaunch).await;
        let build_id = enqueue_fixture_build(&context, &store, "revoke-after-launch").await;
        wait_for_build(&store, &build_id, |build| {
            engine.waits.load(Ordering::SeqCst) > 0
                && build.step == "launching"
                && build.engine_operation_id.is_none()
        })
        .await;
        assert_eq!(engine.stops.load(Ordering::SeqCst), 0);
        let runtime: Arc<crate::access::DynDevContainerImageRuntime> = engine.clone();
        recover_interrupted(store.clone(), runtime).await;
        let recovered = wait_for_build(&store, &build_id, |build| build.state == "failed").await;
        assert_eq!(recovered.error_kind.as_deref(), Some("restart_interrupted"));
        assert_eq!(engine.cleanups.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn narrowed_base_capabilities_block_the_next_effect() {
        let (_directory, context, store, engine, _config_guard) =
            image_fixture(BuildBehavior::NarrowBaseAfterLaunch).await;
        let build_id = enqueue_fixture_build(&context, &store, "narrow-base").await;
        let build = wait_for_build(&store, &build_id, |build| {
            engine.waits.load(Ordering::SeqCst) > 0
                && build.step == "launching"
                && build.engine_operation_id.is_none()
        })
        .await;
        assert_eq!(build.state, "building");
        assert_eq!(engine.stops.load(Ordering::SeqCst), 0);
    }
}
