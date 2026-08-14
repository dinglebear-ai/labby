//! First-run self-bootstrap: create a minimal `~/.labby/.env` so the server can
//! start and the operator can reach `/setup`. Non-destructive — a no-op when
//! the file already exists, so it is safe to call unconditionally at startup.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

use super::client::env_path;
use super::dispatch::map_merge_err;
use super::token::generate_mcp_token;

const TOKEN_ENCRYPTION_KEY_ENV: &str = "LABBY_TOKEN_ENCRYPTION_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthEncryptionKeyOutcome {
    pub changed: bool,
    pub backup_path: Option<PathBuf>,
}

/// Ensure an existing OAuth environment has its mandatory at-rest key before
/// a service install/restart. The key itself is never returned or logged.
pub fn ensure_oauth_encryption_key_at(env: &Path) -> Result<OAuthEncryptionKeyOutcome, ToolError> {
    if !env.exists() {
        return Ok(OAuthEncryptionKeyOutcome {
            changed: false,
            backup_path: None,
        });
    }
    let entries = dotenvy::from_path_iter(env).map_err(|error| ToolError::Sdk {
        sdk_kind: "oauth_encryption_preflight_failed".into(),
        message: format!(
            "could not inspect OAuth environment {}: {error}",
            env.display()
        ),
    })?;
    let mut oauth = false;
    let mut configured_key = None;
    for entry in entries {
        let (key, value) = entry.map_err(|error| ToolError::Sdk {
            sdk_kind: "oauth_encryption_preflight_failed".into(),
            message: format!(
                "could not inspect OAuth environment {}: {error}",
                env.display()
            ),
        })?;
        if key == "LABBY_AUTH_MODE" {
            oauth = value.trim().eq_ignore_ascii_case("oauth");
        } else if key == TOKEN_ENCRYPTION_KEY_ENV {
            configured_key = Some(value);
        }
    }
    if !oauth {
        return Ok(OAuthEncryptionKeyOutcome {
            changed: false,
            backup_path: None,
        });
    }
    let replace_confirmed_empty = configured_key
        .as_deref()
        .is_some_and(|key| key.trim().is_empty());
    if let Some(key) = configured_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        labby_auth::at_rest::TokenEncryptionKey::from_encoded(key).map_err(|_| ToolError::Sdk {
            sdk_kind: "oauth_encryption_key_invalid".into(),
            message: format!(
                "{TOKEN_ENCRYPTION_KEY_ENV} is invalid; refusing to restart OAuth service"
            ),
        })?;
        return Ok(OAuthEncryptionKeyOutcome {
            changed: false,
            backup_path: None,
        });
    }

    let outcome = env_merge::merge(
        env,
        MergeRequest {
            entries: vec![EnvEntry::new(
                TOKEN_ENCRYPTION_KEY_ENV,
                generate_mcp_token(),
            )],
            // Overwrite only when this preflight just parsed the existing key
            // and confirmed that its value is empty. Missing keys remain an
            // append, while non-empty keys returned above without mutation.
            force: replace_confirmed_empty,
            expected_mtime: env_merge::snapshot_mtime(env),
        },
    )
    .map_err(map_merge_err)?;
    if outcome.written != 1 {
        return Err(ToolError::Sdk {
            sdk_kind: "oauth_encryption_key_provision_failed".into(),
            message: format!(
                "failed to provision {TOKEN_ENCRYPTION_KEY_ENV}; refusing to restart OAuth service"
            ),
        });
    }
    Ok(OAuthEncryptionKeyOutcome {
        changed: true,
        backup_path: outcome.backup_path,
    })
}

/// Result of a first-run bootstrap attempt.
///
/// `Created` carries the freshly generated token so callers (serve) can make
/// it authoritative in-process without depending on a successful env reload.
/// `AlreadyPresent` means the operator already has a `~/.labby/.env` — it is left
/// byte-for-byte untouched (the don't-clobber-operator-creds safety property).
pub enum BootstrapOutcome {
    Created { env_path: PathBuf, token: String },
    AlreadyPresent { env_path: PathBuf },
}

/// Decide whether `labby serve` should self-bootstrap: only when there is no
/// MCP bearer token configured AND OAuth is not the active mode. `oauth_mode`
/// is `true` when `LABBY_AUTH_MODE=oauth`.
#[must_use]
pub fn should_bootstrap(token_configured: bool, oauth_mode: bool) -> bool {
    !token_configured && !oauth_mode
}

/// Create `~/.labby/.env` with a generated bearer token + loopback MCP defaults
/// when it does not exist. Non-destructive — returns
/// [`BootstrapOutcome::AlreadyPresent`] when the file is already there.
pub fn bootstrap() -> Result<BootstrapOutcome, ToolError> {
    bootstrap_at(&env_path())
}

/// MCP/CLI dispatch adapter: run [`bootstrap`] and serialize to the stable JSON
/// envelope `{ "created": bool, "env_path": string, "token": string|null }`.
pub fn bootstrap_action() -> Result<Value, ToolError> {
    Ok(match bootstrap()? {
        BootstrapOutcome::Created { env_path, token } => json!({
            "created": true,
            "env_path": env_path.display().to_string(),
            "token": token,
        }),
        BootstrapOutcome::AlreadyPresent { env_path } => json!({
            "created": false,
            "env_path": env_path.display().to_string(),
            "token": Value::Null,
        }),
    })
}

/// Path-parameterized core of [`bootstrap`]. Kept separate so unit tests can
/// drive it against a temp path without mutating `LABBY_HOME` — the crate forbids
/// `unsafe_code`, so env mutation inside tests is unavailable (see `state.rs`).
fn bootstrap_at(env: &Path) -> Result<BootstrapOutcome, ToolError> {
    if env.exists() {
        return Ok(BootstrapOutcome::AlreadyPresent {
            env_path: env.to_path_buf(),
        });
    }

    let token = generate_mcp_token();
    let entries = vec![
        EnvEntry::new("LABBY_MCP_HTTP_TOKEN", token.clone()),
        EnvEntry::new("LABBY_MCP_TRANSPORT", "http"),
        EnvEntry::new("LABBY_MCP_HTTP_HOST", "127.0.0.1"),
        EnvEntry::new("LABBY_MCP_HTTP_PORT", "8765"),
        EnvEntry::new("LABBY_AUTH_MODE", "bearer"),
    ];

    // `env_merge::merge` creates the parent dir (`create_dir_all`) and applies
    // 0600 perms on Unix, so no manual create_dir_all is needed here. Reuse the
    // canonical merge-error mapper so failures carry the stable `kind` from
    // docs/dev/ERRORS.md (merge_write_conflict, merge_temp_create, …).
    env_merge::merge(
        env,
        MergeRequest {
            entries,
            force: false,
            expected_mtime: None,
        },
    )
    .map_err(map_merge_err)?;

    Ok(BootstrapOutcome::Created {
        env_path: env.to_path_buf(),
        token,
    })
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::{BootstrapOutcome, bootstrap_at, ensure_oauth_encryption_key_at, should_bootstrap};

    #[test]
    fn should_bootstrap_only_without_token_and_oauth() {
        assert!(should_bootstrap(false, false));
        assert!(!should_bootstrap(true, false));
        assert!(!should_bootstrap(false, true));
        assert!(!should_bootstrap(true, true));
    }

    #[test]
    fn bootstrap_creates_env_with_token_then_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_file = dir.path().join(".env");

        let first = bootstrap_at(&env_file).expect("first bootstrap");
        let token = match first {
            BootstrapOutcome::Created { token, .. } => token,
            BootstrapOutcome::AlreadyPresent { .. } => panic!("expected Created on first run"),
        };
        assert_eq!(token.len(), 64);

        let body = std::fs::read_to_string(&env_file).expect("read .env");
        assert!(body.contains("LABBY_MCP_HTTP_TOKEN="));
        assert!(body.contains("LABBY_AUTH_MODE=bearer"));

        // Second call must be a no-op (file already exists).
        let second = bootstrap_at(&env_file).expect("second bootstrap");
        assert!(
            matches!(second, BootstrapOutcome::AlreadyPresent { .. }),
            "expected AlreadyPresent on second run"
        );
    }

    #[test]
    fn bootstrap_never_clobbers_an_existing_operator_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env_file = dir.path().join(".env");
        std::fs::write(
            &env_file,
            "LABBY_MCP_HTTP_TOKEN=preexisting-operator-token\n",
        )
        .expect("seed operator .env");

        let outcome = bootstrap_at(&env_file).expect("bootstrap over existing file");
        assert!(
            matches!(outcome, BootstrapOutcome::AlreadyPresent { .. }),
            "must not create over an existing operator .env"
        );

        let body = std::fs::read_to_string(&env_file).expect("read .env");
        assert!(
            body.contains("preexisting-operator-token"),
            "operator credentials must be preserved byte-for-byte"
        );
    }

    #[test]
    fn oauth_upgrade_provisions_key_with_backup_and_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = dir.path().join(".env");
        std::fs::write(
            &env,
            "# operator comment\nLABBY_AUTH_MODE=oauth\nLABBY_GOOGLE_CLIENT_ID=id\n",
        )
        .expect("seed OAuth env");

        let first = ensure_oauth_encryption_key_at(&env).expect("provision key");
        assert!(first.changed);
        let backup = first.backup_path.expect("existing env must be backed up");
        let backup_body = std::fs::read_to_string(backup).expect("read backup");
        assert!(!backup_body.contains("TOKEN_ENCRYPTION_KEY"));
        let body = std::fs::read_to_string(&env).expect("read updated env");
        assert!(body.contains("# operator comment"));
        let key = dotenvy::from_path_iter(&env)
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|(key, value)| (key == "LABBY_TOKEN_ENCRYPTION_KEY").then_some(value))
            .expect("generated key");
        assert!(labby_auth::at_rest::TokenEncryptionKey::from_encoded(&key).is_ok());

        let before = body;
        let second = ensure_oauth_encryption_key_at(&env).expect("idempotent preflight");
        assert!(!second.changed);
        assert!(second.backup_path.is_none());
        assert_eq!(std::fs::read_to_string(env).unwrap(), before);
    }

    #[test]
    fn bearer_upgrade_does_not_generate_oauth_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = dir.path().join(".env");
        std::fs::write(&env, "LABBY_AUTH_MODE=bearer\n").unwrap();
        let outcome = ensure_oauth_encryption_key_at(&env).unwrap();
        assert!(!outcome.changed);
        assert!(
            !std::fs::read_to_string(env)
                .unwrap()
                .contains("TOKEN_ENCRYPTION_KEY")
        );
    }

    #[test]
    fn oauth_upgrade_replaces_confirmed_empty_key_with_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = dir.path().join(".env");
        let original = "# operator comment\nLABBY_AUTH_MODE=oauth\nLABBY_TOKEN_ENCRYPTION_KEY=\n";
        std::fs::write(&env, original).expect("seed OAuth env with empty key");

        let outcome = ensure_oauth_encryption_key_at(&env).expect("replace empty key");
        assert!(outcome.changed);
        let backup = outcome.backup_path.expect("existing env must be backed up");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), original);

        let body = std::fs::read_to_string(&env).expect("read updated env");
        assert!(body.contains("# operator comment"));
        let key = dotenvy::from_path_iter(&env)
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|(key, value)| (key == "LABBY_TOKEN_ENCRYPTION_KEY").then_some(value))
            .expect("generated replacement key");
        assert!(labby_auth::at_rest::TokenEncryptionKey::from_encoded(&key).is_ok());
    }

    #[test]
    fn invalid_existing_oauth_key_fails_preflight_without_rewrite() {
        let dir = tempfile::tempdir().expect("tempdir");
        let env = dir.path().join(".env");
        let original = "LABBY_AUTH_MODE=oauth\nLABBY_TOKEN_ENCRYPTION_KEY=invalid\n";
        std::fs::write(&env, original).unwrap();
        let error = ensure_oauth_encryption_key_at(&env).unwrap_err();
        assert_eq!(error.kind(), "oauth_encryption_key_invalid");
        assert_eq!(std::fs::read_to_string(env).unwrap(), original);
    }
}
