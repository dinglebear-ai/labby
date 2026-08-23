use rusqlite::{Connection, TransactionBehavior, params};

use super::error::{AccessStoreError, AccessStoreResult};

pub(super) const SCHEMA_VERSION: i64 = 2;
pub(super) const APPLICATION_ID: i64 = 0x4c_41_43_31;
pub(super) const SCHEMA_FINGERPRINT: &str = "labby-access-v2-20260823";
pub(super) const V1_SCHEMA_VERSION: i64 = 1;
pub(super) const V1_SCHEMA_FINGERPRINT: &str = "labby-access-v1-20260823";

pub(super) fn migrate(connection: &mut Connection) -> AccessStoreResult<()> {
    let found = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(super::store::map_sqlite_error)?;
    if found > SCHEMA_VERSION {
        return Err(AccessStoreError::UnsupportedSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found == 0 {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch(SCHEMA_V2_METADATA)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute_batch(DOMAIN_SCHEMA)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO access_metadata(
                singleton, schema_version, schema_fingerprint, global_revision, updated_at,
                bootstrap_generation, bootstrap_identity_fingerprint
             ) VALUES (1, ?1, ?2, 0, unixepoch(), 0, NULL)",
                params![SCHEMA_VERSION, SCHEMA_FINGERPRINT],
            )
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
        return Ok(());
    }
    if found == V1_SCHEMA_VERSION {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(super::store::map_sqlite_error)?;
        super::integrity::validate_v1_before_migration(&transaction)?;
        rebuild_metadata_v2(&transaction)?;
        transaction
            .pragma_update(None, "user_version", 2)
            .map_err(super::store::map_sqlite_error)?;
        transaction
            .commit()
            .map_err(super::store::map_sqlite_error)?;
    }
    Ok(())
}

fn rebuild_metadata_v2(transaction: &rusqlite::Transaction<'_>) -> AccessStoreResult<()> {
    transaction
        .execute_batch(SCHEMA_V2_REBUILD_BEGIN)
        .map_err(super::store::map_sqlite_error)?;
    transaction
        .execute_batch(SCHEMA_V2_METADATA)
        .map_err(super::store::map_sqlite_error)?;
    transaction.execute("INSERT INTO access_metadata(singleton, schema_version, schema_fingerprint, global_revision, updated_at, bootstrap_generation, bootstrap_identity_fingerprint) SELECT singleton, ?1, ?2, global_revision, updated_at, 0, NULL FROM access_metadata_v1", params![SCHEMA_VERSION, SCHEMA_FINGERPRINT]).map_err(super::store::map_sqlite_error)?;
    transaction
        .execute_batch(SCHEMA_V2_REBUILD_END)
        .map_err(super::store::map_sqlite_error)
}

pub(super) const SCHEMA_V2_REBUILD_BEGIN: &str = "
ALTER TABLE access_metadata RENAME TO access_metadata_v1;
";

pub(super) const SCHEMA_V2_METADATA: &str = "
CREATE TABLE access_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 2),
    schema_fingerprint TEXT NOT NULL,
    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
    updated_at INTEGER NOT NULL,
    bootstrap_generation INTEGER NOT NULL DEFAULT 0 CHECK(bootstrap_generation IN (0, 1)),
    bootstrap_identity_fingerprint TEXT,
    CHECK (
      (bootstrap_generation = 0 AND bootstrap_identity_fingerprint IS NULL)
      OR
      (bootstrap_generation = 1 AND bootstrap_identity_fingerprint IS NOT NULL
       AND length(trim(bootstrap_identity_fingerprint)) > 0)
    )
) STRICT;
";
pub(super) const SCHEMA_V2_REBUILD_END: &str = "DROP TABLE access_metadata_v1;";

pub(super) const V1_METADATA_SCHEMA: &str = "
CREATE TABLE access_metadata (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    schema_fingerprint TEXT NOT NULL,
    global_revision INTEGER NOT NULL CHECK(global_revision >= 0),
    updated_at INTEGER NOT NULL
) STRICT;
";

pub(super) const DOMAIN_SCHEMA: &str = "
CREATE TABLE organizations (
    organization_id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK(length(trim(name)) > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    policy_epoch INTEGER NOT NULL DEFAULT 0 CHECK(policy_epoch >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, policy_epoch)
) STRICT;

CREATE TABLE principals (
    principal_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('user', 'service_account')),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    display_name TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, principal_id),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE principal_links (
    link_id TEXT PRIMARY KEY,
    principal_id TEXT NOT NULL,
    link_kind TEXT NOT NULL CHECK(link_kind IN ('external', 'local_credential')),
    issuer TEXT,
    subject TEXT,
    credential_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('active', 'revoked')),
    verification_generation INTEGER NOT NULL CHECK(verification_generation > 0),
    link_generation INTEGER NOT NULL CHECK(link_generation > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
      (link_kind = 'external' AND issuer IS NOT NULL AND length(trim(issuer)) > 0
       AND subject IS NOT NULL AND length(trim(subject)) > 0 AND credential_id IS NULL)
      OR
      (link_kind = 'local_credential' AND issuer IS NULL AND subject IS NULL
       AND credential_id IS NOT NULL AND length(trim(credential_id)) > 0)
    ),
    FOREIGN KEY (principal_id) REFERENCES principals(principal_id) ON DELETE RESTRICT
) STRICT;
CREATE UNIQUE INDEX principal_links_external_unique
    ON principal_links(issuer, subject) WHERE link_kind = 'external';
CREATE UNIQUE INDEX principal_links_local_unique
    ON principal_links(credential_id) WHERE link_kind = 'local_credential';

CREATE TABLE projects (
    project_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK(length(trim(name)) > 0),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    project_policy_epoch INTEGER NOT NULL DEFAULT 0 CHECK(project_policy_epoch >= 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, project_id),
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE project_memberships (
    membership_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('owner', 'admin', 'member', 'viewer')),
    status TEXT NOT NULL CHECK(status IN ('active', 'suspended', 'disabled')),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (organization_id, project_id, principal_id),
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE CASCADE,
    FOREIGN KEY (organization_id, principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE project_loadouts (
    organization_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    loadout_name TEXT NOT NULL CHECK(length(trim(loadout_name)) > 0),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (organization_id, project_id),
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE CASCADE,
    FOREIGN KEY (organization_id, created_by)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE access_audit (
    event_id TEXT PRIMARY KEY,
    occurred_at INTEGER NOT NULL,
    correlation_id TEXT,
    actor_principal_id TEXT NOT NULL,
    organization_id TEXT NOT NULL,
    project_id TEXT,
    action TEXT NOT NULL CHECK(length(trim(action)) > 0),
    target_kind TEXT NOT NULL CHECK(length(trim(target_kind)) > 0),
    target_fingerprint TEXT NOT NULL CHECK(length(trim(target_fingerprint)) > 0),
    decision TEXT NOT NULL CHECK(decision IN ('allow', 'deny')),
    reason_code TEXT NOT NULL CHECK(length(trim(reason_code)) > 0),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    FOREIGN KEY (organization_id) REFERENCES organizations(organization_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, actor_principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE RESTRICT
) STRICT;

";
