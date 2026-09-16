//! First-run detection + state-machine evaluator for `setup.state`.

use crate::dispatch::error::ToolError;
use crate::dispatch::setup::{SetupSnapshot, SetupState};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::env_merge::snapshot_mtime;
use crate::registry::{ToolRegistry, service_meta};

use super::client::{draft_path, env_path};
use super::draft;

/// Read every required env var from the registry. A service contributes its
/// required vars unconditionally — wizards skip optional ones.
fn registry_required_keys(registry: &ToolRegistry) -> Vec<String> {
    let mut keys = Vec::new();
    for entry in registry.services() {
        if let Some(meta) = service_meta(entry.name) {
            for var in meta.required_env {
                keys.push(var.name.to_string());
            }
        }
    }
    keys
}

fn effective_entries(
    env: &Path,
    draft: &Path,
) -> Result<std::collections::BTreeMap<String, String>, ToolError> {
    let mut values = std::collections::BTreeMap::new();
    if env.exists() {
        for entry in draft::read_entries(env)? {
            values.insert(entry.key, entry.value);
        }
    }
    if draft.exists() {
        for entry in draft::read_entries(draft)? {
            values.insert(entry.key, entry.value);
        }
    }
    Ok(values)
}

fn has_nonempty(values: &std::collections::BTreeMap<String, String>, key: &str) -> bool {
    values
        .get(key)
        .is_some_and(|value| !value.trim().is_empty())
}

fn personal_oauth_configured(values: &std::collections::BTreeMap<String, String>) -> bool {
    values
        .get("LABBY_AUTH_MODE")
        .is_some_and(|value| value.eq_ignore_ascii_case("oauth"))
        && values
            .get("LABBY_AUTH_PROVIDER")
            .is_some_and(|value| value.eq_ignore_ascii_case("google"))
        && has_nonempty(values, "LABBY_PUBLIC_URL")
        && has_nonempty(values, "LABBY_GOOGLE_CLIENT_ID")
        && has_nonempty(values, "LABBY_GOOGLE_CLIENT_SECRET")
        && has_nonempty(values, "LABBY_AUTH_ADMIN_EMAIL")
}

fn claude_code_configured() -> bool {
    let Ok(path) = crate::config::config_toml_path() else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    crate::config::load_toml(&[path]).is_ok_and(|config| {
        config.upstream.iter().any(|upstream| {
            let command_is_claude = upstream.command.as_deref().is_some_and(|command| {
                Path::new(command)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("claude"))
            });
            let remote_claude = upstream.command.as_deref().is_some_and(|command| {
                Path::new(command)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("ssh"))
            }) && upstream
                .args
                .windows(2)
                .any(|args| args == ["mcp", "serve"]);
            command_is_claude || remote_claude
        })
    })
}

fn personal_resume_progress(
    env_exists: bool,
    values: &std::collections::BTreeMap<String, String>,
    oauth_configured: bool,
    claude_configured: bool,
) -> (u8, String) {
    if claude_configured && oauth_configured && env_exists {
        return (4, "verify_readiness".into());
    }
    if oauth_configured && env_exists {
        return (3, "connect_claude_code".into());
    }
    if oauth_configured {
        return (2, "commit_configuration".into());
    }
    if !values.is_empty() {
        return (1, "configure_oauth".into());
    }
    (0, "configure_runtime".into())
}

/// Build a `SetupSnapshot` describing the current state of `~/.labby/.env`.
pub fn snapshot(registry: &ToolRegistry) -> Result<SetupSnapshot, ToolError> {
    let env = env_path();
    let draft = draft_path();
    let env_exists = env.exists();
    let has_draft = draft.exists();
    let draft_stale = draft_is_stale(&env, &draft);
    let draft_metadata = draft_metadata(&env, &draft)?;
    let effective = effective_entries(&env, &draft)?;
    let personal_oauth_configured = personal_oauth_configured(&effective);
    let claude_code_configured = claude_code_configured();
    let (last_completed_step, resume_from) = personal_resume_progress(
        env_exists,
        &effective,
        personal_oauth_configured,
        claude_code_configured,
    );

    let state = if !env_exists {
        SetupState::Uninitialized
    } else {
        let entries = draft::read_entries(&env)?;
        let registered: Vec<String> = registry_required_keys(registry);
        let missing: Vec<String> = registered
            .into_iter()
            .filter(|key| !entries.iter().any(|e| &e.key == key && !e.value.is_empty()))
            .collect();
        if missing.is_empty() {
            SetupState::Ready
        } else if entries.is_empty() {
            SetupState::ConfigMissing { envars: missing }
        } else {
            SetupState::PartiallyConfigured { missing }
        }
    };

    Ok(SetupSnapshot {
        first_run: matches!(
            state,
            SetupState::Uninitialized | SetupState::ConfigMissing { .. }
        ),
        env_path: env,
        draft_path: draft,
        last_completed_step,
        resume_from,
        personal_oauth_configured,
        claude_code_configured,
        draft_stale,
        has_draft,
        draft_entry_count: draft_metadata.draft_entry_count,
        env_mtime_unix_seconds: draft_metadata.env_mtime_unix_seconds,
        draft_mtime_unix_seconds: draft_metadata.draft_mtime_unix_seconds,
        state,
    })
}

struct DraftMetadata {
    draft_entry_count: usize,
    env_mtime_unix_seconds: Option<u64>,
    draft_mtime_unix_seconds: Option<u64>,
}

fn draft_metadata(env: &Path, draft: &Path) -> Result<DraftMetadata, ToolError> {
    let draft_entry_count = if draft.exists() {
        draft::read_entries(draft)?.len()
    } else {
        0
    };
    Ok(DraftMetadata {
        draft_entry_count,
        env_mtime_unix_seconds: unix_seconds(snapshot_mtime(env)),
        draft_mtime_unix_seconds: unix_seconds(snapshot_mtime(draft)),
    })
}

fn unix_seconds(mtime: Option<SystemTime>) -> Option<u64> {
    mtime
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn draft_is_stale(env: &Path, draft: &Path) -> bool {
    if !draft.exists() {
        return false;
    }
    let env_mtime = snapshot_mtime(env);
    let draft_mtime = snapshot_mtime(draft);
    match (env_mtime, draft_mtime) {
        (Some(e), Some(d)) => e > d,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{draft_metadata, unix_seconds};
    use std::time::{Duration, SystemTime};

    // Note: snapshot() reads LABBY_HOME via env, which Rust 2024 marks unsafe.
    // The crate forbids unsafe, so we can't mutate the env var inside tests
    // here. End-to-end coverage of the state machine ships in the smoke test
    // recipe (`just smoke-setup`) added in Chunk F.

    #[test]
    fn draft_metadata_counts_entries_and_reports_unix_mtimes() {
        let temp = tempfile::tempdir().unwrap();
        let env = temp.path().join(".env");
        let draft = temp.path().join(".env.draft");
        std::fs::write(&env, "LABBY_MCP_HTTP_TOKEN=abc\n").unwrap();
        std::fs::write(&draft, "LABBY_TEST=1\n# comment\nOTHER=2\n").unwrap();

        let metadata = draft_metadata(&env, &draft).unwrap();

        assert_eq!(metadata.draft_entry_count, 2);
        assert!(metadata.env_mtime_unix_seconds.is_some());
        assert!(metadata.draft_mtime_unix_seconds.is_some());
    }

    #[test]
    fn unix_seconds_returns_none_before_epoch() {
        let before_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(1);

        assert_eq!(unix_seconds(Some(before_epoch)), None);
        assert_eq!(unix_seconds(None), None);
    }
}
