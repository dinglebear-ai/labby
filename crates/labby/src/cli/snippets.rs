use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

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
    /// Execute a snippet and report pass/fail.
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
}

#[derive(Debug, Args)]
pub struct SnippetTestArgs {
    pub name: Option<String>,
    /// Run every listed snippet with default params.
    #[arg(long, conflicts_with = "name", default_value_t = false)]
    pub all: bool,
    /// Input values passed to the snippet as key=value pairs.
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
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

pub async fn run(
    args: SnippetsArgs,
    format: OutputFormat,
    config: &LabConfig,
    team_id: Option<&str>,
) -> Result<ExitCode> {
    let needs_upstreams = matches!(
        args.command,
        SnippetsCommand::Exec(_) | SnippetsCommand::Test(_)
    );
    if needs_upstreams {
        if let Some(live) = crate::live_gateway::detect(config, "cli").await? {
            return run_on_selected_daemon(
                args.command,
                live.with_team_id(team_id.map(str::to_owned)),
                format,
            )
            .await;
        }
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
                "params": crate::cli::params::parse_kv_params(args.params)?,
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
                "params": crate::cli::params::parse_kv_params(args.params)?,
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

    run_action_command(
        "snippets",
        action,
        params,
        format,
        |action, params| async move { crate::dispatch::snippets::dispatch(&action, params).await },
    )
    .await
}

async fn run_on_selected_daemon(
    command: SnippetsCommand,
    live: crate::live_gateway::LiveGateway,
    format: OutputFormat,
) -> Result<ExitCode> {
    match command {
        SnippetsCommand::Exec(args) => {
            let params = crate::cli::params::parse_kv_params(args.params)?;
            let response = execute_remote_snippet(&live, &args.name, params).await?;
            crate::output::print(&response, format)?;
        }
        SnippetsCommand::Test(args) => {
            let params = crate::cli::params::parse_kv_params(args.params)?;
            if args.all {
                let snippets = crate::dispatch::snippets::store::list_snippets(
                    &crate::dispatch::helpers::lab_home(),
                    &crate::dispatch::snippets::store::builtin_snippet_dir(),
                )?;
                let mut results = Vec::with_capacity(snippets.len());
                for snippet in snippets {
                    let result = test_remote_snippet(
                        &live,
                        &snippet.name,
                        Value::Object(Default::default()),
                    )
                    .await;
                    results.push(match result {
                        Ok(value) => value,
                        Err(error) => json!({"name": snippet.name, "passed": false, "error": error.to_string()}),
                    });
                }
                let passed = results.iter().all(|value| value["passed"] == true);
                crate::output::print(&json!({"passed": passed, "results": results}), format)?;
            } else {
                let name = args
                    .name
                    .ok_or_else(|| anyhow::anyhow!("provide a snippet name or --all"))?;
                let result = test_remote_snippet(&live, &name, params).await?;
                crate::output::print(&result, format)?;
            }
        }
        _ => unreachable!("only executable snippet commands select a daemon"),
    }
    Ok(ExitCode::SUCCESS)
}

async fn test_remote_snippet(
    live: &crate::live_gateway::LiveGateway,
    name: &str,
    params: Value,
) -> Result<Value> {
    let response = execute_remote_snippet(live, name, params).await?;
    let passed = remote_snippet_passed(&response);
    Ok(json!({"name": name, "passed": passed, "response": response}))
}

fn remote_snippet_passed(response: &Value) -> bool {
    let calls_passed = response["calls"]
        .as_array()
        .is_some_and(|calls| calls.iter().all(|call| call["ok"] == true));
    let result_passed = response["result"]
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    calls_passed && result_passed
}

async fn execute_remote_snippet(
    live: &crate::live_gateway::LiveGateway,
    name: &str,
    params: Value,
) -> Result<Value> {
    use crate::dispatch::snippets::store::{
        builtin_snippet_dir, code_for_snippet, merge_snippet_input, resolve_snippet,
    };

    let snippet = resolve_snippet(
        &crate::dispatch::helpers::lab_home(),
        &builtin_snippet_dir(),
        name,
    )?;
    let input = merge_snippet_input(&snippet, params)?;
    let code = code_for_snippet(&snippet)?;
    let code = crate::dispatch::snippets::dispatch::wrap_snippet_with_input_bounded(
        &code,
        &input,
        labby_codemode::MAX_SOURCE_BYTES,
    )?;
    if snippet
        .tools
        .as_ref()
        .is_some_and(|tools| tools.as_slice().is_empty())
    {
        anyhow::bail!(
            "snippet `{name}` declares no upstream tools; remote execution cannot preserve that restriction"
        );
    }
    Ok(live
        .call_codemode_tool_scoped(&code, snippet.tools.as_ref().map(|tools| tools.as_slice()))
        .await?)
}

fn read_snippet_body(code: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (code, file) {
        (Some(code), None) => Ok(code),
        (None, Some(path)) => Ok(std::fs::read_to_string(path)?),
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}
