//! OAuth credential persistence. Non-secret metadata lives beside
//! `settings.json`; access and refresh tokens live in the platform credential
//! vault.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::oauth::secret::Secret;

const CREDENTIALS_FILE: &str = "oauth.json";

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
struct CredentialMetadata {
    client_id: String,
    token_endpoint: String,
    #[serde(default)]
    revocation_endpoint: Option<String>,
    expires_at_unix: i64,
    scope: String,
    server_url: String,
    vault_account: String,
}

#[derive(Serialize, Deserialize)]
struct VaultSecrets {
    access_token: String,
    refresh_token: Option<String>,
}

#[cfg(not(test))]
const VAULT_SERVICE: &str = "dev.dinglebear.labby.palette.oauth";

fn vault_account(path: &Path) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(path.to_string_lossy().as_bytes()))
}

#[cfg(not(test))]
fn vault_set(account: &str, secret: &str) -> Result<(), String> {
    keyring::Entry::new(VAULT_SERVICE, account)
        .and_then(|entry| entry.set_password(secret))
        .map_err(|error| format!("platform credential vault write failed: {error}"))
}

enum VaultState {
    Missing,
    Present(String),
}

#[cfg(not(test))]
fn vault_get_state(account: &str) -> Result<VaultState, String> {
    let entry = keyring::Entry::new(VAULT_SERVICE, account)
        .map_err(|error| format!("platform credential vault open failed: {error}"))?;
    match entry.get_password() {
        Ok(secret) => Ok(VaultState::Present(secret)),
        Err(keyring::Error::NoEntry) => Ok(VaultState::Missing),
        Err(error) => Err(format!("platform credential vault read failed: {error}")),
    }
}

#[cfg(not(test))]
fn vault_delete(account: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(VAULT_SERVICE, account)
        .map_err(|error| format!("platform credential vault open failed: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("platform credential vault delete failed: {error}")),
    }
}

#[cfg(test)]
fn test_vault() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static VAULT: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    VAULT.get_or_init(Default::default)
}

#[cfg(test)]
thread_local! {
    static VAULT_FAILURES: std::cell::Cell<(bool, bool, bool)> = const { std::cell::Cell::new((false, false, false)) };
    static ROLLBACK_FAILURES: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, false)) };
    static FILE_FAILURES: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, false)) };
}

#[cfg(test)]
fn set_test_vault_failures(write: bool, read: bool, delete: bool) {
    VAULT_FAILURES.set((write, read, delete));
}

#[cfg(test)]
fn set_test_rollback_failures(set: bool, delete: bool) {
    ROLLBACK_FAILURES.set((set, delete));
}

#[cfg(test)]
fn set_test_file_failures(write: bool, delete: bool) {
    FILE_FAILURES.set((write, delete));
}

fn write_metadata(path: &Path, contents: &[u8]) -> Result<(), String> {
    #[cfg(test)]
    if FILE_FAILURES.get().0 {
        return Err("injected OAuth metadata write failure".into());
    }
    crate::persistence::atomic_write(path, contents).map_err(|error| error.to_string())
}

fn delete_metadata(path: &Path) -> Result<(), String> {
    #[cfg(test)]
    if FILE_FAILURES.get().1 {
        return Err("injected OAuth metadata delete failure".into());
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
fn vault_set(account: &str, secret: &str) -> Result<(), String> {
    if VAULT_FAILURES.get().0 {
        return Err("injected platform credential vault write failure".into());
    }
    test_vault()
        .lock()
        .unwrap()
        .insert(account.into(), secret.into());
    Ok(())
}

#[cfg(test)]
fn vault_get_state(account: &str) -> Result<VaultState, String> {
    if VAULT_FAILURES.get().1 {
        return Err("injected platform credential vault read failure".into());
    }
    Ok(match test_vault().lock().unwrap().get(account).cloned() {
        Some(secret) => VaultState::Present(secret),
        None => VaultState::Missing,
    })
}

#[cfg(test)]
fn vault_delete(account: &str) -> Result<(), String> {
    if VAULT_FAILURES.get().2 {
        return Err("injected platform credential vault delete failure".into());
    }
    test_vault().lock().unwrap().remove(account);
    Ok(())
}

fn vault_get(account: &str) -> Result<String, String> {
    match vault_get_state(account)? {
        VaultState::Present(secret) => Ok(secret),
        VaultState::Missing => Err("platform credential vault entry missing".to_string()),
    }
}

fn rollback_vault(account: &str, previous: VaultState) -> Result<(), String> {
    match previous {
        VaultState::Present(secret) => {
            #[cfg(test)]
            if ROLLBACK_FAILURES.get().0 {
                return Err("injected platform credential vault rollback write failure".into());
            }
            vault_set(account, &secret)
        }
        VaultState::Missing => {
            #[cfg(test)]
            if ROLLBACK_FAILURES.get().1 {
                return Err("injected platform credential vault rollback delete failure".into());
            }
            vault_delete(account)
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
pub(crate) fn load(path: &Path) -> Result<Option<StoredCredentials>, String> {
    if let Some(parent) = path.parent()
        && let Err(err) = crate::persistence::harden_secret_directory(parent)
    {
        crate::warn(format!(
            "refusing to load OAuth credentials before ACL hardening: {err}"
        ));
        return Err(format!("OAuth credential directory is unavailable: {err}"));
    }
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            crate::warn(format!("failed to read oauth credentials: {err}"));
            return Err(format!("failed to read OAuth credentials: {err}"));
        }
    };
    if let Ok(metadata) = serde_json::from_str::<CredentialMetadata>(&contents) {
        let secrets = vault_get(&metadata.vault_account).and_then(|secret| {
            serde_json::from_str::<VaultSecrets>(&secret).map_err(|e| e.to_string())
        });
        return match secrets {
            Ok(secrets) => Ok(Some(StoredCredentials {
                client_id: metadata.client_id,
                access_token: secrets.access_token.into(),
                refresh_token: secrets.refresh_token.map(Secret::from),
                token_endpoint: metadata.token_endpoint,
                revocation_endpoint: metadata.revocation_endpoint,
                expires_at_unix: metadata.expires_at_unix,
                scope: metadata.scope,
                server_url: metadata.server_url,
            })),
            Err(error) => Err(format!(
                "failed to load OAuth credentials from platform vault: {error}"
            )),
        };
    }
    // Migrate the legacy plaintext shape only after the vault write succeeds.
    match serde_json::from_str::<StoredCredentials>(&contents) {
        Ok(creds) => match save(path, &creds) {
            Ok(()) => Ok(Some(creds)),
            Err(error) => {
                crate::warn(format!(
                    "failed to migrate legacy OAuth credentials: {error}"
                ));
                Err(format!(
                    "failed to migrate legacy OAuth credentials: {error}"
                ))
            }
        },
        Err(error) => {
            crate::warn(format!("ignoring unparseable oauth credentials: {error}"));
            Err(format!("unparseable OAuth credentials: {error}"))
        }
    }
}

/// Persist credentials atomically with `0o600` perms.
pub(crate) fn save(path: &Path, creds: &StoredCredentials) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let account = vault_account(path);
    let secret = serde_json::to_string(&VaultSecrets {
        access_token: creds.access_token.expose().to_string(),
        refresh_token: creds
            .refresh_token
            .as_ref()
            .map(|value| value.expose().to_string()),
    })
    .map_err(|error| error.to_string())?;
    let previous_secret = vault_get_state(&account)?;
    vault_set(&account, &secret)?;
    let metadata = CredentialMetadata {
        client_id: creds.client_id.clone(),
        token_endpoint: creds.token_endpoint.clone(),
        revocation_endpoint: creds.revocation_endpoint.clone(),
        expires_at_unix: creds.expires_at_unix,
        scope: creds.scope.clone(),
        server_url: creds.server_url.clone(),
        vault_account: account.clone(),
    };
    let json = serde_json::to_string_pretty(&metadata).map_err(|err| err.to_string())?;
    if let Err(error) = write_metadata(path, json.as_bytes()) {
        if let Err(rollback) = rollback_vault(&account, previous_secret) {
            return Err(format!(
                "{error}; rollback failed and credential state is uncertain: {rollback}"
            ));
        }
        return Err(error);
    }
    Ok(())
}

/// Remove the credentials file. Missing file is success (idempotent).
pub(crate) fn clear(path: &Path) -> Result<(), String> {
    let account = vault_account(path);
    let previous_secret = vault_get_state(&account)?;
    vault_delete(&account)?;
    match delete_metadata(path) {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Err(rollback) = rollback_vault(&account, previous_secret) {
                return Err(format!(
                    "{err}; rollback failed and credential state is uncertain: {rollback}"
                ));
            }
            Err(err)
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
