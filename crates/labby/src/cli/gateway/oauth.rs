use std::process::ExitCode;

use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::cli::gateway::{GatewayOauthUpstreamArgs, LazyGatewayManager};
use crate::config::LabConfig;
use crate::output::OutputFormat;

use super::dispatch::dispatch_gateway_action;

#[derive(Debug, Deserialize)]
struct GatewayOauthStartView {
    authorization_url: String,
}

pub(super) async fn run_gateway_oauth_start(
    manager: &LazyGatewayManager<'_>,
    config: &LabConfig,
    args: GatewayOauthUpstreamArgs,
    format: OutputFormat,
) -> Result<ExitCode> {
    let params = json!({ "upstream": args.name });
    let start = std::time::Instant::now();
    let value = dispatch_gateway_action(manager, config, "gateway.oauth.start".to_string(), params)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{}",
                serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
            )
        })?;
    tracing::info!(
        surface = "cli",
        service = "gateway",
        action = "gateway.oauth.start",
        elapsed_ms = start.elapsed().as_millis(),
        "dispatch ok"
    );

    if format.is_json() && !args.wait {
        crate::output::print(&value, format)?;
    }

    let start_view: GatewayOauthStartView =
        serde_json::from_value(value.clone()).map_err(|error| {
            anyhow::anyhow!("failed to decode gateway oauth start response: {error}")
        })?;

    let theme = crate::output::theme::CliTheme::from_context(format.render_context());

    if args.open {
        open_in_browser(&start_view.authorization_url)?;
        eprintln!(
            "{}",
            theme.muted("Opened authorization URL in your browser.")
        );
    } else {
        eprintln!(
            "{}\n{}",
            theme.muted("Open this URL to authorize:"),
            theme.accent(&start_view.authorization_url)
        );
    }

    if args.wait {
        eprintln!(
            "{}",
            theme.muted(format!(
                "Waiting for OAuth completion for the shared gateway credential on `{}`...",
                args.name
            ))
        );
        let progress = crate::output::progress::ProgressPhase::spinner(
            format,
            format!("Waiting for OAuth completion for `{}`", args.name),
        );
        let wait_value = dispatch_gateway_action(
            manager,
            config,
            "gateway.oauth.wait".to_string(),
            json!({
                "upstream": args.name,
                "timeout_secs": args.wait_timeout_secs,
            }),
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "{}",
                serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
            )
        })?;
        drop(progress);

        let (result, exit_code) = oauth_wait_outcome(value, wait_value)?;
        let authenticated = exit_code == ExitCode::SUCCESS;

        if authenticated {
            eprintln!(
                "{}",
                theme
                    .success("OAuth completed. The callback stored the shared gateway credential.")
            );
        } else {
            eprintln!(
                "{}",
                theme.warn(
                    "Timed out waiting for OAuth completion. The browser callback may still succeed later; re-run `labby server auth status <name>` to check."
                )
            );
        }
        if format.is_json() {
            crate::output::print(&result, format)?;
        }
        return Ok(exit_code);
    }

    Ok(ExitCode::SUCCESS)
}

fn oauth_wait_outcome(
    mut start: serde_json::Value,
    wait: serde_json::Value,
) -> Result<(serde_json::Value, ExitCode)> {
    let authenticated = wait["authenticated"].as_bool().ok_or_else(|| {
        crate::dispatch::error::ToolError::Sdk {
            sdk_kind: "decode_error".into(),
            message: "Gateway OAuth wait response omitted authenticated state. Inspect server auth status before retrying.".into(),
        }
    })?;
    start["authenticated"] = json!(authenticated);
    start["timed_out"] = json!(!authenticated);
    Ok((
        start,
        if authenticated {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        },
    ))
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer.exe")
            .arg(url)
            .status()?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "opening a browser is not supported on this platform"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_wait_reports_final_state_and_failure_on_timeout() {
        for authenticated in [true, false] {
            let (result, exit_code) = oauth_wait_outcome(
                json!({"authorization_url":"https://example.test/authorize"}),
                json!({"authenticated":authenticated,"timed_out":!authenticated}),
            )
            .unwrap();
            assert_eq!(result["authenticated"], authenticated);
            assert_eq!(result["timed_out"], !authenticated);
            assert_eq!(exit_code == ExitCode::SUCCESS, authenticated);
            assert!(result["authorization_url"].is_string());
        }
        assert!(oauth_wait_outcome(json!({}), json!({})).is_err());
    }
}
