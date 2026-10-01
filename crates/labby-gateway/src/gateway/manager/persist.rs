//! Env-file persistence: canonical `.env` path resolution and gateway
//! bearer-token writes, both delegated to the host-owned `GatewayConfigStore`.
//!
//! The manager owns only the gateway-specific policy (token normalization and
//! env-name validation); the actual `.env` backup/atomic-write and any cached
//! service-client refresh live behind the store seam in the host (`lab`).

use std::path::PathBuf;

use labby_runtime::error::ToolError;

use crate::gateway::config::validate_bearer_token_env_name;

use super::GatewayManager;

impl GatewayManager {
    pub(super) async fn snapshot_gateway_bearer_token(
        &self,
        env_name: &str,
    ) -> Result<Option<String>, ToolError> {
        let path = self.store.env_path();
        let env_name = env_name.to_string();
        tokio::task::spawn_blocking(move || {
            snapshot_gateway_bearer_token_from_path(&path, &env_name, cfg!(windows))
        })
        .await
        .map_err(|_| ToolError::internal_message("gateway credential snapshot task failed"))?
    }

    pub(super) fn env_path(&self) -> PathBuf {
        self.store.env_path()
    }

    pub(super) async fn persist_gateway_bearer_token(
        &self,
        env_name: &str,
        token_value: &str,
    ) -> Result<(), ToolError> {
        validate_bearer_token_env_name(env_name)?;
        let auth_header = normalize_gateway_bearer_token(token_value);
        self.store
            .persist_gateway_bearer_token(env_name, &auth_header)
            .await
    }
}

fn snapshot_gateway_bearer_token_from_path(
    path: &std::path::Path,
    env_name: &str,
    case_insensitive: bool,
) -> Result<Option<String>, ToolError> {
    let entries = match dotenvy::from_path_iter(path) {
        Ok(entries) => entries,
        Err(error) if error.not_found() => return Ok(None),
        Err(_) => {
            return Err(ToolError::internal_message(
                "cannot snapshot gateway credential file",
            ));
        }
    };
    for entry in entries {
        let (key, candidate) = entry
            .map_err(|_| ToolError::internal_message("cannot parse gateway credential file"))?;
        if labby_runtime::helpers::environment_names_equal_with_case(
            &key,
            env_name,
            case_insensitive,
        ) {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn normalize_gateway_bearer_token(token_value: &str) -> String {
    let trimmed = token_value.trim();
    if trimmed
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("bearer "))
    {
        format!("Bearer {}", &trimmed[7..])
    } else {
        format!("Bearer {trimmed}")
    }
}

#[cfg(test)]
mod environment_name_tests {
    use super::*;

    #[test]
    fn credential_snapshot_preserves_windows_alias_for_rollback() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".env");
        std::fs::write(
            &path,
            "Labby_Token=\"Bearer original-secret\"\nUNRELATED=untouched\n",
        )
        .unwrap();
        assert_eq!(
            snapshot_gateway_bearer_token_from_path(&path, "LABBY_TOKEN", true).unwrap(),
            Some("Bearer original-secret".into()),
            "rollback must retain a file-managed Windows alias"
        );
        assert_eq!(
            snapshot_gateway_bearer_token_from_path(&path, "LABBY_TOKEN", false).unwrap(),
            None,
            "Unix snapshots retain exact environment names"
        );
        std::fs::write(
            &path,
            "Labby_Token=\nLABBY_TOKEN=\"Bearer lower-priority-secret\"\n",
        )
        .unwrap();
        assert_eq!(
            snapshot_gateway_bearer_token_from_path(&path, "LABBY_TOKEN", true).unwrap(),
            Some(String::new()),
            "an empty first alias is still the exact prior assignment"
        );
    }
}
