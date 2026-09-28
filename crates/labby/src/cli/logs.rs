//! Bounded local process-log queries and explicitly selected deployment journals.

use crate::output::OutputFormat;
use anyhow::{Context as _, Result};
use clap::{Args, Subcommand};
use serde_json::json;
use std::process::ExitCode;

/// Read local rolling logs without following or probing other machines.
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct LogsArgs {
    #[command(flatten)]
    pub query: LogQueryArgs,
    #[command(subcommand)]
    pub command: Option<LogsCommand>,
}

#[derive(Debug, Subcommand)]
pub enum LogsCommand {
    /// Read the systemd deployment journal. Select --follow to stream; no rolling-file fallback.
    Journal(JournalArgs),
}

#[derive(Debug, Args)]
pub struct LogQueryArgs {
    /// Maximum local log entries to return, from 1 through 1000.
    #[arg(short = 'n', long, default_value_t = 200, value_parser = clap::value_parser!(u16).range(1..=1000))]
    pub lines: u16,
    /// Filter by severity.
    #[arg(long, value_parser = ["error", "warn", "info", "debug", "trace"], ignore_case = true)]
    pub level: Option<String>,
    /// Filter structured service names.
    #[arg(long)]
    pub service: Option<String>,
    /// Filter structured action names.
    #[arg(long)]
    pub action: Option<String>,
    /// Match text or a request ID across redacted fields.
    #[arg(long)]
    pub query: Option<String>,
    /// Restrict results to a matching rolling-log filename.
    #[arg(long)]
    pub file: Option<String>,
}

#[derive(Debug, Args)]
pub struct JournalArgs {
    /// Maximum historical journal lines to print.
    #[arg(short = 'n', long, default_value_t = 200, value_parser = clap::value_parser!(u16).range(1..=1000))]
    pub lines: u16,
    /// Stream new journal entries until interrupted. The default is a finite result.
    #[arg(short = 'f', long)]
    pub follow: bool,
    /// Select an Incus deployment explicitly; otherwise use existing deployment detection.
    #[arg(long)]
    pub container: Option<String>,
}

pub async fn run(args: LogsArgs, format: OutputFormat) -> Result<ExitCode> {
    if let Some(LogsCommand::Journal(args)) = args.command {
        if format.is_json() {
            return Err(crate::dispatch::error::ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "The journal adapter forwards raw deployment output and cannot produce the rolling-log JSON schema. Use `labby logs --json` for bounded structured logs, or omit --json for `labby logs journal`. No journal process was started.".to_string(),
            }.into());
        }
        crate::dispatch::server_logs::tail_service_journal(usize::from(args.lines), args.follow, args.container)
            .context("Cannot read the selected deployment journal. This mode requires journalctl on the host or inside the selected Incus container. Check `labby host service status` and container availability. Use `labby logs` for local rolling logs; no automatic source fallback occurred.")?;
        return Ok(ExitCode::SUCCESS);
    }
    super::helpers::run_action_command(
        "server_logs", "server_logs.query".to_string(),
        json!({"limit":args.query.lines,"level":args.query.level,"service":args.query.service,"action":args.query.action,"query":args.query.query,"file":args.query.file}),
        format, |action, params| async move { crate::dispatch::server_logs::dispatch(&action, params).await },
    ).await
}

#[cfg(test)]
mod tests {
    use super::LogsCommand;
    use crate::cli::{Cli, Command};
    use clap::Parser;

    #[test]
    fn logs_default_to_finite_local_queries() {
        let cli = Cli::try_parse_from(["labby", "logs"]).unwrap();
        let Command::Logs(args) = cli.command else {
            panic!("logs command")
        };
        assert_eq!(args.query.lines, 200);
        assert!(args.command.is_none());
    }

    #[test]
    fn journal_following_is_explicit_and_query_filters_do_not_leak_across_sources() {
        let cli = Cli::try_parse_from(["labby", "logs", "journal", "--follow"]).unwrap();
        let Command::Logs(args) = cli.command else {
            panic!("logs command")
        };
        let Some(LogsCommand::Journal(args)) = args.command else {
            panic!("journal command")
        };
        assert!(args.follow);
        assert!(Cli::try_parse_from(["labby", "logs", "--level", "error", "journal"]).is_err());
        assert!(Cli::try_parse_from(["labby", "logs", "--lines", "0"]).is_err());
        assert!(Cli::try_parse_from(["labby", "logs", "--lines", "1001"]).is_err());
    }
}
