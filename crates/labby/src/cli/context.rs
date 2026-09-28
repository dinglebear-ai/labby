//! Thin CLI adapter for the canonical non-secret connection preference model.

use crate::{
    config::{
        LabConfig,
        cli::{self as preferences, Change, ConnectionContext},
    },
    output::{OutputFormat, print},
};
use clap::{Args, Subcommand};
use serde_json::json;
use std::process::ExitCode;

#[derive(Debug, Args)]
#[command(arg_required_else_help = true)]
pub struct ContextArgs {
    #[command(subcommand)]
    pub command: ContextCommand,
}

#[derive(Debug, Subcommand)]
pub enum ContextCommand {
    /// List saved destinations without connecting to them.
    List,
    /// Read one context, or the selected context when NAME is omitted.
    Get { name: Option<String> },
    /// Save a non-secret destination. Requires --server URL. Does not authenticate.
    Add {
        name: String,
        #[arg(long = "use")]
        select: bool,
    },
    /// Patch the supplied --server or --team-id; leave omitted fields unchanged.
    Set {
        name: String,
        #[arg(long, conflicts_with = "team_id")]
        clear_team: bool,
    },
    /// Select an existing context for subsequent daemon-backed commands.
    Use { name: String },
    /// Remove an inactive context, without revoking any credentials.
    Remove { name: String },
    /// Clear the convenience selection without deleting saved contexts.
    Clear,
}

/// Apply invocation target selectors only to adapters that actually support them.
/// An implicit saved selection never changes host-local operations.
pub fn prepare(cli: &mut super::Cli, config: &mut LabConfig) -> anyhow::Result<()> {
    if matches!(
        cli.command,
        super::Command::Context(_) | super::Command::Help(_) | super::Command::Docs(_)
    ) || matches!(&cli.command,super::Command::Completions(args) if args.metadata_only())
    {
        return Ok(());
    }
    if !supports_target(&cli.command) {
        if cli.server.is_some() || cli.context.is_some() {
            return Err(preferences::invalid("This command operates on the local installation and does not accept --server or --context. No remote or local operation was dispatched.").into());
        }
        return Ok(());
    }
    let environment_selects_target = ["CLAUDE_PLUGIN_OPTION_SERVER_URL", "LABBY_SERVER_URL"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()));
    let selected = preferences::select(
        &config.cli,
        cli.server.as_deref(),
        cli.context.as_deref(),
        environment_selects_target,
    )?;
    if let Some(selected) = &selected {
        if cli.team_id.is_none() {
            cli.team_id = selected.team_id.clone();
        }
        tracing::debug!(surface = "cli", target_source = ?selected.source, "selected an explicit CLI authority; fallback disabled");
    }
    config.cli_target = selected;
    Ok(())
}

fn supports_target(command: &super::Command) -> bool {
    match command {
        super::Command::Completions(args) => !args.metadata_only(),
        super::Command::Mcp(_) | super::Command::Login(_) | super::Command::Session(_) => true,
        #[cfg(feature = "gateway")]
        super::Command::Skill(super::skill::SkillArgs {
            command: super::skill::SkillCommand::Source(_),
        }) => true,
        super::Command::Auth(args) => match &args.command {
            super::operator::AuthCommand::Login(_)
            | super::operator::AuthCommand::Status
            | super::operator::AuthCommand::Logout => true,
            #[cfg(feature = "gateway")]
            super::operator::AuthCommand::Provider { .. } => true,
            _ => false,
        },
        #[cfg(feature = "gateway")]
        super::Command::Gateway(_)
        | super::Command::Server(_)
        | super::Command::Route(_)
        | super::Command::Loadout(_)
        | super::Command::Code(_) => true,
        _ => false,
    }
}

pub async fn run(
    args: ContextArgs,
    server: Option<String>,
    team_id: Option<String>,
    format: OutputFormat,
) -> anyhow::Result<ExitCode> {
    let path = crate::installation::InstallationPaths::resolve()?.config_toml();
    if !matches!(
        args.command,
        ContextCommand::Add { .. } | ContextCommand::Set { .. }
    ) && (server.is_some() || team_id.is_some())
    {
        return Err(preferences::invalid("Only context add and context set accept --server or --team-id values. Context reads never retarget or change authority.").into());
    }
    let change = match args.command {
        ContextCommand::List => {
            let preferences = preferences::read(&path)?;
            preferences.validate()?;
            print(
                &json!({"current_context":preferences.current_context, "contexts":preferences.contexts}),
                format,
            )?;
            return Ok(ExitCode::SUCCESS);
        }
        ContextCommand::Get { name } => {
            let preferences = preferences::read(&path)?;
            let name = name.or(preferences.current_context).ok_or_else(|| {
                preferences::invalid(
                    "No context is selected. Use context get NAME or context use NAME.",
                )
            })?;
            let context = preferences.contexts.get(&name).ok_or_else(|| {
                preferences::invalid(
                    "Context does not exist. Use context list to inspect saved names.",
                )
            })?;
            let context = ConnectionContext::new(&context.server, context.team_id.clone())?;
            print(
                &json!({"name":name, "server":context.server, "team_id":context.team_id}),
                format,
            )?;
            return Ok(ExitCode::SUCCESS);
        }
        ContextCommand::Add { name, select } => Change::Add {
            name,
            select,
            entry: ConnectionContext::new(
                server.as_deref().ok_or_else(|| {
                    preferences::invalid(
                        "A context needs a destination: labby context add NAME --server URL.",
                    )
                })?,
                team_id,
            )?,
        },
        ContextCommand::Set { name, clear_team } => Change::Set {
            name,
            server,
            team_id,
            clear_team,
        },
        ContextCommand::Use { name } => Change::Use(name),
        ContextCommand::Remove { name } => Change::Remove(name),
        ContextCommand::Clear => Change::Clear,
    };
    let (preferences, changed) =
        tokio::task::spawn_blocking(move || preferences::change(&path, change)).await??;
    print(
        &json!({"changed":changed, "current_context":preferences.current_context, "contexts":preferences.contexts}),
        format,
    )?;
    Ok(ExitCode::SUCCESS)
}
