//! Read-only host configuration inspection using the canonical parser and validators.
use crate::output::{OutputFormat, print};
use serde_json::json;
use std::process::ExitCode;

fn diagnostic_config(mut config: crate::config::LabConfig) -> anyhow::Result<serde_json::Value> {
    // These maps contain operator-supplied values, not merely secret references.
    // Their arbitrary keys cannot establish whether a value is safe to disclose.
    for upstream in config
        .upstream
        .iter_mut()
        .chain(&mut config.upstream_pending)
    {
        for value in upstream
            .env
            .values_mut()
            .chain(upstream.headers.values_mut())
        {
            *value = "[redacted]".to_owned();
        }
    }
    Ok(super::helpers::diagnostic_value(
        &serde_json::to_value(config)?,
        1024 * 1024,
    ))
}

#[derive(Debug)]
pub enum Operation {
    Show,
    Check,
}

pub fn run(operation: Operation, format: OutputFormat) -> anyhow::Result<ExitCode> {
    let path = crate::installation::InstallationPaths::resolve()?.config_toml();
    let raw = crate::config::host_write::read_config_snapshot(&path)?;
    let config = crate::config::parse_snapshot(&raw).map_err(|error| {
        crate::config::cli::invalid(format!(
            "Configuration validation failed: {error}. No files were changed."
        ))
    })?;
    config.cli.validate()?;
    let result = match operation {
        Operation::Check => {
            json!({"valid":true,"path":path,"configured":!raw.is_empty(),"source":"host configuration snapshot; environment overrides not applied","warnings":ignored_key_warnings(&raw)})
        }
        Operation::Show => {
            json!({"path":path,"configured":!raw.is_empty(),"source":"host configuration snapshot; environment overrides not applied","config":diagnostic_config(config)?})
        }
    };
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
}

/// Keys the snapshot sets that Labby reads past. They do not make the config
/// invalid, since serve starts with them, but each one has no effect.
fn ignored_key_warnings(raw: &str) -> Vec<serde_json::Value> {
    labby_runtime::gateway_config::ignored_config_keys::<crate::config::LabConfig>(
        raw,
        crate::config::OWNED_TOP_LEVEL_KEYS,
    )
    .iter()
    .map(|key| {
        json!({
            "key": key,
            "message": labby_runtime::gateway_config::ignored_config_key_message(key),
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_warns_about_a_setting_written_inside_a_table() {
        let warnings = ignored_key_warnings(
            "[mcp]\nport = 8765\n\n[gateway]\nupstream_request_timeout_ms = 300000\n",
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0]["key"], "gateway.upstream_request_timeout_ms");
        assert!(
            warnings[0]["message"]
                .as_str()
                .unwrap()
                .contains("move it above the first [table] header")
        );
        assert_eq!(
            ignored_key_warnings("upstream_request_timeout_ms = 300000\n"),
            Vec::<serde_json::Value>::new()
        );
    }

    #[test]
    fn check_names_the_bad_key_without_echoing_its_value() {
        let error =
            crate::config::parse_snapshot("[mcp]\nport = \"unique-secret-looking-value\"\n")
                .unwrap_err()
                .to_string();
        assert!(
            error.contains("key `mcp.port` (line 2, column 8): wrong type"),
            "{error}"
        );
        assert!(!error.contains("unique-secret-looking-value"), "{error}");
    }

    #[test]
    fn configuration_masks_arbitrary_upstream_credentials_including_pending_entries() {
        let raw = r#"
[[upstream]]
name = "database"
command = "database-mcp"
[upstream.env]
DATABASE_URL = "postgres://user:unique-database-password@db/database"
CUSTOM_CREDENTIAL = "unique-opaque-credential"

[[upstream_pending]]
name = "pending-http"
url = "https://example.invalid/mcp"
[upstream_pending.headers]
X-Custom-Auth = "unique-header-credential"
"#;
        let config = crate::config::parse_snapshot(raw).unwrap();
        let output = diagnostic_config(config).unwrap();
        let serialized = output.to_string();
        for secret in [
            "unique-database-password",
            "unique-opaque-credential",
            "unique-header-credential",
        ] {
            assert!(
                !serialized.contains(secret),
                "config output disclosed a credential fixture"
            );
        }
        assert_eq!(output["upstream"][0]["env"]["DATABASE_URL"], "[redacted]");
        assert_eq!(
            output["upstream_pending"][0]["headers"]["X-Custom-Auth"],
            "[redacted]"
        );
        assert_eq!(output["upstream"][0]["name"], "database");
        assert_eq!(
            output["upstream_pending"][0]["url"],
            "https://example.invalid/mcp"
        );
    }
}
