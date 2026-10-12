//! Prospective bearer resolution for an installation-file transaction.

use std::path::Path;

use super::{EXTERNAL_ENVIRONMENT_KEYS, configured_bearer_token_with_dotenv, dotenv_path};

/// Project the transport's winning credential before publishing a file write.
/// `file_value` has already undergone the host persistence normalization.
pub(crate) fn configured_bearer_token_after_file_update(
    env_name: &str,
    updated_path: &Path,
    file_value: &str,
) -> Option<String> {
    let selected_path = dotenv_path();
    // A host store may select another file. Such a write cannot replace the
    // credential the transport actually reads from its selected installation.
    if selected_path.as_deref() != Some(updated_path) {
        return configured_bearer_token_with_dotenv(env_name, selected_path.as_deref());
    }
    if EXTERNAL_ENVIRONMENT_KEYS
        .get()
        .is_some_and(|keys| !labby_runtime::helpers::environment_keys_contain(keys, env_name))
    {
        return super::normalize_bearer_token(file_value);
    }
    match std::env::var(env_name) {
        Ok(token) => super::normalize_bearer_token(&token),
        Err(std::env::VarError::NotPresent) => super::normalize_bearer_token(file_value),
        // Preserve the transport's fail-closed process-override precedence.
        Err(std::env::VarError::NotUnicode(_)) => None,
    }
}
