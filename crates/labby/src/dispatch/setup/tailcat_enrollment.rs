//! Journaled native custody for authenticated Tailcat credential enrollment.
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

use super::{
    SetupCaller,
    tailcat::{denied, unavailable},
};
use crate::installation::{
    InstallationPaths,
    secure_file::{self, PrepareFileIdentity},
};
use crate::{
    access::{AccessRuntime, AccessStore, TailcatEnrollmentInput},
    dispatch::error::ToolError,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnrollmentRequest {
    pub project_id: String,
    pub idempotency_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u8,
    identity: String,
    project_id: String,
    credential_id: String,
    credential_digest: [u8; 32],
    idempotency_digest: [u8; 32],
    request_digest: [u8; 32],
    policy_fingerprint: [u8; 32],
    resource: String,
    issued_at: i64,
    expires_at: i64,
    state: State,
    file: Option<PrepareFileIdentity>,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum State {
    Allocating,
    Published,
    Committed,
}

struct Custody {
    journal: Journal,
    path: PathBuf,
    _lock: crate::config::host_write::HostConfigLock,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnableRequest {
    pub project_id: String,
    pub credential_id: String,
}

pub(crate) async fn enable(
    caller: SetupCaller,
    runtime: AccessRuntime,
    manager: labby_gateway::gateway::manager::GatewayManager,
    identity: labby_auth::VerifiedIdentity,
    installation_id: String,
    oauth_enabled: bool,
    request: EnableRequest,
) -> Result<Value, ToolError> {
    if caller != SetupCaller::Operator
        || identity.authenticator() != labby_auth::Authenticator::BrowserSession
    {
        return Err(denied());
    }
    let store = runtime.store().await.map_err(|_| unavailable())?;
    if !store
        .list_effective_projects(identity.clone())
        .await
        .map_err(|_| denied())?
        .iter()
        .any(|role| {
            role.project_id == request.project_id
                && role.direct
                && role.role == crate::access::ProjectRole::Owner
        })
    {
        return Err(denied());
    }
    if !oauth_enabled {
        return Err(ToolError::Sdk {
            sdk_kind: "oauth_required".into(),
            message: "Enable native OAuth browser authentication before enabling Tailcat".into(),
        });
    }
    if !labby_runtime::gateway_config::is_canonical_project_id(&request.project_id)
        || ulid::Ulid::from_string(&request.credential_id).is_err()
    {
        return Err(denied());
    }
    let name = format!(
        "tailcat-{}",
        &hex::encode(Sha256::digest(request.project_id.as_bytes()))[..16]
    );
    let snapshot = manager
        .published_project_route_snapshot(&name, &request.project_id, &name)
        .await
        .map_err(|_| unavailable())?;
    let lease = manager
        .acquire_published_bootstrap_policy_lease(&name, &name)
        .await
        .map_err(|_| unavailable())?;
    let loadout = snapshot.effective_loadout();
    if u64::from_be_bytes(snapshot.runtime_config_generation().fingerprint_bytes())
        != lease.loadout_generation()
        || loadout.upstreams != [name.clone()]
        || !loadout.services.is_empty()
        || !loadout.expose_tools
        || loadout.expose_code_mode
        || loadout.expose_resources
        || loadout.expose_prompts
        || loadout.expose_skills
        || !snapshot.effective_service_names().is_empty()
        || snapshot.resource() != lease.resource()
        || lease.resource() != lease.audience()
        || lease.scopes() != ["lab".to_owned()]
    {
        return Err(denied());
    }
    let _writer = runtime
        .acquire_bootstrap_writer()
        .await
        .map_err(|_| unavailable())?;
    let paths = InstallationPaths::resolve().map_err(|_| unavailable())?;
    if super::access_bootstrap::existing_installation_id(&paths).map_err(|_| unavailable())?
        != installation_id
    {
        return Err(denied());
    }
    reconcile_pending(&paths, &store).await?;
    let actor = identity.safe_fingerprint();
    let project = request.project_id.clone();
    let resource = lease.resource().to_owned();
    let fingerprint = lease.policy_fingerprint();
    let prepared_name = name.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let name = prepared_name;
        let journal = find_committed_journal(
            paths.root(),
            &actor,
            &request.credential_id,
            &project,
            &resource,
            fingerprint,
        )?;
        let config_path = crate::config::config_toml_path().map_err(|_| unavailable())?;
        let lock = crate::config::host_write::HostConfigLock::acquire(&config_path)
            .map_err(|_| unavailable())?;
        let raw = lock.read_raw().map_err(|_| unavailable())?;
        let mut config: crate::config::LabConfig =
            toml::from_str(&raw).map_err(|_| unavailable())?;
        let bundle =
            crate::dispatch::tailcat::assets::discover(config.tailcat.bundle_path.as_deref())
                .map_err(|_| unavailable())?;
        let upstream = config
            .upstream
            .iter()
            .find(|upstream| upstream.name == name)
            .ok_or_else(unavailable)?;
        let configure = super::tailcat::ConfigureRequest {
            project_id: project,
            public_resource: resource,
            derp_map_url: config
                .tailcat
                .derp_map_url
                .clone()
                .ok_or_else(unavailable)?,
            node_path: upstream
                .command
                .clone()
                .map(PathBuf::from)
                .ok_or_else(unavailable)?,
            dry_run: true,
        };
        // The exact native route, restricted upstream and loadout must already exist;
        // rendering is validation only, never repairs configuration during activation.
        if !config.loadouts.iter().any(|loadout| loadout.name == name)
            || !config
                .protected_mcp_routes
                .iter()
                .any(|route| route.name == name)
        {
            return Err(denied());
        }
        super::tailcat::render_live_config(&raw, &mut config, &configure, &bundle.adapter)?;
        let mut document: toml_edit::DocumentMut = raw.parse().map_err(|_| unavailable())?;
        document["tailcat"]["enabled"] = toml_edit::value(true);
        Ok::<_, ToolError>((journal, lock, document.to_string(), config.tailcat.enabled))
    })
    .await
    .map_err(|_| unavailable())??;
    let (journal, lock, rendered, already_enabled) = prepared;
    let input = TailcatEnrollmentInput {
        identity,
        installation_id,
        project_id: journal.project_id,
        loadout_id: name.clone(),
        route_id: name,
        resource: journal.resource,
        policy_fingerprint: journal.policy_fingerprint,
        credential_id: journal.credential_id,
        credential_digest: journal.credential_digest,
        request_digest: journal.request_digest,
        idempotency_digest: journal.idempotency_digest,
        now: labby_auth::util::now_unix(),
        expires_at: journal.expires_at,
    };
    store
        .commit_tailcat_enrolled_configuration(input, move || {
            secure_file::verify_identity(journal.file.as_ref().ok_or_else(recovery_required)?)
                .map_err(|_| recovery_required())?;
            if !already_enabled {
                lock.write(&rendered)
                    .map_err(super::tailcat::map_config_error)?;
            }
            Ok::<_, ToolError>(())
        })
        .await
        .map_err(map_enrollment_error)??;
    Ok(json!({"enabled":true,"changed":!already_enabled,"restart_required":true}))
}

fn find_committed_journal(
    root: &Path,
    actor: &str,
    id: &str,
    project: &str,
    resource: &str,
    fingerprint: [u8; 32],
) -> Result<Journal, ToolError> {
    let folder = root.join("tailcat/enrollments");
    std::fs::symlink_metadata(&folder).map_err(|_| recovery_required())?;
    secure_file::create_private_dir(&folder).map_err(|_| recovery_required())?;
    let entries = std::fs::read_dir(&folder)
        .map_err(|_| recovery_required())?
        .take(257)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| recovery_required())?;
    if entries.len() > 256 {
        return Err(recovery_required());
    }
    let mut found = None;
    for entry in entries {
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let journal: Journal = serde_json::from_slice(
            &secure_file::read_private(&entry.path()).map_err(|_| recovery_required())?,
        )
        .map_err(|_| recovery_required())?;
        if journal.credential_id != id {
            continue;
        }
        if found.is_some()
            || journal.version != 1
            || journal.state != State::Committed
            || journal.identity != actor
            || journal.project_id != project
            || journal.resource != resource
            || journal.policy_fingerprint != fingerprint
            || entry.file_name()
                != std::ffi::OsString::from(format!(
                    "{}.json",
                    hex::encode(journal.idempotency_digest)
                ))
        {
            return Err(recovery_required());
        }
        let file = journal.file.as_ref().ok_or_else(recovery_required)?;
        if file.path
            != root
                .join("tailcat/credentials")
                .join(format!("{id}.credential"))
            || file.digest_hex != hex::encode(journal.credential_digest)
        {
            return Err(recovery_required());
        }
        secure_file::verify_identity(file).map_err(|_| recovery_required())?;
        found = Some(journal);
    }
    found.ok_or_else(recovery_required)
}

pub(crate) async fn enroll(
    caller: SetupCaller,
    runtime: AccessRuntime,
    manager: labby_gateway::gateway::manager::GatewayManager,
    identity: labby_auth::VerifiedIdentity,
    installation_id: String,
    request: EnrollmentRequest,
) -> Result<Value, ToolError> {
    if caller != SetupCaller::Operator
        || identity.authenticator() != labby_auth::Authenticator::BrowserSession
    {
        return Err(denied());
    }
    if !labby_runtime::gateway_config::is_canonical_project_id(&request.project_id)
        || request.idempotency_key.len() < 16
        || request.idempotency_key.len() > 128
        || !request
            .idempotency_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return Err(ToolError::InvalidParam { param:"params".into(), message:"Enrollment requires a canonical project ID and an opaque 16–128 character idempotency key".into() });
    }
    // Reject wrong-project and revoked ownership before observing native routing state.
    let admission_store = runtime.store().await.map_err(|_| unavailable())?;
    let admission = admission_store
        .list_effective_projects(identity.clone())
        .await
        .map_err(|_| denied())?;
    if !admission.iter().any(|role| {
        role.project_id == request.project_id
            && role.direct
            && role.role == crate::access::ProjectRole::Owner
    }) {
        return Err(denied());
    }
    let suffix = hex::encode(Sha256::digest(request.project_id.as_bytes()));
    let name = format!("tailcat-{}", &suffix[..16]);
    let snapshot = manager
        .published_project_route_snapshot(&name, &request.project_id, &name)
        .await
        .map_err(|_| unavailable())?;
    let lease = manager
        .acquire_published_bootstrap_policy_lease(&name, &name)
        .await
        .map_err(|_| unavailable())?;
    if u64::from_be_bytes(snapshot.runtime_config_generation().fingerprint_bytes())
        != lease.loadout_generation()
    {
        return Err(unavailable());
    }
    let loadout = snapshot.effective_loadout();
    if loadout.upstreams != [name.clone()]
        || !loadout.services.is_empty()
        || !loadout.expose_tools
        || loadout.expose_code_mode
        || loadout.expose_resources
        || loadout.expose_prompts
        || loadout.expose_skills
        || !snapshot.effective_service_names().is_empty()
        || snapshot.resource() != lease.resource()
        || lease.resource() != lease.audience()
        || lease.scopes() != ["lab".to_owned()]
    {
        return Err(denied());
    }
    // Same established order as bootstrap: published policy -> bounded runtime writer -> custody -> SQLite.
    let _writer = runtime
        .acquire_bootstrap_writer()
        .await
        .map_err(|_| unavailable())?;
    let store = runtime.store().await.map_err(|_| unavailable())?;
    // Reject inactive/non-owner identity before filesystem publication.
    let roles = store
        .list_effective_projects(identity.clone())
        .await
        .map_err(|_| denied())?;
    if !roles.iter().any(|role| {
        role.project_id == request.project_id
            && role.direct
            && role.role == crate::access::ProjectRole::Owner
    }) {
        return Err(denied());
    }
    store
        .require_tailcat_enrollment_assignment(
            identity.clone(),
            request.project_id.clone(),
            name.clone(),
            lease.policy_fingerprint(),
        )
        .await
        .map_err(map_enrollment_error)?;
    let paths = InstallationPaths::resolve().map_err(|_| unavailable())?;
    let disk_installation =
        super::access_bootstrap::existing_installation_id(&paths).map_err(|_| unavailable())?;
    if disk_installation != installation_id {
        return Err(denied());
    }
    reconcile_pending(&paths, &store).await?;
    let fingerprint = lease.policy_fingerprint();
    let resource = lease.resource().to_owned();
    let actor = identity.safe_fingerprint();
    let custody = tokio::task::spawn_blocking(move || {
        prepare_custody(paths.root(), &actor, &request, &resource, fingerprint)
    })
    .await
    .map_err(|_| unavailable())??;
    let mut custody = custody;
    let file = custody
        .journal
        .file
        .as_ref()
        .ok_or_else(recovery_required)?;
    // Exact inode/parent identity, digest and private permissions must survive before activating the credential.
    secure_file::verify_identity(file).map_err(|_| recovery_required())?;
    let now = labby_auth::util::now_unix();
    let input = TailcatEnrollmentInput {
        identity,
        installation_id,
        project_id: custody.journal.project_id.clone(),
        loadout_id: name.clone(),
        route_id: name,
        resource: custody.journal.resource.clone(),
        policy_fingerprint: custody.journal.policy_fingerprint,
        credential_id: custody.journal.credential_id.clone(),
        credential_digest: custody.journal.credential_digest,
        request_digest: custody.journal.request_digest,
        idempotency_digest: custody.journal.idempotency_digest,
        now,
        expires_at: custody.journal.expires_at,
    };
    store
        .enroll_tailcat_credential(input)
        .await
        .map_err(map_enrollment_error)?;
    custody.journal.state = State::Committed;
    secure_file::replace_journal(
        &custody.path,
        &serde_json::to_vec(&custody.journal).map_err(|_| unavailable())?,
    )
    .map_err(|_| recovery_required())?;
    Ok(
        json!({"enrolled":true,"credential_id":custody.journal.credential_id,"credential_file":file.path,
        "expires_at":custody.journal.expires_at,"controller_enabled":false,"restart_required":true}),
    )
}

/// Recover the file-publication/SQLite-commit gap before any new enrollment writes.
/// Unrecorded inode authority is preserved for manual recovery, never adopted or deleted.
pub(crate) async fn reconcile_pending(
    paths: &InstallationPaths,
    store: &AccessStore,
) -> Result<(), ToolError> {
    let folder = paths.root().join("tailcat/enrollments");
    let root = paths.root().to_owned();
    let journals = tokio::task::spawn_blocking(move || {
        match std::fs::symlink_metadata(&folder) {
            Ok(_) => secure_file::create_private_dir(&folder).map_err(|_| recovery_required())?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(recovery_required()),
        }
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(recovery_required()),
        };
        let mut files = Vec::new();
        let mut count = 0;
        for entry in entries.take(257) {
            count += 1;
            let path = entry.map_err(|_| recovery_required())?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(path);
            }
        }
        if files.len() > 128 || count > 256 {
            return Err(recovery_required());
        }
        Ok(files)
    })
    .await
    .map_err(|_| unavailable())??;
    let started = std::time::Instant::now();
    for path in journals {
        if started.elapsed() > std::time::Duration::from_secs(5) {
            return Err(recovery_required());
        }
        let root = root.clone();
        let (lock, path, mut journal) = tokio::task::spawn_blocking(move || {
            let lock = crate::config::host_write::HostConfigLock::acquire_with_timeout(
                &path,
                std::time::Duration::from_millis(100),
            )
            .map_err(|_| recovery_required())?;
            let journal: Journal = serde_json::from_slice(
                &secure_file::read_private(&path).map_err(|_| recovery_required())?,
            )
            .map_err(|_| recovery_required())?;
            if journal.version != 1
                || path.file_name().and_then(|name| name.to_str())
                    != Some(format!("{}.json", hex::encode(journal.idempotency_digest)).as_str())
                || ulid::Ulid::from_string(&journal.credential_id).is_err()
            {
                return Err(recovery_required());
            }
            let file = journal.file.as_ref().ok_or_else(recovery_required)?;
            if file.path
                != root
                    .join("tailcat/credentials")
                    .join(format!("{}.credential", journal.credential_id))
                || file.digest_hex != hex::encode(journal.credential_digest)
            {
                return Err(recovery_required());
            }
            secure_file::verify_identity(file).map_err(|_| recovery_required())?;
            Ok((lock, path, journal))
        })
        .await
        .map_err(|_| unavailable())??;
        let committed = store
            .tailcat_enrollment_receipt(
                journal.idempotency_digest,
                journal.request_digest,
                journal.credential_id.clone(),
                journal.credential_digest,
            )
            .await
            .map_err(|_| recovery_required())?;
        if journal.state == State::Committed && !committed {
            return Err(recovery_required());
        }
        if committed && journal.state != State::Committed {
            journal.state = State::Committed;
            tokio::task::spawn_blocking(move || {
                secure_file::replace_journal(
                    &path,
                    &serde_json::to_vec(&journal).map_err(|_| recovery_required())?,
                )
                .map_err(|_| recovery_required())?;
                drop(lock);
                Ok::<_, ToolError>(())
            })
            .await
            .map_err(|_| unavailable())??;
        }
    }
    Ok(())
}

fn map_enrollment_error(error: crate::access::AccessStoreError) -> ToolError {
    match error {
        crate::access::AccessStoreError::ProjectLoadoutConflict => ToolError::Sdk { sdk_kind:"tailcat_assignment_required".into(), message:"Assign this project's exact Tailcat loadout through the existing authorized project assignment action, then retry; enrollment never replaces another assignment or changes policy epochs".into() },
        crate::access::AccessStoreError::NotAuthorized => denied(),
        crate::access::AccessStoreError::BootstrapConflict => ToolError::Sdk { sdk_kind:"idempotency_conflict".into(),message:"The enrollment operation conflicts with its retained receipt; inspect native recovery".into() },
        _ => recovery_required(),
    }
}
fn recovery_required() -> ToolError {
    ToolError::Sdk { sdk_kind:"tailcat_enrollment_recovery_required".into(), message:"Enrollment custody requires native recovery; preserve the private journal and credential file, then retry the same operation after correcting the reported prerequisite".into() }
}

fn prepare_custody(
    root: &Path,
    actor: &str,
    request: &EnrollmentRequest,
    resource: &str,
    fingerprint: [u8; 32],
) -> Result<Custody, ToolError> {
    let folder = root.join("tailcat/enrollments");
    let credentials = root.join("tailcat/credentials");
    secure_file::create_private_dir(&folder).map_err(|_| unavailable())?;
    secure_file::create_private_dir(&credentials).map_err(|_| unavailable())?;
    let operation: [u8; 32] = Sha256::digest(
        serde_json::to_vec(&(
            "labby.tailcat.enrollment/v1",
            actor,
            &request.project_id,
            &request.idempotency_key,
        ))
        .map_err(|_| unavailable())?,
    )
    .into();
    let key = hex::encode(operation);
    // Serialize capacity admission across processes before creating an operation lock.
    // The fixed admission lock lives outside the bounded journal directory.
    let _admission = crate::config::host_write::HostConfigLock::acquire(
        &root.join("tailcat/enrollment-admission"),
    )
    .map_err(|_| unavailable())?;
    let path = folder.join(format!("{key}.json"));
    if !path.try_exists().map_err(|_| recovery_required())? {
        let count = std::fs::read_dir(&folder)
            .map_err(|_| unavailable())?
            .take(257)
            .count();
        // A fresh operation needs both its persistent lock and journal.
        if count > 254 {
            return Err(recovery_required());
        }
    }
    let lock =
        crate::config::host_write::HostConfigLock::acquire(&path).map_err(|_| unavailable())?;
    let digest: [u8; 32] = Sha256::digest(
        serde_json::to_vec(&(actor, &request.project_id, resource, fingerprint))
            .map_err(|_| unavailable())?,
    )
    .into();
    match secure_file::read_private(&path) {
        Ok(bytes) => {
            let journal: Journal =
                serde_json::from_slice(&bytes).map_err(|_| recovery_required())?;
            if journal.version != 1
                || journal.identity != actor
                || journal.project_id != request.project_id
                || journal.request_digest != digest
                || journal.idempotency_digest != operation
                || journal.policy_fingerprint != fingerprint
                || journal.resource != resource
            {
                return Err(recovery_required());
            }
            if ulid::Ulid::from_string(&journal.credential_id).is_err() {
                return Err(recovery_required());
            }
            let expected = credentials.join(format!("{}.credential", journal.credential_id));
            let file = journal.file.as_ref().ok_or_else(recovery_required)?;
            if file.path != expected
                || file.digest_hex != hex::encode(journal.credential_digest)
                || journal.expires_at <= labby_auth::util::now_unix()
                || journal.expires_at.checked_sub(journal.issued_at) != Some(86400)
            {
                return Err(recovery_required());
            }
            secure_file::verify_identity(file).map_err(|_| recovery_required())?;
            return Ok(Custody {
                journal,
                path,
                _lock: lock,
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(recovery_required()),
    }
    // Bound retained operations, never prune another operation as onboarding side work.
    let count = std::fs::read_dir(&folder)
        .map_err(|_| unavailable())?
        .take(257)
        .count();
    if count >= 256 {
        return Err(recovery_required());
    }
    let id = ulid::Ulid::new().to_string();
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|_| unavailable())?;
    let wire = format!(
        "lby_pc_v1_{id}_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random)
    );
    let now = labby_auth::util::now_unix();
    let mut journal = Journal {
        version: 1,
        identity: actor.into(),
        project_id: request.project_id.clone(),
        credential_id: id.clone(),
        credential_digest: Sha256::digest(wire.as_bytes()).into(),
        idempotency_digest: operation,
        request_digest: digest,
        policy_fingerprint: fingerprint,
        resource: resource.into(),
        issued_at: now,
        expires_at: now.checked_add(86400).ok_or_else(unavailable)?,
        state: State::Allocating,
        file: None,
    };
    secure_file::publish_new(
        &path,
        &serde_json::to_vec(&journal).map_err(|_| unavailable())?,
    )
    .map_err(|_| recovery_required())?;
    journal.file = Some(
        secure_file::publish_new(
            &credentials.join(format!("{id}.credential")),
            wire.as_bytes(),
        )
        .map_err(|_| recovery_required())?,
    );
    journal.state = State::Published;
    secure_file::replace_journal(
        &path,
        &serde_json::to_vec(&journal).map_err(|_| unavailable())?,
    )
    .map_err(|_| recovery_required())?;
    Ok(Custody {
        journal,
        path,
        _lock: lock,
    })
}

#[cfg(test)]
mod tests;
