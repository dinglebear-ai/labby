//! Operator session inspection and local logout reuse the existing OAuth store.
use crate::{
    config::LabConfig,
    output::{OutputFormat, print},
};
use serde_json::json;
use std::process::ExitCode;

#[derive(Debug)]
pub enum Operation {
    Status,
    Logout,
}

pub async fn run(
    operation: Operation,
    config: &LabConfig,
    format: OutputFormat,
) -> anyhow::Result<ExitCode> {
    let raw=config.cli_target.as_ref().map(|target|target.server.clone())
        .or_else(||std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL").ok().filter(|url|!url.trim().is_empty()))
        .or_else(||std::env::var("LABBY_SERVER_URL").ok().filter(|url|!url.trim().is_empty()))
        .ok_or_else(||crate::config::cli::invalid("Select a destination with --server URL, --context NAME, or context use NAME. No credential operation was performed."))?;
    let server = url::Url::parse(&crate::config::cli::normalize_server(&raw)?)?;
    let result = match operation {
        Operation::Status => crate::oauth::cli_session::status(&server)?,
        Operation::Logout => {
            json!({"server":server.as_str(),"changed":crate::oauth::cli_session::logout(&server).await?,"scope":"local OAuth session only","provider_revoked":false})
        }
    };
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
}
