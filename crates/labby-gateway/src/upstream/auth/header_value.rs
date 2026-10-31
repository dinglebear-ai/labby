//! Raw custom-header credential resolution with installation/environment authority.

use std::collections::BTreeSet;
use std::path::Path;

use super::{EXTERNAL_ENVIRONMENT_KEYS, dotenv_path};

/// Resolve a custom-header value without stripping a bearer prefix or padding.
/// Validation and sensitive-header marking belong to the connection boundary.
pub(crate) fn configured_header_value(env_name: &str) -> Option<String> {
    configured_header_value_from_sources(
        env_name,
        dotenv_path().as_deref(),
        std::env::var(env_name),
        EXTERNAL_ENVIRONMENT_KEYS.get(),
    )
}

fn configured_header_value_from_sources(
    env_name: &str,
    path: Option<&Path>,
    environment: Result<String, std::env::VarError>,
    external_keys: Option<&BTreeSet<String>>,
) -> Option<String> {
    // A value loaded into the process from dotenv at startup is not an external
    // override. Reread the selected installation on every reconnect, including
    // deletion, rather than resurrecting an obsolete process snapshot.
    if external_keys
        .is_some_and(|keys| !labby_runtime::helpers::environment_keys_contain(keys, env_name))
    {
        return path.and_then(|path| header_value_from_dotenv(env_name, path));
    }
    match environment {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => {
            path.and_then(|path| header_value_from_dotenv(env_name, path))
        }
        // A present invalid override must never fall back to a file secret.
        Err(std::env::VarError::NotUnicode(_)) => None,
    }
}

fn header_value_from_dotenv(env_name: &str, path: &Path) -> Option<String> {
    dotenvy::from_path_iter(path)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|(key, value)| {
            labby_runtime::helpers::environment_names_equal_with_case(&key, env_name, cfg!(windows))
                .then_some(value)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_credentials_reread_selected_dotenv_after_rotation_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let external_keys = BTreeSet::new();
        let resolve = || {
            configured_header_value_from_sources(
                "CF_SECRET",
                Some(&path),
                Ok("stale-startup-snapshot".into()),
                Some(&external_keys),
            )
        };
        std::fs::write(&path, "CF_SECRET=first-fixture\n").unwrap();
        assert_eq!(resolve().as_deref(), Some("first-fixture"));
        std::fs::write(&path, "CF_SECRET=rotated-fixture\n").unwrap();
        assert_eq!(resolve().as_deref(), Some("rotated-fixture"));
        std::fs::write(&path, "OTHER=unrelated-fixture\n").unwrap();
        assert_eq!(resolve(), None);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(resolve(), None);
    }

    #[test]
    fn external_header_override_keeps_authority_and_raw_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "CF_SECRET=file-fixture\n").unwrap();
        let external_keys = BTreeSet::from(["CF_SECRET".into()]);
        let raw = " Bearer external-fixture ";
        assert_eq!(
            configured_header_value_from_sources(
                "CF_SECRET",
                Some(&path),
                Ok(raw.into()),
                Some(&external_keys),
            )
            .as_deref(),
            Some(raw)
        );
        for value in ["", "   "] {
            assert_eq!(
                configured_header_value_from_sources(
                    "CF_SECRET",
                    Some(&path),
                    Ok(value.into()),
                    Some(&external_keys),
                )
                .as_deref(),
                Some(value)
            );
        }
        assert_eq!(
            configured_header_value_from_sources(
                "CF_SECRET",
                Some(&path),
                Err(std::env::VarError::NotUnicode("fixture".into())),
                Some(&external_keys),
            ),
            None
        );
    }

    #[test]
    fn header_dotenv_fallback_does_not_normalize_bearer_prefix_or_padding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, "CF_SECRET=\" Bearer file-fixture \"\n").unwrap();
        assert_eq!(
            configured_header_value_from_sources(
                "CF_SECRET",
                Some(&path),
                Err(std::env::VarError::NotPresent),
                None,
            )
            .as_deref(),
            Some(" Bearer file-fixture ")
        );
        assert_eq!(
            configured_header_value_from_sources(
                "CF_SECRET",
                None,
                Err(std::env::VarError::NotPresent),
                None,
            ),
            None
        );
    }
}
