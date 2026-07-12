use std::process::ExitCode;

use anyhow::Result;
use labby_codemode::MAX_SOURCE_BYTES;
use serde_json::json;

use crate::cli::gateway::{GatewayCodeArgs, GatewayCodeCommand, LazyGatewayManager};
use crate::config::LabConfig;
use crate::dispatch::gateway::code_mode::{CodeModeBroker, CodeModeCaller, CodeModeSurface};
use crate::output::OutputFormat;

use super::dispatch::dispatch_gateway_action;
use crate::live_gateway as remote;

pub(super) async fn run_gateway_code(
    manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    args: GatewayCodeArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    match args.command {
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
        GatewayCodeCommand::Exec { code, file } => {
            let code = read_code_mode_source(code, file, MAX_SOURCE_BYTES as u64)?;
            let response = execute_code_mode(manager, config, &code).await?;
            crate::output::print(&response, format)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Prefer executing against the live daemon's actual `codemode` MCP tool
/// (warm upstream connections, real circuit-breaker/OAuth state) over the
/// CLI's own throwaway `CodeModeBroker`, which lazily cold-connects whatever
/// the snippet touches and never shares in-memory state (OAuth refresh
/// circuit breaker included) with the process actually serving traffic.
async fn execute_code_mode(
    manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    code: &str,
) -> Result<serde_json::Value> {
    if let Some(live) = remote::detect(config).await {
        match live.call_codemode_tool(code).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                tracing::warn!(
                    surface = "cli",
                    service = "gateway",
                    action = "gateway.code.exec",
                    error = %error,
                    "remote code mode execution failed, falling back to local broker"
                );
            }
        }
    }

    let manager = manager.get().await?;
    let broker = CodeModeBroker::new(Some(manager.as_ref()));
    let response = broker
        .execute(
            code,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            manager.code_mode_config().await,
            crate::dispatch::gateway::code_mode::ToolScope::default(),
            // No durable-run execution id on the local CLI broker path: journaling
            // is driven through the MCP `codemode` tool where an execution id +
            // owner context exist. `None` keeps `record_step` write-free here.
            None,
        )
        .await?;
    Ok(serde_json::to_value(response)?)
}

fn read_code_mode_source(
    code: Option<String>,
    file: Option<std::path::PathBuf>,
    max_source_bytes: u64,
) -> Result<String> {
    match (code, file) {
        (Some(code), None) => {
            // Check the inline string length before any further buffering.
            if code.len() as u64 > max_source_bytes {
                anyhow::bail!("Code Mode source exceeds {max_source_bytes} bytes");
            }
            Ok(code)
        }
        (None, Some(path)) => {
            let metadata = std::fs::metadata(&path)?;
            if metadata.len() > max_source_bytes {
                anyhow::bail!("Code Mode source file exceeds {max_source_bytes} bytes");
            }
            use std::io::Read as _;
            let mut buf = String::new();
            std::fs::File::open(&path)?
                .take(max_source_bytes + 1)
                .read_to_string(&mut buf)?;
            if buf.len() as u64 > max_source_bytes {
                anyhow::bail!("Code Mode source file exceeds {max_source_bytes} bytes");
            }
            Ok(buf)
        }
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_source_limit_is_shared_const_boundary() {
        let at_limit = "a".repeat(MAX_SOURCE_BYTES);
        assert!(read_code_mode_source(Some(at_limit), None, MAX_SOURCE_BYTES as u64).is_ok());

        let over_limit = "a".repeat(MAX_SOURCE_BYTES + 1);
        assert!(read_code_mode_source(Some(over_limit), None, MAX_SOURCE_BYTES as u64).is_err());
    }
}
