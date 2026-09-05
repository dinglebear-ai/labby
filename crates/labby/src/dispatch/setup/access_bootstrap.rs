//! Offline preparation of one-time local access-bootstrap artifacts.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

use crate::access::{AccessStore, ActivateProofInput};
use crate::installation::{InstallationLifecycleLock, InstallationPaths};

use super::secure_file::{
    create_private_dir, delete_exact, publish_new, read_private, read_verified, replace_journal,
    verify_identity,
};
use super::types::{
    AccessBootstrapManifest, AccessBootstrapPrepare, AccessBootstrapPrepareOutcome, PrepareJournal,
    PrepareJournalState,
};

const JOURNAL_DIR: &str = "bootstrap-prepares";
const INSTALLATION_ID_FILE: &str = "installation-id";
const MAX_ACTIVE_PREPARES: usize = 4;
const MAX_TTL_SECONDS: u64 = 600;

#[derive(Serialize)]
struct ProofBundleRef<'a> {
    version: u8,
    prepare_id: &'a str,
    proof: &'a str,
    manifest: &'a AccessBootstrapManifest,
    manifest_digest_hex: &'a str,
    request_digest_hex: &'a str,
    credential_digest_hex: &'a str,
}

#[derive(Deserialize)]
struct ProofBundle {
    version: u8,
    prepare_id: String,
    proof: String,
    manifest: AccessBootstrapManifest,
    manifest_digest_hex: String,
    request_digest_hex: String,
    credential_digest_hex: String,
}

pub async fn prepare_access_bootstrap(
    request: AccessBootstrapPrepare,
) -> anyhow::Result<AccessBootstrapPrepareOutcome> {
    let paths = InstallationPaths::resolve()?;
    let _lifecycle = InstallationLifecycleLock::acquire_offline(&paths)?;
    let journal_dir = paths.root().join(JOURNAL_DIR);
    create_private_dir(&journal_dir)?;
    enforce_active_bound(&journal_dir)?;

    let installation_id = installation_id(&paths)?;
    let prepare_id = ulid::Ulid::new().to_string();
    let proof_id = ulid::Ulid::new().to_string();
    let credential_id = ulid::Ulid::new().to_string();
    let idempotency_key = ulid::Ulid::new().to_string();
    let manifest = normalize_manifest(
        &installation_id,
        &credential_id,
        &idempotency_key,
        request.clone(),
    )?;
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let manifest_digest_hex = digest_hex(&manifest_bytes);
    let request_digest_hex = manifest_digest_hex.clone();
    let idempotency_digest_hex = digest_hex(idempotency_key.as_bytes());

    let proof = secret_wire("lby_bp_v1", &proof_id)?;
    let credential = secret_wire("lby_pc_v1", &credential_id)?;
    let proof_digest_hex = digest_hex(proof.as_bytes());
    let credential_digest_hex = digest_hex(credential.as_bytes());
    let now = unix_seconds()?;
    let expires_at = now + i64::try_from(manifest.ttl_seconds)?;
    let journal_path = journal_dir.join(format!("{prepare_id}.json"));
    let mut journal = PrepareJournal {
        version: 1,
        prepare_id: prepare_id.clone(),
        installation_id,
        proof_id: proof_id.clone(),
        proof_digest_hex,
        credential_id: credential_id.clone(),
        credential_digest_hex: credential_digest_hex.clone(),
        manifest,
        manifest_digest_hex,
        request_digest_hex,
        idempotency_digest_hex,
        state: PrepareJournalState::Allocating,
        created_at: now,
        expires_at,
        proof_path: request.proof_file.clone(),
        credential_path: request.credential_file.clone(),
        proof_file: None,
        credential_file: None,
    };
    publish_new(&journal_path, &serde_json::to_vec(&journal)?)?;

    let proof_bundle = ProofBundleRef {
        version: 1,
        prepare_id: &prepare_id,
        proof: &proof,
        manifest: &journal.manifest,
        manifest_digest_hex: &journal.manifest_digest_hex,
        request_digest_hex: &journal.request_digest_hex,
        credential_digest_hex: &journal.credential_digest_hex,
    };
    journal.proof_file = Some(publish_new(
        &request.proof_file,
        &serde_json::to_vec(&proof_bundle)?,
    )?);
    replace_journal(&journal_path, &serde_json::to_vec(&journal)?)?;
    journal.credential_file = match publish_new(&request.credential_file, credential.as_bytes()) {
        Ok(identity) => Some(identity),
        Err(error) => {
            return abort_partial_prepare(&paths, journal, error.into()).await;
        }
    };
    journal.state = PrepareJournalState::FilesPublished;
    replace_journal(&journal_path, &serde_json::to_vec(&journal)?)?;

    let store = AccessStore::open(paths.access_db()).await?;
    store
        .activate_bootstrap_proof(activation(&journal)?)
        .await?;
    journal.state = PrepareJournalState::ProofActive;
    replace_journal(&journal_path, &serde_json::to_vec(&journal)?)?;

    Ok(AccessBootstrapPrepareOutcome {
        prepare_id,
        proof_id,
        credential_id,
        journal_state: journal.state,
        proof_file: request.proof_file,
        credential_file: request.credential_file,
    })
}

async fn abort_partial_prepare<T>(
    paths: &InstallationPaths,
    mut journal: PrepareJournal,
    cause: anyhow::Error,
) -> anyhow::Result<T> {
    // The allocating journal is durable before either secret is published. A
    // second-output failure must never leave output one paired with a proof
    // that can later be activated. Persist the terminal intent first, then
    // tombstone and delete only the exact recorded file identity.
    journal.state = PrepareJournalState::ManualFileCleanupRequired;
    write_journal(paths, &journal)?;
    match tombstone_and_cleanup_owned(paths, journal, "partial_prepare_publication").await {
        Ok(_) => Err(cause.context("credential publication failed; partial prepare was revoked")),
        Err(cleanup) => Err(cause.context(format!(
            "credential publication failed; partial prepare is non-active and requires cleanup: {cleanup}"
        ))),
    }
}

/// Reconcile every bounded prepare journal before the daemon begins serving.
///
/// The caller already owns the daemon lifecycle lock. This function performs
/// no network work and returns only after each stale or partial state is either
/// completed, durably tombstoned and exactly deleted, or left in the stable
/// `manual_file_cleanup_required` state. An error therefore prevents serving.
pub(crate) async fn reconcile_daemon_prepares(paths: &InstallationPaths) -> anyhow::Result<()> {
    let directory = paths.root().join(JOURNAL_DIR);
    if !directory.exists() {
        return Ok(());
    }
    let now = unix_seconds()?;
    for entry in fs::read_dir(&directory)?.take(MAX_ACTIVE_PREPARES + 1) {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = read_private(&path)?;
        let mut journal: PrepareJournal = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("prepare journal directory contains an invalid entry"))?;
        if journal.installation_id != existing_installation_id(paths)? {
            anyhow::bail!("prepare journal installation identity mismatch");
        }
        match journal.state {
            PrepareJournalState::Allocating => {
                tombstone_and_cleanup_owned(paths, journal, "startup_partial_prepare").await?;
            }
            PrepareJournalState::FilesPublished => {
                if journal.expires_at <= now || verify_outputs(&journal).is_err() {
                    tombstone_and_cleanup_owned(paths, journal, "startup_invalid_prepare").await?;
                } else {
                    AccessStore::open(paths.access_db())
                        .await?
                        .activate_bootstrap_proof(activation(&journal)?)
                        .await?;
                    journal.state = PrepareJournalState::ProofActive;
                    write_journal(paths, &journal)?;
                }
            }
            PrepareJournalState::ProofActive => {
                if journal.expires_at <= now || verify_outputs(&journal).is_err() {
                    tombstone_and_cleanup_owned(paths, journal, "startup_invalid_active_proof")
                        .await?;
                }
            }
            PrepareJournalState::Revoked | PrepareJournalState::ManualFileCleanupRequired => {
                journal.state = if delete_outputs(&journal).is_ok() {
                    PrepareJournalState::Cleaned
                } else {
                    PrepareJournalState::ManualFileCleanupRequired
                };
                write_journal(paths, &journal)?;
            }
            PrepareJournalState::Consumed | PrepareJournalState::Cleaned => {}
        }
    }
    Ok(())
}

/// Submit the reproducible prepared request to the running daemon. This thin
/// client never writes the journal or deletes either artifact.
pub async fn consume_prepare(prepare_id: &str) -> anyhow::Result<serde_json::Value> {
    let bundle = load_bundle(prepare_id)?;
    let base =
        std::env::var("LABBY_API_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8765".to_owned());
    let origin = bootstrap_api_origin(&base)?;
    let url = origin.join("auth/bootstrap/consume")?;
    let client = bootstrap_http_client()?;
    let response = client
        .post(url)
        .header("X-Labby-Bootstrap-Proof", &bundle.proof)
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .json(&bundle.manifest)
        .send()
        .await?;
    if response.status().is_success() {
        return Ok(response.json().await?);
    }
    let credential = load_credential(prepare_id)?;
    let self_url = origin.join("v1/access/credentials/self")?;
    let probe = client.get(self_url).bearer_auth(&credential).send().await?;
    if probe.status().is_success() {
        return Ok(probe.json().await?);
    }
    let retry = client
        .post(origin.join("auth/bootstrap/consume")?)
        .header("X-Labby-Bootstrap-Proof", &bundle.proof)
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .json(&bundle.manifest)
        .send()
        .await?;
    if !retry.status().is_success() {
        anyhow::bail!("bootstrap consume and credential recovery were denied");
    }
    Ok(retry.json().await?)
}

/// Ask the daemon for authoritative status. Like consume, this operation is
/// request-only and cannot advance local journal state from the CLI process.
pub async fn status_prepare(prepare_id: &str) -> anyhow::Result<serde_json::Value> {
    let bundle = load_bundle(prepare_id)?;
    let base =
        std::env::var("LABBY_API_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8765".to_owned());
    let origin = bootstrap_api_origin(&base)?;
    let url = origin.join("auth/bootstrap/status")?;
    let client = bootstrap_http_client()?;
    let response = client
        .post(url)
        .header("X-Labby-Bootstrap-Proof", bundle.proof)
        .json(&serde_json::json!({ "prepare_id": prepare_id }))
        .send()
        .await?;
    if response.status().is_success() {
        return Ok(response.json().await?);
    }
    let credential = load_credential(prepare_id)?;
    let self_url = origin.join("v1/access/credentials/self")?;
    let probe = client.get(self_url).bearer_auth(credential).send().await?;
    if !probe.status().is_success() {
        anyhow::bail!("bootstrap status and credential recovery were denied");
    }
    Ok(probe.json().await?)
}

fn bootstrap_api_origin(raw: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw)
        .map_err(|_| anyhow::anyhow!("LABBY_API_BASE_URL must be an absolute URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("LABBY_API_BASE_URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        anyhow::bail!(
            "LABBY_API_BASE_URL must contain only a trusted origin, without path, query, or fragment"
        );
    }
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        anyhow::bail!("LABBY_API_BASE_URL must use HTTPS, except for explicit loopback HTTP");
    }
    Ok(url)
}

fn bootstrap_http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

fn load_credential(prepare_id: &str) -> anyhow::Result<String> {
    let journal = inspect_prepare(prepare_id)?;
    let identity = journal
        .credential_file
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("prepare has no published credential file"))?;
    let bytes = read_verified(identity)?;
    let token = std::str::from_utf8(&bytes)?.to_owned();
    labby_primitives::product_credential::ProductCredential::parse(&token)
        .map_err(|_| anyhow::anyhow!("prepared credential has invalid canonical form"))?;
    Ok(token)
}

fn load_bundle(prepare_id: &str) -> anyhow::Result<ProofBundle> {
    let journal = inspect_prepare(prepare_id)?;
    let proof_identity = journal
        .proof_file
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("prepare has no published proof file"))?;
    let bundle: ProofBundle = serde_json::from_slice(&read_verified(proof_identity)?)?;
    if bundle.version != 1
        || bundle.prepare_id != journal.prepare_id
        || bundle.manifest != journal.manifest
        || bundle.manifest_digest_hex != journal.manifest_digest_hex
        || bundle.request_digest_hex != journal.request_digest_hex
        || bundle.credential_digest_hex != journal.credential_digest_hex
    {
        anyhow::bail!("prepared proof bundle does not match its journal");
    }
    Ok(bundle)
}

/// Inspect a prepare without mutating it. Online consume/status adapters may
/// use this, but only the daemon may advance journal state while it is running.
pub fn inspect_prepare(prepare_id: &str) -> anyhow::Result<PrepareJournal> {
    validate_public_id("prepare_id", prepare_id)?;
    let paths = InstallationPaths::resolve()?;
    let journal = read_journal(&paths, prepare_id)?;
    if let Some(identity) = &journal.proof_file {
        verify_identity(identity)?;
    }
    if let Some(identity) = &journal.credential_file {
        verify_identity(identity)?;
    }
    Ok(journal)
}

/// Offline recovery inspection. Acquiring the shared lifecycle lock proves the
/// daemon is stopped before any later completion/revocation operation is added.
pub fn recover_prepare(prepare_id: &str) -> anyhow::Result<PrepareJournal> {
    let paths = InstallationPaths::resolve()?;
    let _lifecycle = InstallationLifecycleLock::acquire_offline(&paths)?;
    let journal = read_journal(&paths, prepare_id)?;
    if let Some(identity) = &journal.proof_file {
        verify_identity(identity)?;
    }
    if let Some(identity) = &journal.credential_file {
        verify_identity(identity)?;
    }
    Ok(journal)
}

pub async fn complete_prepare(prepare_id: &str) -> anyhow::Result<PrepareJournal> {
    let paths = InstallationPaths::resolve()?;
    let _lifecycle = InstallationLifecycleLock::acquire_offline(&paths)?;
    let mut journal = read_journal(&paths, prepare_id)?;
    verify_outputs(&journal)?;
    if !matches!(
        journal.state,
        PrepareJournalState::FilesPublished | PrepareJournalState::ProofActive
    ) {
        anyhow::bail!("prepare is not eligible for completion");
    }
    if journal.expires_at <= unix_seconds()? {
        anyhow::bail!("prepare proof has expired and must be revoked");
    }
    let store = AccessStore::open(paths.access_db()).await?;
    store
        .activate_bootstrap_proof(activation(&journal)?)
        .await?;
    journal.state = PrepareJournalState::ProofActive;
    write_journal(&paths, &journal)?;
    Ok(journal)
}

pub async fn revoke_prepare(prepare_id: &str) -> anyhow::Result<PrepareJournal> {
    tombstone_and_cleanup(prepare_id, "operator_recovery_revoke").await
}

/// Fail-closed placeholder until the access-store lane exposes the mandatory
/// durable proof+credential tombstone transaction. Deleting first is forbidden.
pub async fn cleanup_prepare(prepare_id: &str) -> anyhow::Result<PrepareJournal> {
    tombstone_and_cleanup(prepare_id, "operator_cleanup").await
}

async fn tombstone_and_cleanup(prepare_id: &str, reason: &str) -> anyhow::Result<PrepareJournal> {
    let paths = InstallationPaths::resolve()?;
    let _lifecycle = InstallationLifecycleLock::acquire_offline(&paths)?;
    tombstone_and_cleanup_owned(&paths, read_journal(&paths, prepare_id)?, reason).await
}

/// Daemon-owned cleanup. The caller is the process holding the installation
/// lifecycle lock for its full lifetime; online adapters never call this
/// helper directly.
pub(crate) async fn cleanup_daemon_prepare(
    journal: PrepareJournal,
) -> anyhow::Result<PrepareJournal> {
    let paths = InstallationPaths::resolve()?;
    tombstone_and_cleanup_owned(&paths, journal, "daemon_authenticated_cleanup").await
}

async fn tombstone_and_cleanup_owned(
    paths: &InstallationPaths,
    mut journal: PrepareJournal,
    reason: &str,
) -> anyhow::Result<PrepareJournal> {
    let store = AccessStore::open(paths.access_db()).await?;
    let now = unix_seconds()?;
    store
        .tombstone_access_artifacts(
            journal.installation_id.clone(),
            vec![
                (
                    "credential".into(),
                    journal.credential_id.clone(),
                    digest32(&journal.credential_digest_hex)?,
                    1,
                ),
                (
                    "proof".into(),
                    journal.proof_id.clone(),
                    digest32(&journal.proof_digest_hex)?,
                    1,
                ),
                (
                    "prepare".into(),
                    journal.prepare_id.clone(),
                    Sha256::digest(journal.prepare_id.as_bytes()).into(),
                    1,
                ),
            ],
            reason.into(),
            now,
        )
        .await?;
    journal.state = PrepareJournalState::Revoked;
    write_journal(&paths, &journal)?;
    let deleted = delete_outputs(&journal);
    journal.state = if deleted.is_ok() {
        PrepareJournalState::Cleaned
    } else {
        PrepareJournalState::ManualFileCleanupRequired
    };
    write_journal(&paths, &journal)?;
    deleted?;
    Ok(journal)
}

/// Authenticate proof possession against the bounded installation journal.
/// A manifest is supplied for consume and must match byte-for-byte after
/// normalization; status/cleanup supply the public prepare ID instead.
pub(crate) fn authenticate_daemon_prepare(
    proof: &str,
    manifest: Option<&AccessBootstrapManifest>,
    prepare_id: Option<&str>,
) -> anyhow::Result<PrepareJournal> {
    let paths = InstallationPaths::resolve()?;
    let directory = paths.root().join(JOURNAL_DIR);
    let expected = Sha256::digest(proof.as_bytes());
    for entry in fs::read_dir(directory)?.take(MAX_ACTIVE_PREPARES + 1) {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = read_private(&path)?;
        let journal: PrepareJournal = serde_json::from_slice(&bytes)?;
        if prepare_id.is_some_and(|id| journal.prepare_id != id)
            || manifest.is_some_and(|value| journal.manifest != *value)
        {
            continue;
        }
        let stored = hex::decode(&journal.proof_digest_hex)?;
        if stored.len() == expected.len() && bool::from(stored.ct_eq(expected.as_slice())) {
            return read_journal(&paths, &journal.prepare_id);
        }
    }
    anyhow::bail!("bootstrap proof is not authorized")
}

pub(crate) fn advance_daemon_prepare_consumed(
    mut journal: PrepareJournal,
) -> anyhow::Result<PrepareJournal> {
    let paths = InstallationPaths::resolve()?;
    journal.state = PrepareJournalState::Consumed;
    write_journal(&paths, &journal)?;
    Ok(journal)
}

fn normalize_manifest(
    installation_id: &str,
    credential_id: &str,
    idempotency_key: &str,
    request: AccessBootstrapPrepare,
) -> anyhow::Result<AccessBootstrapManifest> {
    if !(1..=MAX_TTL_SECONDS).contains(&request.ttl_seconds) {
        anyhow::bail!("ttl_seconds must be between 1 and {MAX_TTL_SECONDS}");
    }
    let mut scopes = request
        .scopes
        .iter()
        .map(|scope| normalize_text("scope", scope))
        .collect::<anyhow::Result<Vec<_>>>()?;
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() || scopes.len() > 32 {
        anyhow::bail!("scopes must contain 1 to 32 unique values");
    }
    let resource = url::Url::parse(request.resource.trim())?;
    if resource.cannot_be_a_base() && resource.scheme().is_empty() {
        anyhow::bail!("resource must be a canonical absolute URI");
    }
    Ok(AccessBootstrapManifest {
        version: 1,
        installation_id: installation_id.to_owned(),
        canonical_issuer: format!("urn:labby:local-operator:{installation_id}"),
        organization_name: normalize_text("organization_name", &request.organization_name)?,
        project_name: normalize_text("project_name", &request.project_name)?,
        subject: normalize_text("subject", &request.subject)?,
        loadout_id: normalize_text("loadout_id", &request.loadout_id)?,
        route_id: normalize_text("route_id", &request.route_id)?,
        resource: resource.to_string(),
        scopes,
        ttl_seconds: request.ttl_seconds,
        credential_id: credential_id.to_owned(),
        idempotency_key: idempotency_key.to_owned(),
    })
}

fn normalize_text(field: &str, value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        anyhow::bail!("{field} must be non-empty, at most 256 bytes, and contain no controls");
    }
    Ok(value.to_owned())
}

fn installation_id(paths: &InstallationPaths) -> anyhow::Result<String> {
    let path = paths.root().join(INSTALLATION_ID_FILE);
    match existing_installation_id(paths) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let id = ulid::Ulid::new().to_string();
            publish_new(&path, id.as_bytes())?;
            Ok(id)
        }
        Err(error) => Err(error.into()),
    }
}

fn existing_installation_id(paths: &InstallationPaths) -> std::io::Result<String> {
    let bytes = read_private(&paths.root().join(INSTALLATION_ID_FILE))?;
    let value = std::str::from_utf8(&bytes)
        .map_err(|_| std::io::Error::other("installation ID is not UTF-8"))?
        .trim();
    validate_public_id("installation_id", value)
        .map_err(|_| std::io::Error::other("installation ID is invalid"))?;
    Ok(value.to_owned())
}

fn secret_wire(prefix: &str, public_id: &str) -> anyhow::Result<String> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret)?;
    Ok(format!(
        "{prefix}_{public_id}_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
    ))
}

fn read_journal(paths: &InstallationPaths, prepare_id: &str) -> anyhow::Result<PrepareJournal> {
    validate_public_id("prepare_id", prepare_id)?;
    let bytes = read_private(
        &paths
            .root()
            .join(JOURNAL_DIR)
            .join(format!("{prepare_id}.json")),
    )?;
    let journal: PrepareJournal = serde_json::from_slice(&bytes)?;
    if journal.prepare_id != prepare_id
        || journal.installation_id != existing_installation_id(paths)?
    {
        anyhow::bail!("prepare journal identity mismatch");
    }
    Ok(journal)
}

fn enforce_active_bound(directory: &Path) -> anyhow::Result<()> {
    let mut active = 0;
    for entry in fs::read_dir(directory)?.take(MAX_ACTIVE_PREPARES + 1) {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let bytes = read_private(&path)?;
        let journal: PrepareJournal = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("prepare journal directory contains an invalid entry"))?;
        if !matches!(
            journal.state,
            PrepareJournalState::Cleaned | PrepareJournalState::Revoked
        ) {
            active += 1;
        }
    }
    if active >= MAX_ACTIVE_PREPARES {
        anyhow::bail!("maximum active access-bootstrap prepares reached");
    }
    Ok(())
}

fn validate_public_id(field: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid {field}");
    }
    Ok(())
}

fn digest_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn digest32(value: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(value)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("stored digest has invalid length"))
}

fn activation(journal: &PrepareJournal) -> anyhow::Result<ActivateProofInput> {
    Ok(ActivateProofInput {
        proof_id: journal.proof_id.clone(),
        prepare_id: journal.prepare_id.clone(),
        installation_id: journal.installation_id.clone(),
        installation_generation: 1,
        proof_digest: digest32(&journal.proof_digest_hex)?,
        manifest_digest: digest32(&journal.manifest_digest_hex)?,
        request_digest: digest32(&journal.request_digest_hex)?,
        idempotency_digest: digest32(&journal.idempotency_digest_hex)?,
        credential_id: journal.credential_id.clone(),
        credential_digest: digest32(&journal.credential_digest_hex)?,
        proof_generation: 1,
        created_at: journal.created_at,
        expires_at: journal.expires_at,
    })
}

fn verify_outputs(journal: &PrepareJournal) -> anyhow::Result<()> {
    verify_identity(
        journal
            .proof_file
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("proof output missing from journal"))?,
    )?;
    verify_identity(
        journal
            .credential_file
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("credential output missing from journal"))?,
    )?;
    Ok(())
}

fn write_journal(paths: &InstallationPaths, journal: &PrepareJournal) -> anyhow::Result<()> {
    replace_journal(
        &paths
            .root()
            .join(JOURNAL_DIR)
            .join(format!("{}.json", journal.prepare_id)),
        &serde_json::to_vec(journal)?,
    )?;
    Ok(())
}

fn delete_outputs(journal: &PrepareJournal) -> anyhow::Result<()> {
    let mut first_error = None;
    for identity in [&journal.proof_file, &journal.credential_file]
        .into_iter()
        .flatten()
    {
        if let Err(error) = delete_exact(identity) {
            first_error.get_or_insert(error);
        }
    }
    for (path, identity) in [
        (&journal.proof_path, &journal.proof_file),
        (&journal.credential_path, &journal.credential_file),
    ] {
        if identity.is_none() && path.exists() {
            first_error.get_or_insert_with(|| {
                std::io::Error::other(format!(
                    "{} exists without a recorded exact identity",
                    path.display()
                ))
            });
        }
    }
    if let Some(error) = first_error {
        Err(error.into())
    } else {
        Ok(())
    }
}

fn unix_seconds() -> anyhow::Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> AccessBootstrapPrepare {
        AccessBootstrapPrepare {
            proof_file: "/tmp/proof".into(),
            credential_file: "/tmp/credential".into(),
            organization_name: " Example Org ".into(),
            project_name: " Project ".into(),
            subject: " operator ".into(),
            loadout_id: "loadout-1".into(),
            route_id: "route-1".into(),
            resource: "https://example.test/mcp".into(),
            scopes: vec!["lab:read".into(), "lab:read".into(), "lab".into()],
            ttl_seconds: 600,
        }
    }

    #[test]
    fn manifest_is_canonical_and_replay_stable() {
        let manifest = normalize_manifest("install", "credential", "idem", request()).unwrap();
        assert_eq!(manifest.organization_name, "Example Org");
        assert_eq!(manifest.scopes, ["lab", "lab:read"]);
        assert_eq!(
            manifest.canonical_issuer,
            "urn:labby:local-operator:install"
        );
        assert_eq!(
            serde_json::to_vec(&manifest).unwrap(),
            serde_json::to_vec(&manifest).unwrap()
        );
    }

    #[test]
    fn manifest_rejects_unbounded_ttl_and_empty_scopes() {
        let mut invalid = request();
        invalid.ttl_seconds = 601;
        assert!(normalize_manifest("install", "credential", "idem", invalid).is_err());
        let mut invalid = request();
        invalid.scopes.clear();
        assert!(normalize_manifest("install", "credential", "idem", invalid).is_err());
    }

    #[test]
    fn credential_endpoint_policy_allows_https_and_loopback_http_only() {
        for allowed in [
            "https://lab.example.com",
            "https://lab.example.com:8443/",
            "http://127.0.0.1:8765",
            "http://[::1]:8765",
            "http://localhost:8765",
        ] {
            assert!(
                bootstrap_api_origin(allowed).is_ok(),
                "should allow {allowed}"
            );
        }
        for rejected in [
            "http://lab.example.com",
            "http://192.168.1.10:8765",
            "https://user:secret@lab.example.com",
            "https://lab.example.com/prefix",
            "https://lab.example.com?token=secret",
            "https://lab.example.com/#fragment",
            "file:///tmp/socket",
            "not a URL",
        ] {
            assert!(
                bootstrap_api_origin(rejected).is_err(),
                "must reject {rejected}"
            );
        }
    }

    #[test]
    fn credential_client_refuses_all_redirects() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let policy = bootstrap_http_client().expect("client policy");
        // Debug output is stable enough to prove construction succeeds, while
        // the two-server async test below proves the no-forward behavior.
        assert!(!format!("{policy:?}").is_empty());
    }

    #[tokio::test]
    async fn credential_client_never_forwards_secrets_to_redirect_origin() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        drop(rustls::crypto::ring::default_provider().install_default());
        let redirect = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_addr = redirect.local_addr().unwrap();
        let sink_addr = sink.local_addr().unwrap();
        let responder = tokio::spawn(async move {
            let (mut socket, _) = redirect.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let read = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).contains("secret-sentinel"));
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{sink_addr}/capture\r\nContent-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let response = bootstrap_http_client()
            .unwrap()
            .get(format!("http://{redirect_addr}/start"))
            .bearer_auth("secret-sentinel")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        responder.await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), sink.accept())
                .await
                .is_err(),
            "redirect target received a credential-bearing connection"
        );
    }

    #[cfg(unix)]
    fn crashed_allocating_prepare(paths: &InstallationPaths, output: &Path) -> PrepareJournal {
        create_private_dir(paths.root()).unwrap();
        publish_new(&paths.root().join(INSTALLATION_ID_FILE), b"install-1").unwrap();
        let journal_dir = paths.root().join(JOURNAL_DIR);
        create_private_dir(&journal_dir).unwrap();
        let proof = "lby_bp_v1_proof-1_secret";
        let proof_file = publish_new(output, proof.as_bytes()).unwrap();
        let manifest =
            normalize_manifest("install-1", "credential-1", "idem-1", request()).unwrap();
        let manifest_digest_hex = digest_hex(&serde_json::to_vec(&manifest).unwrap());
        let journal = PrepareJournal {
            version: 1,
            prepare_id: "prepare-1".into(),
            installation_id: "install-1".into(),
            proof_id: "proof-1".into(),
            proof_digest_hex: digest_hex(proof.as_bytes()),
            credential_id: "credential-1".into(),
            credential_digest_hex: digest_hex(b"credential-secret"),
            manifest,
            manifest_digest_hex: manifest_digest_hex.clone(),
            request_digest_hex: manifest_digest_hex,
            idempotency_digest_hex: digest_hex(b"idem-1"),
            state: PrepareJournalState::Allocating,
            created_at: unix_seconds().unwrap(),
            expires_at: unix_seconds().unwrap() + 600,
            proof_path: output.to_path_buf(),
            credential_path: output.with_extension("credential"),
            proof_file: Some(proof_file),
            credential_file: None,
        };
        write_journal(paths, &journal).unwrap();
        journal
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_reconciliation_tombstones_and_deletes_partial_first_output() {
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("home")).unwrap();
        let output_dir = temp.path().join("outputs");
        create_private_dir(&output_dir).unwrap();
        let output = output_dir.join("proof.json");
        let journal = crashed_allocating_prepare(&paths, &output);

        reconcile_daemon_prepares(&paths).await.unwrap();

        assert!(!output.exists());
        assert_eq!(
            read_journal(&paths, &journal.prepare_id).unwrap().state,
            PrepareJournalState::Cleaned
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_reconciliation_never_deletes_replacement_path() {
        let temp = tempfile::tempdir().unwrap();
        let paths = InstallationPaths::from_root(temp.path().join("home")).unwrap();
        let output_dir = temp.path().join("outputs");
        create_private_dir(&output_dir).unwrap();
        let output = output_dir.join("proof.json");
        let journal = crashed_allocating_prepare(&paths, &output);
        fs::remove_file(&output).unwrap();
        publish_new(&output, b"replacement").unwrap();

        assert!(reconcile_daemon_prepares(&paths).await.is_err());

        assert_eq!(fs::read(&output).unwrap(), b"replacement");
        assert_eq!(
            read_journal(&paths, &journal.prepare_id).unwrap().state,
            PrepareJournalState::ManualFileCleanupRequired
        );
    }
}
