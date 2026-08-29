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
    /// Rotate, roll back, or emergency-revoke inbound JWT signing keys.
    SigningKey(SigningKeyArgs),
}

#[derive(Debug, Args)]
pub struct SigningKeyArgs {
    #[command(subcommand)]
    pub command: SigningKeyCommand,
}

#[derive(Debug, Subcommand)]
pub enum SigningKeyCommand {
    /// Promote a new active key while retaining the prior key for verification.
    Rotate {
        #[arg(long)]
        key_path: PathBuf,
        #[arg(long, default_value_t = 3600)]
        overlap_secs: u64,
    },
    /// Restore the newest retired key and retain the displaced active key.
    Rollback {
        #[arg(long)]
        key_path: PathBuf,
        #[arg(long, default_value_t = 3600)]
        overlap_secs: u64,
    },
    /// Stage replacement of the active key and discard all verification overlap.
    /// A running Labby server must be restarted before revocation is live.
    EmergencyRevoke {
        #[arg(long)]
        key_path: PathBuf,
        #[arg(long)]
        yes: bool,
    },
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
        OauthCommand::SigningKey(args) => run_signing_key(args, format, config),
    }
}

fn run_signing_key(
    args: SigningKeyArgs,
    format: OutputFormat,
    config: &LabConfig,
) -> Result<ExitCode> {
    let maximum_ttl = config
        .auth
        .as_ref()
        .and_then(|auth| auth.access_token_ttl_secs)
        .unwrap_or(3600);
    let mode;
    let keys = match args.command {
        SigningKeyCommand::Rotate {
            key_path,
            overlap_secs,
        } => {
            mode = "rotate";
            labby_auth::jwt::SigningKeys::rotate_with_minimum(
                &key_path,
                Duration::from_secs(overlap_secs),
                Duration::from_secs(maximum_ttl),
            )?
        }
        SigningKeyCommand::Rollback {
            key_path,
            overlap_secs,
        } => {
            mode = "rollback";
            labby_auth::jwt::SigningKeys::rollback_with_minimum(
                &key_path,
                Duration::from_secs(overlap_secs),
                Duration::from_secs(maximum_ttl),
            )?
        }
        SigningKeyCommand::EmergencyRevoke { key_path, yes } => {
            if !yes {
                anyhow::bail!(
                    "emergency revocation requires --yes because it stages invalidation of every outstanding access token and requires an immediate Labby restart"
                );
            }
            mode = "emergency_revoke";
            labby_auth::jwt::SigningKeys::emergency_revoke(&key_path)?
        }
    };
    if format.is_json() {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "mode": mode,
                "active_kid": keys.key_id,
                "published_keys": keys.jwks().keys.len(),
                "restart_required": true,
                "live_revocation_complete": false
            }))?
        );
    } else {
        println!(
            "signing-key operation {mode} staged on disk: active key {} ({} JWKS keys published); restart Labby immediately before treating the operation as live",
            keys.key_id,
            keys.jwks().keys.len()
        );
    }
    Ok(ExitCode::SUCCESS)
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
            let entry = PublicRelayEntry::new(
                MachineId::parse(&machine).context("parse machine id")?,
                target_url,
                description,
                disabled,
            );
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
            "devhost",
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
                assert_eq!(machine, "devhost");
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
            "devhost",
            "--target-url",
            "http://100.99.0.1:38935/callback/devhost",
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
                assert_eq!(machine, "devhost");
                assert_eq!(target_url, "http://100.99.0.1:38935/callback/devhost");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn confirm_destructive_relay_action_with_yes_skips_the_prompt() {
        // `yes=true` must return `Ok(())` immediately without consulting
        // stdin/TTY state at all -- this is the fast, no-prompt path used by
        // `-y`/`--yes` and any non-interactive automation.
        let result = confirm_destructive_relay_action("relay-registry remove", "devhost", true);
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn confirm_destructive_relay_action_without_yes_bails_on_non_interactive_stdin() {
        // cargo test's stdin is never a TTY, so `yes=false` must hit the
        // non-interactive-stdin branch and bail with a message telling the
        // caller to pass `-y`, rather than hanging on `Confirm::interact()`.
        let result = confirm_destructive_relay_action("relay-registry import", "3 entries", false);
        let error = result.expect_err("expected non-interactive stdin to bail");
        let message = error.to_string();
        assert!(
            message.contains("-y") || message.contains("--yes"),
            "expected message to mention -y/--yes, got: {message}"
        );
        assert!(
            message.contains("relay-registry import"),
            "expected message to mention the action, got: {message}"
        );
        assert!(
            message.contains("3 entries"),
            "expected message to mention the detail, got: {message}"
        );
    }

    #[test]
    fn signing_key_lifecycle_commands_parse_explicit_safety_arguments() {
        for args in [
            vec![
                "labby",
                "oauth",
                "signing-key",
                "rotate",
                "--key-path",
                "/tmp/key",
                "--overlap-secs",
                "7200",
            ],
            vec![
                "labby",
                "oauth",
                "signing-key",
                "rollback",
                "--key-path",
                "/tmp/key",
                "--overlap-secs",
                "7200",
            ],
            vec![
                "labby",
                "oauth",
                "signing-key",
                "emergency-revoke",
                "--key-path",
                "/tmp/key",
                "--yes",
            ],
        ] {
            let cli = Cli::try_parse_from(args).expect("signing-key command should parse");
            assert!(matches!(cli.command, crate::cli::Command::Oauth(_)));
        }
    }

    #[test]
    fn signing_key_cli_executes_rotate_rejects_short_overlap_rolls_back_and_revokes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth-jwt.pem");
        let mut config = LabConfig {
            auth: Some(Default::default()),
            ..Default::default()
        };
        config.auth.as_mut().unwrap().access_token_ttl_secs = Some(3600);
        let format = OutputFormat::from_json_flag(
            false,
            crate::output::ColorPolicy::Plain,
            crate::output::RenderEnv::stdout(),
        );

        run_signing_key(
            SigningKeyArgs {
                command: SigningKeyCommand::Rotate {
                    key_path: path.clone(),
                    overlap_secs: 3600,
                },
            },
            format,
            &config,
        )
        .unwrap();
        let rotated = std::fs::read(&path).unwrap();
        assert!(
            run_signing_key(
                SigningKeyArgs {
                    command: SigningKeyCommand::Rotate {
                        key_path: path.clone(),
                        overlap_secs: 3599
                    }
                },
                format,
                &config
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), rotated);
        run_signing_key(
            SigningKeyArgs {
                command: SigningKeyCommand::Rollback {
                    key_path: path.clone(),
                    overlap_secs: 3600,
                },
            },
            format,
            &config,
        )
        .unwrap();
        assert!(
            run_signing_key(
                SigningKeyArgs {
                    command: SigningKeyCommand::EmergencyRevoke {
                        key_path: path,
                        yes: false
                    }
                },
                format,
                &config
            )
            .is_err()
        );
        run_signing_key(
            SigningKeyArgs {
                command: SigningKeyCommand::EmergencyRevoke {
                    key_path: dir.path().join("auth-jwt.pem"),
                    yes: true,
                },
            },
            format,
            &config,
        )
        .unwrap();
    }
}
