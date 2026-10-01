//! Thin local CLI adapter for external-client registration.
use crate::dispatch::setup::client_registration::{self as operation, ExternalClient};
use crate::output::{OutputFormat, print};
use anyhow::Result;
#[cfg(unix)]
use anyhow::bail;
use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ClientArg {
    Codex,
    ClaudeCode,
}
impl From<ClientArg> for ExternalClient {
    fn from(value: ClientArg) -> Self {
        match value {
            ClientArg::Codex => Self::Codex,
            ClientArg::ClaudeCode => Self::ClaudeCode,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ConnectionArg {
    SavedCli,
    #[value(name = "oauth", alias = "o-auth")]
    OAuth,
}

#[derive(Debug, Args)]
pub struct ClientsArgs {
    #[command(subcommand)]
    pub command: ClientsCommand,
}
#[derive(Debug, Subcommand)]
pub enum ClientsCommand {
    /// Serve stdio MCP using the protected saved connection for exactly this gateway.
    Bridge {
        #[arg(long)]
        gateway_url: String,
        #[arg(long)]
        state_root: std::path::PathBuf,
        #[arg(long, value_enum)]
        client: Option<ClientArg>,
    },
    /// Verify the selected connection and register selected local clients with backups.
    Connect {
        #[arg(long, value_enum, value_delimiter = ',', required = true)]
        clients: Vec<ClientArg>,
        #[arg(long)]
        gateway_url: Option<String>,
        #[arg(long, value_enum, default_value = "saved-cli")]
        connection: ConnectionArg,
        #[arg(long)]
        state_root: Option<std::path::PathBuf>,
    },
    /// Detect supported client executables and configuration files on this computer.
    List,
    /// Review registration using protected saved CLI credentials or client-owned OAuth.
    Plan {
        #[arg(long, value_enum)]
        client: ClientArg,
        #[arg(long)]
        gateway_url: String,
        #[arg(long, value_enum, default_value = "saved-cli")]
        connection: ConnectionArg,
        #[arg(long)]
        state_root: Option<std::path::PathBuf>,
    },
    /// Register Labby using the exact reviewed config version. Client use remains unverified.
    Register {
        #[arg(long, value_enum)]
        client: ClientArg,
        #[arg(long)]
        gateway_url: String,
        #[arg(long)]
        expected_version: String,
        #[arg(long, value_enum, default_value = "saved-cli")]
        connection: ConnectionArg,
        #[arg(long)]
        state_root: Option<std::path::PathBuf>,
    },
}

pub async fn run(args: ClientsArgs, format: OutputFormat) -> Result<()> {
    #[cfg(unix)]
    if nix::unistd::Uid::effective().is_root() {
        bail!("Run client registration as the user who runs Codex or Claude Code, without sudo");
    }
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Current user's home is unavailable"))?;
    match args.command {
        ClientsCommand::Bridge {
            gateway_url,
            state_root,
            client,
        } => {
            crate::dispatch::setup::client_bridge::run_stdio(
                state_root,
                gateway_url,
                client.map(Into::into),
            )
            .await?;
        }
        ClientsCommand::Connect {
            clients,
            gateway_url,
            connection,
            state_root,
        } => {
            let clients = clients.into_iter().map(Into::into).collect();
            let results = match connection {
                ConnectionArg::SavedCli => {
                    let root = selected_root(state_root)?;
                    let (saved, bearer) =
                        crate::dispatch::setup::client_bridge::saved_connection_identity(&root)?;
                    if !bearer {
                        anyhow::bail!(
                            "The saved connection uses OAuth. Select --connection oauth and the gateway MCP URL"
                        );
                    }
                    let gateway = gateway_url.unwrap_or(saved);
                    operation::register_saved_clients(
                        home,
                        root,
                        gateway,
                        clients,
                        std::env::current_exe()?,
                    )
                    .await
                }
                ConnectionArg::OAuth => {
                    let gateway = gateway_url.ok_or_else(|| {
                        anyhow::anyhow!(
                            "OAuth registration requires --gateway-url with the gateway MCP address"
                        )
                    })?;
                    operation::register_oauth_clients(
                        home,
                        selected_root(state_root)?,
                        gateway,
                        clients,
                    )
                    .await
                }
            };
            print(&results, format)?;
            if results.iter().any(|result| {
                result.get("status").and_then(serde_json::Value::as_str) == Some("needs_attention")
            }) {
                anyhow::bail!(
                    "Some selected clients need attention. See the registration results above"
                );
            }
        }
        ClientsCommand::List => print(&operation::detect(&home), format)?,
        ClientsCommand::Plan {
            client,
            gateway_url,
            connection,
            state_root,
        } => {
            let result = match connection {
                ConnectionArg::OAuth => {
                    operation::verified_plan(home, client.into(), gateway_url).await?
                }
                ConnectionArg::SavedCli => {
                    operation::saved_plan(
                        home,
                        selected_root(state_root)?,
                        client.into(),
                        gateway_url,
                        std::env::current_exe()?,
                    )
                    .await?
                }
            };
            print(&result, format)?;
        }
        ClientsCommand::Register {
            client,
            gateway_url,
            expected_version,
            connection,
            state_root,
        } => {
            let result = match connection {
                ConnectionArg::OAuth => {
                    operation::verified_register(home, client.into(), gateway_url, expected_version)
                        .await?
                }
                ConnectionArg::SavedCli => {
                    operation::saved_register(
                        home,
                        selected_root(state_root)?,
                        client.into(),
                        gateway_url,
                        std::env::current_exe()?,
                        expected_version,
                    )
                    .await?
                }
            };
            print(&result, format)?;
        }
    }
    Ok(())
}

fn selected_root(root: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    let paths = match root {
        Some(root) => crate::installation::InstallationPaths::from_root(root)?,
        None => crate::installation::InstallationPaths::resolve()?,
    };
    Ok(paths.root().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[derive(clap::Parser)]
    struct Fixture {
        #[command(flatten)]
        args: ClientsArgs,
    }

    #[test]
    fn connect_accepts_only_supported_selected_clients() {
        let parsed =
            Fixture::try_parse_from(["fixture", "connect", "--clients", "codex,claude-code"])
                .unwrap();
        assert!(
            matches!(parsed.args.command, ClientsCommand::Connect { clients, .. } if clients.len() == 2)
        );
        for spelling in ["oauth", "o-auth"] {
            let parsed = Fixture::try_parse_from([
                "fixture",
                "connect",
                "--clients",
                "codex",
                "--connection",
                spelling,
            ])
            .unwrap();
            assert!(matches!(
                parsed.args.command,
                ClientsCommand::Connect {
                    connection: ConnectionArg::OAuth,
                    ..
                }
            ));
        }
        assert!(Fixture::try_parse_from(["fixture", "connect"]).is_err());
        assert!(
            Fixture::try_parse_from(["fixture", "connect", "--clients", "unsupported"]).is_err()
        );
    }
}
