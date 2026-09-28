//! Shared helpers for thin CLI dispatch shims.

use std::future::Future;
use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::Result;
use dialoguer::Confirm;
use serde_json::Value;

use labby_primitives::action::ActionSpec;

use crate::dispatch::error::ToolError;
use crate::output::theme::CliTheme;

tokio::task_local! {
    /// Invocation-local prompting policy; never changes the process environment.
    pub(crate) static INTERACTIVE: bool;
    /// Correlates finite multi-outcome responses with the existing CLI audit span.
    pub(crate) static REQUEST_ID: String;
}

/// Prompts require a terminal and an invocation that permits interactive input.
pub fn interactive_allowed() -> bool {
    INTERACTIVE.try_with(|enabled| *enabled).unwrap_or(true)
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
}

/// Adapt structured CLI values to the canonical redactors, including stdio argv arrays.
#[must_use]
pub fn diagnostic_value(value: &Value, max_bytes: usize) -> Value {
    fn redact_arguments(value: &mut Value) {
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    if key == "args" || key == "argv" {
                        if let Some(values) = value
                            .as_array()
                            .filter(|values| values.iter().all(Value::is_string))
                        {
                            let arguments = values
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect::<Vec<_>>();
                            *value = serde_json::json!(labby_runtime::redact::redact_stdio_args(
                                &arguments
                            ));
                            continue;
                        }
                    }
                    redact_arguments(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    redact_arguments(value);
                }
            }
            _ => {}
        }
    }
    let mut value = value.clone();
    redact_arguments(&mut value);
    labby_runtime::redact::redact_trace_value(&value, max_bytes)
}

/// Build a bounded, redacted preview. This function never dispatches an operation.
#[must_use]
pub fn dry_run_value(service: &str, action: &str, params: &Value) -> Value {
    serde_json::json!({
        "ok": true,
        "dry_run": true,
        "executed": false,
        "service": service,
        "action": action,
        "side_effects": "none_expected",
        "params": diagnostic_value(params, 16 * 1024),
    })
}

/// Render a safe preview in the requested output format.
pub fn print_dry_run(
    service: &str,
    action: &str,
    params: &Value,
    format: OutputFormat,
) -> Result<()> {
    let preview = dry_run_value(service, action, params);
    if format.is_json() {
        return print(&preview, format);
    }
    use std::io::Write as _;
    let theme = CliTheme::from_context(format.render_context());
    writeln!(std::io::stdout().lock(), "{} {}", theme.warn("[dry-run]"),
        theme.muted(format!("would dispatch {service} action `{action}`; no operation was executed. Redacted parameters: {}", preview["params"])))?;
    Ok(())
}

use crate::output::{OutputFormat, print};

/// Run an action-style CLI command and emit the canonical dispatch log shape.
pub fn run_action_command<F, Fut>(
    service: &'static str,
    action: String,
    params: Value,
    format: OutputFormat,
    dispatch: F,
) -> impl Future<Output = Result<ExitCode>>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ToolError>>,
{
    // Keep the complete action future out of each command arm's poll frame.
    // These adapters run once per invocation; one allocation avoids stacking
    // copies of the dispatch state on Windows' small main-thread stack.
    Box::pin(async move {
        #[cfg(feature = "gateway")]
        {
            if let Some(manager) = crate::dispatch::gateway::current_gateway_manager() {
                if !manager.surface_enabled_for_service(service, "cli").await {
                    let error = ToolError::Sdk {
                        sdk_kind: "not_found".to_string(),
                        message: format!("service `{service}` is not enabled on the cli surface"),
                    };
                    return Err(anyhow::Error::new(error));
                }
            }
        }

        let start = std::time::Instant::now();
        let result = dispatch(action.clone(), params).await;
        let elapsed_ms = start.elapsed().as_millis();

        match &result {
            Ok(_) => tracing::info!(surface = "cli", service, action, elapsed_ms, "dispatch ok"),
            Err(e) if e.is_internal() => tracing::error!(
                surface = "cli",
                service,
                action,
                elapsed_ms,
                kind = e.kind(),
                "dispatch error"
            ),
            Err(e) => tracing::warn!(
                surface = "cli",
                service,
                action,
                elapsed_ms,
                kind = e.kind(),
                "dispatch error"
            ),
        }

        let value = result.map_err(anyhow::Error::new)?;
        print(&value, format)?;
        Ok(ExitCode::SUCCESS)
    })
}

/// Run an action-style CLI command with destructive confirmation support.
pub async fn run_confirmable_action_command<F, Fut>(
    service: &'static str,
    actions: &[ActionSpec],
    action: String,
    params: Value,
    yes: bool,
    format: OutputFormat,
    dispatch: F,
) -> Result<ExitCode>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, ToolError>>,
{
    #[cfg(feature = "gateway")]
    {
        if let Some(manager) = crate::dispatch::gateway::current_gateway_manager() {
            if !manager.surface_enabled_for_service(service, "cli").await {
                let error = ToolError::Sdk {
                    sdk_kind: "not_found".to_string(),
                    message: format!("service `{service}` is not enabled on the cli surface"),
                };
                return Err(anyhow::Error::new(error));
            }
        }
    }

    if !yes
        && actions
            .iter()
            .any(|spec| spec.name == action && spec.destructive)
    {
        if !interactive_allowed() {
            tracing::warn!(
                surface = "cli",
                service,
                action,
                "destructive action blocked: non-interactive stdin, pass -y"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "confirmation_required".to_string(),
                message: format!("Action `{action}` was not executed because interactive confirmation is disabled. Review its effects and pass --yes to confirm."),
            }.into());
        }
        let confirmed = Confirm::new()
            .with_prompt(format!(
                "{service} action `{action}` is destructive. Continue?"
            ))
            .default(false)
            .interact()
            .map_err(|e| anyhow::anyhow!("failed to read confirmation: {e}"))?;
        if !confirmed {
            tracing::info!(
                surface = "cli",
                service,
                action,
                "destructive action aborted by user"
            );
            anyhow::bail!("aborted by user");
        }
    }
    run_action_command(service, action, params, format, dispatch).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    use super::*;
    use crate::test_support::{SharedBuf, captured_logs};

    #[test]
    fn action_command_logs_cli_success_shape() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=info"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );

        let _guard = tracing::subscriber::set_default(subscriber);
        crate::test_support::rebuild_tracing_interest_cache();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            run_action_command(
                "unifi",
                "sites.list".to_string(),
                json!({}),
                OutputFormat::from_json_flag(
                    true,
                    crate::output::ColorPolicy::Auto,
                    crate::output::RenderEnv::stdout(),
                ),
                |_action, _params| async { Ok(json!({"ok": true})) },
            )
            .await
            .unwrap();
        });

        drop(_guard);
        let logs = captured_logs(&buf);
        assert!(logs.contains("\"surface\":\"cli\""));
        assert!(logs.contains("\"service\":\"unifi\""));
        assert!(logs.contains("\"action\":\"sites.list\""));
        assert!(logs.contains("\"elapsed_ms\""));
    }

    #[test]
    fn action_command_logs_cli_failure_kind() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("labby=warn"))
            .with(
                fmt::layer()
                    .json()
                    .with_writer(buf.clone())
                    .with_ansi(false)
                    .without_time(),
            );

        let _guard = tracing::subscriber::set_default(subscriber);
        crate::test_support::rebuild_tracing_interest_cache();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let err = run_action_command(
                "bytestash",
                "snippets.get".to_string(),
                json!({}),
                OutputFormat::from_json_flag(
                    true,
                    crate::output::ColorPolicy::Auto,
                    crate::output::RenderEnv::stdout(),
                ),
                |_action, _params| async {
                    Err(ToolError::MissingParam {
                        message: "missing required parameter `id`".into(),
                        param: "id".into(),
                    })
                },
            )
            .await
            .unwrap_err();
            assert_eq!(
                err.downcast_ref::<ToolError>()
                    .expect("typed dispatch error")
                    .kind(),
                "missing_param"
            );
        });

        drop(_guard);
        let logs = captured_logs(&buf);
        assert!(logs.contains("\"surface\":\"cli\""));
        assert!(logs.contains("\"service\":\"bytestash\""));
        assert!(logs.contains("\"action\":\"snippets.get\""));
        assert!(logs.contains("\"kind\":\"missing_param\""));
    }
    #[test]
    fn previews_are_bounded_and_redact_nested_credentials_and_stdio_arguments() {
        let params = serde_json::json!({"spec": {
            "name":"test-server", "token":"unique-bearer-secret",
            "args":["--password", "unique-password-secret", "--verbose"],
            "url":"https://example.invalid/mcp?api_key=unique-query-secret"
        }});
        let preview = dry_run_value("gateway", "gateway.add", &params);
        let output = serde_json::to_string(&preview).unwrap();
        for secret in [
            "unique-bearer-secret",
            "unique-password-secret",
            "unique-query-secret",
        ] {
            assert!(!output.contains(secret), "preview leaked a credential");
        }
        assert_eq!(preview["executed"], false);
        assert_eq!(preview["side_effects"], "none_expected");
        assert!(output.contains("test-server"));
        assert_eq!(
            params["spec"]["token"], "unique-bearer-secret",
            "redaction must not mutate dispatch input"
        );
    }
}
