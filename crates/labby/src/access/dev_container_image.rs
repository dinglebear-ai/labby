//! Durable drafts and recoverable immutable Dev Container image builds.

use labby_primitives::access::{OwnerKind, OwnerScope};
use labby_runtime::dev_container_runtime::IncusProfileReference;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::error::AccessStoreResult;
use super::store::map_sqlite_error;
use super::{AccessStore, AccessStoreError, AuthorityRequest};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DevContainerTemplateDraft {
    pub template_id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub base_template_id: String,
    pub definition_json: String,
    pub max_active_instances: i64,
    pub cpu_millis: i64,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub max_lifetime_seconds: i64,
    pub revision: i64,
    pub authority_fingerprint: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct CreateTemplateDraft {
    pub template_id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub base_template_id: String,
    pub definition_json: String,
    pub requested_max_active_instances: Option<i64>,
    pub requested_cpu_millis: Option<i64>,
    pub requested_memory_bytes: Option<i64>,
    pub requested_disk_bytes: Option<i64>,
    pub requested_max_lifetime_seconds: Option<i64>,
    pub authority_fingerprint: String,
    pub now: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemplateEnvironmentEntry {
    pub name: String,
    pub value_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_reference: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DevContainerImageBuild {
    pub build_id: String,
    pub request_id: String,
    pub actor_principal_id: String,
    #[serde(skip_serializing)]
    pub identity_ref_json: String,
    #[serde(skip_serializing)]
    pub ceiling_json: String,
    pub template_id: String,
    pub source_revision: i64,
    pub source_digest: String,
    #[serde(skip_serializing)]
    pub source_snapshot_json: String,
    pub lifecycle_nonce: String,
    pub builder_instance_name: String,
    pub request_kind: String,
    pub state: String,
    pub step: String,
    pub progress: i64,
    pub engine_operation_id: Option<String>,
    pub output_image_digest: Option<String>,
    pub error_kind: Option<String>,
    pub error_summary: Option<String>,
    pub authority_fingerprint: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct EnqueueImageBuild {
    pub build_id: String,
    pub request_id: String,
    pub template_id: String,
    pub expected_revision: i64,
    pub base_image_digest: String,
    pub source_digest: String,
    pub source_snapshot_json: String,
    pub lifecycle_nonce: String,
    pub builder_instance_name: String,
    pub request_kind: String,
    pub identity_ref_json: String,
    pub ceiling_json: String,
    pub authority_fingerprint: String,
    pub now: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DevContainerBuildSource {
    pub draft: DevContainerTemplateDraft,
    pub environment: Vec<TemplateEnvironmentEntry>,
    pub base_image_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_catalog: Option<DevContainerBuildCatalogSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DevContainerBuildCatalogSnapshot {
    pub generation: String,
    pub digest: String,
    pub builder_profiles: Vec<IncusProfileReference>,
    pub runtime_network_mask: u8,
    pub runtime_profiles: Vec<IncusProfileReference>,
    pub environment: Vec<DevContainerLaunchEnvironmentEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DevContainerLaunchEnvironmentEntry {
    Literal {
        name: String,
        value: String,
    },
    SecretReference {
        name: String,
        reference: String,
        source_env: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DevContainerLaunchManifest {
    pub manifest_digest: String,
    pub template_id: String,
    pub source_build_id: String,
    pub source_revision: i64,
    pub source_digest: String,
    pub image_digest: String,
    pub catalog_generation: String,
    pub catalog_digest: String,
    pub network_mask: u8,
    pub profiles: Vec<IncusProfileReference>,
    pub environment: Vec<DevContainerLaunchEnvironmentEntry>,
}

impl DevContainerLaunchManifest {
    pub(crate) fn computed_digest(&self) -> Result<String, serde_json::Error> {
        let canonical = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "template_id": &self.template_id,
            "source_build_id": &self.source_build_id,
            "source_revision": self.source_revision,
            "source_digest": &self.source_digest,
            "image_digest": &self.image_digest,
            "catalog_generation": &self.catalog_generation,
            "catalog_digest": &self.catalog_digest,
            "network_mask": self.network_mask,
            "profiles": &self.profiles,
            "environment": &self.environment,
        }))?;
        Ok(format!("sha256:{}", hex::encode(Sha256::digest(canonical))))
    }
}

const DRAFT_COLUMNS: &str = "template_id,owner_kind,owner_id,base_template_id,definition_json,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,revision,authority_fingerprint,created_at,updated_at";
const BUILD_COLUMNS: &str = "build_id,request_id,actor_principal_id,identity_ref_json,ceiling_json,template_id,source_revision,source_digest,source_snapshot_json,lifecycle_nonce,builder_instance_name,request_kind,state,step,progress,engine_operation_id,output_image_digest,error_kind,error_summary,authority_fingerprint,created_at,updated_at";

fn decode_draft(row: &rusqlite::Row<'_>) -> rusqlite::Result<DevContainerTemplateDraft> {
    Ok(DevContainerTemplateDraft {
        template_id: row.get(0)?,
        owner_kind: row.get(1)?,
        owner_id: row.get(2)?,
        base_template_id: row.get(3)?,
        definition_json: row.get(4)?,
        max_active_instances: row.get(5)?,
        cpu_millis: row.get(6)?,
        memory_bytes: row.get(7)?,
        disk_bytes: row.get(8)?,
        max_lifetime_seconds: row.get(9)?,
        revision: row.get(10)?,
        authority_fingerprint: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn decode_build(row: &rusqlite::Row<'_>) -> rusqlite::Result<DevContainerImageBuild> {
    Ok(DevContainerImageBuild {
        build_id: row.get(0)?,
        request_id: row.get(1)?,
        actor_principal_id: row.get(2)?,
        identity_ref_json: row.get(3)?,
        ceiling_json: row.get(4)?,
        template_id: row.get(5)?,
        source_revision: row.get(6)?,
        source_digest: row.get(7)?,
        source_snapshot_json: row.get(8)?,
        lifecycle_nonce: row.get(9)?,
        builder_instance_name: row.get(10)?,
        request_kind: row.get(11)?,
        state: row.get(12)?,
        step: row.get(13)?,
        progress: row.get(14)?,
        engine_operation_id: row.get(15)?,
        output_image_digest: row.get(16)?,
        error_kind: row.get(17)?,
        error_summary: row.get(18)?,
        authority_fingerprint: row.get(19)?,
        created_at: row.get(20)?,
        updated_at: row.get(21)?,
    })
}

impl AccessStore {
    pub(crate) async fn dev_container_build_source(
        &self,
        template_id: String,
    ) -> AccessStoreResult<Option<DevContainerBuildSource>> {
        self.with_connection(move |connection| {
            let draft = connection
                .query_row(
                    &format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),
                    [&template_id],
                    decode_draft,
                )
                .optional()
                .map_err(map_sqlite_error)?;
            let Some(draft) = draft else { return Ok(None); };
            let base_image_digest = connection
                .query_row(
                    "SELECT image_digest FROM dev_container_templates WHERE template_id=?1 AND status='approved'",
                    [&draft.base_template_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(map_sqlite_error)?
                .ok_or(AccessStoreError::NotAuthorized)?;
            let mut statement = connection.prepare("SELECT name,value_kind,literal_value,secret_reference FROM dev_container_template_environment WHERE template_id=?1 ORDER BY name").map_err(map_sqlite_error)?;
            let environment = statement.query_map([&template_id],|row|Ok(TemplateEnvironmentEntry{name:row.get(0)?,value_kind:row.get(1)?,literal_value:row.get(2)?,secret_reference:row.get(3)?})).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)?;
            Ok(Some(DevContainerBuildSource {
                draft,
                environment,
                base_image_digest,
                build_catalog: None,
            }))
        }).await
    }
    pub(crate) async fn dev_container_draft_get(
        &self,
        template_id: String,
    ) -> AccessStoreResult<Option<DevContainerTemplateDraft>> {
        self.with_connection(move |connection| {
            connection.query_row(
                &format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),
                [template_id], decode_draft,
            ).optional().map_err(map_sqlite_error)
        }).await
    }

    pub(crate) async fn dev_container_draft_page(
        &self,
        owner_kind: String,
        owner_id: String,
        after: String,
        limit: usize,
    ) -> AccessStoreResult<Vec<DevContainerTemplateDraft>> {
        self.with_connection(move |connection| {
            let mut statement = connection.prepare(&format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE owner_kind=?1 AND owner_id=?2 AND template_id>?3 ORDER BY template_id LIMIT ?4")).map_err(map_sqlite_error)?;
            statement.query_map(params![owner_kind, owner_id, after, i64::try_from(limit).unwrap_or(100)], decode_draft).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)
        }).await
    }

    pub(crate) async fn dev_container_draft_create(
        &self,
        input: CreateTemplateDraft,
        request: AuthorityRequest,
    ) -> AccessStoreResult<DevContainerTemplateDraft> {
        self.with_connection(move |connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&transaction, request)?;
            if lease.binding().resource_id().as_str() != input.template_id
                || !binding_owner_matches(lease.binding().owner_scope(), &input.owner_kind, &input.owner_id)
            { return Err(AccessStoreError::NotAuthorized); }
            let base: Option<(i64,i64,i64,i64,i64)> = transaction.query_row("SELECT max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds FROM dev_container_templates WHERE template_id=?1 AND status='approved'", [&input.base_template_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(map_sqlite_error)?;
            let (max_active,cpu,memory,disk,lifetime)=base.ok_or(AccessStoreError::NotAuthorized)?;
            let clamp = |requested: Option<i64>, ceiling: i64| requested.unwrap_or(ceiling).clamp(1, ceiling);
            transaction.execute("INSERT INTO dev_container_template_drafts(template_id,owner_kind,owner_id,base_template_id,definition_json,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,revision,authority_fingerprint,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11,?12,?12)", params![input.template_id,input.owner_kind,input.owner_id,input.base_template_id,input.definition_json,clamp(input.requested_max_active_instances,max_active),clamp(input.requested_cpu_millis,cpu),clamp(input.requested_memory_bytes,memory),clamp(input.requested_disk_bytes,disk),clamp(input.requested_max_lifetime_seconds,lifetime),input.authority_fingerprint,input.now]).map_err(map_sqlite_error)?;
            let draft=transaction.query_row(&format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),[lease.binding().resource_id().as_str()],decode_draft).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?;
            Ok(draft)
        }).await
    }

    pub(crate) async fn dev_container_draft_update(
        &self,
        template_id: String,
        expected_revision: i64,
        definition_json: String,
        authority_fingerprint: String,
        now: i64,
        request: AuthorityRequest,
    ) -> AccessStoreResult<DevContainerTemplateDraft> {
        self.with_connection(move |connection| {
            let transaction=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease=super::authority::authorize_action_in_transaction(&transaction,request)?;
            let owner:Option<(String,String)>=transaction.query_row("SELECT owner_kind,owner_id FROM dev_container_template_drafts WHERE template_id=?1",[&template_id],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(map_sqlite_error)?;
            let (kind,id)=owner.ok_or(AccessStoreError::NotAuthorized)?;
            if lease.binding().resource_id().as_str()!=template_id || !binding_owner_matches(lease.binding().owner_scope(),&kind,&id){return Err(AccessStoreError::NotAuthorized);}
            let changed=transaction.execute("UPDATE dev_container_template_drafts SET definition_json=?2,revision=revision+1,authority_fingerprint=?3,updated_at=?4 WHERE template_id=?1 AND revision=?5",params![template_id,definition_json,authority_fingerprint,now,expected_revision]).map_err(map_sqlite_error)?;
            if changed!=1{return Err(AccessStoreError::NotAuthorized);}
            let draft=transaction.query_row(&format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),[&template_id],decode_draft).map_err(map_sqlite_error)?;
            transaction.commit().map_err(map_sqlite_error)?; Ok(draft)
        }).await
    }

    pub(crate) async fn dev_container_environment_replace(
        &self,
        template_id: String,
        expected_revision: i64,
        entries: Vec<TemplateEnvironmentEntry>,
        authority_fingerprint: String,
        now: i64,
        request: AuthorityRequest,
    ) -> AccessStoreResult<DevContainerTemplateDraft> {
        self.with_connection(move|connection|{
            let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease=super::authority::authorize_action_in_transaction(&tx,request)?;
            let owner:Option<(String,String)>=tx.query_row("SELECT owner_kind,owner_id FROM dev_container_template_drafts WHERE template_id=?1 AND revision=?2",params![template_id,expected_revision],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(map_sqlite_error)?;
            let(kind,id)=owner.ok_or(AccessStoreError::NotAuthorized)?;
            if lease.binding().resource_id().as_str()!=template_id || !binding_owner_matches(lease.binding().owner_scope(),&kind,&id){return Err(AccessStoreError::NotAuthorized);}
            tx.execute("DELETE FROM dev_container_template_environment WHERE template_id=?1",[&template_id]).map_err(map_sqlite_error)?;
            for entry in entries { tx.execute("INSERT INTO dev_container_template_environment(template_id,name,value_kind,literal_value,secret_reference) VALUES(?1,?2,?3,?4,?5)",params![template_id,entry.name,entry.value_kind,entry.literal_value,entry.secret_reference]).map_err(map_sqlite_error)?; }
            tx.execute("UPDATE dev_container_template_drafts SET revision=revision+1,authority_fingerprint=?2,updated_at=?3 WHERE template_id=?1",params![template_id,authority_fingerprint,now]).map_err(map_sqlite_error)?;
            let draft=tx.query_row(&format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),[&template_id],decode_draft).map_err(map_sqlite_error)?;
            tx.commit().map_err(map_sqlite_error)?; Ok(draft)
        }).await
    }

    pub(crate) async fn dev_container_environment_get(
        &self,
        template_id: String,
    ) -> AccessStoreResult<Vec<TemplateEnvironmentEntry>> {
        self.with_connection(move|connection|{let mut statement=connection.prepare("SELECT name,value_kind,literal_value,secret_reference FROM dev_container_template_environment WHERE template_id=?1 ORDER BY name").map_err(map_sqlite_error)?;statement.query_map([template_id],|row|Ok(TemplateEnvironmentEntry{name:row.get(0)?,value_kind:row.get(1)?,literal_value:row.get(2)?,secret_reference:row.get(3)?})).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)}).await
    }

    pub(crate) async fn dev_container_build_enqueue(
        &self,
        input: EnqueueImageBuild,
        request: AuthorityRequest,
    ) -> AccessStoreResult<DevContainerImageBuild> {
        self.with_connection(move|connection|{
            let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease=super::authority::authorize_action_in_transaction(&tx,request)?;
            let draft:Option<DevContainerTemplateDraft>=tx.query_row(&format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),[&input.template_id],decode_draft).optional().map_err(map_sqlite_error)?;
            let draft=draft.ok_or(AccessStoreError::NotAuthorized)?;
            if lease.binding().resource_id().as_str()!=input.template_id || !binding_owner_matches(lease.binding().owner_scope(),&draft.owner_kind,&draft.owner_id) || draft.revision!=input.expected_revision{return Err(AccessStoreError::NotAuthorized);}
            let base:Option<(String,i64,i64,i64,i64,i64)>=tx.query_row("SELECT image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds FROM dev_container_templates WHERE template_id=?1 AND status='approved'",[&draft.base_template_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional().map_err(map_sqlite_error)?;
            let Some((base_digest,max_active,cpu,memory,disk,lifetime))=base else{return Err(AccessStoreError::NotAuthorized);};
            if base_digest!=input.base_image_digest || draft.max_active_instances>max_active || draft.cpu_millis>cpu || draft.memory_bytes>memory || draft.disk_bytes>disk || draft.max_lifetime_seconds>lifetime{return Err(AccessStoreError::NotAuthorized);}
            let existing:Option<DevContainerImageBuild>=tx.query_row(&format!("SELECT {BUILD_COLUMNS} FROM dev_container_image_builds WHERE actor_principal_id=?1 AND request_id=?2"),params![lease.binding().principal_id(),input.request_id],decode_build).optional().map_err(map_sqlite_error)?;
            if let Some(existing)=existing { if existing.template_id==input.template_id && existing.source_revision==input.expected_revision && existing.source_digest==input.source_digest && existing.request_kind==input.request_kind {tx.commit().map_err(map_sqlite_error)?;return Ok(existing);} return Err(AccessStoreError::NotAuthorized); }
            tx.execute("INSERT INTO dev_container_image_builds(build_id,request_id,actor_principal_id,identity_ref_json,ceiling_json,template_id,source_revision,source_digest,source_snapshot_json,lifecycle_nonce,builder_instance_name,request_kind,state,step,progress,authority_fingerprint,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'queued','queued',0,?13,?14,?14)",params![input.build_id,input.request_id,lease.binding().principal_id(),input.identity_ref_json,input.ceiling_json,input.template_id,input.expected_revision,input.source_digest,input.source_snapshot_json,input.lifecycle_nonce,input.builder_instance_name,input.request_kind,input.authority_fingerprint,input.now]).map_err(map_sqlite_error)?;
            let build=tx.query_row(&format!("SELECT {BUILD_COLUMNS} FROM dev_container_image_builds WHERE build_id=?1"),[&input.build_id],decode_build).map_err(map_sqlite_error)?;
            tx.commit().map_err(map_sqlite_error)?;Ok(build)
        }).await
    }

    pub(crate) async fn dev_container_build_get(
        &self,
        build_id: String,
    ) -> AccessStoreResult<Option<DevContainerImageBuild>> {
        self.with_connection(move |connection| {
            connection
                .query_row(
                    &format!(
                        "SELECT {BUILD_COLUMNS} FROM dev_container_image_builds WHERE build_id=?1"
                    ),
                    [build_id],
                    decode_build,
                )
                .optional()
                .map_err(map_sqlite_error)
        })
        .await
    }

    pub(crate) async fn dev_container_build_active(
        &self,
        after_build_id: String,
    ) -> AccessStoreResult<Vec<DevContainerImageBuild>> {
        self.with_connection(move |connection| {
            let mut statement = connection.prepare(&format!("SELECT {BUILD_COLUMNS} FROM dev_container_image_builds WHERE state IN ('queued','building','paused') AND build_id>?1 ORDER BY build_id LIMIT 100")).map_err(map_sqlite_error)?;
            statement.query_map([after_build_id],decode_build).map_err(map_sqlite_error)?.collect::<rusqlite::Result<Vec<_>>>().map_err(map_sqlite_error)
        }).await
    }

    /// Persists the next external effect before it is submitted. A matching
    /// step with no operation ID remains active because the engine may have
    /// accepted the request even if its response was lost.
    pub(crate) async fn dev_container_build_prepare_effect(
        &self,
        build_id: String,
        expected_step: String,
        next_step: String,
        progress: i64,
        catalog_generation: String,
        catalog_digest: String,
        now: i64,
        request: AuthorityRequest,
    ) -> AccessStoreResult<()> {
        self.with_connection(move|connection|{
            let tx=connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(map_sqlite_error)?;
            let lease=super::authority::authorize_action_in_transaction(&tx,request)?;
            verify_build_binding(&tx,&lease,&build_id)?;
            let build:DevContainerImageBuild=tx.query_row(&format!("SELECT {BUILD_COLUMNS} FROM dev_container_image_builds WHERE build_id=?1"),[&build_id],decode_build).map_err(map_sqlite_error)?;
            if !build_source_is_current(&tx,&build)? || !build_catalog_matches(&build,&catalog_generation,&catalog_digest)?{return Err(AccessStoreError::NotAuthorized);}
            let changed=tx.execute("UPDATE dev_container_image_builds SET state='building',step=?3,progress=?4,engine_operation_id=NULL,started_at=coalesce(started_at,?5),updated_at=?5 WHERE build_id=?1 AND step=?2 AND engine_operation_id IS NULL AND state IN ('queued','building')",params![build_id,expected_step,next_step,progress,now]).map_err(map_sqlite_error)?;
            if changed!=1{return Err(AccessStoreError::NotAuthorized);}
            tx.commit().map_err(map_sqlite_error)?;Ok(())
        }).await
    }

    pub(crate) async fn dev_container_published_image_get(
        &self,
        template_id: String,
    ) -> AccessStoreResult<Option<String>> {
        self.with_connection(move|connection|connection.query_row("SELECT image_digest FROM dev_container_published_images WHERE template_id=?1 AND is_current=1",[template_id],|row|row.get(0)).optional().map_err(map_sqlite_error)).await
    }

    pub(crate) async fn dev_container_build_record_operation(
        &self,
        build_id: String,
        prepared_step: String,
        progress: i64,
        operation_id: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move|connection|{
            let changed=connection.execute(
                "UPDATE dev_container_image_builds SET engine_operation_id=?4,updated_at=?5 WHERE build_id=?1 AND step=?2 AND progress=?3 AND engine_operation_id IS NULL AND state='building'",
                params![build_id,prepared_step,progress,operation_id,now],
            ).map_err(map_sqlite_error)?;
            if changed!=1{return Err(AccessStoreError::NotAuthorized);}
            Ok(())
        }).await
    }

    /// Persists recovery cleanup/discard intent before the exact, ownership-
    /// verified engine effect is submitted. Recovery remains allowed after
    /// revocation so managed resources cannot outlive their former authority.
    pub(crate) async fn dev_container_build_prepare_recovery_effect(
        &self,
        build_id: String,
        expected_operation_id: Option<String>,
        progress: i64,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let changed = match expected_operation_id {
                Some(expected) => connection.execute(
                    "UPDATE dev_container_image_builds SET state='building',step='cleanup',progress=?3,engine_operation_id=NULL,updated_at=?4 WHERE build_id=?1 AND engine_operation_id=?2 AND state IN ('queued','building','paused')",
                    params![build_id, expected, progress, now],
                ),
                None => connection.execute(
                    "UPDATE dev_container_image_builds SET state='building',step='cleanup',progress=?2,engine_operation_id=NULL,updated_at=?3 WHERE build_id=?1 AND engine_operation_id IS NULL AND state IN ('queued','building','paused')",
                    params![build_id, progress, now],
                ),
            }.map_err(map_sqlite_error)?;
            if changed != 1 { return Err(AccessStoreError::NotAuthorized); }
            Ok(())
        }).await
    }

    /// Attaches the engine operation returned for a previously persisted
    /// recovery effect. This records an observation and does not grant new
    /// authority.
    pub(crate) async fn dev_container_build_record_recovery_cleanup(
        &self,
        build_id: String,
        progress: i64,
        operation_id: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let changed = connection.execute(
                "UPDATE dev_container_image_builds SET engine_operation_id=?3,updated_at=?4 WHERE build_id=?1 AND step='cleanup' AND progress=?2 AND engine_operation_id IS NULL AND state='building'",
                params![build_id, progress, operation_id, now],
            ).map_err(map_sqlite_error)?;
            if changed != 1 { return Err(AccessStoreError::NotAuthorized); }
            Ok(())
        }).await
    }

    pub(crate) async fn dev_container_build_clear_operation(
        &self,
        build_id: String,
        expected_operation_id: String,
        next_step: String,
        progress: i64,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move|connection|{
            let changed=connection.execute("UPDATE dev_container_image_builds SET step=?3,progress=?4,engine_operation_id=NULL,updated_at=?5 WHERE build_id=?1 AND engine_operation_id=?2 AND state='building'",params![build_id,expected_operation_id,next_step,progress,now]).map_err(map_sqlite_error)?;
            if changed!=1{return Err(AccessStoreError::NotAuthorized);}
            Ok(())
        }).await
    }

    pub(crate) async fn dev_container_build_succeed(
        &self,
        build_id: String,
        image_digest: String,
        catalog_generation: String,
        catalog_digest: String,
        now: i64,
        request: AuthorityRequest,
    ) -> AccessStoreResult<()> {
        self.with_connection(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            let lease = super::authority::authorize_action_in_transaction(&tx, request)?;
            verify_build_binding(&tx, &lease, &build_id)?;
            let build = tx
                .query_row(
                    &format!("SELECT {BUILD_COLUMNS} FROM dev_container_image_builds WHERE build_id=?1"),
                    [&build_id],
                    decode_build,
                )
                .map_err(map_sqlite_error)?;
            let draft = tx
                .query_row(
                    &format!("SELECT {DRAFT_COLUMNS} FROM dev_container_template_drafts WHERE template_id=?1"),
                    [&build.template_id],
                    decode_draft,
                )
                .map_err(map_sqlite_error)?;
            if build.state != "building"
                || build.step != "cleanup"
                || build.engine_operation_id.is_some()
                || draft.revision != build.source_revision
                || !build_source_is_current(&tx, &build)?
                || !build_catalog_matches(&build, &catalog_generation, &catalog_digest)?
            {
                return Err(AccessStoreError::NotAuthorized);
            }
            let manifest = launch_manifest_for_build(&build, &image_digest)?;
            let profiles_json = serde_json::to_string(&manifest.profiles)
                .map_err(|_| AccessStoreError::NotAuthorized)?;
            let environment_json = serde_json::to_string(&manifest.environment)
                .map_err(|_| AccessStoreError::NotAuthorized)?;
            let publication = tx
                .query_row(
                    "SELECT coalesce(max(publication_revision),0)+1 FROM dev_container_published_images WHERE template_id=?1",
                    [&build.template_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_sqlite_error)?;
            tx.execute(
                "UPDATE dev_container_published_images SET is_current=0 WHERE template_id=?1",
                [&build.template_id],
            )
            .map_err(map_sqlite_error)?;
            tx.execute(
                "INSERT INTO dev_container_published_images(template_id,publication_revision,source_revision,source_digest,image_digest,build_id,is_current,published_at) VALUES(?1,?2,?3,?4,?5,?6,1,?7)",
                params![build.template_id,publication,build.source_revision,build.source_digest,image_digest,build_id,now],
            )
            .map_err(map_sqlite_error)?;
            tx.execute(
                "INSERT INTO dev_container_launch_manifests(manifest_digest,template_id,source_build_id,source_revision,source_digest,image_digest,catalog_generation,catalog_digest,network_mask,profiles_json,environment_json,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![manifest.manifest_digest,manifest.template_id,manifest.source_build_id,manifest.source_revision,manifest.source_digest,manifest.image_digest,manifest.catalog_generation,manifest.catalog_digest,manifest.network_mask,profiles_json,environment_json,now],
            )
            .map_err(map_sqlite_error)?;
            tx.execute(
                "INSERT INTO dev_container_templates(template_id,image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,max_lifetime_seconds,host_capabilities_json,status,policy_epoch,created_at,updated_at,launch_manifest_digest) VALUES(?1,?2,?3,?4,?5,?6,?7,'[]','approved',1,?8,?8,?9) ON CONFLICT(template_id) DO UPDATE SET image_digest=excluded.image_digest,max_active_instances=excluded.max_active_instances,cpu_millis=excluded.cpu_millis,memory_bytes=excluded.memory_bytes,disk_bytes=excluded.disk_bytes,max_lifetime_seconds=excluded.max_lifetime_seconds,status='approved',policy_epoch=dev_container_templates.policy_epoch+1,updated_at=excluded.updated_at,launch_manifest_digest=excluded.launch_manifest_digest",
                params![draft.template_id,image_digest,draft.max_active_instances,draft.cpu_millis,draft.memory_bytes,draft.disk_bytes,draft.max_lifetime_seconds,now,manifest.manifest_digest],
            )
            .map_err(map_sqlite_error)?;
            let changed = tx
                .execute(
                    "UPDATE dev_container_image_builds SET state='succeeded',step='complete',progress=100,output_image_digest=?2,completed_at=?3,updated_at=?3 WHERE build_id=?1 AND state='building' AND step='cleanup' AND engine_operation_id IS NULL",
                    params![build_id,image_digest,now],
                )
                .map_err(map_sqlite_error)?;
            if changed != 1 {
                return Err(AccessStoreError::NotAuthorized);
            }
            tx.commit().map_err(map_sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn dev_container_build_fail(
        &self,
        build_id: String,
        error_kind: String,
        error_summary: String,
        now: i64,
    ) -> AccessStoreResult<()> {
        self.with_connection(move|connection|{connection.execute("UPDATE dev_container_image_builds SET state='failed',step='complete',engine_operation_id=NULL,error_kind=?2,error_summary=?3,completed_at=?4,updated_at=?4 WHERE build_id=?1 AND state IN ('queued','building','paused')",params![build_id,error_kind,error_summary,now]).map_err(map_sqlite_error)?;Ok(())}).await
    }
}

fn launch_manifest_for_build(
    build: &DevContainerImageBuild,
    image_digest: &str,
) -> AccessStoreResult<DevContainerLaunchManifest> {
    let source = serde_json::from_str::<DevContainerBuildSource>(&build.source_snapshot_json)
        .map_err(|_| AccessStoreError::NotAuthorized)?;
    let catalog = source
        .build_catalog
        .ok_or(AccessStoreError::NotAuthorized)?;
    let mut manifest = DevContainerLaunchManifest {
        manifest_digest: String::new(),
        template_id: build.template_id.clone(),
        source_build_id: build.build_id.clone(),
        source_revision: build.source_revision,
        source_digest: build.source_digest.clone(),
        image_digest: image_digest.to_owned(),
        catalog_generation: catalog.generation,
        catalog_digest: catalog.digest,
        network_mask: catalog.runtime_network_mask,
        profiles: catalog.runtime_profiles,
        environment: catalog.environment,
    };
    manifest.manifest_digest = manifest
        .computed_digest()
        .map_err(|_| AccessStoreError::NotAuthorized)?;
    Ok(manifest)
}

fn build_catalog_matches(
    build: &DevContainerImageBuild,
    generation: &str,
    digest: &str,
) -> AccessStoreResult<bool> {
    let source = serde_json::from_str::<DevContainerBuildSource>(&build.source_snapshot_json)
        .map_err(|_| AccessStoreError::NotAuthorized)?;
    Ok(source
        .build_catalog
        .is_some_and(|catalog| catalog.generation == generation && catalog.digest == digest))
}

fn build_source_is_current(
    tx: &rusqlite::Transaction<'_>,
    build: &DevContainerImageBuild,
) -> AccessStoreResult<bool> {
    let expected_digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(build.source_snapshot_json.as_bytes()))
    );
    if expected_digest != build.source_digest {
        return Ok(false);
    }
    let source = serde_json::from_str::<DevContainerBuildSource>(&build.source_snapshot_json)
        .map_err(|_| AccessStoreError::NotAuthorized)?;
    if source.draft.template_id != build.template_id
        || source.draft.revision != build.source_revision
    {
        return Ok(false);
    }
    let current: Option<(i64, String, i64, i64, i64, i64, i64)> = tx
        .query_row(
            "SELECT d.revision,b.image_digest,b.max_active_instances,b.cpu_millis,b.memory_bytes,b.disk_bytes,b.max_lifetime_seconds FROM dev_container_template_drafts d JOIN dev_container_templates b ON b.template_id=d.base_template_id AND b.status='approved' WHERE d.template_id=?1",
            [&build.template_id],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    Ok(current.is_some_and(
        |(revision, digest, max_active, cpu, memory, disk, lifetime)| {
            revision == source.draft.revision
                && digest == source.base_image_digest
                && source.draft.max_active_instances <= max_active
                && source.draft.cpu_millis <= cpu
                && source.draft.memory_bytes <= memory
                && source.draft.disk_bytes <= disk
                && source.draft.max_lifetime_seconds <= lifetime
        },
    ))
}

fn verify_build_binding(
    tx: &rusqlite::Transaction<'_>,
    lease: &labby_runtime::authority::AuthorityLease,
    build_id: &str,
) -> AccessStoreResult<()> {
    let row: Option<(String, String, String)> = tx
        .query_row(
            "SELECT d.owner_kind,d.owner_id,b.template_id FROM dev_container_image_builds b JOIN dev_container_template_drafts d ON d.template_id=b.template_id WHERE b.build_id=?1",
            [build_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    let (kind, id, template) = row.ok_or(AccessStoreError::NotAuthorized)?;
    if lease.binding().resource_id().as_str() != template
        || !binding_owner_matches(lease.binding().owner_scope(), &kind, &id)
    {
        return Err(AccessStoreError::NotAuthorized);
    }
    Ok(())
}

fn binding_owner_matches(owner: &OwnerScope, kind: &str, id: &str) -> bool {
    matches!(
        (owner, kind),
        (OwnerScope::Installation(_), "installation")
            | (OwnerScope::Team(_), "team")
            | (OwnerScope::Project(_), "project")
            | (OwnerScope::Personal(_), "personal")
    ) && owner_id(owner) == id
}
fn owner_id(owner: &OwnerScope) -> &str {
    match owner {
        OwnerScope::Installation(value) => value.as_str(),
        OwnerScope::Team(value) => value.as_str(),
        OwnerScope::Project(value) => value.as_str(),
        OwnerScope::Personal(value) => value.as_str(),
    }
}

pub(crate) fn owner_kind_wire(kind: OwnerKind) -> &'static str {
    match kind {
        OwnerKind::Installation => "installation",
        OwnerKind::Team => "team",
        OwnerKind::Project => "project",
        OwnerKind::Personal => "personal",
    }
}
