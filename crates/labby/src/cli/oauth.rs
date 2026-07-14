use std::io::IsTerminal;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{ArgGroup, Args, Subcommand};
use dialoguer::Confirm;
use serde_json::json;

use crate::config::LabConfig;
use crate::oauth::local_relay::{LocalRelayConfig, run_local_relay};
use crate::oauth::public_relay::{
    MachineId, MutationReport, PublicRelayEntry, PublicRelayRegistryManager,
    PublicRelayRegistryStore,
};
use crate::oauth::target::{resolve_explicit_target, resolve_machine_target};
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct OauthArgs {
    #[command(subcommand)]
    pub command: OauthCommand,
}

#[derive(Debug, Subcommand)]
pub enum OauthCommand {
    /// Run a local OAuth callback relay that forwards to a machine or explicit target.
    RelayLocal(RelayLocalArgs),
    /// Manage the public OAuth callback relay sidecar registry.
    RelayRegistry(RelayRegistryArgs),
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("target")
        .required(true)
        .multiple(false)
        .args(["machine", "forward_base"])
))]
pub struct RelayLocalArgs {
    #[arg(long)]
    pub machine: Option<String>,
    #[arg(long)]
    pub forward_base: Option<String>,
    #[arg(long)]
    pub port: u16,
}

#[derive(Debug, Args)]
pub struct RelayRegistryArgs {
    #[command(subcommand)]
    pub command: RelayRegistryCommand,
}

#[derive(Debug, Subcommand)]
pub enum RelayRegistryCommand {
    /// List registered public callback relay machines.
    List,
    /// Import a standalone callback-relay registry JSON file.
    ///
    /// Destructive: replaces the entire sidecar registry. Requires `-y` /
    /// `--yes` when stdin is not a TTY; otherwise prompts for confirmation.
    Import {
        #[arg(long)]
        file: PathBuf,
        /// Skip confirmation for this destructive action.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Register or update a public callback relay machine.
    Register {
        #[arg(long)]
        machine: String,
        #[arg(long)]
        target_url: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, default_value_t = false)]
        disabled: bool,
    },
    /// Remove a public callback relay machine.
    ///
    /// Destructive: deletes the entry. Requires `-y` / `--yes` when stdin is
    /// not a TTY; otherwise prompts for confirmation.
    Remove {
        #[arg(long)]
        machine: String,
        /// Skip confirmation for this destructive action.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Disable a public callback relay machine without removing it.
    Disable {
        #[arg(long)]
        machine: String,
    },
    /// Enable a public callback relay machine.
    Enable {
        #[arg(long)]
        machine: String,
    },
}

pub async fn run(args: OauthArgs, format: OutputFormat, config: &LabConfig) -> Result<ExitCode> {
    match args.command {
        OauthCommand::RelayLocal(args) => run_relay_local(args, config).await,
        OauthCommand::RelayRegistry(args) => run_relay_registry(args, format).await,
    }
}

async fn run_relay_local(args: RelayLocalArgs, config: &LabConfig) -> Result<ExitCode> {
    let resolved_target = match (&args.machine, &args.forward_base) {
        (Some(machine_id), None) => resolve_machine_target(&config.oauth.machines, machine_id)
            .with_context(|| format!("resolve oauth relay machine `{machine_id}`"))?,
        (None, Some(forward_base)) => resolve_explicit_target(forward_base, Some(args.port))
            .context("resolve explicit oauth relay target")?,
        _ => anyhow::bail!("exactly one of --machine or --forward-base is required"),
    };

    run_local_relay(LocalRelayConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.port),
        resolved_target,
        request_timeout: Duration::from_secs(10),
    })
    .await?;

    Ok(ExitCode::SUCCESS)
}

async fn run_relay_registry(args: RelayRegistryArgs, format: OutputFormat) -> Result<ExitCode> {
    match args.command {
        RelayRegistryCommand::List => {
            let manager = load_registry_manager().await?;
            crate::output::print(&json!({ "machines": manager.list().await }), format)?;
        }
        RelayRegistryCommand::Import { file, yes } => {
            let raw = tokio::fs::read_to_string(&file)
                .await
                .with_context(|| format!("read relay registry import file {}", file.display()))?;
            let report = PublicRelayRegistryStore::parse_standalone_registry(&raw)
                .context("parse relay registry import")?;
            report.ensure_complete_import()?;
            confirm_destructive_relay_action(
                "relay-registry import",
                "this replaces the entire public callback relay registry",
                yes,
            )?;
            let store = default_store();
            let outcome = store
                .save_entries(report.entries)
                .await
                .context("write relay registry")?;
            crate::output::print(
                &json!({
                    "report": {
                        "accepted": report.accepted,
                        "quarantined": report.quarantined,
                    },
                    "restart_required": true,
                    "outcome": outcome,
                }),
                format,
            )?;
        }
        RelayRegistryCommand::Register {
            machine,
            target_url,
            description,
            disabled,
        } => {
            let manager = load_registry_manager().await?;
            let entry = PublicRelayEntry {
                machine_id: MachineId::parse(&machine).context("parse machine id")?,
                target_url,
                description,
                disabled,
            };
            let outcome = manager
                .upsert(entry)
                .await
                .context("write relay registry")?;
            crate::output::print(
                &MutationReport {
                    restart_required: true,
                    outcome,
                },
                format,
            )?;
        }
        RelayRegistryCommand::Remove { machine, yes } => {
            confirm_destructive_relay_action(
                "relay-registry remove",
                &format!(
                    "this removes machine `{machine}` from the public callback relay registry"
                ),
                yes,
            )?;
            let manager = load_registry_manager().await?;
            let machine = MachineId::parse(&machine).context("parse machine id")?;
            let outcome = manager
                .remove(&machine)
                .await
                .context("write relay registry")?;
            crate::output::print(
                &MutationReport {
                    restart_required: true,
                    outcome,
                },
                format,
            )?;
        }
        RelayRegistryCommand::Disable { machine } => {
            set_relay_registry_disabled(machine, true, format).await?;
        }
        RelayRegistryCommand::Enable { machine } => {
            set_relay_registry_disabled(machine, false, format).await?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

async fn set_relay_registry_disabled(
    machine: String,
    disabled: bool,
    format: OutputFormat,
) -> Result<()> {
    let manager = load_registry_manager().await?;
    let machine = MachineId::parse(&machine).context("parse machine id")?;
    let outcome = manager
        .set_disabled(&machine, disabled)
        .await
        .context("write relay registry")?;
    crate::output::print(
        &MutationReport {
            restart_required: true,
            outcome,
        },
        format,
    )?;
    Ok(())
}

async fn load_registry_manager() -> Result<PublicRelayRegistryManager> {
    PublicRelayRegistryManager::load(default_store())
        .await
        .context("load public relay registry")
}

fn default_store() -> PublicRelayRegistryStore {
    PublicRelayRegistryStore::new(PublicRelayRegistryStore::default_path())
}

/// Confirm a destructive `relay-registry` mutation.
///
/// `relay-registry import` (whole-registry replace) and `remove` (entry
/// delete) are hand-rolled CLI subcommands outside the `ActionSpec`-driven
/// dispatch layer, so they don't get `run_confirmable_action_command`'s
/// automatic destructive gate for free. This mirrors that gate directly:
/// `-y`/`--yes` skips the prompt, a non-TTY stdin without `-y` refuses with a
/// clear message, and an interactive TTY prompts for confirmation.
fn confirm_destructive_relay_action(action: &str, detail: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        tracing::warn!(
            surface = "cli",
            service = "oauth_relay",
            action,
            "destructive action blocked: non-interactive stdin, pass -y"
        );
        anyhow::bail!("pass -y / --yes to confirm destructive action `{action}` ({detail})");
    }
    let confirmed = Confirm::new()
        .with_prompt(format!(
            "oauth {action} is destructive ({detail}). Continue?"
        ))
        .default(false)
        .interact()
        .map_err(|e| anyhow::anyhow!("failed to read confirmation: {e}"))?;
    if !confirmed {
        tracing::info!(
            surface = "cli",
            service = "oauth_relay",
            action,
            "destructive action aborted by user"
        );
        anyhow::bail!("aborted by user");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    use crate::cli::Cli;

    #[test]
    fn oauth_relay_local_cli_parses_machine_target() {
        Cli::command().debug_assert();

        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-local",
            "--machine",
            "node-a",
            "--port",
            "38935",
        ])
        .expect("machine target should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayLocal(RelayLocalArgs {
                        machine,
                        forward_base,
                        port,
                    }),
            }) => {
                assert_eq!(machine.as_deref(), Some("node-a"));
                assert!(forward_base.is_none());
                assert_eq!(port, 38935);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_local_cli_parses_explicit_target() {
        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-local",
            "--forward-base",
            "http://100.64.0.10:38935/callback/node-a",
            "--port",
            "38935",
        ])
        .expect("explicit target should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayLocal(RelayLocalArgs {
                        machine,
                        forward_base,
                        port,
                    }),
            }) => {
                assert!(machine.is_none());
                assert_eq!(
                    forward_base.as_deref(),
                    Some("http://100.64.0.10:38935/callback/node-a")
                );
                assert_eq!(port, 38935);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_local_cli_rejects_both_target_flags() {
        let result = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-local",
            "--machine",
            "node-a",
            "--forward-base",
            "http://100.64.0.10:38935/callback/node-a",
            "--port",
            "38935",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn oauth_relay_local_cli_resolves_explicit_target() {
        let resolved =
            resolve_explicit_target("http://100.64.0.10:38935/callback/node-a", Some(38935))
                .expect("explicit target should resolve");

        assert_eq!(resolved.machine_id, None);
        assert_eq!(
            resolved.target_url.as_str(),
            "http://100.64.0.10:38935/callback/node-a"
        );
        assert_eq!(resolved.default_port, Some(38935));
    }

    #[test]
    fn oauth_relay_registry_cli_parses_import() {
        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-registry",
            "import",
            "--file",
            "/tmp/registry.json",
        ])
        .expect("relay registry import should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayRegistry(RelayRegistryArgs {
                        command: RelayRegistryCommand::Import { file, yes },
                    }),
            }) => {
                assert_eq!(file, PathBuf::from("/tmp/registry.json"));
                assert!(!yes, "--yes should default to false");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_registry_cli_parses_import_yes_flag() {
        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-registry",
            "import",
            "--file",
            "/tmp/registry.json",
            "--yes",
        ])
        .expect("relay registry import with --yes should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayRegistry(RelayRegistryArgs {
                        command: RelayRegistryCommand::Import { yes, .. },
                    }),
            }) => {
                assert!(yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_registry_cli_parses_remove_yes_flag() {
        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-registry",
            "remove",
            "--machine",
            "dookie",
            "-y",
        ])
        .expect("relay registry remove with -y should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayRegistry(RelayRegistryArgs {
                        command: RelayRegistryCommand::Remove { machine, yes },
                    }),
            }) => {
                assert_eq!(machine, "dookie");
                assert!(yes);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn oauth_relay_registry_cli_parses_register() {
        let cli = Cli::try_parse_from([
            "lab",
            "oauth",
            "relay-registry",
            "register",
            "--machine",
            "dookie",
            "--target-url",
            "http://100.88.16.79:38935/callback/dookie",
        ])
        .expect("relay registry register should parse");

        match cli.command {
            crate::cli::Command::Oauth(OauthArgs {
                command:
                    OauthCommand::RelayRegistry(RelayRegistryArgs {
                        command:
                            RelayRegistryCommand::Register {
                                machine,
                                target_url,
                                ..
                            },
                    }),
            }) => {
                assert_eq!(machine, "dookie");
                assert_eq!(target_url, "http://100.88.16.79:38935/callback/dookie");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
