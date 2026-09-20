//! Read-only host configuration inspection using the canonical parser and validators.
use crate::output::{OutputFormat, print};
use serde_json::json;
use std::process::ExitCode;

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
            json!({"path":path,"configured":!raw.is_empty(),"source":"host configuration snapshot; environment overrides not applied","config":super::helpers::diagnostic_value(&serde_json::to_value(&config)?,1024*1024)})
        }
    };
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
}
