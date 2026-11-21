//! Resolve typed installation-admin registrations into regular MCP upstreams.
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationRequest {
    spec: NamedUpstream,
    adapter: AdapterRegistration,
    confirm: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedUpstream {
    name: String,
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum AdapterRegistration {
    Cli { manifest_path: PathBuf },
    Openapi(crate::openapi_registration::OpenApiRegistration),
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "adapter".into(),
    }
}

pub(crate) async fn normalize(
    manager: &super::manager::GatewayManager,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    if action != "gateway.add" || params.get("adapter").is_none() {
        return Ok(params);
    }
    let request: RegistrationRequest = serde_json::from_value(params).map_err(|_| {
        invalid("provide a server name, a supported adapter configuration and confirmation")
    })?;
    if !request.confirm {
        return Err(invalid(
            "confirm the adapter registration before adding the server",
        ));
    }
    let (command, args) = match request.adapter {
        AdapterRegistration::Cli { manifest_path } => cli_command(manifest_path).await?,
        AdapterRegistration::Openapi(registration) => {
            tokio::task::spawn_blocking(move || crate::openapi_registration::command(registration))
                .await
                .map_err(|_| invalid("could not validate the OpenAPI MCP registration"))?
                .map_err(|error| invalid(error.to_string()))?
        }
    };
    let config = manager.current_config().await.gateway;
    if config.disable_spawn_guard {
        return Err(invalid(
            "guided adapters require the stdio spawn guard to be enabled",
        ));
    }
    if command != labby_gateway::security::guided_cli::SELECTOR
        && !config.extra_stdio_commands.contains(&command)
    {
        return Err(invalid(
            "the host operator must explicitly allow the adapter executable's exact path with the stdio spawn guard enabled",
        ));
    }
    Ok(json!({
        "spec": {
            "name": request.spec.name,
            "display_name": request.spec.display_name,
            "command": command,
            "args": args,
            "proxy_prompts": false,
            "proxy_resources": false
        }
    }))
}

#[cfg(target_os = "linux")]
async fn cli_command(manifest_path: PathBuf) -> Result<(String, Vec<String>), ToolError> {
    tokio::task::spawn_blocking(move || crate::cli_mcp::registration_command(&manifest_path))
        .await
        .map_err(|_| invalid("could not validate the CLI MCP registration"))?
        .map_err(|error| invalid(error.to_string()))
}

#[cfg(not(target_os = "linux"))]
async fn cli_command(_manifest_path: PathBuf) -> Result<(String, Vec<String>), ToolError> {
    Err(invalid("Add CLI is supported on Linux hosts only"))
}
