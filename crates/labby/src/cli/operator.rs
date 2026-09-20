//! Operator command groups. These adapters reuse the established operation handlers.

use super::{incus, login, oauth, setup, update};
use clap::{Args, Subcommand};

fn setup_action(command: setup::SetupCommand) -> super::Command {
    super::Command::Setup(setup::SetupArgs {
        command: Some(command),
        ..Default::default()
    })
}

/// Operator authentication, separate from individual upstream credentials.
#[derive(Debug, Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Sign in to an explicitly selected Labby server.
    Login(login::LoginArgs),
    /// Inspect the saved OAuth session without refreshing credentials or contacting the server.
    Status,
    /// Remove only the local OAuth session for the selected destination.
    Logout,
    /// Prepare, consume, or recover the existing credential bootstrap workflow.
    Bootstrap(setup::AccessBootstrapArgs),
    /// Approve an existing owner identity link while the gateway is stopped.
    Owner {
        #[command(subcommand)]
        command: OwnerCommand,
    },
    /// Operate OAuth callback relays and their registry.
    Relay(oauth::OauthArgs),
    /// Manage credentials shared across upstreams and dependent grants.
    #[cfg(feature = "gateway")]
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum OwnerCommand {
    /// Approve the protected identity-link manifest. Does not perform a login.
    Link(setup::OwnerLinkPrepareArgs),
}

#[cfg(feature = "gateway")]
#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// Manage the central Google provider credential.
    Google {
        #[command(subcommand)]
        command: GoogleCommand,
    },
}

#[cfg(feature = "gateway")]
#[derive(Debug, Subcommand)]
pub enum GoogleCommand {
    /// Revoke the central credential AND every dependent Labby grant. Requires --confirm.
    Revoke(super::gateway::GatewayOauthRevokeArgs),
}

impl AuthArgs {
    pub fn operation(self) -> super::Command {
        match self.command {
            AuthCommand::Login(args) => super::Command::Login(args),
            AuthCommand::Status => super::Command::Session(super::session::Operation::Status),
            AuthCommand::Logout => super::Command::Session(super::session::Operation::Logout),
            AuthCommand::Bootstrap(args) => {
                setup_action(setup::SetupCommand::AccessBootstrap(args))
            }
            AuthCommand::Owner {
                command: OwnerCommand::Link(args),
            } => setup_action(setup::SetupCommand::OwnerLinkPrepare(args)),
            AuthCommand::Relay(args) => super::Command::Oauth(args),
            #[cfg(feature = "gateway")]
            AuthCommand::Provider {
                command:
                    ProviderCommand::Google {
                        command: GoogleCommand::Revoke(args),
                    },
            } => {
                use super::gateway::*;
                super::Command::Gateway(GatewayArgs {
                    command: GatewayCommand::Mcp(GatewayMcpArgs {
                        command: GatewayMcpCommand::Auth(GatewayMcpAuthArgs {
                            command: GatewayMcpAuthCommand::RevokeGoogle(args),
                        }),
                    }),
                })
            }
        }
    }
}

#[derive(Debug, Args)]
pub struct HostArgs {
    #[command(subcommand)]
    pub command: HostCommand,
}

#[derive(Debug, Subcommand)]
pub enum HostCommand {
    /// Install this binary into the current user's executable directory.
    Install,
    /// Update the host binary and, unless disabled, synchronize its Incus deployment.
    Update(HostUpdateArgs),
    /// Manage the host system service.
    Service(setup::HostServiceArgs),
    /// Manage the Incus installation, binary synchronization, backups, and SSH trust.
    Incus(HostIncusArgs),
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct HostUpdateArgs {
    #[command(flatten)]
    pub options: update::UpdateArgs,
    #[command(subcommand)]
    pub command: Option<HostUpdateCommand>,
}

#[derive(Debug, Subcommand)]
pub enum HostUpdateCommand {
    /// Manage the daily native macOS update schedule.
    Auto(AutoUpdateArgs),
}

#[derive(Debug, Args)]
pub struct AutoUpdateArgs {
    #[command(subcommand)]
    pub command: AutoUpdateCommand,
    /// Preview the schedule change without installing or changing it.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum AutoUpdateCommand {
    /// Enable daily native updates.
    Enable,
    /// Disable daily native updates.
    Disable,
    /// Report the native update schedule.
    Status,
}

#[derive(Debug, Args)]
pub struct HostIncusArgs {
    #[command(subcommand)]
    pub command: HostIncusCommand,
}

#[derive(Debug, Subcommand)]
pub enum HostIncusCommand {
    /// Bootstrap or converge the supported container.
    Setup(Box<incus::IncusSetupArgs>),
    /// Synchronize a binary and optional assets, then verify the runtime.
    Sync(incus::IncusSyncArgs),
    /// Validate or apply the container backup policy.
    Backup(setup::IncusBackupArgs),
    /// Bootstrap or verify container SSH trust.
    Ssh(setup::IncusSshArgs),
}

impl HostArgs {
    pub fn operation(self) -> super::Command {
        match self.command {
            HostCommand::Install => setup_action(setup::SetupCommand::Install),
            HostCommand::Service(args) => setup_action(setup::SetupCommand::HostService(args)),
            HostCommand::Update(mut args) => {
                if let Some(HostUpdateCommand::Auto(auto)) = args.command {
                    args.options.auto_update = Some(
                        match auto.command {
                            AutoUpdateCommand::Enable => "enable",
                            AutoUpdateCommand::Disable => "disable",
                            AutoUpdateCommand::Status => "status",
                        }
                        .to_string(),
                    );
                    args.options.dry_run = auto.dry_run;
                }
                super::Command::Update(args.options)
            }
            HostCommand::Incus(args) => match args.command {
                HostIncusCommand::Setup(args) => super::Command::Incus(incus::IncusArgs {
                    command: incus::IncusCommand::Setup(args),
                }),
                HostIncusCommand::Sync(args) => super::Command::Incus(incus::IncusArgs {
                    command: incus::IncusCommand::Sync(args),
                }),
                HostIncusCommand::Backup(args) => {
                    setup_action(setup::SetupCommand::Incusbackup(args))
                }
                HostIncusCommand::Ssh(args) => setup_action(setup::SetupCommand::IncusSsh(args)),
            },
        }
    }
}

#[derive(Debug, Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub command: PluginCommand,
}

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// List installed plugins.
    List {
        /// Bypass the short in-process cache.
        #[arg(long)]
        force: bool,
    },
    /// Install a service plugin.
    Install(setup::PluginMutationArgs),
    /// Uninstall a service plugin.
    Uninstall(setup::PluginMutationArgs),
    /// Synchronize plugin options into the existing installation environment.
    Sync(setup::PluginSyncArgs),
    /// Export plugin configuration. Output can contain sensitive values.
    Export,
    /// Check plugin connectivity to the selected Labby server.
    Check {
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Run the binary-owned local plugin setup hook.
    Hook {
        #[arg(long)]
        no_repair: bool,
    },
}

impl PluginArgs {
    pub fn operation(self) -> super::Command {
        setup_action(match self.command {
            PluginCommand::List { force } => setup::SetupCommand::InstalledPlugins { force },
            PluginCommand::Install(args) => setup::SetupCommand::InstallPlugin(args),
            PluginCommand::Uninstall(args) => setup::SetupCommand::UninstallPlugin(args),
            PluginCommand::Sync(args) => setup::SetupCommand::PluginSync(args),
            PluginCommand::Export => setup::SetupCommand::PluginExport,
            PluginCommand::Check { server_url } => {
                setup::SetupCommand::PluginConnectivity { server_url }
            }
            PluginCommand::Hook { no_repair } => setup::SetupCommand::PluginHook { no_repair },
        })
    }
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show the redacted host configuration snapshot without applying environment overrides.
    Show,
    /// Validate the host configuration without changing files or starting services.
    Check,
    /// Report joined service configuration, setup draft, and plugin state.
    Status,
    /// Manage the local setup draft.
    Draft(setup::DraftArgs),
    /// Configure defaults for the ephemeral MCP proxy.
    Proxy {
        #[command(subcommand)]
        command: ProxyConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProxyConfigCommand {
    /// Set proxy defaults; --dry-run previews the changes.
    Set(setup::SetupProxyArgs),
}

impl ConfigArgs {
    pub fn operation(self) -> super::Command {
        setup_action(match self.command {
            ConfigCommand::Show => {
                return super::Command::ConfigInspect(super::config_inspect::Operation::Show);
            }
            ConfigCommand::Check => {
                return super::Command::ConfigInspect(super::config_inspect::Operation::Check);
            }
            ConfigCommand::Status => setup::SetupCommand::ServicesStatus,
            ConfigCommand::Draft(args) => setup::SetupCommand::Draft(args),
            ConfigCommand::Proxy {
                command: ProxyConfigCommand::Set(args),
            } => setup::SetupCommand::Proxy(args),
        })
    }
}
