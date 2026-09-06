//! Durable Dev Container contract ledger. No container runtime work belongs here.

use labby_primitives::access::{OwnerKind, OwnerScope};
use labby_primitives::dev_container::{
    ApprovedTemplate, DesiredState, HostCapability, ObservedState, OwnedDevContainer,
    SecretReference,
};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

pub(super) const DEV_CONTAINER_SCHEMA: &str = "
CREATE TABLE dev_container_templates (
    template_id TEXT PRIMARY KEY CHECK(length(trim(template_id)) > 0),
    image_digest TEXT NOT NULL CHECK(length(image_digest) = 71 AND substr(image_digest,1,7) = 'sha256:' AND substr(image_digest,8) NOT GLOB '*[^0-9a-f]*'),
    max_active_instances INTEGER NOT NULL CHECK(max_active_instances > 0),
    cpu_millis INTEGER NOT NULL CHECK(cpu_millis > 0),
    memory_bytes INTEGER NOT NULL CHECK(memory_bytes > 0),
    disk_bytes INTEGER NOT NULL CHECK(disk_bytes > 0),
    max_lifetime_seconds INTEGER NOT NULL CHECK(max_lifetime_seconds > 0),
    host_capabilities_json TEXT NOT NULL CHECK(json_valid(host_capabilities_json)),
    status TEXT NOT NULL CHECK(status IN ('approved','revoked')),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE dev_container_owner_quotas (
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),
    owner_id TEXT NOT NULL CHECK(length(trim(owner_id)) > 0),
    max_active_instances INTEGER NOT NULL CHECK(max_active_instances > 0),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(owner_kind, owner_id)
) STRICT;

CREATE TABLE dev_container_instances (
    instance_id TEXT PRIMARY KEY CHECK(length(trim(instance_id)) > 0),
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('installation','team','project','personal')),
    owner_id TEXT NOT NULL CHECK(length(trim(owner_id)) > 0),
    template_id TEXT NOT NULL,
    image_digest TEXT NOT NULL CHECK(length(image_digest) = 71 AND substr(image_digest,1,7) = 'sha256:' AND substr(image_digest,8) NOT GLOB '*[^0-9a-f]*'),
    lifecycle_nonce TEXT NOT NULL UNIQUE CHECK(length(lifecycle_nonce) BETWEEN 32 AND 128),
    desired_state TEXT NOT NULL CHECK(desired_state IN ('running','stopped','deleted')),
    observed_state TEXT NOT NULL CHECK(observed_state IN ('pending','starting','running','stopping','stopped','failed','deleted')),
    cpu_millis INTEGER NOT NULL CHECK(cpu_millis > 0),
    memory_bytes INTEGER NOT NULL CHECK(memory_bytes > 0),
    disk_bytes INTEGER NOT NULL CHECK(disk_bytes > 0),
    lifetime_seconds INTEGER NOT NULL CHECK(lifetime_seconds > 0),
    secret_references_json TEXT NOT NULL CHECK(json_valid(secret_references_json)),
    authority_fingerprint TEXT NOT NULL CHECK(length(trim(authority_fingerprint)) > 0),
    revision INTEGER NOT NULL CHECK(revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER,
    CHECK ((desired_state = 'deleted') = (deleted_at IS NOT NULL)),
    FOREIGN KEY(template_id) REFERENCES dev_container_templates(template_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX dev_container_instances_owner_state
    ON dev_container_instances(owner_kind, owner_id, desired_state, observed_state, instance_id);

CREATE TABLE dev_container_ledger (
    event_id TEXT PRIMARY KEY CHECK(length(trim(event_id)) > 0),
    instance_id TEXT NOT NULL,
    lifecycle_nonce TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    event_kind TEXT NOT NULL CHECK(event_kind IN ('created','desired_changed','observed_changed','reconciled')),
    occurred_at INTEGER NOT NULL,
    detail_json TEXT NOT NULL CHECK(json_valid(detail_json) AND length(detail_json) <= 4096),
    UNIQUE(instance_id, revision),
    FOREIGN KEY(instance_id) REFERENCES dev_container_instances(instance_id) ON DELETE RESTRICT
) STRICT;
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReservedResources {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub lifetime_seconds: u64,
}

#[derive(Clone, Debug)]
pub(super) struct CreateInstance<'a> {
    pub instance: &'a OwnedDevContainer,
    pub resources: ReservedResources,
    pub authority_fingerprint: &'a str,
    pub event_id: &'a str,
    pub occurred_at: i64,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub(super) enum DevContainerLedgerError {
    #[error("Dev Container ledger input is invalid")]
    InvalidInput,
    #[error("Dev Container template is unavailable")]
    TemplateUnavailable,
    #[error("Dev Container owner quota is exhausted")]
    QuotaExhausted,
    #[error("Dev Container ledger storage failed")]
    Storage,
}

pub(super) fn install_schema(connection: &Connection) -> Result<(), DevContainerLedgerError> {
    connection
        .execute_batch(DEV_CONTAINER_SCHEMA)
        .map_err(|_| DevContainerLedgerError::Storage)
}

pub(super) fn approve_template(
    connection: &Connection,
    template: &ApprovedTemplate,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    let quota = template.quota_ceiling();
    let memory_bytes = sqlite_u64(quota.memory_bytes)?;
    let disk_bytes = sqlite_u64(quota.disk_bytes)?;
    let max_lifetime_seconds = sqlite_u64(quota.max_lifetime_seconds)?;
    let host_capabilities = template
        .host_capabilities()
        .values()
        .iter()
        .map(|capability| host_capability_name(*capability))
        .collect::<Vec<_>>();
    let host_capabilities = serde_json::to_string(&host_capabilities)
        .map_err(|_| DevContainerLedgerError::InvalidInput)?;
    connection
        .execute(
            "INSERT INTO dev_container_templates(
                template_id,image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,
                max_lifetime_seconds,host_capabilities_json,status,policy_epoch,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'approved',1,?9,?9)
             ON CONFLICT(template_id) DO UPDATE SET
                image_digest=excluded.image_digest,
                max_active_instances=excluded.max_active_instances,
                cpu_millis=excluded.cpu_millis,
                memory_bytes=excluded.memory_bytes,
                disk_bytes=excluded.disk_bytes,
                max_lifetime_seconds=excluded.max_lifetime_seconds,
                host_capabilities_json=excluded.host_capabilities_json,
                status='approved',policy_epoch=policy_epoch+1,updated_at=excluded.updated_at",
            params![
                template.id().as_str(),
                template.image().as_str(),
                quota.max_active_instances,
                quota.cpu_millis,
                memory_bytes,
                disk_bytes,
                max_lifetime_seconds,
                host_capabilities,
                now,
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    Ok(())
}

pub(super) fn set_owner_quota(
    connection: &Connection,
    owner: &OwnerScope,
    max_active_instances: u32,
    now: i64,
) -> Result<(), DevContainerLedgerError> {
    if max_active_instances == 0 || now < 0 {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    connection
        .execute(
            "INSERT INTO dev_container_owner_quotas(
                owner_kind,owner_id,max_active_instances,policy_epoch,updated_at)
             VALUES(?1,?2,?3,1,?4)
             ON CONFLICT(owner_kind,owner_id) DO UPDATE SET
                max_active_instances=excluded.max_active_instances,
                policy_epoch=policy_epoch+1,updated_at=excluded.updated_at",
            params![
                owner_kind_name(owner.kind()),
                owner.id(),
                max_active_instances,
                now
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    Ok(())
}

pub(super) fn create_instance(
    connection: &mut Connection,
    input: &CreateInstance<'_>,
) -> Result<(), DevContainerLedgerError> {
    validate_create(input)?;
    let transaction = connection
        .transaction()
        .map_err(|_| DevContainerLedgerError::Storage)?;
    let owner = input.instance.owner();
    let owner_kind = owner_kind_name(owner.kind());
    let template = transaction
        .query_row(
            "SELECT image_digest,max_active_instances,cpu_millis,memory_bytes,disk_bytes,
                    max_lifetime_seconds,status
             FROM dev_container_templates WHERE template_id=?1",
            [input.instance.template_id().as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|_| DevContainerLedgerError::Storage)?
        .filter(|template| template.6 == "approved")
        .ok_or(DevContainerLedgerError::TemplateUnavailable)?;
    if template.0 != input.instance.image().as_str()
        || input.resources.cpu_millis > template.2
        || sqlite_u64(input.resources.memory_bytes)? > template.3
        || sqlite_u64(input.resources.disk_bytes)? > template.4
        || sqlite_u64(input.resources.lifetime_seconds)? > template.5
    {
        return Err(DevContainerLedgerError::TemplateUnavailable);
    }
    let owner_max_active = transaction
        .query_row(
            "SELECT max_active_instances FROM dev_container_owner_quotas
             WHERE owner_kind=?1 AND owner_id=?2",
            params![owner_kind, owner.id()],
            |row| row.get::<_, u32>(0),
        )
        .optional()
        .map_err(|_| DevContainerLedgerError::Storage)?
        .ok_or(DevContainerLedgerError::QuotaExhausted)?;
    let active = transaction
        .query_row(
            "SELECT count(*) FROM dev_container_instances
             WHERE owner_kind=?1 AND owner_id=?2
               AND observed_state IN ('pending','starting','running','stopping')",
            params![owner_kind, owner.id()],
            |row| row.get::<_, u32>(0),
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    if active >= template.1.min(owner_max_active) {
        return Err(DevContainerLedgerError::QuotaExhausted);
    }
    let secret_references = input
        .instance
        .secret_references()
        .iter()
        .map(SecretReference::as_str)
        .collect::<Vec<_>>();
    let secret_references = serde_json::to_string(&secret_references)
        .map_err(|_| DevContainerLedgerError::InvalidInput)?;
    let memory_bytes = sqlite_u64(input.resources.memory_bytes)?;
    let disk_bytes = sqlite_u64(input.resources.disk_bytes)?;
    let lifetime_seconds = sqlite_u64(input.resources.lifetime_seconds)?;
    transaction
        .execute(
            "INSERT INTO dev_container_instances(
                instance_id,owner_kind,owner_id,template_id,image_digest,lifecycle_nonce,
                desired_state,observed_state,cpu_millis,memory_bytes,disk_bytes,lifetime_seconds,
                secret_references_json,authority_fingerprint,revision,created_at,updated_at,deleted_at)
             VALUES(?1,?2,?3,?4,?5,?6,'running','pending',?7,?8,?9,?10,?11,?12,1,?13,?13,NULL)",
            params![
                input.instance.id().as_str(),
                owner_kind,
                owner.id(),
                input.instance.template_id().as_str(),
                input.instance.image().as_str(),
                input.instance.lifecycle_nonce().as_str(),
                input.resources.cpu_millis,
                memory_bytes,
                disk_bytes,
                lifetime_seconds,
                secret_references,
                input.authority_fingerprint,
                input.occurred_at,
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .execute(
            "INSERT INTO dev_container_ledger(
                event_id,instance_id,lifecycle_nonce,revision,event_kind,occurred_at,detail_json)
             VALUES(?1,?2,?3,1,'created',?4,'{}')",
            params![
                input.event_id,
                input.instance.id().as_str(),
                input.instance.lifecycle_nonce().as_str(),
                input.occurred_at,
            ],
        )
        .map_err(|_| DevContainerLedgerError::Storage)?;
    transaction
        .commit()
        .map_err(|_| DevContainerLedgerError::Storage)
}

fn validate_create(input: &CreateInstance<'_>) -> Result<(), DevContainerLedgerError> {
    if input.authority_fingerprint.trim().is_empty()
        || input.event_id.trim().is_empty()
        || input.occurred_at < 0
        || input.resources.cpu_millis == 0
        || input.resources.memory_bytes == 0
        || input.resources.disk_bytes == 0
        || input.resources.lifetime_seconds == 0
        || input.instance.desired_state() != DesiredState::Running
        || input.instance.observed_state() != ObservedState::Pending
    {
        return Err(DevContainerLedgerError::InvalidInput);
    }
    Ok(())
}

fn sqlite_u64(value: u64) -> Result<i64, DevContainerLedgerError> {
    i64::try_from(value).map_err(|_| DevContainerLedgerError::InvalidInput)
}

fn owner_kind_name(kind: OwnerKind) -> &'static str {
    match kind {
        OwnerKind::Installation => "installation",
        OwnerKind::Team => "team",
        OwnerKind::Project => "project",
        OwnerKind::Personal => "personal",
    }
}

fn host_capability_name(capability: HostCapability) -> &'static str {
    match capability {
        HostCapability::Privileged => "privileged",
        HostCapability::HostFilesystem => "host_filesystem",
        HostCapability::ContainerRuntimeSocket => "container_runtime_socket",
        HostCapability::HostNetwork => "host_network",
        HostCapability::HostDevice => "host_device",
        HostCapability::KernelAdministration => "kernel_administration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::access::PrincipalId;
    use labby_primitives::dev_container::{
        DevContainerId, DevContainerQuota, DevContainerTemplateId, HostCapabilityPolicy,
        ImageDigest, LifecycleNonce,
    };

    fn fixture() -> (ApprovedTemplate, OwnedDevContainer) {
        let template = ApprovedTemplate::new(
            DevContainerTemplateId::new("rust").unwrap(),
            ImageDigest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            DevContainerQuota {
                max_active_instances: 1,
                cpu_millis: 1_000,
                memory_bytes: 2_000,
                disk_bytes: 3_000,
                max_lifetime_seconds: 60,
            },
            HostCapabilityPolicy::deny_all(),
        )
        .unwrap();
        let instance = OwnedDevContainer::new(
            DevContainerId::new("dc-1").unwrap(),
            OwnerScope::Personal(PrincipalId::new("principal-1").unwrap()),
            &template,
            LifecycleNonce::new("11111111111111111111111111111111").unwrap(),
            vec![SecretReference::new("secret-ref").unwrap()],
        )
        .unwrap();
        (template, instance)
    }

    #[test]
    fn create_is_atomic_and_owner_quota_is_durable() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        install_schema(&connection).unwrap();
        let (template, first) = fixture();
        approve_template(&connection, &template, 1).unwrap();
        set_owner_quota(&connection, first.owner(), 1, 1).unwrap();
        let resources = ReservedResources {
            cpu_millis: 500,
            memory_bytes: 1_000,
            disk_bytes: 2_000,
            lifetime_seconds: 30,
        };
        create_instance(
            &mut connection,
            &CreateInstance {
                instance: &first,
                resources,
                authority_fingerprint: "sha256:authority",
                event_id: "event-1",
                occurred_at: 2,
            },
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM dev_container_ledger", [], |row| row
                    .get::<_, u32>(
                    0
                ))
                .unwrap(),
            1
        );

        let second = OwnedDevContainer::new(
            DevContainerId::new("dc-2").unwrap(),
            first.owner().clone(),
            &template,
            LifecycleNonce::new("22222222222222222222222222222222").unwrap(),
            vec![],
        )
        .unwrap();
        assert_eq!(
            create_instance(
                &mut connection,
                &CreateInstance {
                    instance: &second,
                    resources,
                    authority_fingerprint: "sha256:authority",
                    event_id: "event-2",
                    occurred_at: 3,
                }
            ),
            Err(DevContainerLedgerError::QuotaExhausted)
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM dev_container_instances", [], |row| {
                    row.get::<_, u32>(0)
                })
                .unwrap(),
            1
        );
    }
}
