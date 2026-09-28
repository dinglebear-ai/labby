//! Resource-oriented adapters for the existing gateway operation handlers.

use super::gateway::*;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ServerArgs {
    #[command(subcommand)]
    pub command: ServerCommand,
}

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// List configured upstream servers and their reported state.
    List,
    /// Get one configured upstream server.
    Get(GatewayGetArgs),
    /// Add an upstream server and reconcile runtime state.
    Add(GatewayAddArgs),
    /// Patch only the supplied fields of an existing upstream server.
    Set(GatewayUpdateArgs),
    /// Remove an upstream server and reconcile runtime state.
    Remove(GatewayRemoveArgs),
    /// Actively test a configured upstream without saving changes.
    Test(GatewayTestArgs),
    /// Report all upstream runtime states, discovery counts, and stale processes.
    Status,
    /// Enable an upstream for new sessions.
    Enable(super::server_lifecycle::ToggleArgs),
    /// Disable an upstream, optionally cleaning up its processes.
    Disable(super::server_lifecycle::ToggleArgs),
    /// Reconnect an enabled upstream and clean up its old runtime.
    Restart(super::server_lifecycle::RestartArgs),
    /// Preview or clean up processes associated with one upstream.
    Cleanup(GatewayMcpCleanupArgs),
    /// Authenticate one upstream, not the operator's Labby account.
    Auth(ServerAuthArgs),
    /// Discover MCP configurations from installed editors without importing them.
    Discover(GatewayDiscoverArgs),
    /// Import discovered upstreams, disabled by default.
    Import(GatewayImportArgs),
    /// Approve or reject discovered servers.
    Pending(GatewayPendingArgs),
    /// Inspect or restore quarantined virtual servers.
    Quarantine(GatewayQuarantineArgs),
}

#[derive(Debug, Args)]
pub struct ServerAuthArgs {
    #[command(subcommand)]
    pub command: ServerAuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum ServerAuthCommand {
    /// Begin upstream OAuth and open the authorization URL unless --no-browser is set.
    Login(ServerLoginArgs),
    /// Read the shared gateway credential status for one upstream.
    Status(GatewayGetArgs),
    /// Clear one dedicated upstream credential. Shared provider revocation is under auth provider.
    Logout(GatewayGetArgs),
}

#[derive(Debug, Args)]
pub struct ServerLoginArgs {
    pub name: String,
    /// Print the authorization URL instead of launching a browser.
    #[arg(long)]
    pub no_browser: bool,
    /// Wait for OAuth completion.
    #[arg(long)]
    pub wait: bool,
    /// Bound OAuth completion with explicit units, such as 120s or 2m.
    #[arg(long = "timeout", default_value = "120s", value_parser = crate::cli::duration::seconds)]
    pub wait_timeout_secs: u64,
}

impl ServerArgs {
    pub fn operation(self) -> GatewayCommand {
        let lifecycle = |command| GatewayCommand::Mcp(GatewayMcpArgs { command });
        match self.command {
            ServerCommand::List => GatewayCommand::List,
            ServerCommand::Get(args) => GatewayCommand::Get(args),
            ServerCommand::Add(args) => GatewayCommand::Add(args),
            ServerCommand::Set(args) => GatewayCommand::Update(args),
            ServerCommand::Remove(args) => GatewayCommand::Remove(args),
            ServerCommand::Test(args) => GatewayCommand::Test(args),
            ServerCommand::Status => lifecycle(GatewayMcpCommand::List),
            ServerCommand::Enable(args) => {
                GatewayCommand::Lifecycle(super::server_lifecycle::Request::Enable(args))
            }
            ServerCommand::Disable(args) => {
                GatewayCommand::Lifecycle(super::server_lifecycle::Request::Disable(args))
            }
            ServerCommand::Restart(args) => {
                GatewayCommand::Lifecycle(super::server_lifecycle::Request::Restart(args))
            }
            ServerCommand::Cleanup(args) => lifecycle(GatewayMcpCommand::Cleanup(args)),
            ServerCommand::Discover(args) => GatewayCommand::Discover(args),
            ServerCommand::Import(args) => GatewayCommand::Import(args),
            ServerCommand::Pending(args) => GatewayCommand::Pending(args),
            ServerCommand::Quarantine(args) => GatewayCommand::Quarantine(args),
            ServerCommand::Auth(args) => {
                let command = match args.command {
                    ServerAuthCommand::Login(args) => {
                        let no_browser = args.no_browser;
                        let args = GatewayOauthUpstreamArgs {
                            name: args.name,
                            open: !no_browser,
                            wait: args.wait,
                            wait_timeout_secs: args.wait_timeout_secs,
                        };
                        if no_browser {
                            GatewayMcpAuthCommand::Start(args)
                        } else {
                            GatewayMcpAuthCommand::Open(args)
                        }
                    }
                    ServerAuthCommand::Status(args) => {
                        GatewayMcpAuthCommand::Status(auth_name(args))
                    }
                    ServerAuthCommand::Logout(args) => {
                        GatewayMcpAuthCommand::Clear(auth_name(args))
                    }
                };
                lifecycle(GatewayMcpCommand::Auth(GatewayMcpAuthArgs { command }))
            }
        }
    }
}

fn auth_name(args: GatewayGetArgs) -> GatewayOauthUpstreamArgs {
    GatewayOauthUpstreamArgs {
        name: args.name,
        open: false,
        wait: false,
        wait_timeout_secs: 120,
    }
}

#[derive(Debug, Args)]
pub struct CodeArgs {
    #[command(subcommand)]
    pub command: CodeCommand,
}

#[derive(Debug, Subcommand)]
pub enum CodeCommand {
    #[command(flatten)]
    Core(GatewayCodeCommand),
    /// Preview and approve upstream metadata hints.
    Hints(HintArgs),
}

impl CodeArgs {
    pub fn operation(self) -> GatewayCommand {
        match self.command {
            CodeCommand::Core(command) => GatewayCommand::Code(GatewayCodeArgs { command }),
            CodeCommand::Hints(args) => GatewayCommand::Enrich(match args.command {
                HintCommand::Preview(args) => args,
                HintCommand::Apply(args) => GatewayEnrichArgs {
                    command: Some(GatewayEnrichCommand::Apply(args)),
                    upstreams: Vec::new(),
                    all: false,
                    provider: "deterministic".to_string(),
                    max_upstreams: None,
                    timeout_ms: None,
                    yes: false,
                },
            }),
        }
    }
}

#[derive(Debug, Args)]
pub struct HintArgs {
    #[command(subcommand)]
    pub command: HintCommand,
}

#[derive(Debug, Subcommand)]
pub enum HintCommand {
    /// Generate and preview metadata suggestions before applying them.
    Preview(GatewayEnrichArgs),
    /// Apply an explicitly approved, hash-bound metadata suggestion.
    Apply(GatewayEnrichApplyArgs),
}
