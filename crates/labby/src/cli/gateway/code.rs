use std::process::ExitCode;

use anyhow::Result;
use serde_json::json;

use crate::cli::gateway::{
    GatewayCodeArgs, GatewayCodeCommand, GatewayCodeUiCommand, LazyGatewayManager,
};
use crate::config::LabConfig;
use crate::dispatch::gateway::code_mode::{CodeModeBroker, CodeModeCaller, CodeModeSurface};
use crate::output::OutputFormat;

use super::dispatch::dispatch_gateway_action;
use crate::live_gateway as remote;

pub(super) fn run_gateway_code(
    manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    args: GatewayCodeArgs,
    format: OutputFormat,
) -> impl Future<Output = Result<ExitCode>> {
    // Keep this invocation's state out of the parent poll frame while the
    // nested broker/catalog call chain executes.
    Box::pin(async move {
        match args.command {
            GatewayCodeCommand::Search { query, limit } => {
                let source = format!(
                    "async () => await codemode.search({})",
                    serde_json::to_string(&json!({ "query": query, "limit": limit }))?
                );
                let response = execute_code_mode(manager, config, &source).await?;
                crate::output::print(&response, format)?;
            }
            GatewayCodeCommand::Describe { path } => {
                let source = format!(
                    "async () => await codemode.describe({})",
                    serde_json::to_string(&path)?
                );
                let response = execute_code_mode(manager, config, &source).await?;
                crate::output::print(&response, format)?;
            }
            GatewayCodeCommand::Status => {
                let value = dispatch_gateway_action(
                    manager,
                    config,
                    "gateway.code_mode.get".to_string(),
                    json!({}),
                )
                .await?;
                crate::output::print(&value, format)?;
            }
            GatewayCodeCommand::Enable => {
                let value = dispatch_gateway_action(
                    manager,
                    config,
                    "gateway.code_mode.set".to_string(),
                    json!({ "enabled": true }),
                )
                .await?;
                crate::output::print(&value, format)?;
            }
            GatewayCodeCommand::Disable => {
                let value = dispatch_gateway_action(
                    manager,
                    config,
                    "gateway.code_mode.set".to_string(),
                    json!({ "enabled": false }),
                )
                .await?;
                crate::output::print(&value, format)?;
            }
            GatewayCodeCommand::Ui { command } => {
                let params = match command {
                    GatewayCodeUiCommand::Status => None,
                    GatewayCodeUiCommand::Enable => Some(json!({ "mcp_ui_enabled": true })),
                    GatewayCodeUiCommand::Disable => Some(json!({ "mcp_ui_enabled": false })),
                };
                let value = dispatch_gateway_action(
                    manager,
                    config,
                    if params.is_some() {
                        "gateway.code_mode.set".to_string()
                    } else {
                        "gateway.code_mode.get".to_string()
                    },
                    params.unwrap_or_else(|| json!({})),
                )
                .await?;
                crate::output::print(&value, format)?;
            }
            GatewayCodeCommand::Exec { code, file } => {
                // The selected daemon owns its configured source limit. The CLI only
                // applies the shared allocation ceiling before target resolution;
                // local and remote execution paths enforce their own lower limit.
                let code =
                    read_code_mode_source(code, file, labby_codemode::MAX_SOURCE_BYTES as u64)?;
                let response = execute_code_mode(manager, config, &code).await?;
                crate::output::print(&response, format)?;
            }
        }

        Ok(ExitCode::SUCCESS)
    })
}

/// Prefer executing against the live daemon's actual `codemode` MCP tool
/// (warm upstream connections, real circuit-breaker/OAuth state) over the
/// CLI's own throwaway `CodeModeBroker`, which lazily cold-connects whatever
/// the snippet touches and never shares in-memory state (OAuth refresh
/// circuit breaker included) with the process actually serving traffic.
fn execute_code_mode(
    manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    code: &str,
) -> impl Future<Output = Result<serde_json::Value>> {
    Box::pin(async move {
        let selected = remote::detect(config, "cli").await?;
        let remote_call = selected.map(|live| async move {
            let source = live.source();
            live.with_team_id(manager.team_id().map(str::to_owned))
                .call_codemode_tool(code).await
                .map_err(|error| anyhow::Error::new(error).context(format!(
                    "Code Mode execution through the selected Labby server ({source}) failed. The operation was not retried locally. Side effects may already have occurred; inspect gateway logs before retrying."
                )))
        });
        execute_selected(remote_call, || async move {
            tracing::debug!(
                surface = "cli",
                service = "gateway",
                action = "gateway.code.exec",
                target_mode = "local",
                "no daemon selected; executing in the local broker"
            );
            let manager = manager.get().await?;
            let broker = CodeModeBroker::new(Some(manager.as_ref()));
            let response = broker
                .execute(
                    code,
                    CodeModeCaller::TrustedLocal,
                    CodeModeSurface::Cli,
                    manager.code_mode_config().await,
                    crate::dispatch::gateway::code_mode::ToolScope::default(),
                    None,
                )
                .await?;
            Ok(serde_json::to_value(response)?)
        })
        .await
    })
}

/// Target selection is final once execution starts. A lost response must never
/// replay a possibly committed operation through another broker.
fn execute_selected<R, L, F>(
    remote: Option<R>,
    local: F,
) -> impl Future<Output = Result<serde_json::Value>>
where
    R: Future<Output = Result<serde_json::Value>>,
    L: Future<Output = Result<serde_json::Value>>,
    F: FnOnce() -> L,
{
    Box::pin(async move {
        match remote {
            Some(call) => call.await,
            None => Box::pin(local()).await,
        }
    })
}

fn source_error(message: String) -> anyhow::Error {
    crate::dispatch::error::ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("{message} No Code Mode operation was executed."),
    }
    .into()
}

fn read_code_mode_source(
    code: Option<String>,
    file: Option<std::path::PathBuf>,
    max_source_bytes: u64,
) -> Result<String> {
    use std::io::{IsTerminal as _, Read as _};
    let too_large = || {
        source_error(format!(
            "Code Mode source exceeds the {max_source_bytes}-byte allocation limit. Use a smaller source file."
        ))
    };
    match (code, file) {
        (Some(code), None) => {
            if code.len() as u64 > max_source_bytes {
                return Err(too_large());
            }
            Ok(code)
        }
        (None, Some(path)) => {
            let mut bytes = Vec::new();
            if path.as_os_str() == "-" {
                let stdin = std::io::stdin();
                if stdin.is_terminal() {
                    return Err(source_error("--file - requires redirected standard input. Pipe a source file, or pass --file PATH.".to_string()));
                }
                stdin
                    .lock()
                    .take(max_source_bytes.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| {
                        source_error(format!("Cannot read Code Mode source from stdin: {error}."))
                    })?;
            } else {
                let file = std::fs::File::open(&path).map_err(|error| source_error(format!(
                    "Cannot open Code Mode source file `{}`: {error}. Check --file and its permissions.", path.display())))?;
                if file
                    .metadata()
                    .map_err(|error| {
                        source_error(format!("Cannot inspect Code Mode source file: {error}."))
                    })?
                    .len()
                    > max_source_bytes
                {
                    return Err(too_large());
                }
                file.take(max_source_bytes.saturating_add(1))
                    .read_to_end(&mut bytes)
                    .map_err(|error| {
                        source_error(format!(
                            "Cannot read Code Mode source file `{}`: {error}.",
                            path.display()
                        ))
                    })?;
            }
            if bytes.len() as u64 > max_source_bytes {
                return Err(too_large());
            }
            String::from_utf8(bytes)
                .map_err(|_| source_error("Code Mode source must be valid UTF-8 text.".to_string()))
        }
        _ => Err(source_error(
            "Provide exactly one of --code or --file. Use --file - to read stdin.".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_limit_matches_shared_catalog_cap() {
        use clap::Parser as _;
        for limit in ["1", "50"] {
            assert!(
                crate::cli::Cli::try_parse_from([
                    "labby", "code", "search", "catalog", "--limit", limit,
                ])
                .is_ok()
            );
        }
        for limit in ["0", "51", "100"] {
            assert!(
                crate::cli::Cli::try_parse_from([
                    "labby", "code", "search", "catalog", "--limit", limit,
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn cli_source_read_uses_shared_hard_ceiling() {
        let max_source_bytes = labby_codemode::MAX_SOURCE_BYTES;
        let at_limit = "a".repeat(max_source_bytes);
        assert!(read_code_mode_source(Some(at_limit), None, max_source_bytes as u64).is_ok());

        let over_limit = "a".repeat(max_source_bytes + 1);
        assert!(read_code_mode_source(Some(over_limit), None, max_source_bytes as u64).is_err());
    }
    #[tokio::test]
    async fn failed_remote_execution_never_invokes_local_broker() {
        let local_calls = std::cell::Cell::new(0);
        let result = execute_selected(
            Some(async { Err(anyhow::anyhow!("response lost after possible commit")) }),
            || async {
                local_calls.set(local_calls.get() + 1);
                Ok(json!({"replayed": true}))
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            local_calls.get(),
            0,
            "an uncertain remote result must not be replayed"
        );
    }

    #[test]
    fn missing_source_has_a_typed_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let error =
            read_code_mode_source(None, Some(dir.path().join("missing.js")), 1024).unwrap_err();
        let error = error
            .downcast_ref::<crate::dispatch::error::ToolError>()
            .unwrap();
        assert_eq!(error.kind(), "invalid_param");
        assert!(error.user_message().contains("missing.js"));
        assert!(
            error
                .user_message()
                .contains("No Code Mode operation was executed")
        );
    }
}
