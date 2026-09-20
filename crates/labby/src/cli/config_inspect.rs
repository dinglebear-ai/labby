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
            json!({"valid":true,"path":path,"configured":!raw.is_empty(),"source":"host configuration snapshot; environment overrides not applied"})
        }
        Operation::Show => {
            json!({"path":path,"configured":!raw.is_empty(),"source":"host configuration snapshot; environment overrides not applied","config":diagnostic_config(config)?})
        }
    };
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "config output disclosed {secret}"
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
