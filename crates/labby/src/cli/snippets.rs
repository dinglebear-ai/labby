use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::cli::helpers::{print_dry_run, run_action_command, run_confirmable_action_command};
use crate::config::LabConfig;
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct SnippetsArgs {
    #[command(subcommand)]
    pub command: SnippetsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SnippetsCommand {
    /// List built-in and user snippets.
    List,
    /// Show one snippet body and metadata.
    Get(SnippetNameArgs),
    /// Execute a snippet through gateway Code Mode.
    #[command(name = "run")]
    Exec(SnippetExecArgs),
    /// Add a user snippet. Existing snippets require explicit --force replacement.
    #[command(name = "add")]
    Create(SnippetCreateArgs),
    /// Validate a snippet without saving or executing it.
    Validate(SnippetValidateArgs),
    /// Remove a user snippet.
    Remove(SnippetRemoveArgs),
    /// Test with an offline fixture, or explicitly opt into live upstream calls.
    Test(SnippetTestArgs),
}

#[derive(Debug, Args)]
pub struct SnippetNameArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct SnippetExecArgs {
    pub name: String,
    /// Input values passed to the snippet as key=value pairs.
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
    /// Supply arbitrary JSON input instead of scalar key=value parameters.
    #[arg(long, value_name = "JSON", conflicts_with = "params")]
    pub input: Option<String>,
}

#[derive(Debug, Args)]
pub struct SnippetTestArgs {
    #[arg(required_unless_present = "all")]
    pub name: Option<String>,
    /// Run every listed snippet against live upstreams with default params.
    #[arg(
        long,
        conflicts_with = "name",
        requires = "live",
        default_value_t = false
    )]
    pub all: bool,
    /// Read an offline JSON fixture. No gateway or upstreams are initialized.
    #[arg(long, requires = "name", conflicts_with_all = ["all", "live"])]
    pub fixture: Option<PathBuf>,
    /// Explicitly allow real upstream calls instead of fixture responses.
    #[arg(long, conflicts_with = "fixture", required_unless_present = "fixture")]
    pub live: bool,
    /// Elapsed-time assertion threshold in milliseconds, not a timeout override.
    #[arg(long, requires = "live")]
    pub max_runtime_ms: Option<u64>,
    /// Attempted-call assertion threshold, not an execution cap; defaults to 40.
    #[arg(long, requires = "live")]
    pub max_calls: Option<u64>,
    /// Maximum serialized live-test result bytes; defaults to 16000.
    #[arg(long, requires = "live")]
    pub max_output_bytes: Option<usize>,
    /// Input values passed to the snippet as key=value pairs.
    #[arg(long = "param", value_name = "KEY=VALUE", conflicts_with = "all")]
    pub params: Vec<String>,
    /// Supply arbitrary JSON input instead of scalar key=value parameters.
    #[arg(long, value_name = "JSON", conflicts_with_all = ["params", "all"])]
    pub input: Option<String>,
}

#[derive(Debug, Args)]
pub struct SnippetCreateArgs {
    pub name: String,
    /// Read snippet body from a file.
    #[arg(long, conflicts_with = "code")]
    pub file: Option<PathBuf>,
    /// Inline snippet body.
    #[arg(long, conflicts_with = "file")]
    pub code: Option<String>,
    /// Human-readable snippet description for generated frontmatter.
    #[arg(long)]
    pub description: Option<String>,
    /// Overwrite an existing user snippet.
    #[arg(long, short = 'f', default_value_t = false)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct SnippetValidateArgs {
    /// Existing snippet name or filename stem for --file/--code validation.
    pub name: String,
    /// Validate snippet body from a file instead of an existing snippet.
    #[arg(long, conflicts_with = "code")]
    pub file: Option<PathBuf>,
    /// Validate inline snippet body instead of an existing snippet.
    #[arg(long, conflicts_with = "file")]
    pub code: Option<String>,
}

#[derive(Debug, Args)]
pub struct SnippetRemoveArgs {
    pub name: String,
    /// Confirm removal without prompting.
    #[arg(short = 'y', long, default_value_t = false)]
    pub yes: bool,
    /// Alias for --yes.
    #[arg(long = "no-confirm", default_value_t = false)]
    pub no_confirm: bool,
    /// Show what would be removed without deleting it.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

pub async fn run(args: SnippetsArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    let needs_upstreams = matches!(
        &args.command,
        SnippetsCommand::Exec(_) | SnippetsCommand::Test(SnippetTestArgs { live: true, .. })
    );
    if needs_upstreams {
        crate::cli::gateway::build_manager(config, true).await?;
    }

    let (action, params, yes, dry_run) = match args.command {
        SnippetsCommand::List => ("snippets.list".to_string(), json!({}), true, false),
        SnippetsCommand::Get(args) => (
            "snippets.get".to_string(),
            json!({ "name": args.name }),
            true,
            false,
        ),
        SnippetsCommand::Exec(args) => (
            "snippets.exec".to_string(),
            json!({
                "name": args.name,
                "params": parse_snippet_input(args.input, args.params)?,
            }),
            true,
            false,
        ),
        SnippetsCommand::Create(args) => (
            "snippets.create".to_string(),
            json!({
                "name": args.name,
                "body": read_snippet_body(args.code, args.file)?,
                "description": args.description,
                "force": args.force,
            }),
            true,
            false,
        ),
        SnippetsCommand::Validate(args) => {
            let body = match (args.code, args.file) {
                (Some(code), None) => Some(code),
                (None, Some(path)) => Some(std::fs::read_to_string(path)?),
                (None, None) => None,
                (Some(_), Some(_)) => unreachable!("clap enforces conflicts_with"),
            };
            (
                "snippets.validate".to_string(),
                json!({
                    "name": args.name,
                    "body": body,
                }),
                true,
                false,
            )
        }
        SnippetsCommand::Remove(args) => (
            "snippets.remove".to_string(),
            json!({ "name": args.name }),
            args.yes || args.no_confirm,
            args.dry_run,
        ),
        SnippetsCommand::Test(args) => (
            "snippets.test".to_string(),
            json!({
                "name": args.name,
                "all": args.all,
                "live": args.live,
                "fixture": args.fixture.map(read_fixture).transpose()?,
                "budgets": {
                    "wall_clock_ms": args.max_runtime_ms.unwrap_or(20000),
                    "tool_calls": args.max_calls.unwrap_or(40),
                    "output_bytes": args.max_output_bytes.unwrap_or(16000),
                },
                "params": parse_snippet_input(args.input, args.params)?,
            }),
            true,
            false,
        ),
    };

    if dry_run {
        print_dry_run("snippets", &action, &params, format)?;
        return Ok(ExitCode::SUCCESS);
    }

    if action == "snippets.remove" {
        return run_confirmable_action_command(
            "snippets",
            crate::dispatch::snippets::ACTIONS,
            action,
            params,
            yes,
            format,
            |action, params| async move {
                crate::dispatch::snippets::dispatch(&action, params).await
            },
        )
        .await;
    }

    let failed = std::sync::atomic::AtomicBool::new(false);
    let failed_ref = &failed;
    let exit = run_action_command(
        "snippets",
        action,
        params,
        format,
        move |action, params| async move {
            let value = crate::dispatch::snippets::dispatch(&action, params).await?;
            if action == "snippets.test"
                && value.get("passed") == Some(&serde_json::Value::Bool(false))
            {
                failed_ref.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(value)
        },
    )
    .await?;
    Ok(if failed.load(std::sync::atomic::Ordering::Relaxed) {
        ExitCode::FAILURE
    } else {
        exit
    })
}

fn read_snippet_body(code: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (code, file) {
        (Some(code), None) => Ok(code),
        (None, Some(path)) => Ok(std::fs::read_to_string(path)?),
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}

fn parse_snippet_input(input: Option<String>, params: Vec<String>) -> Result<serde_json::Value> {
    match input {
        Some(raw) => {
            if !params.is_empty() {
                anyhow::bail!("--input conflicts with --param");
            }
            if raw.len() > labby_codemode::snippet::harness::MAX_FIXTURE_BYTES {
                anyhow::bail!("snippet input exceeds 1 MiB");
            }
            Ok(serde_json::from_str(&raw)?)
        }
        None => crate::cli::params::parse_kv_params(params),
    }
}

fn read_fixture(path: PathBuf) -> Result<serde_json::Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((labby_codemode::snippet::harness::MAX_FIXTURE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > labby_codemode::snippet::harness::MAX_FIXTURE_BYTES {
        anyhow::bail!("fixture exceeds 1 MiB");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: SnippetTestArgs,
    }

    #[test]
    fn tests_require_explicit_offline_or_live_mode() {
        assert!(TestCli::try_parse_from(["test", "demo"]).is_err());
        assert!(TestCli::try_parse_from(["test", "--all"]).is_err());
        assert!(TestCli::try_parse_from(["test", "demo", "--fixture", "case.json"]).is_ok());
        assert!(TestCli::try_parse_from(["test", "demo", "--live"]).is_ok());
        assert!(TestCli::try_parse_from(["test", "--all", "--live"]).is_ok());
        assert!(
            TestCli::try_parse_from(["test", "demo", "--fixture", "case.json", "--live"]).is_err()
        );
    }

    #[test]
    fn json_input_preserves_arrays_objects_and_nulls() {
        let value = parse_snippet_input(
            Some(r#"{"repos":["unraid/core"],"nested":{"value":null}}"#.to_owned()),
            vec![],
        )
        .unwrap();
        assert_eq!(value["repos"], serde_json::json!(["unraid/core"]));
        assert!(value["nested"]["value"].is_null());
    }

    #[test]
    fn json_input_rejects_malformed_oversized_and_conflicting_values() {
        assert!(parse_snippet_input(Some("not json".to_owned()), vec![]).is_err());
        assert!(parse_snippet_input(Some("{}".to_owned()), vec!["a=b".to_owned()]).is_err());
        assert!(parse_snippet_input(Some(" ".repeat(1024 * 1024 + 1)), vec![]).is_err());
        assert!(
            TestCli::try_parse_from([
                "test",
                "demo",
                "--fixture",
                "case.json",
                "--input",
                "{}",
                "--param",
                "x=1"
            ])
            .is_err()
        );
    }
}
