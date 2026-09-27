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
    /// Test with offline fixtures, or explicitly opt into live upstreams.
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
    /// Offline JSON fixture. Never contacts configured upstreams.
    #[arg(long, conflicts_with_all = ["live", "all", "params"], required_unless_present = "live")]
    pub fixture: Option<PathBuf>,
    /// Explicitly allow the snippet to call real configured upstreams.
    #[arg(long, required_unless_present = "fixture")]
    pub live: bool,
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

pub async fn run(args: SnippetsArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    let needs_upstreams = matches!(&args.command, SnippetsCommand::Exec(_))
        || matches!(&args.command, SnippetsCommand::Test(test) if test.live);
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
                "live": args.live,
                "fixture": args.fixture.map(read_fixture).transpose()?,
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

    let test_failed = std::sync::atomic::AtomicBool::new(false);
    let exit = run_action_command("snippets", action, params, format, |action, params| {
        let test_failed = &test_failed;
        async move {
            let value = crate::dispatch::snippets::dispatch(&action, params).await?;
            if action == "snippets.test"
                && value.get("passed").and_then(serde_json::Value::as_bool) == Some(false)
            {
                test_failed.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(value)
        }
    })
    .await?;
    Ok(if test_failed.load(std::sync::atomic::Ordering::Relaxed) {
        ExitCode::FAILURE
    } else {
        exit
    })
}

fn read_fixture(path: PathBuf) -> Result<serde_json::Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(1_048_577)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        anyhow::bail!("fixture exceeds 1 MiB");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_snippet_body(code: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (code, file) {
        (Some(code), None) => Ok(code),
        (None, Some(path)) => Ok(std::fs::read_to_string(path)?),
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}
