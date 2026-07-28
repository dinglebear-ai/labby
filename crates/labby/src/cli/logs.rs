//! Deployment-aware `labby logs` command.

use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

/// Tail the systemd journal for the active Labby deployment.
#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Number of historical log lines to print before following.
    #[arg(short = 'n', long, default_value_t = 200)]
    pub lines: usize,
    /// Print the selected lines and exit instead of following the journal.
    #[arg(long)]
    pub no_follow: bool,
    /// Explicit Incus container name. Auto-detects a running Labby container otherwise.
    #[arg(long)]
    pub container: Option<String>,
}

pub async fn run(args: LogsArgs) -> Result<ExitCode> {
    crate::dispatch::server_logs::tail_service_journal(
        args.lines,
        !args.no_follow,
        args.container,
    )?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn parses_logs_defaults() {
        let cli = Cli::try_parse_from(["labby", "logs"]).expect("parse logs");
        let crate::cli::Command::Logs(args) = cli.command else {
            panic!("logs command")
        };
        assert_eq!(args.lines, 200);
        assert!(!args.no_follow);
    }
}
