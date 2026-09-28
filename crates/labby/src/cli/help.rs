//! Offline CLI discovery, derived exclusively from the executable Clap tree.

use std::io::Write as _;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, CommandFactory};
use serde::Serialize;

use crate::dispatch::error::ToolError;
use crate::output::{OutputFormat, print};

#[derive(Debug, Args, Default)]
pub struct HelpArgs {
    /// Command path to explain, for example: server auth.
    pub path: Vec<String>,
    /// List every public command in this subtree without truncation.
    #[arg(long)]
    pub all: bool,
    /// Find commands by name, description, or option help. Does not execute anything.
    #[arg(long, value_name = "QUERY")]
    pub search: Option<String>,
}

/// A machine-readable public command, including its fully qualified usage.
#[derive(Debug, Serialize)]
pub struct CommandHelp {
    pub command: String,
    pub description: String,
    pub usage: String,
    pub help: String,
}

#[derive(Serialize)]
struct HelpDocument {
    schema_version: u8,
    #[serde(flatten)]
    selected: CommandHelp,
    commands: Vec<CommandHelp>,
}

/// Describe the selected command even when it has no children. Both JSON help
/// and the complete inventory use this projection of the built Clap command.
fn describe(command: &mut clap::Command, path: String) -> CommandHelp {
    CommandHelp {
        command: path,
        description: command
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default(),
        usage: command.render_usage().to_string(),
        help: command.render_long_help().to_string(),
    }
}

fn collect(command: &clap::Command, path: &str, recursive: bool, output: &mut Vec<CommandHelp>) {
    for child in command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
    {
        let child_path = format!("{path} {}", child.get_name());
        let mut built = child.clone().bin_name(child_path.clone());
        built.build();
        output.push(describe(&mut built, child_path.clone()));
        if recursive {
            collect(&built, &child_path, true, output);
        }
    }
}

/// Enumerate all public command paths. Hidden implementation helpers are excluded.
pub fn inventory() -> Vec<CommandHelp> {
    let mut command = super::Cli::command().color(clap::ColorChoice::Never);
    command.build();
    let mut output = Vec::new();
    collect(&command, "labby", true, &mut output);
    output
}

/// Render help without configuration, environment files, credentials, or a daemon.
pub fn run(args: HelpArgs, format: OutputFormat) -> Result<ExitCode> {
    let mut command = super::Cli::command().color(clap::ColorChoice::Never);
    command.build();
    let mut path = "labby".to_string();
    for name in &args.path {
        let next = command.get_subcommands()
            .find(|child| child.get_name() == name && !child.is_hide_set())
            .cloned()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: format!("No public command at this help path. Use `{path} --help` to list its commands, or `labby help --search QUERY` to search."),
            })?;
        command = next;
        path.push(' ');
        path.push_str(name);
    }
    command = command.bin_name(path.clone());
    command.build();
    if !args.all && args.search.is_none() && !format.is_json() {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{}", command.render_long_help())?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut commands = Vec::new();
    collect(
        &command,
        &path,
        args.all || args.search.is_some(),
        &mut commands,
    );
    if let Some(query) = &args.search {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".into(),
                message: "Help search requires a non-empty query. Try `labby help --search oauth`."
                    .into(),
            }
            .into());
        }
        commands.retain(|entry| {
            format!("{} {} {}", entry.command, entry.description, entry.help)
                .to_lowercase()
                .contains(&query)
        });
    }
    if format.is_json() {
        print(
            &HelpDocument {
                schema_version: 1,
                selected: describe(&mut command, path),
                commands,
            },
            format,
        )?;
    } else {
        let mut stdout = std::io::stdout().lock();
        if commands.is_empty() {
            writeln!(
                stdout,
                "No matching commands. Use `labby help --all` to see the complete CLI."
            )?;
        } else {
            for entry in commands {
                writeln!(stdout, "{}\n    {}", entry.command, entry.description)?;
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
