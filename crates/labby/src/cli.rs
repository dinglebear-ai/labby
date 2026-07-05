//! Top-level CLI — clap derive definitions and dispatch router.
//!
//! Every subcommand is a thin shim that parses args, calls into a
//! `lab-apis` client (or a lab-local subsystem), and formats output.
//! See `crates/lab/src/cli/CLAUDE.md` for the rulebook.

pub mod completions;
pub mod docs;
pub mod doctor;
#[cfg(feature = "gateway")]
pub mod gateway;
pub mod health;
pub mod help;
pub mod helpers;
pub mod incus;
#[cfg(feature = "gateway")]
pub mod internal;
pub mod oauth;
pub mod params;
pub mod serve;
pub mod setup;
#[cfg(feature = "gateway")]
pub mod snippets;
pub mod style;
pub mod update;
// [lab-scaffold: cli-modules]

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::LabConfig;
use crate::output::{ColorPolicy, OutputFormat, RenderEnv};

/// `lab` — pluggable homelab CLI + MCP server SDK.
#[derive(Debug, Parser)]
#[command(name = "labby", version, about, long_about = None, styles = style::AURORA_STYLES)]
pub struct Cli {
    /// Emit JSON instead of human-readable tables.
    #[arg(long, global = true)]
    pub json: bool,

    /// Control human-readable CLI styling.
    #[arg(long, global = true, value_enum, default_value_t = ColorPolicy::Auto)]
    pub color: ColorPolicy,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Resolved output format based on the `--json` flag.
    #[must_use]
    pub fn format(&self) -> OutputFormat {
        OutputFormat::from_json_flag(self.json, self.color, RenderEnv::stdout())
    }
}

/// Every top-level subcommand. Service subcommands are added in later
/// plans as each service comes online.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the MCP server (stdio or HTTP transport).
    Serve(serve::ServeArgs),
    /// Start the MCP server over stdio.
    Mcp(serve::McpServeArgs),
    /// Audit configured services and report problems.
    Doctor(doctor::DoctorArgs),
    /// Generate and verify code-owned documentation artifacts.
    Docs(docs::DocsArgs),
    /// Quick reachability check for configured services.
    Health,
    /// Bootstrap the supported Incus Labby gateway container.
    Setup(setup::SetupArgs),
    /// Manage the supported Incus Labby gateway container.
    Incus(incus::IncusArgs),
    /// Update labby from the latest GitHub release.
    Update(update::UpdateArgs),
    /// Generate shell completions.
    Completions(completions::CompletionsArgs),
    /// Manage proxied upstream MCP gateways.
    #[cfg(feature = "gateway")]
    Gateway(gateway::GatewayArgs),
    /// Manage executable Code Mode snippets.
    #[cfg(feature = "gateway")]
    Snippets(snippets::SnippetsArgs),
    /// Run local OAuth callback relay helpers.
    Oauth(oauth::OauthArgs),
    /// Hidden internal process helpers.
    #[cfg(feature = "gateway")]
    #[command(hide = true)]
    Internal(internal::InternalArgs),
    // [lab-scaffold: cli-variants]
}

/// Dispatch a parsed [`Cli`] to the correct handler.
pub async fn dispatch(cli: Cli, config: LabConfig) -> Result<ExitCode> {
    let format = cli.format();
    match cli.command {
        Command::Serve(args) => serve::run(args, &config).await,
        Command::Mcp(args) => serve::run_mcp(args, &config).await,
        Command::Doctor(args) => doctor::run(args, format).await,
        Command::Docs(args) => docs::run(args, format),
        Command::Health => health::run(format).await,
        Command::Setup(args) => setup::run(args, format).await,
        Command::Incus(args) => incus::run(args, format).await,
        Command::Update(args) => update::run(args, format).await,
        Command::Completions(args) => completions::run(&args),
        #[cfg(feature = "gateway")]
        Command::Gateway(args) => gateway::run(args, format, &config).await,
        #[cfg(feature = "gateway")]
        Command::Snippets(args) => snippets::run(args, format, &config).await,
        Command::Oauth(args) => oauth::run(args, &config).await,
        #[cfg(feature = "gateway")]
        Command::Internal(args) => internal::run(args),
        // [lab-scaffold: cli-dispatch]
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn cli_parses_global_color_flag() {
        let cli = Cli::parse_from(["lab", "--color", "plain", "doctor"]);
        assert_eq!(cli.color, ColorPolicy::Plain);
        assert!(matches!(cli.command, Command::Doctor(_)));
    }

    #[test]
    fn cli_defaults_color_policy_to_auto() {
        let cli = Cli::parse_from(["lab", "doctor"]);
        assert_eq!(cli.color, ColorPolicy::Auto);
        assert!(matches!(cli.command, Command::Doctor(_)));
    }

    #[test]
    fn cli_doctor_accepts_auth_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "auth"]);
        assert!(matches!(
            cli.command,
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::Auth)
            })
        ));
    }

    #[test]
    fn cli_doctor_accepts_system_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "system"]);
        assert!(matches!(
            cli.command,
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::System)
            })
        ));
    }

    #[test]
    fn cli_accepts_top_level_incus_sync() {
        let cli = Cli::parse_from(["labby", "incus", "sync"]);
        assert!(matches!(
            cli.command,
            Command::Incus(incus::IncusArgs {
                command: incus::IncusCommand::Sync(_)
            })
        ));
    }

    #[test]
    fn cli_accepts_update_default() {
        let cli = Cli::parse_from(["labby", "update"]);
        assert!(matches!(
            cli.command,
            Command::Update(update::UpdateArgs { .. })
        ));
    }

    #[test]
    fn cli_doctor_accepts_services_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "services"]);
        assert!(matches!(
            cli.command,
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::Services)
            })
        ));
    }

    #[test]
    fn cli_rejects_legacy_install_uninstall_init_stubs() {
        for command in ["install", "uninstall", "init"] {
            let err =
                Cli::try_parse_from(["labby", command]).expect_err("legacy stub must be gone");
            assert!(
                err.to_string().contains("unrecognized subcommand"),
                "{command}: {err}"
            );
        }
    }

    #[test]
    fn replacement_setup_commands_parse() {
        let cli = Cli::try_parse_from(["labby", "setup"]).expect("setup parses");
        assert!(matches!(cli.command, Command::Setup(_)));

        let cli = Cli::try_parse_from(["labby", "setup", "install-plugin", "gateway", "-y"])
            .expect("setup install-plugin parses");
        assert!(matches!(cli.command, Command::Setup(_)));
    }

    #[test]
    fn cli_parses_completions_subcommand() {
        let cli = Cli::parse_from(["labby", "completions", "bash"]);
        assert!(matches!(cli.command, Command::Completions(_)));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn cli_parses_snippets_subcommands() {
        let cli = Cli::parse_from(["labby", "snippets", "list"]);
        assert!(matches!(cli.command, Command::Snippets(_)));

        let cli = Cli::parse_from([
            "labby",
            "snippets",
            "exec",
            "homelab-readonly-pulse",
            "--param",
            "host=node-a",
        ]);
        assert!(matches!(cli.command, Command::Snippets(_)));

        let cli = Cli::parse_from([
            "labby",
            "snippets",
            "create",
            "daily",
            "--file",
            "daily.md",
            "--description",
            "Daily check",
        ]);
        assert!(matches!(cli.command, Command::Snippets(_)));

        let cli = Cli::parse_from(["labby", "snippets", "remove", "daily", "-y"]);
        assert!(matches!(cli.command, Command::Snippets(_)));

        let cli = Cli::parse_from([
            "labby", "snippets", "validate", "daily", "--file", "daily.md",
        ]);
        assert!(matches!(cli.command, Command::Snippets(_)));

        let cli = Cli::parse_from(["labby", "snippets", "test", "daily", "--param", "limit=3"]);
        assert!(matches!(cli.command, Command::Snippets(_)));

        let cli = Cli::parse_from(["labby", "snippets", "test", "--all"]);
        assert!(matches!(cli.command, Command::Snippets(_)));
    }
}
