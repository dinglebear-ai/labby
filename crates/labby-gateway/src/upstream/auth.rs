use std::path::{Path, PathBuf};

use labby_runtime::gateway_config::UpstreamConfig;

/// Standard location for the `.env` file: `$LABBY_HOME/.env`, normally
/// `~/.labby/.env`.
///
/// Vendored from `lab`'s `crate::config::dotenv_path` so the upstream pool's
/// dotenv-fallback bearer-token resolution does not reach back into the Labby
/// binary crate. An explicit `LABBY_HOME` is exclusive, so an isolated
/// installation never reads another installation's upstream bearer tokens.
/// Returns `None` when neither root can be resolved.
///
/// This crate stays independent of the Labby product crate, which owns the
/// full installation-root contract and validates the root (absolute, not a
/// symlink, securely owned) at startup. The local check here is the part that
/// protects this read on its own: a non-absolute root would otherwise resolve
/// a `.env` relative to whatever directory the process happened to start in.
fn dotenv_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    dotenv_path_from(std::env::var_os("LABBY_HOME"), home)
}

fn dotenv_path_from(
    labby_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(root) = labby_home.filter(|root| !root.is_empty()) {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            // Only the variable name is logged: the value is operator-supplied
            // and a rejected path adds nothing an operator cannot re-read.
            tracing::warn!(
                variable = "LABBY_HOME",
                "ignoring a non-absolute Labby installation root; no upstream dotenv was read"
            );
            return None;
        }
        return Some(root.join(".env"));
    }
    home.filter(|home| Path::new(home).is_absolute())
        .map(|home| PathBuf::from(home).join(".labby").join(".env"))
}

pub fn configured_bearer_token(env_name: &str) -> Option<String> {
    let dotenv_path = dotenv_path();
    configured_bearer_token_with_dotenv(env_name, dotenv_path.as_deref())
}

pub(crate) fn configured_bearer_token_from_path(
    env_name: &str,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    configured_bearer_token_with_dotenv(env_name, dotenv_path)
}

fn configured_bearer_token_with_dotenv(
    env_name: &str,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    let token = std::env::var(env_name).ok().or_else(|| {
        dotenv_path.and_then(|path| configured_bearer_token_from_dotenv_path(env_name, path))
    })?;
    normalize_bearer_token(&token)
}

fn configured_bearer_token_from_dotenv_path(env_name: &str, path: &Path) -> Option<String> {
    dotenvy::from_path_iter(path).ok().and_then(|iter| {
        iter.filter_map(Result::ok)
            .find_map(|(key, value)| (key == env_name).then_some(value))
            .and_then(|value| normalize_bearer_token(&value))
    })
}

fn normalize_bearer_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if token.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let raw = if token
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
    {
        token[7..].trim()
    } else {
        token
    };
    (!raw.is_empty()).then(|| raw.to_string())
}

fn configured_authorization_header_with_dotenv(
    env_name: &str,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    configured_bearer_token_with_dotenv(env_name, dotenv_path)
        .map(|token| format!("Bearer {token}"))
}

pub(super) fn websocket_authorization_header(config: &UpstreamConfig) -> Option<String> {
    let dotenv_path = dotenv_path();
    websocket_authorization_header_with_dotenv(config, dotenv_path.as_deref())
}

fn websocket_authorization_header_with_dotenv(
    config: &UpstreamConfig,
    dotenv_path: Option<&Path>,
) -> Option<String> {
    config
        .bearer_token_env
        .as_deref()
        .and_then(|env_name| configured_authorization_header_with_dotenv(env_name, dotenv_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_upstream_config() -> UpstreamConfig {
        UpstreamConfig {
            display_name: None,
            lifecycle: None,
            enabled: true,
            name: "test".into(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }
    }

    #[test]
    fn configured_bearer_token_reads_and_normalizes_dotenv_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        std::fs::write(&path, "WS_TOKEN=\"Bearer dotenv-secret\"\nOTHER=ignored\n")
            .expect("write env");

        assert_eq!(
            configured_bearer_token_from_dotenv_path("WS_TOKEN", &path),
            Some("dotenv-secret".to_string())
        );
    }

    #[test]
    fn websocket_authorization_uses_dotenv_only_bearer_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".env");
        std::fs::write(&path, "WS_TOKEN=dotenv-secret\n").expect("write env");

        let mut config = test_upstream_config();
        config.url = Some("wss://upstream.example.com/mcp".into());
        config.bearer_token_env = Some("WS_TOKEN".into());

        assert_eq!(
            websocket_authorization_header_with_dotenv(&config, Some(&path)),
            Some("Bearer dotenv-secret".to_string())
        );
    }

    #[test]
    fn dotenv_path_prefers_explicit_labby_home_over_user_home() {
        assert_eq!(
            dotenv_path_from(
                Some("/srv/labby-preview".into()),
                Some("/Users/operator".into())
            ),
            Some(PathBuf::from("/srv/labby-preview/.env"))
        );
    }

    /// A relative root never resolves a dotenv against the process working
    /// directory: it is refused here, independently of the product crate's
    /// startup validation.
    #[test]
    fn dotenv_path_refuses_a_non_absolute_installation_root() {
        assert_eq!(
            dotenv_path_from(Some("relative/root".into()), Some("/Users/operator".into())),
            None
        );
        assert_eq!(dotenv_path_from(None, Some("relative/home".into())), None);
    }

    #[test]
    fn dotenv_path_falls_back_to_user_home_installation() {
        assert_eq!(
            dotenv_path_from(Some("".into()), Some("/Users/operator".into())),
            Some(PathBuf::from("/Users/operator/.labby/.env"))
        );
        assert_eq!(dotenv_path_from(None, None), None);
    }

    #[test]
    fn normalize_bearer_token_rejects_empty_values() {
        assert_eq!(normalize_bearer_token("   "), None);
        assert_eq!(normalize_bearer_token("Bearer   "), None);
        assert_eq!(
            normalize_bearer_token(" raw-token "),
            Some("raw-token".to_string())
        );
        assert_eq!(
            normalize_bearer_token("Bearer raw-token"),
            Some("raw-token".to_string())
        );
    }
}
