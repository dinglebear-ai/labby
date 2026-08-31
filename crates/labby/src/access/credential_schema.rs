//! Durable schema for locally bootstrapped, project-bound credentials.
//!
//! This module contains persistence vocabulary only. Authentication, file-journal
//! reconciliation, issuance, and cleanup remain owned by their runtime lanes.

macro_rules! credential_schema_sql {
    () => {
        r#"
CREATE TABLE access_installations (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    installation_id TEXT NOT NULL UNIQUE CHECK(length(trim(installation_id)) > 0),
    installation_generation INTEGER NOT NULL CHECK(installation_generation > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE bootstrap_proofs (
    proof_id TEXT PRIMARY KEY CHECK(length(trim(proof_id)) > 0),
    prepare_id TEXT NOT NULL UNIQUE CHECK(length(trim(prepare_id)) > 0),
    installation_id TEXT NOT NULL CHECK(length(trim(installation_id)) > 0),
    installation_generation INTEGER NOT NULL CHECK(installation_generation > 0),
    proof_digest BLOB NOT NULL UNIQUE CHECK(length(proof_digest) = 32),
    manifest_digest BLOB NOT NULL CHECK(length(manifest_digest) = 32),
    request_digest BLOB NOT NULL CHECK(length(request_digest) = 32),
    idempotency_digest BLOB NOT NULL CHECK(length(idempotency_digest) = 32),
    credential_id TEXT NOT NULL CHECK(length(trim(credential_id)) > 0),
    credential_digest BLOB NOT NULL CHECK(length(credential_digest) = 32),
    proof_generation INTEGER NOT NULL CHECK(proof_generation > 0),
    semantic_attempts INTEGER NOT NULL DEFAULT 0 CHECK(semantic_attempts BETWEEN 0 AND 8),
    status TEXT NOT NULL CHECK(status IN ('active','consumed','expired','revoked','tombstoned')),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK(expires_at > created_at),
    consumed_at INTEGER,
    revoked_at INTEGER,
    updated_at INTEGER NOT NULL,
    CHECK ((status = 'consumed') = (consumed_at IS NOT NULL)),
    CHECK ((status IN ('revoked','tombstoned')) = (revoked_at IS NOT NULL)),
    UNIQUE(installation_id, idempotency_digest),
    UNIQUE(installation_id, credential_id),
    UNIQUE(installation_id, credential_digest)
) STRICT;
CREATE INDEX bootstrap_proofs_active_expiry
    ON bootstrap_proofs(installation_id, expires_at)
    WHERE status = 'active';

CREATE TABLE project_credentials (
    credential_id TEXT PRIMARY KEY CHECK(length(trim(credential_id)) > 0),
    installation_id TEXT NOT NULL CHECK(length(trim(installation_id)) > 0),
    installation_generation INTEGER NOT NULL CHECK(installation_generation > 0),
    credential_digest BLOB NOT NULL UNIQUE CHECK(length(credential_digest) = 32),
    credential_generation INTEGER NOT NULL CHECK(credential_generation > 0),
    canonical_issuer TEXT NOT NULL CHECK(length(trim(canonical_issuer)) > 0),
    subject TEXT NOT NULL CHECK(length(trim(subject)) > 0),
    organization_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    membership_generation INTEGER NOT NULL CHECK(membership_generation > 0),
    organization_policy_epoch INTEGER NOT NULL CHECK(organization_policy_epoch >= 0),
    project_policy_epoch INTEGER NOT NULL CHECK(project_policy_epoch >= 0),
    loadout_id TEXT NOT NULL CHECK(length(trim(loadout_id)) > 0),
    loadout_generation INTEGER NOT NULL CHECK(loadout_generation > 0),
    loadout_assignment_generation INTEGER NOT NULL CHECK(loadout_assignment_generation > 0),
    catalog_generation INTEGER NOT NULL CHECK(catalog_generation > 0),
    loadout_policy_fingerprint BLOB NOT NULL CHECK(length(loadout_policy_fingerprint) = 32),
    route_id TEXT NOT NULL CHECK(length(trim(route_id)) > 0),
    route_generation INTEGER NOT NULL CHECK(route_generation > 0),
    resource TEXT NOT NULL CHECK(length(trim(resource)) > 0),
    audience TEXT NOT NULL CHECK(length(trim(audience)) > 0),
    scopes_json TEXT NOT NULL CHECK(json_valid(scopes_json) AND json_type(scopes_json) = 'array'),
    status TEXT NOT NULL CHECK(status IN ('active','revoked','expired','tombstoned')),
    issued_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL CHECK(expires_at > issued_at),
    revoked_at INTEGER,
    revocation_generation INTEGER NOT NULL DEFAULT 0 CHECK(revocation_generation >= 0),
    updated_at INTEGER NOT NULL,
    CHECK ((status IN ('revoked','tombstoned')) = (revoked_at IS NOT NULL)),
    UNIQUE(installation_id, canonical_issuer, subject, credential_id),
    FOREIGN KEY (organization_id, principal_id)
      REFERENCES principals(organization_id, principal_id) ON DELETE RESTRICT,
    FOREIGN KEY (organization_id, project_id)
      REFERENCES projects(organization_id, project_id) ON DELETE RESTRICT
) STRICT;
CREATE INDEX project_credentials_authority
    ON project_credentials(installation_id, project_id, status, expires_at);

CREATE TABLE project_policy_publications (
    project_id TEXT PRIMARY KEY CHECK(length(trim(project_id)) > 0),
    policy_fingerprint BLOB NOT NULL CHECK(length(policy_fingerprint) = 32),
    policy_epoch INTEGER NOT NULL CHECK(policy_epoch > 0),
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE access_admission_buckets (
    admission_class TEXT NOT NULL CHECK(admission_class IN
      ('proof_global','proof_peer','credential_global','credential_peer')),
    bucket_fingerprint BLOB NOT NULL CHECK(length(bucket_fingerprint) = 32),
    window_started_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 64),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(admission_class, bucket_fingerprint)
) STRICT;
CREATE INDEX access_admission_buckets_updated
    ON access_admission_buckets(updated_at);

CREATE TABLE access_security_events (
    event_id TEXT PRIMARY KEY CHECK(length(event_id) BETWEEN 1 AND 96),
    occurred_at INTEGER NOT NULL,
    event_kind TEXT NOT NULL CHECK(event_kind IN
      ('proof','credential_verify','credential_issue','credential_revoke')),
    decision TEXT NOT NULL CHECK(decision IN ('allow','deny')),
    reason_code TEXT NOT NULL CHECK(length(reason_code) BETWEEN 1 AND 64),
    target_fingerprint BLOB NOT NULL CHECK(length(target_fingerprint) = 32),
    peer_fingerprint BLOB CHECK(peer_fingerprint IS NULL OR length(peer_fingerprint) = 32),
    metadata_json TEXT NOT NULL DEFAULT '{}'
      CHECK(json_valid(metadata_json) AND length(metadata_json) <= 1024)
) STRICT;
CREATE INDEX access_security_events_retention
    ON access_security_events(occurred_at, event_id);

CREATE TABLE credential_idempotency (
    idempotency_digest BLOB PRIMARY KEY CHECK(length(idempotency_digest) = 32),
    installation_id TEXT NOT NULL CHECK(length(trim(installation_id)) > 0),
    operation TEXT NOT NULL CHECK(operation IN ('bootstrap_consume','issue','revoke','cleanup')),
    request_digest BLOB NOT NULL CHECK(length(request_digest) = 32),
    proof_id TEXT,
    credential_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('pending','committed','conflict','tombstoned')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK(proof_id IS NOT NULL OR credential_id IS NOT NULL),
    UNIQUE(installation_id, operation, request_digest)
) STRICT;

CREATE TABLE access_tombstones (
    tombstone_id TEXT PRIMARY KEY CHECK(length(trim(tombstone_id)) > 0),
    installation_id TEXT NOT NULL CHECK(length(trim(installation_id)) > 0),
    artifact_kind TEXT NOT NULL CHECK(artifact_kind IN ('prepare','proof','credential','session_source')),
    public_id TEXT NOT NULL CHECK(length(trim(public_id)) > 0),
    canonical_digest BLOB NOT NULL CHECK(length(canonical_digest) = 32),
    artifact_generation INTEGER NOT NULL CHECK(artifact_generation > 0),
    reason_code TEXT NOT NULL CHECK(length(trim(reason_code)) > 0),
    created_at INTEGER NOT NULL,
    UNIQUE(installation_id, artifact_kind, public_id),
    UNIQUE(installation_id, artifact_kind, canonical_digest)
) STRICT;
CREATE INDEX access_tombstones_lookup
    ON access_tombstones(artifact_kind, public_id, canonical_digest);
"#
    };
}

pub(super) use credential_schema_sql;
