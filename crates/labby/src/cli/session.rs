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

/// Resolve operator authentication against the same destination as live dispatch.
pub(crate) fn selected_server(config: &LabConfig) -> anyhow::Result<String> {
    selected_server_from(
        config
            .cli_target
            .as_ref()
            .map(|target| target.server.as_str()),
        std::env::var("CLAUDE_PLUGIN_OPTION_SERVER_URL")
            .ok()
            .as_deref(),
        std::env::var("LABBY_SERVER_URL").ok().as_deref(),
    )
}

fn selected_server_from(
    selected: Option<&str>,
    plugin: Option<&str>,
    operator: Option<&str>,
) -> anyhow::Result<String> {
    let raw = selected.or_else(|| plugin.filter(|url| !url.trim().is_empty()))
        .or_else(|| operator.filter(|url| !url.trim().is_empty()))
        .ok_or_else(|| crate::config::cli::invalid("Select a destination with --server URL, --context NAME, or context use NAME. No credential operation was performed."))?;
    Ok(crate::config::cli::normalize_server(raw)?)
}

pub async fn run(
    operation: Operation,
    config: &LabConfig,
    format: OutputFormat,
) -> anyhow::Result<ExitCode> {
    let server = url::Url::parse(&selected_server(config)?)?;
    let result = match operation {
        Operation::Status => crate::oauth::cli_session::status(&server)?,
        Operation::Logout => {
            json!({"server":server.as_str(),"changed":crate::oauth::cli_session::logout(&server).await?,"scope":"local OAuth session only","provider_revoked":false})
        }
    };
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_auth_uses_dispatch_destination_precedence() {
        let plugin = Some("https://plugin.example/mcp");
        let operator = Some("https://operator.example");
        assert_eq!(
            selected_server_from(Some("https://selected.example"), plugin, operator).unwrap(),
            "https://selected.example/"
        );
        assert_eq!(
            selected_server_from(None, plugin, operator).unwrap(),
            "https://plugin.example/"
        );
        assert_eq!(
            selected_server_from(None, plugin, None).unwrap(),
            "https://plugin.example/"
        );
        assert_eq!(
            selected_server_from(None, Some(" "), operator).unwrap(),
            "https://operator.example/"
        );
        assert!(selected_server_from(None, None, Some(" ")).is_err());
        assert!(
            selected_server_from(None, Some("invalid"), operator).is_err(),
            "invalid higher-priority targets must not fall back"
        );
    }
}
