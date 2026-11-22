//! Write policy for secret stdio environment values.
//!
//! [`UpstreamConfig::secret_env`](labby_runtime::gateway_config::UpstreamConfig::secret_env)
//! maps a stdio child's environment variable to a credential variable in the
//! installation `.env` file. The values are write-only operator secrets, so
//! every error built here names the variable and never the value.

use labby_runtime::error::ToolError;

use super::spawn_guard::validate_stdio_env_name;

/// Most secret environment values one upstream may hold.
pub const MAX_SECRET_ENV_ENTRIES: usize = 32;

/// Largest accepted secret environment value, in bytes.
pub const MAX_SECRET_ENV_VALUE_BYTES: usize = 16 * 1024;

/// Names the stdio launcher or sandbox owns, or that make an interpreter load
/// code. A secret must not replace them, even where a plain `env` entry may.
const RESERVED_NAMES: &[&str] = &[
    "TMPDIR",
    "TMP",
    "TEMP",
    "UPSTREAM_STATE_DIR",
    "UPSTREAM_READ_ONLY_PATHS",
    "NODE_OPTIONS",
    "NODE_PATH",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "PYTHONHOME",
    "BASH_ENV",
    "ENV",
];

/// Prefixes for loader-injection and Labby-owned names.
const RESERVED_PREFIXES: &[&str] = &["LD_", "DYLD_", "LABBY_"];

fn invalid(message: String) -> ToolError {
    ToolError::InvalidParam {
        message,
        param: "secret_env".to_string(),
    }
}

/// Validate the child environment variable name of a secret value.
///
/// The name must pass the stdio spawn guard (`^[A-Z][A-Z0-9_]*$`, not a
/// protected process or `LABBY_*` variable). It must also not be a name the
/// launcher forwards from the gateway environment or the sandbox sets.
pub fn validate_secret_env_name(name: &str) -> Result<(), ToolError> {
    validate_stdio_env_name(name).map_err(|_| {
        invalid(format!(
            "secret env var name `{name}` is invalid; use ^[A-Z][A-Z0-9_]*$ and not a protected process or LABBY_* variable"
        ))
    })?;
    let reserved = RESERVED_NAMES.contains(&name)
        || RESERVED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || crate::upstream::pool::STDIO_ENV_ALLOWLIST.contains(&name);
    if reserved {
        return Err(invalid(format!(
            "secret env var name `{name}` is reserved by the stdio launcher or sandbox"
        )));
    }
    Ok(())
}

/// Credential-file namespace for secret stdio env values.
pub const SECRET_ENV_CREDENTIAL_PREFIX: &str = "LABBY_GW_";

/// Validate the credential variable that `child` reads from.
///
/// It must be `LABBY_GW_<UPSTREAM>_ENV_<child>`, the namespace this feature
/// writes. A configuration therefore cannot hand a stdio child an unrelated
/// installation secret, such as an OAuth encryption key, that the stdio
/// sandbox otherwise keeps out of reach.
pub fn validate_secret_env_credential_name(child: &str, credential: &str) -> Result<(), ToolError> {
    let suffix = format!("_ENV_{child}");
    let upstream = credential
        .strip_prefix(SECRET_ENV_CREDENTIAL_PREFIX)
        .and_then(|rest| rest.strip_suffix(&suffix));
    let valid = upstream.is_some_and(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    });
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "the credential for secret env var `{child}` must be named {SECRET_ENV_CREDENTIAL_PREFIX}<UPSTREAM>{suffix}"
        )))
    }
}

/// Validate one secret value. The error names `name`, never the value.
pub fn validate_secret_env_value(name: &str, value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        return Err(invalid(format!(
            "secret env var `{name}` value must not be empty"
        )));
    }
    if value.len() > MAX_SECRET_ENV_VALUE_BYTES {
        return Err(invalid(format!(
            "secret env var `{name}` value exceeds {MAX_SECRET_ENV_VALUE_BYTES} bytes"
        )));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err(invalid(format!(
            "secret env var `{name}` value must not contain newline, carriage return, or null bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: &str = "fake-secret-value-0001";

    #[test]
    fn accepts_ordinary_api_key_names() {
        for name in ["FIGMA_API_KEY", "GITHUB_TOKEN", "API_KEY_2"] {
            validate_secret_env_name(name).expect(name);
        }
    }

    #[test]
    fn rejects_invalid_syntax() {
        for name in [
            "",
            "figma_api_key",
            "1KEY",
            "KEY-NAME",
            "KEY NAME",
            "KEY=VALUE",
        ] {
            let error = validate_secret_env_name(name).unwrap_err();
            assert_eq!(error.kind(), "invalid_param", "{name}");
        }
    }

    #[test]
    fn rejects_names_owned_by_labby_launcher_or_sandbox() {
        for name in [
            "PATH",
            "HOME",
            "LD_PRELOAD",
            "LD_AUDIT",
            "DYLD_INSERT_LIBRARIES",
            "LABBY_HOME",
            "LABBY_STDIO_SANDBOX_BACKEND",
            "TMPDIR",
            "UPSTREAM_STATE_DIR",
            "UPSTREAM_READ_ONLY_PATHS",
            "SSL_CERT_FILE",
            "NODE_EXTRA_CA_CERTS",
            "XDG_CONFIG_HOME",
            "NODE_OPTIONS",
        ] {
            assert!(validate_secret_env_name(name).is_err(), "{name} accepted");
        }
    }

    #[test]
    fn every_forwarded_parent_name_is_reserved() {
        for name in crate::upstream::pool::STDIO_ENV_ALLOWLIST {
            if name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                assert!(validate_secret_env_name(name).is_err(), "{name} accepted");
            }
        }
    }

    #[test]
    fn credential_names_stay_in_the_secret_env_namespace() {
        validate_secret_env_credential_name("FIGMA_API_KEY", "LABBY_GW_FIGMA_ENV_FIGMA_API_KEY")
            .expect("derived name");
        validate_secret_env_credential_name(
            "FIGMA_API_KEY",
            &crate::gateway::config::default_secret_env_credential_name(
                "my.figma-server",
                "FIGMA_API_KEY",
            ),
        )
        .expect("default name");
        for credential in [
            "LABBY_OAUTH_ENCRYPTION_KEY",
            "LABBY_GW_GITHUB_AUTH_HEADER",
            "LABBY_GW__ENV_FIGMA_API_KEY",
            "LABBY_GW_OTHER_ENV_OTHER_TOKEN",
            "FIGMA_API_KEY",
            "LABBY_GW_figma_ENV_FIGMA_API_KEY",
        ] {
            assert!(
                validate_secret_env_credential_name("FIGMA_API_KEY", credential).is_err(),
                "{credential} accepted"
            );
        }
    }

    #[test]
    fn refuses_empty_and_whitespace_values() {
        for value in ["", " ", "\t"] {
            let error = validate_secret_env_value("FIGMA_API_KEY", value).unwrap_err();
            assert!(error.to_string().contains("must not be empty"));
        }
    }

    #[test]
    fn caps_value_size_without_echoing_the_value() {
        let at_cap = "a".repeat(MAX_SECRET_ENV_VALUE_BYTES);
        validate_secret_env_value("FIGMA_API_KEY", &at_cap).expect("value at cap");
        let over = format!("{FAKE}{}", "a".repeat(MAX_SECRET_ENV_VALUE_BYTES));
        let error = validate_secret_env_value("FIGMA_API_KEY", &over).unwrap_err();
        assert!(error.to_string().contains("exceeds"));
        assert!(!error.to_string().contains(FAKE));
    }

    #[test]
    fn rejects_line_breaks_and_nul_without_echoing_the_value() {
        for value in [
            format!("{FAKE}\nX=1"),
            format!("{FAKE}\r"),
            format!("{FAKE}\0"),
        ] {
            let error = validate_secret_env_value("FIGMA_API_KEY", &value).unwrap_err();
            assert!(!error.to_string().contains(FAKE));
        }
    }
}
