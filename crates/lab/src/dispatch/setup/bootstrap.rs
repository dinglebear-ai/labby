//! First-run self-bootstrap: create a minimal `~/.lab/.env` so the server can
//! start and the operator can reach `/setup`. Non-destructive — a no-op when
//! the file already exists, so it is safe to call unconditionally at startup.

use std::path::Path;

use serde_json::{Value, json};

use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::dispatch::error::ToolError;

use super::client::env_path;
use super::dispatch::map_merge_err;
use super::token::generate_mcp_token;

/// Decide whether `labby serve` should self-bootstrap: only when there is no
/// MCP bearer token configured AND OAuth is not the active mode. `oauth_mode`
/// is `true` when `LAB_AUTH_MODE=oauth`.
#[must_use]
pub fn should_bootstrap(token_configured: bool, oauth_mode: bool) -> bool {
    !token_configured && !oauth_mode
}

/// Create `~/.lab/.env` with a generated bearer token + loopback MCP defaults
/// when it does not exist. Returns `{ created, env_path, token }` — `token` is
/// the generated value on creation, or `null` when the file already existed.
pub fn bootstrap() -> Result<Value, ToolError> {
    bootstrap_at(&env_path())
}

/// Path-parameterized core of [`bootstrap`]. Kept separate so unit tests can
/// drive it against a temp path without mutating `LAB_HOME` — the crate forbids
/// `unsafe_code`, so env mutation inside tests is unavailable (see `state.rs`).
fn bootstrap_at(env: &Path) -> Result<Value, ToolError> {
    if env.exists() {
        return Ok(json!({
            "created": false,
            "env_path": env.display().to_string(),
            "token": Value::Null,
        }));
    }

    if let Some(parent) = env.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ToolError::Sdk {
            sdk_kind: "write_failed".into(),
            message: format!("create {}: {e}", parent.display()),
        })?;
    }

    let token = generate_mcp_token();
    let entries = vec![
        EnvEntry::new("LAB_MCP_HTTP_TOKEN", token.clone()),
        EnvEntry::new("LAB_MCP_TRANSPORT", "http"),
        EnvEntry::new("LAB_MCP_HTTP_HOST", "127.0.0.1"),
        EnvEntry::new("LAB_MCP_HTTP_PORT", "8765"),
        EnvEntry::new("LAB_AUTH_MODE", "bearer"),
    ];

    // Reuse the canonical merge-error mapper so failures carry the stable
    // `kind` from docs/dev/ERRORS.md (merge_write_conflict, merge_temp_create,
    // …) instead of a flattened "write_failed" (eng-review HIGH-1).
    env_merge::merge(
        env,
        MergeRequest {
            entries,
            force: false,
            expected_mtime: None,
        },
    )
    .map_err(map_merge_err)?;

    Ok(json!({
        "created": true,
        "env_path": env.display().to_string(),
        "token": token,
    }))
}

#[cfg(test)]
mod tests {
    use super::{bootstrap_at, should_bootstrap};

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
        assert_eq!(first["created"], serde_json::json!(true));
        let token = first["token"].as_str().expect("token string");
        assert_eq!(token.len(), 64);

        let body = std::fs::read_to_string(&env_file).expect("read .env");
        assert!(body.contains("LAB_MCP_HTTP_TOKEN="));
        assert!(body.contains("LAB_AUTH_MODE=bearer"));

        // Second call must be a no-op (file already exists).
        let second = bootstrap_at(&env_file).expect("second bootstrap");
        assert_eq!(second["created"], serde_json::json!(false));
        assert_eq!(second["token"], serde_json::Value::Null);
    }
}
