//! Gateway-observed use of client-specific, principal-bound registration sessions.
//! Session proofs supplement authentication and never grant access themselves.
use super::{
    client_registration::ExternalClient,
    readiness::{self, Check},
};
use crate::{access::AccessStore, dispatch::error::ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq as _;

pub const HEADER: &str = "x-labby-client-session";
const TTL: u64 = 3600;
#[derive(Clone, Debug)]
pub struct Binding {
    principal: String,
    generation: String,
    digest: String,
    client: String,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    generation: String,
    configuration: String,
    expires: u64,
    selected: Vec<String>,
    sessions: BTreeMap<String, Session>,
    revoked: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    client: String,
    hash: String,
    completed: bool,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Issued {
    pub generation: String,
    pub expires: u64,
    pub sessions: BTreeMap<String, String>,
}
#[derive(Serialize, schemars::JsonSchema)]
pub struct Revocation {
    pub revoked: bool,
}
fn failure() -> ToolError {
    ToolError::Forbidden { message: "Client session proof is unavailable, expired, revoked, or belongs to another authenticated user".into(), required_scopes: vec![] }
}
fn now() -> Result<u64, ToolError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| failure())
}
fn path(root: &Path, principal: &str) -> PathBuf {
    root.join("first-use-clients").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(principal.as_bytes()))
    ))
}
fn lock(
    root: &Path,
    principal: &str,
) -> Result<crate::config::host_write::HostConfigLock, ToolError> {
    let path = path(root, principal);
    super::secure_file::create_private_dir(path.parent().unwrap()).map_err(|_| failure())?;
    crate::config::host_write::HostConfigLock::acquire(&path).map_err(|_| failure())
}
fn read(path: &Path) -> Result<Journal, ToolError> {
    match super::secure_file::read_private(path) {
        Ok(raw) => serde_json::from_slice(&raw).map_err(|_| failure()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(_) => Err(failure()),
    }
}
fn save(lock: &crate::config::host_write::HostConfigLock, row: &Journal) -> Result<(), ToolError> {
    super::secure_file::replace_journal(
        lock.path(),
        &serde_json::to_vec(row).map_err(|_| failure())?,
    )
    .map_err(|_| failure())
}
fn selected(params: &Value) -> Result<Vec<String>, ToolError> {
    if params
        .as_object()
        .is_none_or(|row| row.keys().any(|key| key != "clients"))
    {
        return Err(failure());
    }
    let clients: Vec<ExternalClient> =
        serde_json::from_value(params.get("clients").cloned().ok_or_else(failure)?)
            .map_err(|_| failure())?;
    let mut clients: Vec<String> = clients
        .into_iter()
        .map(|c| {
            match c {
                ExternalClient::Codex => "codex",
                ExternalClient::ClaudeCode => "claude-code",
            }
            .into()
        })
        .collect();
    clients.sort();
    clients.dedup();
    if clients.is_empty() || clients.len() > 2 {
        return Err(failure());
    }
    Ok(clients)
}
pub async fn start(
    store: AccessStore,
    identity: labby_auth::VerifiedIdentity,
    params: Value,
) -> Result<Value, ToolError> {
    let clients = selected(&params)?;
    let principal = crate::access::resolve_personal_owner(&store, identity)
        .await
        .map_err(|_| failure())?
        .id()
        .to_owned();
    tokio::task::spawn_blocking(move || {
        let root = store.storage_dir();
        let lock = lock(&root, &principal)?;
        let configuration =
            readiness::configuration_digest_for_store(&store, Check::SelectedClients)?;
        let expires = now()?.saturating_add(TTL);
        let generation = uuid::Uuid::new_v4().to_string();
        let mut journal = Journal {
            generation: generation.clone(),
            configuration,
            expires,
            selected: clients.clone(),
            ..Default::default()
        };
        let mut sessions = BTreeMap::new();
        for client in clients {
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let hash = hex::encode(Sha256::digest(token.as_bytes()));
            journal.sessions.insert(
                hash.clone(),
                Session {
                    client: client.clone(),
                    hash,
                    completed: false,
                },
            );
            sessions.insert(client, token);
        }
        readiness::invalidate_verified_for_store(&store, &principal, Check::SelectedClients)?;
        save(&lock, &journal)?;
        serde_json::to_value(Issued {
            generation,
            expires,
            sessions,
        })
        .map_err(|_| failure())
    })
    .await
    .map_err(|_| failure())?
}
pub async fn revoke(
    store: AccessStore,
    identity: labby_auth::VerifiedIdentity,
) -> Result<Value, ToolError> {
    let principal = crate::access::resolve_personal_owner(&store, identity)
        .await
        .map_err(|_| failure())?
        .id()
        .to_owned();
    tokio::task::spawn_blocking(move || {
        let root = store.storage_dir();
        let lock = lock(&root, &principal)?;
        let mut row = read(lock.path())?;
        row.revoked = true;
        readiness::invalidate_verified_for_store(&store, &principal, Check::SelectedClients)?;
        save(&lock, &row)?;
        serde_json::to_value(Revocation { revoked: true }).map_err(|_| failure())
    })
    .await
    .map_err(|_| failure())?
}
#[cfg(test)]
fn validate(store: &AccessStore, principal: &str, token: &str) -> Result<Binding, ToolError> {
    validate_observation(store, principal, token)?.ok_or_else(failure)
}
fn validate_observation(
    store: &AccessStore,
    principal: &str,
    token: &str,
) -> Result<Option<Binding>, ToolError> {
    if token.len() != 64 || !token.bytes().all(|v| v.is_ascii_hexdigit()) {
        return Err(failure());
    }
    let root = store.storage_dir();
    let row = read(&path(&root, principal))?;
    if row.revoked
        || row.configuration
            != readiness::configuration_digest_for_store(store, Check::SelectedClients)?
    {
        return Err(failure());
    }
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    let session = row
        .sessions
        .values()
        .find(|session| bool::from(session.hash.as_bytes().ct_eq(digest.as_bytes())))
        .ok_or_else(failure)?;
    // The header grants no authority. A recognized, otherwise current proof
    // may end observation without revoking the independently authenticated
    // client's access. Unknown, revoked and cross-principal proofs still fail.
    if row.expires <= now()? {
        return Ok(None);
    }
    Ok(Some(Binding {
        principal: principal.into(),
        generation: row.generation,
        digest,
        client: session.client.clone(),
    }))
}
pub async fn middleware(
    axum::extract::State(state): axum::extract::State<crate::api::state::AppState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    let Some(token) = request.headers().get(HEADER) else {
        return next.run(request).await;
    };
    let Some(identity) = request
        .extensions()
        .get::<labby_auth::VerifiedIdentity>()
        .cloned()
    else {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    };
    if request.uri().path().trim_end_matches('/') != "/mcp" {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let Ok(token) = token.to_str().map(str::to_owned) else {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    };
    let result = async {
        let store = state.access_runtime.store().await.map_err(|_| failure())?;
        let principal = crate::access::resolve_personal_owner(&store, identity)
            .await
            .map_err(|_| failure())?
            .id()
            .to_owned();
        tokio::task::spawn_blocking(move || validate_observation(&store, &principal, &token))
            .await
            .map_err(|_| failure())?
    }
    .await;
    match result {
        Ok(binding) => {
            if let Some(binding) = binding {
                request.extensions_mut().insert(binding);
            }
            next.run(request).await
        }
        Err(_) => axum::http::StatusCode::FORBIDDEN.into_response(),
    }
}
pub(crate) fn binding_from_extensions(extensions: &rmcp::model::Extensions) -> Option<Binding> {
    extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Binding>())
        .cloned()
}

pub(crate) async fn record_completed_tool(
    runtime: &crate::access::AccessRuntime,
    binding: Option<Binding>,
    result: &rmcp::model::CallToolResponse,
) {
    let Some(binding) = binding else {
        return;
    };
    if !matches!(result,rmcp::model::CallToolResponse::Complete(result) if result.is_error!=Some(true))
    {
        return;
    }
    match runtime.store().await {
        Ok(store) => {
            if observed_success(store, binding).await.is_err() {
                tracing::warn!("client observation evidence could not be recorded");
            }
        }
        Err(_) => tracing::warn!("client observation access store unavailable"),
    }
}

/// Called only by the server after an actual complete, successful tool result.
pub async fn observed_success(store: AccessStore, binding: Binding) -> Result<(), ToolError> {
    tokio::task::spawn_blocking(move || {
        let root = store.storage_dir();
        let lock = lock(&root, &binding.principal)?;
        let mut row = read(lock.path())?;
        if row.revoked
            || row.expires <= now()?
            || row.generation != binding.generation
            || row.configuration
                != readiness::configuration_digest_for_store(&store, Check::SelectedClients)?
        {
            return Err(failure());
        }
        let session = row
            .sessions
            .get_mut(&binding.digest)
            .filter(|s| s.client == binding.client)
            .ok_or_else(failure)?;
        session.completed = true;
        save(&lock, &row)?;
        if row.selected.iter().all(|client| {
            row.sessions
                .values()
                .any(|s| &s.client == client && s.completed)
        }) {
            readiness::record_verified_with_expected_digest_for_store(
                &store,
                &binding.principal,
                Check::SelectedClients,
                &format!("client-session:{}", row.generation),
                &row.configuration,
            )?;
        }
        Ok(())
    })
    .await
    .map_err(|_| failure())?
}

#[cfg(test)]
#[path = "client_evidence_tests.rs"]
mod tests;
