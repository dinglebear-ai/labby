//! Resumable first-use evidence owned by the binary.
//!
//! There is deliberately no dispatch action for asserting success. Operation
//! owners record evidence only after their actual verification succeeds. Saved
//! credentials, installed services and imported artifacts cannot mark readiness.

use std::io::Read as _;
use std::{
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::dispatch::{error::ToolError, helpers::to_json};

const MAX_AGE_SECONDS: u64 = 24 * 60 * 60;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Check {
    GatewayAuthenticated,
    AgentProvider,
    AgentRun,
    SelectedClients,
    CatalogSearch,
    McpToolCall,
}

const REQUIRED: [Check; 6] = [
    Check::GatewayAuthenticated,
    Check::AgentProvider,
    Check::AgentRun,
    Check::SelectedClients,
    Check::CatalogSearch,
    Check::McpToolCall,
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRunEvidence {
    agent_id: String,
    version: u64,
    session_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    verified_at: u64,
    configuration_digest: String,
    resource_id: String,
    #[serde(default)]
    deferred: bool,
    #[serde(default)]
    agent_run: Option<AgentRunEvidence>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    checks: BTreeMap<Check, Evidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Pending,
    Verified,
    NeedsRecheck,
    Deferred,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ReadinessCheck {
    check: Check,
    status: CheckStatus,
    verified_at: Option<u64>,
    resource_id: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ReadinessState {
    ready: bool,
    checks: Vec<ReadinessCheck>,
    evidence_max_age_seconds: u64,
}

fn failure(error: impl std::fmt::Display) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "readiness_state_unavailable".into(),
        message: format!("First-use evidence could not be read or saved: {error}"),
    }
}

fn now() -> Result<u64, ToolError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(failure)
}

fn subject_key(subject: &str) -> Result<String, ToolError> {
    if subject.trim().is_empty() || subject.len() > 512 {
        return Err(ToolError::InvalidParam {
            param: "principal_id".into(),
            message: "A nonempty principal ID of at most 512 bytes is required".into(),
        });
    }
    Ok(hex::encode(Sha256::digest(subject.as_bytes())))
}

fn configuration_digest(root: &Path, check: Check) -> Result<String, ToolError> {
    let mut digest = Sha256::new();
    let config_path = if root == crate::dispatch::helpers::lab_home() {
        crate::config::config_toml_path().map_err(failure)?
    } else {
        root.join("config.toml")
    };
    let sections: &[&str] = match check {
        Check::AgentProvider | Check::AgentRun => &[],
        Check::GatewayAuthenticated | Check::SelectedClients => &["auth", "mcp", "public_urls"],
        Check::CatalogSearch => &["depot"],
        Check::McpToolCall => &["upstream", "gateway", "mcp", "protected_mcp_routes"],
    };
    let mut referenced_env = std::collections::BTreeSet::new();
    if !sections.is_empty() {
        let bytes = match bounded_configuration_read(&config_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(failure(error)),
        };
        let source = String::from_utf8(bytes)
            .map_err(|_| failure("Saved configuration is not valid UTF-8"))?;
        let document: toml::Value = toml::from_str(&source).map_err(|_| {
            failure("Saved configuration is not valid TOML; repair it before verification")
        })?;
        for name in sections {
            digest.update(name.as_bytes());
            if let Some(value) = document.get(*name) {
                digest.update(value.to_string().as_bytes());
                collect_environment_refs(value, &mut referenced_env);
            }
        }
    }
    let relevant = |key: &str| match check {
        Check::AgentProvider | Check::AgentRun => matches!(
            key,
            "LABBY_PHOENIX_OPENAI_BASE_URL"
                | "LABBY_PHOENIX_OPENAI_API_KEY"
                | "LABBY_AGENT_PROVIDER_PROTOCOL"
        ),
        Check::GatewayAuthenticated | Check::SelectedClients => {
            key.starts_with("LABBY_AUTH_")
                || key.starts_with("LABBY_OAUTH_")
                || key.starts_with("LABBY_MCP_HTTP_")
                || matches!(key, "LABBY_PUBLIC_URL" | "LABBY_MCP_PUBLIC_URL")
        }
        Check::CatalogSearch => {
            key.starts_with("LABBY_DEPOT_")
                || key.starts_with("DEPOT_")
                || referenced_env.contains(key)
        }
        Check::McpToolCall => key.starts_with("LABBY_GATEWAY_") || referenced_env.contains(key),
    };
    let mut saved = BTreeMap::new();
    match bounded_configuration_read(&root.join(".env")) {
        Ok(bytes) => {
            for pair in dotenvy::from_read_iter(bytes.as_slice()) {
                let (key, value) = pair.map_err(|_| {
                    failure(
                        "Saved environment has invalid assignments; repair it before verification",
                    )
                })?;
                if relevant(&key) {
                    saved.insert(key, value);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(failure(error)),
    }
    // Saved values and effective process overrides both bind evidence. A saved
    // provider change immediately requires retesting even before restart.
    digest.update(serde_json::to_vec(&saved).map_err(failure)?);
    let effective = std::env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            relevant(&key).then(|| (key, value.to_string_lossy().into_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    digest.update(serde_json::to_vec(&effective).map_err(failure)?);
    Ok(hex::encode(digest.finalize()))
}

fn collect_environment_refs(value: &toml::Value, output: &mut std::collections::BTreeSet<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                if key.ends_with("_env")
                    && let Some(name) = value.as_str()
                {
                    output.insert(name.into());
                }
                collect_environment_refs(value, output);
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                collect_environment_refs(value, output);
            }
        }
        _ => {}
    }
}

fn bounded_configuration_read(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(std::io::Error::other(
            "configuration exceeds readiness evidence budget",
        ));
    }
    Ok(bytes)
}

fn journal_lock(
    root: &Path,
    subject: &str,
) -> Result<crate::config::host_write::HostConfigLock, ToolError> {
    let key = subject_key(subject)?;
    let directory = root.join("first-use");
    super::secure_file::create_private_dir(&directory).map_err(failure)?;
    crate::config::host_write::HostConfigLock::acquire(&directory.join(format!("{key}.json")))
        .map_err(failure)
}

fn read(root: &Path, subject: &str) -> Result<Journal, ToolError> {
    let path = root
        .join("first-use")
        .join(format!("{}.json", subject_key(subject)?));
    match super::secure_file::read_private(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(failure),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(failure(error)),
    }
}

/// Opaque pre-operation configuration identity. Never contains credentials.
#[derive(Clone)]
pub(crate) struct CheckFingerprint {
    check: Check,
    digest: String,
}

pub(crate) fn capture_configuration_for_store(
    store: &crate::access::AccessStore,
    check: Check,
) -> Result<CheckFingerprint, ToolError> {
    Ok(CheckFingerprint {
        check,
        digest: configuration_digest(&store.storage_dir(), check)?,
    })
}
pub(crate) fn configuration_digest_for_store(
    store: &crate::access::AccessStore,
    check: Check,
) -> Result<String, ToolError> {
    configuration_digest(&store.storage_dir(), check)
}
pub(crate) fn record_verified_with_expected_digest_for_store(
    store: &crate::access::AccessStore,
    subject: &str,
    check: Check,
    resource_id: &str,
    expected_digest: &str,
) -> Result<(), ToolError> {
    record_evidence_at(
        &store.storage_dir(),
        subject,
        check,
        resource_id,
        now()?,
        None,
        Some(expected_digest),
    )
}
pub(crate) fn record_verified_if_unchanged_for_store(
    store: &crate::access::AccessStore,
    subject: &str,
    check: Check,
    resource_id: &str,
    expected: CheckFingerprint,
) -> Result<(), ToolError> {
    if expected.check != check {
        return Err(failure(
            "verification configuration belongs to another check",
        ));
    }
    record_evidence_at(
        &store.storage_dir(),
        subject,
        check,
        resource_id,
        now()?,
        None,
        Some(&expected.digest),
    )
}
pub(crate) fn record_agent_run_if_unchanged_for_store(
    store: &crate::access::AccessStore,
    subject: &str,
    agent_id: &str,
    version: u64,
    session_id: &str,
    expected: CheckFingerprint,
) -> Result<(), ToolError> {
    if expected.check != Check::AgentRun
        || agent_id.is_empty()
        || agent_id.len() > 256
        || session_id.is_empty()
        || session_id.len() > 256
        || version == 0
    {
        return Err(failure("invalid Agent verification identity"));
    }
    record_evidence_at(
        &store.storage_dir(),
        subject,
        Check::AgentRun,
        session_id,
        now()?,
        Some(AgentRunEvidence {
            agent_id: agent_id.into(),
            version,
            session_id: session_id.into(),
        }),
        Some(&expected.digest),
    )
}

/// Record using the owning access store's installation root. This preserves
/// isolation for remote, multi-installation and disposable test stores.
pub(crate) fn record_verified_for_store(
    store: &crate::access::AccessStore,
    subject: &str,
    check: Check,
    resource_id: &str,
) -> Result<(), ToolError> {
    record_at(&store.storage_dir(), subject, check, resource_id, now()?)
}

#[cfg(test)]
pub(crate) fn record_agent_run_for_store(
    store: &crate::access::AccessStore,
    subject: &str,
    agent_id: &str,
    version: u64,
    session_id: &str,
) -> Result<(), ToolError> {
    if agent_id.is_empty()
        || agent_id.len() > 256
        || session_id.is_empty()
        || session_id.len() > 256
        || version == 0
    {
        return Err(failure("invalid Agent verification identity"));
    }
    record_evidence_at(
        &store.storage_dir(),
        subject,
        Check::AgentRun,
        session_id,
        now()?,
        Some(AgentRunEvidence {
            agent_id: agent_id.into(),
            version,
            session_id: session_id.into(),
        }),
        None,
    )
}

pub(crate) fn invalidate_verified_for_store(
    store: &crate::access::AccessStore,
    subject: &str,
    check: Check,
) -> Result<(), ToolError> {
    invalidate_at(&store.storage_dir(), subject, check)
}

pub(crate) async fn state_for_identity(
    store: crate::access::AccessStore,
    identity: labby_auth::VerifiedIdentity,
) -> Result<Value, ToolError> {
    current_state(store, identity, false).await
}

pub(crate) async fn defer_clients_for_identity(
    store: crate::access::AccessStore,
    identity: labby_auth::VerifiedIdentity,
) -> Result<Value, ToolError> {
    current_state(store, identity, true).await
}

async fn current_state(
    store: crate::access::AccessStore,
    identity: labby_auth::VerifiedIdentity,
    defer_clients: bool,
) -> Result<Value, ToolError> {
    let principal = crate::access::resolve_personal_owner(&store, identity)
        .await
        .map_err(|error| {
            crate::dispatch::access_errors::map_store_error("setup", error, identity_required)
        })?;
    let principal = principal.id().to_owned();
    let inspection_store = store.clone();
    let (mut state, agent_evidence) = tokio::task::spawn_blocking(move || {
        if defer_clients {
            let root = store.storage_dir();
            let _lock = journal_lock(&root, principal.as_str())?;
            let mut journal = read(&root, principal.as_str())?;
            journal.checks.insert(
                Check::SelectedClients,
                Evidence {
                    verified_at: now()?,
                    configuration_digest: String::new(),
                    resource_id: "no-external-clients-selected".into(),
                    deferred: true,
                    agent_run: None,
                },
            );
            let path = root
                .join("first-use")
                .join(format!("{}.json", subject_key(principal.as_str())?));
            super::secure_file::replace_journal(
                &path,
                &serde_json::to_vec(&journal).map_err(failure)?,
            )
            .map_err(failure)?;
        }
        record_verified_for_store(
            &store,
            principal.as_str(),
            Check::GatewayAuthenticated,
            "authenticated-request",
        )?;
        let state = snapshot_at(&store.storage_dir(), principal.as_str(), now()?)?;
        let agent_evidence = read(&store.storage_dir(), principal.as_str())?
            .checks
            .get(&Check::AgentRun)
            .and_then(|evidence| evidence.agent_run.clone());
        Ok::<_, ToolError>((state, agent_evidence))
    })
    .await
    .map_err(failure)??;
    if let Some(check) = state
        .checks
        .iter_mut()
        .find(|check| check.check == Check::AgentRun && check.status == CheckStatus::Verified)
    {
        let valid = if let Some(evidence) = agent_evidence {
            let definition = inspection_store
                .get_agent_definition(evidence.agent_id.clone())
                .await
                .map_err(|error| {
                    crate::dispatch::access_errors::map_store_error(
                        "setup",
                        error,
                        identity_required,
                    )
                })?;
            let session = inspection_store
                .get_agent_session_status(evidence.agent_id, evidence.session_id)
                .await
                .map_err(|error| {
                    crate::dispatch::access_errors::map_store_error(
                        "setup",
                        error,
                        identity_required,
                    )
                })?;
            definition.is_some_and(|definition| {
                definition.revision.version == evidence.version
                    && definition.state == labby_primitives::agent::AgentState::Active
            }) && session.as_deref() == Some("completed")
        } else {
            false
        };
        if !valid {
            check.status = CheckStatus::NeedsRecheck;
        }
    }
    state.ready = state
        .checks
        .iter()
        .all(|check| matches!(check.status, CheckStatus::Verified | CheckStatus::Deferred));
    to_json(state)
}

fn invalidate_at(root: &Path, subject: &str, check: Check) -> Result<(), ToolError> {
    let _lock = journal_lock(root, subject)?;
    let mut journal = read(root, subject)?;
    if journal.checks.remove(&check).is_none() {
        return Ok(());
    }
    let path = root
        .join("first-use")
        .join(format!("{}.json", subject_key(subject)?));
    super::secure_file::replace_journal(&path, &serde_json::to_vec(&journal).map_err(failure)?)
        .map_err(failure)
}

fn record_at(
    root: &Path,
    subject: &str,
    check: Check,
    resource_id: &str,
    verified_at: u64,
) -> Result<(), ToolError> {
    record_evidence_at(root, subject, check, resource_id, verified_at, None, None)
}

fn record_evidence_at(
    root: &Path,
    subject: &str,
    check: Check,
    resource_id: &str,
    verified_at: u64,
    agent_run: Option<AgentRunEvidence>,
    expected_digest: Option<&str>,
) -> Result<(), ToolError> {
    if resource_id.trim().is_empty() || resource_id.len() > 512 {
        return Err(ToolError::InvalidParam {
            param: "resource_id".into(),
            message: "Verification must identify a bounded, nonempty result".into(),
        });
    }
    let _lock = journal_lock(root, subject)?;
    let current_digest = configuration_digest(root, check)?;
    if expected_digest.is_some_and(|expected| expected != current_digest) {
        return Err(failure(
            "Configuration changed during verification. Review it and run the check again.",
        ));
    }
    let mut journal = read(root, subject)?;
    journal.checks.insert(
        check,
        Evidence {
            verified_at,
            configuration_digest: expected_digest.unwrap_or(&current_digest).to_owned(),
            resource_id: resource_id.into(),
            deferred: false,
            agent_run,
        },
    );
    let path = root
        .join("first-use")
        .join(format!("{}.json", subject_key(subject)?));
    super::secure_file::replace_journal(&path, &serde_json::to_vec(&journal).map_err(failure)?)
        .map_err(failure)
}

fn snapshot_at(root: &Path, subject: &str, at: u64) -> Result<ReadinessState, ToolError> {
    let journal = read(root, subject)?;
    let checks = REQUIRED
        .into_iter()
        .map(|check| {
            let digest = configuration_digest(root, check)?;
            let evidence = journal.checks.get(&check);
            let status = match evidence {
                None => CheckStatus::Pending,
                Some(e) if e.deferred && check == Check::SelectedClients => CheckStatus::Deferred,
                Some(e)
                    if e.configuration_digest != digest
                        || e.verified_at > at
                        || at.saturating_sub(e.verified_at) > MAX_AGE_SECONDS =>
                {
                    CheckStatus::NeedsRecheck
                }
                Some(_) => CheckStatus::Verified,
            };
            Ok(ReadinessCheck {
                check,
                status,
                verified_at: evidence.map(|e| e.verified_at),
                resource_id: evidence.map(|e| e.resource_id.clone()),
            })
        })
        .collect::<Result<Vec<_>, ToolError>>()?;
    Ok(ReadinessState {
        ready: checks
            .iter()
            .all(|check| matches!(check.status, CheckStatus::Verified | CheckStatus::Deferred)),
        checks,
        evidence_max_age_seconds: MAX_AGE_SECONDS,
    })
}

fn identity_required() -> ToolError {
    ToolError::Forbidden {
        message: "First-use evidence requires a host-established identity".into(),
        required_scopes: vec![],
    }
}

pub(crate) fn state_action(_params: &Value) -> Result<Value, ToolError> {
    Err(ToolError::Forbidden {
        message: "First-use evidence requires a host-established identity".into(),
        required_scopes: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn authenticated_identity_owns_its_state_and_unknown_identity_is_denied() {
        let (_root, store, identity) = crate::dispatch::agents::test_support::fixture().await;
        let state = state_for_identity(store.clone(), identity).await.unwrap();
        assert_eq!(state["checks"][0]["status"], "verified");
        assert_eq!(state["checks"][1]["status"], "pending");
        assert_eq!(state["ready"], false);
        let error = state_for_identity(
            store,
            crate::dispatch::agents::test_support::browser("unknown"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "forbidden");
        assert!(state_action(&serde_json::json!({"principal_id":"bootstrap-owner"})).is_err());
    }

    #[tokio::test]
    async fn explicit_no_clients_choice_is_deferred_and_never_connection_proof() {
        let (_root, store, identity) = crate::dispatch::agents::test_support::fixture().await;
        let state = defer_clients_for_identity(store.clone(), identity.clone())
            .await
            .unwrap();
        assert_eq!(state["checks"][3]["status"], "deferred");
        assert_eq!(state["ready"], false);
        for check in REQUIRED
            .into_iter()
            .filter(|check| *check != Check::SelectedClients)
        {
            record_verified_for_store(&store, "bootstrap-owner", check, "actual-result").unwrap();
        }
        let state = state_for_identity(store.clone(), identity).await.unwrap();
        assert_eq!(state["ready"], false);
        assert_eq!(state["checks"][2]["status"], "needs_recheck");
        assert_eq!(state["checks"][3]["status"], "deferred");
        invalidate_verified_for_store(&store, "bootstrap-owner", Check::SelectedClients).unwrap();
        assert!(
            !snapshot_at(&store.storage_dir(), "bootstrap-owner", now().unwrap())
                .unwrap()
                .ready
        );
    }

    #[tokio::test]
    async fn agent_completion_requires_current_revision_and_completed_durable_session() {
        use crate::dispatch::agents::{self, test_support};
        let (_root, store, identity) = test_support::fixture().await;
        let context = test_support::agent_context(&store, &identity);
        agents::dispatch(context.clone(), "agents.create", serde_json::json!({"agent_id":"starter", "owner_kind":"personal", "owner_id":"bootstrap-owner", "instructions":"test"})).await.unwrap();
        let definition = store
            .get_agent_definition("starter".into())
            .await
            .unwrap()
            .unwrap();
        store
            .create_agent_session(
                "session-1".into(),
                definition,
                "bootstrap-owner".into(),
                "test-authority".into(),
                999_999,
                1,
            )
            .await
            .unwrap();
        record_agent_run_for_store(&store, "bootstrap-owner", "starter", 1, "session-1").unwrap();
        let incomplete = state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap();
        assert_eq!(incomplete["checks"][2]["status"], "needs_recheck");
        store
            .set_agent_session_status(
                "starter".into(),
                "session-1".into(),
                "admitted".into(),
                "completed".into(),
            )
            .await
            .unwrap();
        let complete = state_for_identity(store.clone(), identity.clone())
            .await
            .unwrap();
        assert_eq!(complete["checks"][2]["status"], "verified");
        agents::dispatch(
            context,
            "agents.update",
            serde_json::json!({"agent_id":"starter", "instructions":"changed"}),
        )
        .await
        .unwrap();
        // Simulate an old proof surviving a failed post-commit invalidation.
        record_agent_run_for_store(&store, "bootstrap-owner", "starter", 1, "session-1").unwrap();
        let changed = state_for_identity(store, identity).await.unwrap();
        assert_eq!(changed["checks"][2]["status"], "needs_recheck");
    }

    #[test]
    fn unrelated_mcp_changes_preserve_provider_and_agent_proof() {
        let root = tempfile::tempdir().unwrap();
        record_at(root.path(), "user", Check::AgentRun, "session-1", 10).unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "[[upstream]]\nname='new-server'\nurl='https://server.example/mcp'\n",
        )
        .unwrap();
        std::fs::write(root.path().join(".env"), "UPSTREAM_SECRET=changed\n").unwrap();
        assert_eq!(
            snapshot_at(root.path(), "user", 11).unwrap().checks[2].status,
            CheckStatus::Verified
        );
        std::fs::write(
            root.path().join(".env"),
            "LABBY_PHOENIX_OPENAI_API_KEY=changed\n",
        )
        .unwrap();
        assert_eq!(
            snapshot_at(root.path(), "user", 11).unwrap().checks[2].status,
            CheckStatus::NeedsRecheck
        );
    }

    #[test]
    fn required_checks_resume_and_are_isolated_between_principals() {
        let root = tempfile::tempdir().unwrap();
        assert!(!snapshot_at(root.path(), "user", 10).unwrap().ready);
        record_at(root.path(), "user", Check::AgentRun, "session-1", 10).unwrap();
        let partial = snapshot_at(root.path(), "user", 11).unwrap();
        assert!(!partial.ready);
        assert_eq!(partial.checks[2].status, CheckStatus::Verified);
        assert_eq!(
            snapshot_at(root.path(), "other", 11).unwrap().checks[2].status,
            CheckStatus::Pending
        );
        for check in REQUIRED {
            record_at(root.path(), "user", check, "verified-result", 12).unwrap();
        }
        assert!(snapshot_at(root.path(), "user", 13).unwrap().ready);
    }

    #[test]
    fn changed_configuration_expired_or_future_evidence_requires_recheck() {
        let root = tempfile::tempdir().unwrap();
        record_at(root.path(), "user", Check::AgentRun, "session-1", 10).unwrap();
        assert_eq!(
            snapshot_at(root.path(), "user", 9).unwrap().checks[2].status,
            CheckStatus::NeedsRecheck
        );
        assert_eq!(
            snapshot_at(root.path(), "user", 11 + MAX_AGE_SECONDS)
                .unwrap()
                .checks[2]
                .status,
            CheckStatus::NeedsRecheck
        );
        std::fs::write(
            root.path().join(".env"),
            "LABBY_PHOENIX_OPENAI_BASE_URL=https://provider.example/v1\n",
        )
        .unwrap();
        assert_eq!(
            snapshot_at(root.path(), "user", 11).unwrap().checks[2].status,
            CheckStatus::NeedsRecheck
        );
    }

    #[test]
    fn failed_reverification_cannot_leave_previous_success_visible() {
        let root = tempfile::tempdir().unwrap();
        record_at(root.path(), "user", Check::AgentRun, "session-1", 10).unwrap();
        invalidate_at(root.path(), "user", Check::AgentRun).unwrap();
        assert_eq!(
            snapshot_at(root.path(), "user", 11).unwrap().checks[2].status,
            CheckStatus::Pending
        );
    }

    #[test]
    fn empty_result_and_blank_principal_cannot_create_evidence() {
        let root = tempfile::tempdir().unwrap();
        assert!(record_at(root.path(), "user", Check::AgentRun, " ", 10).is_err());
        assert!(record_at(root.path(), " ", Check::AgentRun, "session-1", 10).is_err());
        assert!(!root.path().join("first-use").exists());
    }

    #[cfg(unix)]
    #[test]
    fn journals_are_private_and_symlinked_evidence_is_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        record_at(root.path(), "user", Check::AgentRun, "session-1", 10).unwrap();
        let directory = root.path().join("first-use");
        let path = directory.join(format!("{}.json", subject_key("user").unwrap()));
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let target = root.path().join("other.json");
        std::fs::rename(&path, &target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(snapshot_at(root.path(), "user", 11).is_err());
        assert!(record_at(root.path(), "user", Check::AgentRun, "session-2", 11).is_err());
    }

    #[test]
    fn corrupted_evidence_fails_closed_and_ids_cannot_escape_root() {
        let root = tempfile::tempdir().unwrap();
        record_at(root.path(), "../../user", Check::AgentRun, "session-1", 10).unwrap();
        let path = root
            .path()
            .join("first-use")
            .join(format!("{}.json", subject_key("../../user").unwrap()));
        std::fs::write(path, "{}").unwrap();
        assert!(snapshot_at(root.path(), "../../user", 11).is_err());
    }
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn completed_old_call_cannot_verify_changed_configuration() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config.toml");
        std::fs::write(
            &config,
            "[[upstream]]\nname='server'\nurl='https://old.example/mcp'\n",
        )
        .unwrap();
        let before = configuration_digest(root.path(), Check::McpToolCall).unwrap();
        std::fs::write(
            &config,
            "[[upstream]]\nname='server'\nurl='https://new.example/mcp'\n",
        )
        .unwrap();
        assert!(
            record_evidence_at(
                root.path(),
                "user",
                Check::McpToolCall,
                "server::time",
                12,
                None,
                Some(&before)
            )
            .is_err()
        );
        assert!(
            !read(root.path(), "user")
                .unwrap()
                .checks
                .contains_key(&Check::McpToolCall)
        );
        let current = configuration_digest(root.path(), Check::McpToolCall).unwrap();
        record_evidence_at(
            root.path(),
            "user",
            Check::McpToolCall,
            "server::time",
            12,
            None,
            Some(&current),
        )
        .unwrap();
        assert_eq!(
            read(root.path(), "user").unwrap().checks[&Check::McpToolCall].configuration_digest,
            current
        );
    }
}
