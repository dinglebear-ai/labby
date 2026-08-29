//! OAuth metadata persistence plus OS credential-vault storage. `oauth.json`
//! contains only a random lookup handle and non-secret protocol metadata;
//! access and refresh tokens live in the current user's platform vault.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::oauth::secret::Secret;

const CREDENTIALS_FILE: &str = "oauth.json";
const VAULT_SERVICE: &str = "tv.tootie.lab.palette.oauth";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredCredentials {
    pub client_id: String,
    pub access_token: Secret,
    #[serde(default)]
    pub refresh_token: Option<Secret>,
    /// The token endpoint discovered at login. Refresh posts here rather than
    /// reconstructing `{server_url}/token`, which breaks behind reverse proxies.
    pub token_endpoint: String,
    /// RFC 7009 endpoint discovered at login. Optional for credentials written
    /// by older Palette releases; logout rediscovers it before revocation.
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    pub expires_at_unix: i64,
    pub scope: String,
    pub server_url: String,
}

#[derive(Serialize, Deserialize)]
struct StoredMetadata {
    vault_handle: String,
    #[serde(default)]
    retiring_handles: Vec<String>,
    client_id: String,
    token_endpoint: String,
    #[serde(default)]
    revocation_endpoint: Option<String>,
    expires_at_unix: i64,
    scope: String,
    server_url: String,
}

#[derive(Serialize, Deserialize)]
struct VaultSecrets {
    access_token: Secret,
    #[serde(default)]
    refresh_token: Option<Secret>,
}

trait CredentialVault {
    fn scope(&self) -> &str;
    fn put(&self, handle: &str, value: &str) -> Result<(), String>;
    fn get(&self, handle: &str) -> Result<String, String>;
    fn delete(&self, handle: &str) -> Result<(), String>;
}

struct PlatformVault;

impl PlatformVault {
    fn entry(handle: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(VAULT_SERVICE, handle)
            .map_err(|_| "failed to access the Palette credential vault".to_string())
    }
}

impl CredentialVault for PlatformVault {
    fn scope(&self) -> &str {
        VAULT_SERVICE
    }
    fn put(&self, handle: &str, value: &str) -> Result<(), String> {
        Self::entry(handle)?
            .set_password(value)
            .map_err(|_| "failed to store OAuth credentials in the platform vault".to_string())
    }

    fn get(&self, handle: &str) -> Result<String, String> {
        Self::entry(handle)?
            .get_password()
            .map_err(|_| "failed to read OAuth credentials from the platform vault".to_string())
    }

    fn delete(&self, handle: &str) -> Result<(), String> {
        match Self::entry(handle)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("failed to remove OAuth credentials from the platform vault".to_string()),
        }
    }
}

impl StoredCredentials {
    /// True when the access token is at or past expiry once `skew_secs` of
    /// safety margin is applied.
    pub(crate) fn is_expired(&self, now_unix: i64, skew_secs: i64) -> bool {
        now_unix + skew_secs >= self.expires_at_unix
    }

    /// True when these credentials were issued for `server_url` (trailing
    /// slashes ignored on both sides).
    pub(crate) fn matches_server(&self, server_url: &str) -> bool {
        self.server_url.trim_end_matches('/') == server_url.trim_end_matches('/')
    }
}

/// Resolve the credentials file path (`<app_config_dir>/oauth.json`).
pub(crate) fn credentials_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(CREDENTIALS_FILE))
        .map_err(|err| format!("failed to resolve app config directory: {err}"))
}

/// Load credentials, returning `None` when the file is missing or unparseable
/// (a corrupt file degrades to "signed out", never a hard error). A non-missing
/// read error is logged so it is not silently indistinguishable from absence.
pub(crate) fn load(path: &Path) -> Option<StoredCredentials> {
    match load_from(path, &PlatformVault) {
        Ok(credentials) => credentials,
        Err(error) => {
            crate::warn(error);
            None
        }
    }
}

fn load_from(
    path: &Path,
    vault: &dyn CredentialVault,
) -> Result<Option<StoredCredentials>, String> {
    if let Some(parent) = path.parent()
        && let Err(err) = crate::persistence::harden_secret_directory(parent)
    {
        crate::warn(format!(
            "refusing to load OAuth credentials before ACL hardening: {err}"
        ));
        return Ok(None);
    }
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("failed to read OAuth credential metadata: {err}")),
    };

    if let Ok(mut metadata) = serde_json::from_str::<StoredMetadata>(&contents) {
        retry_retiring_handles(path, &mut metadata, vault)?;
        return credentials_from_metadata(metadata, vault).map(Some);
    }

    let legacy: StoredCredentials = serde_json::from_str(&contents)
        .map_err(|_| "ignoring unparseable OAuth credential metadata".to_string())?;
    migrate_legacy(path, legacy, vault).map(Some)
}

/// Persist credentials atomically with `0o600` perms.
pub(crate) fn save(path: &Path, creds: &StoredCredentials) -> Result<(), String> {
    save_to(path, creds, &PlatformVault)
}

fn save_to(
    path: &Path,
    creds: &StoredCredentials,
    vault: &dyn CredentialVault,
) -> Result<(), String> {
    if vault.scope() != VAULT_SERVICE {
        return Err("OAuth credential vault has an invalid application scope".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let previous = read_metadata(path);
    let handle = uuid::Uuid::new_v4().to_string();
    let secrets = serde_json::to_string(&VaultSecrets {
        access_token: creds.access_token.clone(),
        refresh_token: creds.refresh_token.clone(),
    })
    .map_err(|_| "failed to encode OAuth vault record".to_string())?;
    vault.put(&handle, &secrets)?;
    let verified = match vault.get(&handle) {
        Ok(verified) => verified,
        Err(error) => {
            let _ = vault.delete(&handle);
            return Err(error);
        }
    };
    if verified != secrets {
        let _ = vault.delete(&handle);
        return Err("platform vault verification failed".to_string());
    }
    let mut retiring_handles = previous
        .as_ref()
        .map(|metadata| metadata.retiring_handles.clone())
        .unwrap_or_default();
    if let Some(previous_handle) = previous.map(|metadata| metadata.vault_handle) {
        retiring_handles.push(previous_handle);
    }
    let mut metadata = metadata_for(creds, handle.clone(), retiring_handles);
    if let Err(error) = write_metadata(path, &metadata) {
        let _ = vault.delete(&handle);
        return Err(error);
    }
    retry_retiring_handles(path, &mut metadata, vault)?;
    Ok(())
}

/// Remove the credentials file. Missing file is success (idempotent).
pub(crate) fn clear(path: &Path) -> Result<(), String> {
    clear_from(path, &PlatformVault)
}

fn clear_from(path: &Path, vault: &dyn CredentialVault) -> Result<(), String> {
    let metadata = read_metadata(path);
    if let Some(metadata) = &metadata {
        for handle in metadata
            .retiring_handles
            .iter()
            .chain(std::iter::once(&metadata.vault_handle))
        {
            vault.delete(handle)?;
        }
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn metadata_for(
    creds: &StoredCredentials,
    vault_handle: String,
    retiring_handles: Vec<String>,
) -> StoredMetadata {
    StoredMetadata {
        vault_handle,
        retiring_handles,
        client_id: creds.client_id.clone(),
        token_endpoint: creds.token_endpoint.clone(),
        revocation_endpoint: creds.revocation_endpoint.clone(),
        expires_at_unix: creds.expires_at_unix,
        scope: creds.scope.clone(),
        server_url: creds.server_url.clone(),
    }
}

fn retry_retiring_handles(
    path: &Path,
    metadata: &mut StoredMetadata,
    vault: &dyn CredentialVault,
) -> Result<(), String> {
    if metadata.retiring_handles.is_empty() {
        return Ok(());
    }
    let mut pending = Vec::new();
    for handle in &metadata.retiring_handles {
        if vault.delete(handle).is_err() {
            pending.push(handle.clone());
        }
    }
    if pending != metadata.retiring_handles {
        metadata.retiring_handles = pending;
        write_metadata(path, metadata)?;
    }
    if !metadata.retiring_handles.is_empty() {
        crate::warn("OAuth vault cleanup remains pending and will retry on next load".to_string());
    }
    Ok(())
}

fn credentials_from_metadata(
    metadata: StoredMetadata,
    vault: &dyn CredentialVault,
) -> Result<StoredCredentials, String> {
    let encoded = vault.get(&metadata.vault_handle)?;
    let secrets: VaultSecrets = serde_json::from_str(&encoded)
        .map_err(|_| "platform vault contains an invalid OAuth record".to_string())?;
    Ok(StoredCredentials {
        client_id: metadata.client_id,
        access_token: secrets.access_token,
        refresh_token: secrets.refresh_token,
        token_endpoint: metadata.token_endpoint,
        revocation_endpoint: metadata.revocation_endpoint,
        expires_at_unix: metadata.expires_at_unix,
        scope: metadata.scope,
        server_url: metadata.server_url,
    })
}

fn read_metadata(path: &Path) -> Option<StoredMetadata> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn write_metadata(path: &Path, metadata: &StoredMetadata) -> Result<(), String> {
    let json = serde_json::to_string_pretty(metadata)
        .map_err(|_| "failed to encode OAuth credential metadata".to_string())?;
    crate::persistence::atomic_write(path, json.as_bytes()).map_err(|err| err.to_string())
}

fn migrate_legacy(
    path: &Path,
    legacy: StoredCredentials,
    vault: &dyn CredentialVault,
) -> Result<StoredCredentials, String> {
    save_to(path, &legacy, vault)?;
    Ok(legacy)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
