//! `labby setup` — role-aware Labby onboarding and deployment.
//!
//! Bare `labby setup` interactively configures a server or client, with native
//! and Incus server backends where supported. The legacy web configuration flow
//! remains available as `labby setup wizard`.
//!
//! The wizard is a thin CLI shim over the `setup` dispatch service. It detects
//! first-run via `setup.state`, then prints either:
//!
//! - first-run: instructions to start `labby serve` and visit `/setup`, or
//! - re-run: instructions to visit `/settings`.
//!
//! Honors `LABBY_SKIP_SETUP=1` and `--no-setup` for CI / power users.
//!
//! OAuth client onboarding uses the existing browser authorization flow.
//! `--no-browser` requires bearer client authentication instead.

use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::output::theme::CliTheme;
use crate::output::{OutputFormat, print};

mod onboarding;

const DEFAULT_INCUS_SSH_KEY_PATH: &str = "/home/labby/.ssh/id_ed25519";
/// Versioned contract used by installers and setup clients to prove that the
/// selected Labby binary supports the personal-first onboarding flow.
pub const SETUP_CONTRACT_VERSION: u32 = 2;

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Provision this Ubuntu 26.04/Incus box for the Labby gateway.
    #[arg(long)]
    pub provision: bool,

    /// Print the default Incus/provisioning plan and do not mutate anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Confirm provisioning without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,

    /// Skip runtime dependency installation and only converge user/service state.
    #[arg(long)]
    pub skip_deps: bool,

    /// Configure this machine as a Labby server or as a client of another server.
    #[arg(long, value_enum)]
    pub role: Option<SetupRoleArg>,

    /// Server deployment backend. Native is the fastest path; Incus is isolated.
    #[arg(long, value_enum, requires = "role")]
    pub deployment: Option<SetupDeploymentArg>,

    /// Server listen address. Defaults to 127.0.0.1.
    #[arg(long)]
    pub host: Option<String>,

    /// Server listen or published port. Defaults to 8765.
    #[arg(long)]
    pub port: Option<u16>,

    /// Explicit Labby server URL for client mode.
    #[arg(long)]
    pub server_url: Option<String>,

    /// Public browser/OAuth URL for the server.
    #[arg(long)]
    pub public_url: Option<String>,

    /// Authentication provider to configure during setup. Bearer remains available as break-glass auth.
    #[arg(long, value_enum)]
    pub oauth: Option<SetupOauthArg>,

    /// Install the Labby desktop app when a published package is available for this platform.
    #[arg(long, conflicts_with = "no_desktop")]
    pub desktop: bool,

    /// Do not install the Labby desktop app.
    #[arg(long, conflicts_with = "desktop")]
    pub no_desktop: bool,

    /// Internal setup-plan handoff used for privilege elevation.
    #[arg(long, hide = true)]
    pub apply_plan: Option<PathBuf>,

    /// Internal local owner bootstrap used by container onboarding.
    #[arg(long, hide = true)]
    pub bootstrap_static_owner: bool,

    /// Setup UI mode for `labby setup wizard`.
    #[arg(long, value_enum, default_value_t = SetupModeArg::Full, hide = true)]
    pub mode: SetupModeArg,

    /// Skip the wizard and exit cleanly. Equivalent to LABBY_SKIP_SETUP=1.
    #[arg(long, hide = true)]
    pub no_setup: bool,

    /// Do not open a browser. Client setup requires bearer authentication with this flag.
    #[arg(long, hide = true)]
    pub no_browser: bool,

    /// Smoke-test mode: print the state machine snapshot as JSON and exit.
    /// Used by `just smoke-setup` for CI verification.
    #[arg(long, hide = true)]
    pub smoke: bool,

    #[command(subcommand)]
    pub command: Option<SetupCommand>,
}

impl Default for SetupArgs {
    fn default() -> Self {
        Self {
            provision: false,
            dry_run: false,
            yes: false,
            skip_deps: false,
            role: None,
            deployment: None,
            host: None,
            port: None,
            server_url: None,
            public_url: None,
            oauth: None,
            desktop: false,
            no_desktop: false,
            apply_plan: None,
            bootstrap_static_owner: false,
            mode: SetupModeArg::Full,
            no_setup: false,
            no_browser: false,
            smoke: false,
            command: None,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupRoleArg {
    Server,
    Client,
}

#[derive(Debug, Clone, Copy, ValueEnum, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupDeploymentArg {
    Native,
    Incus,
}

#[derive(Debug, Clone, Copy, ValueEnum, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupOauthArg {
    None,
    Google,
    Authelia,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SetupModeArg {
    Plugin,
    Full,
}

impl SetupModeArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// Open the web-based first-run wizard or settings flow.
    Wizard(WizardArgs),
    /// Manage the local setup draft.
    Draft(DraftArgs),
    /// Prepare and operate the local project-credential bootstrap flow.
    AccessBootstrap(AccessBootstrapArgs),
    /// Approve one specific existing-owner identity link while the gateway is stopped.
    OwnerLinkPrepare(OwnerLinkPrepareArgs),
    /// Manage the systemd Labby gateway service.
    HostService(HostServiceArgs),
    /// List installed Claude Code lab plugins.
    InstalledPlugins {
        /// Bypass the short in-process cache.
        #[arg(long)]
        force: bool,
    },
    /// Join service configuration, draft, and Claude plugin state.
    ServicesStatus,
    /// Run binary-owned local setup checks for Claude plugin hooks.
    PluginHook {
        /// Check only; do not create missing local setup files.
        #[arg(long)]
        no_repair: bool,
    },
    /// Sync CLAUDE_PLUGIN_OPTION_* env vars into ~/.labby/.env as LABBY_* vars.
    PluginSync(PluginSyncArgs),
    /// Read ~/.labby/.env and print current values keyed by userConfig field name.
    PluginExport,
    /// Validate connectivity to the lab MCP server.
    PluginConnectivity {
        /// Requested server URL; it must match the active plugin, persisted, or
        /// http://localhost:40100 host-proxy target.
        #[arg(long)]
        server_url: Option<String>,
    },
    /// Resume the personal onboarding flow from the securely staged setup draft.
    Resume,
    /// Print the setup contract version expected by installers and onboarding clients.
    Contract,
    /// Print the exact Google Auth Platform configuration for a Labby public URL.
    GoogleOauth(GoogleOauthGuideArgs),
    /// Print copy/paste-ready Claude Code MCP setup for local or hardened SSH use.
    ClaudeCode(ClaudeCodeGuideArgs),
    /// Check local setup prerequisites without mutating the filesystem.
    Check,
    /// Repair missing local setup prerequisites without contacting external services.
    Repair,
    /// Configure defaults for the ephemeral stdio MCP proxy.
    Proxy(SetupProxyArgs),
    /// Render copy/paste-ready public HTTPS reverse-proxy configuration.
    #[command(name = "public-proxy")]
    PublicProxy(SetupPublicProxyArgs),
    /// Inspect or safely configure Tailscale Funnel for public Browser + ChatGPT access.
    #[command(name = "tailscale-funnel")]
    TailscaleFunnel(SetupTailscaleFunnelArgs),
    /// Validate or apply local Incus backup policy.
    #[command(alias = "incus-backup")]
    Incusbackup(IncusBackupArgs),
    /// Bootstrap container SSH trust from the host ~/.ssh/config.
    IncusSsh(IncusSshArgs),
    /// Copy the labby binary into ~/.local/bin so it is callable in your own terminal.
    Install,
    /// Install the Claude Code plugin for a configured service.
    InstallPlugin(PluginMutationArgs),
    /// Uninstall the Claude Code plugin for a service.
    UninstallPlugin(PluginMutationArgs),
}

#[derive(Debug, Args)]
pub struct GoogleOauthGuideArgs {
    /// Final browser-visible HTTPS origin for Labby, for example https://labby.example.com.
    #[arg(long)]
    pub public_url: String,
    /// Probe the public OAuth/MCP surface after rendering the provider recipe.
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Args)]
pub struct ClaudeCodeGuideArgs {
    /// Name to assign the Claude Code upstream.
    #[arg(long, default_value = "claude-local")]
    pub name: String,

    /// Exact Claude Code executable. Required for remote SSH; auto-detected from PATH for local use.
    #[arg(long)]
    pub claude_path: Option<PathBuf>,

    /// Remote SSH target in user@host form. Omit for Claude Code running on the Labby host.
    #[arg(long)]
    pub ssh_target: Option<String>,

    /// Dedicated SSH private key used by the Labby service account for the remote target.
    #[arg(long, requires = "ssh_target")]
    pub identity_file: Option<PathBuf>,

    /// Dedicated known-hosts file containing the independently verified remote host key.
    #[arg(long, requires = "ssh_target")]
    pub known_hosts_file: Option<PathBuf>,

    /// Validate, persist, and test this Claude Code upstream. Without this flag the command is read-only.
    #[arg(long, conflicts_with = "rollback")]
    pub apply: bool,

    /// Restore the exact upstream definition saved before the last successful --apply for this name.
    #[arg(long, conflicts_with = "apply")]
    pub rollback: bool,

    /// Confirm replacement when an upstream with this name already exists and differs.
    #[arg(short = 'y', long, requires = "apply")]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct OwnerLinkPrepareArgs {
    /// Protected JSON approval manifest binding the verified identity and existing project.
    #[arg(long)]
    pub approval_file: PathBuf,
}

#[derive(Debug, Args)]
pub struct AccessBootstrapArgs {
    #[command(subcommand)]
    pub command: AccessBootstrapCommand,
}

#[derive(Debug, Subcommand)]
pub enum AccessBootstrapCommand {
    /// Securely create a one-time proof bundle and client credential.
    Prepare {
        #[arg(long)]
        proof_file: PathBuf,
        #[arg(long)]
        credential_file: PathBuf,
        #[arg(long)]
        organization_name: String,
        #[arg(long)]
        project_name: String,
        #[arg(long)]
        subject: String,
        #[arg(long)]
        loadout_id: String,
        #[arg(long)]
        route_id: String,
        #[arg(long)]
        resource: String,
        #[arg(long, required = true)]
        scope: Vec<String>,
        #[arg(long, default_value_t = 600)]
        ttl: u64,
    },
    /// Submit the prepared request to the running daemon without local mutation.
    Consume {
        #[arg(long)]
        prepare_id: String,
    },
    /// Ask the running daemon for authoritative prepare status.
    Status {
        #[arg(long)]
        prepare_id: String,
    },
    /// Verify a prepare and report its current recovery state.
    Recover {
        #[arg(long)]
        prepare_id: String,
        #[arg(long, conflicts_with = "revoke")]
        complete: bool,
        #[arg(long, conflicts_with = "complete")]
        revoke: bool,
    },
    /// Offline cleanup; requires the daemon to be stopped and durable tombstones.
    Cleanup {
        #[arg(long)]
        prepare_id: String,
    },
}

#[derive(Debug, Args, Clone, Copy)]
pub struct WizardArgs {
    /// Setup UI mode. Standalone setup defaults to full; /setup-core passes plugin.
    #[arg(long, value_enum, default_value_t = SetupModeArg::Full)]
    pub mode: SetupModeArg,
    /// Skip the wizard and exit cleanly. Equivalent to LABBY_SKIP_SETUP=1.
    #[arg(long)]
    pub no_setup: bool,
    /// Do not attempt to open the browser.
    #[arg(long)]
    pub no_browser: bool,
    /// Smoke-test mode: print the state machine snapshot as JSON and exit.
    #[arg(long)]
    pub smoke: bool,
}

#[derive(Debug, Args)]
pub struct DraftArgs {
    #[command(subcommand)]
    pub command: DraftCommand,
}

#[derive(Debug, Args)]
pub struct HostServiceArgs {
    #[command(subcommand)]
    pub command: HostServiceCommand,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum HostServiceCommand {
    /// Print the system unit that Labby would install.
    Unit,
    /// Install and start labby.service as a system unit.
    Install {
        /// Copy this labby binary into /usr/local/bin/labby before installing the service.
        #[arg(long)]
        install_self: bool,
        /// Confirm installation and service start.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Read labby.service status.
    Status,
    /// Restart labby.service.
    Restart {
        /// Copy this labby binary into /usr/local/bin/labby before restarting the service.
        #[arg(long)]
        install_self: bool,
        /// Confirm service restart.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Restore the verified release retained by the last --install-self upgrade.
    Rollback {
        /// Confirm release rollback.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Stop, disable, and remove labby.service.
    Uninstall {
        /// Confirm service removal.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DraftCommand {
    /// Delete ~/.labby/.env.draft without modifying ~/.labby/.env.
    Discard(DraftDiscardArgs),
}

#[derive(Debug, Args)]
pub struct DraftDiscardArgs {
    /// Confirm discard without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be dispatched without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PluginSyncArgs {
    /// Skip confirmation for this destructive action.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be dispatched without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PluginMutationArgs {
    /// Service name, for example `unifi` or `apprise`.
    pub service: String,
    /// Skip confirmation for destructive actions.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Print what would be dispatched without executing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct SetupProxyArgs {
    /// Exposure mode to persist.
    #[arg(long, value_enum)]
    pub exposure: Option<crate::proxy::config::ProxyExposure>,
    /// Authentication mode to persist.
    #[arg(long, value_enum)]
    pub auth: Option<crate::proxy::config::ProxyAuthMode>,
    /// MCP HTTP path to persist.
    #[arg(long)]
    pub path: Option<String>,
    /// External Tailscale port, or `random`.
    #[arg(long)]
    pub port: Option<String>,
    /// First candidate in the random external-port range.
    #[arg(long)]
    pub port_range_start: Option<u16>,
    /// Last candidate in the random external-port range.
    #[arg(long)]
    pub port_range_end: Option<u16>,
    /// Environment key used for the proxy bearer secret.
    #[arg(long)]
    pub bearer_token_env: Option<String>,
    /// OAuth scope to require; repeatable and replaces the configured list.
    #[arg(long = "oauth-scope")]
    pub oauth_scopes: Vec<String>,
    /// Ambient environment variable inherited by child servers; repeatable.
    #[arg(long = "inherit-env")]
    pub inherit_env: Vec<String>,
    /// Grace period before forced child shutdown.
    #[arg(long)]
    pub shutdown_grace_ms: Option<u64>,
    /// Read a bearer secret from stdin without echoing or persisting it in TOML.
    #[arg(long)]
    pub bearer_token_stdin: bool,
    /// Accept existing values and built-in defaults without prompting.
    #[arg(short = 'y', long, alias = "no-confirm")]
    pub yes: bool,
    /// Preview exact file changes without mutating config or secret files.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SetupPublicProxyFormat {
    Caddy,
    Nginx,
    Traefik,
    All,
}

impl SetupPublicProxyFormat {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Caddy => "caddy",
            Self::Nginx => "nginx",
            Self::Traefik => "traefik",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Args)]
pub struct SetupPublicProxyArgs {
    /// Browser-visible HTTPS Labby origin, for example https://labby.example.com.
    #[arg(long)]
    pub public_url: String,
    /// Private Labby backend origin.
    #[arg(long, default_value = "http://127.0.0.1:8765")]
    pub backend_url: String,
    /// Reverse-proxy configuration to render. Caddy is the simplest recommended path.
    #[arg(long, value_enum, default_value_t = SetupPublicProxyFormat::Caddy)]
    pub format: SetupPublicProxyFormat,
}

#[derive(Debug, Args)]
pub struct SetupTailscaleFunnelArgs {
    /// Loopback Labby backend origin to expose through Funnel.
    #[arg(long, default_value = "http://127.0.0.1:8765")]
    pub backend_url: String,
    /// Public HTTPS port. Tailscale Funnel supports 443, 8443, or 10000.
    #[arg(long, default_value_t = 443)]
    pub https_port: u16,
    /// Configure Funnel after inspection. Existing non-Labby mappings are never replaced.
    #[arg(long, conflicts_with = "disable")]
    pub apply: bool,
    /// Disable only a Funnel mapping that exactly matches this Labby backend.
    #[arg(long, conflicts_with = "apply")]
    pub disable: bool,
}

#[derive(Debug, Args)]
pub struct IncusBackupArgs {
    #[command(subcommand)]
    pub command: IncusBackupCommand,
}

#[derive(Debug, Args)]
pub struct IncusSshArgs {
    #[command(subcommand)]
    pub command: IncusSshCommand,
}

#[derive(Debug, Subcommand)]
pub enum IncusSshCommand {
    /// Generate an id_ed25519 key in the container and authorize it on configured hosts.
    Bootstrap {
        /// Incus container name.
        #[arg(long, default_value = "labby")]
        container: String,
        /// User inside the Incus container.
        #[arg(long, default_value = "labby")]
        user: String,
        /// Host SSH config to read.
        #[arg(long)]
        ssh_config: Option<PathBuf>,
        /// Private key path inside the container.
        #[arg(long, default_value = DEFAULT_INCUS_SSH_KEY_PATH, hide_default_value = true)]
        key_path: String,
        /// Print the plan without mutating the container or remote hosts.
        #[arg(long)]
        dry_run: bool,
        /// Only process hosts whose alias or HostName matches this filter. Repeatable.
        #[arg(long)]
        include: Vec<String>,
        /// Skip hosts whose alias or HostName matches this filter. Repeatable.
        #[arg(long)]
        exclude: Vec<String>,
        /// Abort on the first failed host instead of continuing and reporting failures.
        #[arg(long, conflicts_with = "continue_on_error")]
        fail_fast: bool,
        /// Continue past failed hosts and report them at the end (default).
        #[arg(long)]
        continue_on_error: bool,
        /// Install a sanitized SSH config into the container. Default for unfiltered runs.
        #[arg(long, conflicts_with = "no_install_config")]
        install_config: bool,
        /// Do not install a sanitized SSH config into the container. Default for filtered runs.
        #[arg(long, conflicts_with = "install_config")]
        no_install_config: bool,
        /// SSH connection timeout in seconds.
        #[arg(long, default_value_t = 10)]
        timeout_seconds: u64,
        /// Confirm remote authorized_keys updates.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
    /// Verify container-side SSH access to configured hosts.
    Verify {
        /// Incus container name.
        #[arg(long, default_value = "labby")]
        container: String,
        /// User inside the Incus container.
        #[arg(long, default_value = "labby")]
        user: String,
        /// Host SSH config to read.
        #[arg(long)]
        ssh_config: Option<PathBuf>,
        /// Private key path inside the container.
        #[arg(long, default_value = DEFAULT_INCUS_SSH_KEY_PATH, hide_default_value = true)]
        key_path: String,
        /// Only process hosts whose alias or HostName matches this filter. Repeatable.
        #[arg(long)]
        include: Vec<String>,
        /// Skip hosts whose alias or HostName matches this filter. Repeatable.
        #[arg(long)]
        exclude: Vec<String>,
        /// Abort on the first failed host instead of continuing and reporting failures.
        #[arg(long, conflicts_with = "continue_on_error")]
        fail_fast: bool,
        /// Continue past failed hosts and report them at the end (default).
        #[arg(long)]
        continue_on_error: bool,
        /// Refresh the sanitized SSH config before verifying. Default for unfiltered runs.
        #[arg(long, conflicts_with = "no_install_config")]
        install_config: bool,
        /// Do not refresh the sanitized SSH config before verifying. Default for filtered runs.
        #[arg(long, conflicts_with = "install_config")]
        no_install_config: bool,
        /// SSH connection timeout in seconds.
        #[arg(long, default_value_t = 10)]
        timeout_seconds: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum IncusBackupCommand {
    /// Validate a backup policy YAML without mutating Incus.
    Validate {
        /// Backup policy YAML to validate.
        #[arg(long, default_value = "config/incus/labby-backup.yaml")]
        config: PathBuf,
    },
    /// Apply a backup policy YAML to an Incus instance.
    Apply {
        /// Incus container name.
        #[arg(long)]
        name: String,
        /// Backup policy YAML to apply.
        #[arg(long, default_value = "config/incus/labby-backup.yaml")]
        config: PathBuf,
        /// Print the changes without mutating Incus.
        #[arg(long)]
        dry_run: bool,
        /// Confirm applying the backup policy without prompting.
        #[arg(short = 'y', long, alias = "no-confirm")]
        yes: bool,
    },
}

/// Default URL for the embedded web UI (per Q1: 127.0.0.1:8765).
const DEFAULT_LAB_URL: &str = "http://127.0.0.1:8765";

fn install_self() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let name = exe
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("cannot determine binary name"))?;
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let bin_dir = PathBuf::from(home).join(".local").join("bin");
    std::fs::create_dir_all(&bin_dir)?;
    let dest = bin_dir.join(name);
    if dest == exe {
        return Ok(dest);
    }
    let tmp = bin_dir.join(format!(".{}.tmp", name.to_string_lossy()));
    std::fs::copy(&exe, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &dest)?;
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == bin_dir))
        .unwrap_or(false);
    if !on_path {
        eprintln!(
            "note: {} is not on your PATH; add:  export PATH=\"$HOME/.local/bin:$PATH\"",
            bin_dir.display()
        );
    }
    Ok(dest)
}

pub async fn run(mut args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if args.bootstrap_static_owner {
        let paths = crate::installation::InstallationPaths::resolve()?;
        onboarding::bootstrap_static_owner_at(paths.root()).await?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(plan_path) = args.apply_plan.take() {
        return onboarding::apply_plan_file(&plan_path, format).await;
    }
    if args.provision {
        return run_provision(args, format).await;
    }
    if let Some(command) = args.command.take() {
        return run_command(command, format).await;
    }
    if setup_skip_requested()
        || args.smoke
        || args.no_setup
        || !matches!(args.mode, SetupModeArg::Full)
    {
        return run_wizard(
            WizardArgs {
                mode: args.mode,
                no_setup: args.no_setup,
                no_browser: args.no_browser,
                smoke: args.smoke,
            },
            format,
        )
        .await;
    }
    if args.skip_deps {
        anyhow::bail!("--skip-deps is only valid with --provision");
    }

    onboarding::run(args, format).await
}

async fn run_wizard(args: WizardArgs, format: OutputFormat) -> Result<ExitCode> {
    let theme = CliTheme::from_context(format.render_context());

    if setup_skip_requested() || args.no_setup {
        eprintln!(
            "{}",
            theme.muted(
                "setup skipped (LABBY_SKIP_SETUP=1 or --no-setup); run `labby setup wizard` manually when ready"
            )
        );
        return Ok(ExitCode::SUCCESS);
    }

    let snapshot = crate::dispatch::setup::dispatch("state", json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("setup.state failed: {e:?}"))?;

    if args.smoke {
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot).unwrap_or_default()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let first_run = snapshot
        .get("first_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let route = if first_run { "/setup" } else { "/settings" };

    let url = format!("{DEFAULT_LAB_URL}{route}?mode={}", args.mode.as_str());
    eprintln!();
    if first_run {
        eprintln!("{}", theme.section("Welcome to lab. First-run detected."));
    } else {
        eprintln!(
            "{}",
            theme.section("lab is already configured. Opening Settings.")
        );
    }
    eprintln!();
    eprintln!(
        "{} Run `labby serve` and visit: {}",
        theme.tertiary("→"),
        theme.accent(&url)
    );
    eprintln!();
    eprintln!(
        "{}",
        theme.muted("Tip: set LABBY_SKIP_SETUP=1 to suppress this message in CI.")
    );
    Ok(ExitCode::SUCCESS)
}

async fn run_provision(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    if args.command.is_some() {
        anyhow::bail!("--provision cannot be combined with a setup subcommand");
    }
    let mut yes = args.yes;
    let plan = crate::dispatch::setup::provision::provision_plan_text(args.skip_deps);
    if !format.is_json() {
        println!("{plan}");
    }
    if !args.dry_run && !yes {
        if !io::stdin().is_terminal() {
            anyhow::bail!("setup --provision requires --yes when stdin is not a TTY");
        }
        eprint!("Proceed? [y/N] ");
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        yes = matches!(answer.trim(), "y" | "Y" | "yes" | "YES");
        if !yes {
            anyhow::bail!("setup --provision cancelled");
        }
    }
    let outcome = crate::dispatch::setup::provision::provision(
        crate::dispatch::setup::provision::ProvisionOptions {
            dry_run: args.dry_run,
            yes,
            skip_deps: args.skip_deps,
        },
    )
    .await?;
    if format.is_json() {
        print(&serde_json::to_value(outcome)?, format)?;
    } else if outcome.dry_run {
        println!("dry-run complete; no changes made");
    } else {
        println!(
            "provision complete: executed={}, skipped={}",
            outcome.executed.len(),
            outcome.skipped.len()
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_command(command: SetupCommand, format: OutputFormat) -> Result<ExitCode> {
    match command {
        SetupCommand::Wizard(args) => {
            return run_wizard(args, format).await;
        }
        SetupCommand::Draft(args) => {
            run_draft_command(args, format).await?;
        }
        SetupCommand::OwnerLinkPrepare(args) => {
            let outcome = crate::dispatch::setup::owner_link::prepare(args.approval_file).await?;
            print(&outcome, format)?;
        }
        SetupCommand::AccessBootstrap(args) => match args.command {
            AccessBootstrapCommand::Prepare {
                proof_file,
                credential_file,
                organization_name,
                project_name,
                subject,
                loadout_id,
                route_id,
                resource,
                scope,
                ttl,
            } => {
                let outcome = crate::dispatch::setup::prepare_access_bootstrap(
                    crate::dispatch::setup::AccessBootstrapPrepare {
                        proof_file,
                        credential_file,
                        organization_name,
                        project_name,
                        subject,
                        loadout_id,
                        route_id,
                        resource,
                        scopes: scope,
                        ttl_seconds: ttl,
                    },
                )
                .await?;
                print(&serde_json::to_value(outcome)?, format)?;
            }
            AccessBootstrapCommand::Consume { prepare_id } => {
                let outcome = crate::dispatch::setup::consume_prepare(&prepare_id).await?;
                print(&outcome, format)?;
            }
            AccessBootstrapCommand::Status { prepare_id } => {
                let outcome = crate::dispatch::setup::status_prepare(&prepare_id).await?;
                print(&outcome, format)?;
            }
            AccessBootstrapCommand::Recover {
                prepare_id,
                complete,
                revoke,
            } => {
                let journal = crate::dispatch::setup::recover_prepare(&prepare_id)?;
                let journal = if complete {
                    crate::dispatch::setup::complete_prepare(&prepare_id).await?
                } else if revoke {
                    crate::dispatch::setup::revoke_prepare(&prepare_id).await?
                } else {
                    journal
                };
                print(&serde_json::to_value(journal)?, format)?;
            }
            AccessBootstrapCommand::Cleanup { prepare_id } => {
                let journal = crate::dispatch::setup::cleanup_prepare(&prepare_id).await?;
                print(&serde_json::to_value(journal)?, format)?;
            }
        },
        SetupCommand::HostService(args) => {
            run_host_service_command(args, format).await?;
        }
        SetupCommand::InstalledPlugins { force } => {
            let value =
                crate::dispatch::setup::dispatch("plugins.installed", json!({ "force": force }))
                    .await?;
            print(&value, format)?;
        }
        SetupCommand::ServicesStatus => {
            let value = crate::dispatch::setup::dispatch("services.status", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::PluginHook { no_repair } => {
            // Keep the user's terminal copy in ~/.local/bin fresh each session.
            // Best-effort: a stale or unwritable copy must not block the hook.
            if let Err(err) = install_self() {
                tracing::debug!(?err, "failed to refresh ~/.local/bin copy of labby");
            }
            let value = crate::dispatch::setup::dispatch_for_caller(
                crate::dispatch::setup::SetupCaller::Operator,
                "plugin_hook",
                json!({ "repair": !no_repair }),
            )
            .await?;
            print(&value, format)?;
        }
        SetupCommand::PluginSync(args) => {
            let params = json!({ "confirm": true });
            if args.dry_run {
                crate::cli::helpers::print_dry_run("setup", "plugin_sync", &params, format);
                return Ok(ExitCode::SUCCESS);
            }
            // Route through the shared destructive-action helper so TTY users
            // get the interactive confirm prompt and non-TTY callers get a
            // structured refusal — matches the cli/CLAUDE.md contract.
            return crate::cli::helpers::run_confirmable_action_command(
                "setup",
                crate::dispatch::setup::ACTIONS,
                "plugin_sync".to_string(),
                params,
                args.yes,
                format,
                |action, params| async move {
                    crate::dispatch::setup::dispatch_for_caller(
                        crate::dispatch::setup::SetupCaller::Operator,
                        &action,
                        params,
                    )
                    .await
                },
            )
            .await;
        }
        SetupCommand::PluginExport => {
            let value = crate::dispatch::setup::dispatch("plugin_export", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::PluginConnectivity { server_url } => {
            let params = match server_url {
                Some(url) => json!({ "server_url": url }),
                None => json!({}),
            };
            let value = crate::dispatch::setup::dispatch("plugin_connectivity", params).await?;
            print(&value, format)?;
        }
        SetupCommand::Resume => {
            let value = crate::dispatch::setup::dispatch("state", json!({})).await?;
            if format.is_json() {
                print(&value, format)?;
            } else {
                let snapshot: crate::dispatch::setup::SetupSnapshot =
                    serde_json::from_value(value)?;
                println!("Personal Labby onboarding");
                println!("  completed step: {}", snapshot.last_completed_step);
                println!("  resume from: {}", snapshot.resume_from);
                println!(
                    "  secure draft: {}",
                    if snapshot.has_draft {
                        snapshot.draft_path.display().to_string()
                    } else {
                        "none".into()
                    }
                );
                if snapshot.draft_stale {
                    println!(
                        "  warning: the staged draft is older than the committed environment; inspect it before continuing"
                    );
                }
                match snapshot.resume_from.as_str() {
                    "configure_runtime" | "configure_oauth" | "commit_configuration" => {
                        println!("  next: labby setup")
                    }
                    "connect_claude_code" => println!("  next: labby setup claude-code --apply"),
                    "verify_readiness" => println!("  next: labby doctor && labby doctor oauth"),
                    other => println!("  next: resume `{other}` from the WebUI or setup flow"),
                }
            }
        }
        SetupCommand::Contract => {
            if format.is_json() {
                print(
                    &json!({
                        "version": SETUP_CONTRACT_VERSION,
                        "capabilities": [
                            "personal_server",
                            "google_oauth",
                            "claude_code_mcp",
                            "chatgpt_oauth",
                            "resumable_setup"
                        ]
                    }),
                    format,
                )?;
            } else {
                println!("{SETUP_CONTRACT_VERSION}");
            }
        }
        SetupCommand::GoogleOauth(args) => {
            let guide = onboarding::google_oauth_guide(&args.public_url)?;
            let checks = if args.check {
                Some(google_oauth_public_check(&args.public_url).await?)
            } else {
                None
            };
            if format.is_json() {
                print(&json!({ "guide": guide, "public_check": checks }), format)?;
            } else {
                println!("{}", onboarding::google_oauth_guide_text(&args.public_url)?);
                if let Some(checks) = checks.as_ref() {
                    println!("{}", google_oauth_public_check_text(checks));
                }
            }
            if checks
                .as_ref()
                .and_then(|value| value.get("all_ok"))
                .and_then(Value::as_bool)
                == Some(false)
            {
                anyhow::bail!(
                    "public Google OAuth/MCP checks failed; fix the failed layer(s) above before connecting ChatGPT"
                );
            }
        }
        SetupCommand::ClaudeCode(args) => {
            if args.apply || args.rollback {
                run_claude_code_mutation(&args, format).await?;
            } else {
                let (value, text) = claude_code_guide(&args)?;
                if format.is_json() {
                    print(&value, format)?;
                } else {
                    println!("{text}");
                }
            }
        }
        SetupCommand::Check => {
            let value = crate::dispatch::setup::dispatch("check", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::Repair => {
            let value = crate::dispatch::setup::dispatch("repair", json!({})).await?;
            print(&value, format)?;
        }
        SetupCommand::Proxy(args) => {
            run_setup_proxy(args, format).await?;
        }
        SetupCommand::PublicProxy(args) => {
            run_public_proxy(args, format).await?;
        }
        SetupCommand::TailscaleFunnel(args) => {
            run_tailscale_funnel(args, format).await?;
        }
        SetupCommand::Incusbackup(args) => {
            run_incus_backup_command(args, format).await?;
        }
        SetupCommand::IncusSsh(args) => {
            run_incus_ssh_command(args, format).await?;
        }
        SetupCommand::Install => {
            let dest = install_self()?;
            println!("installed -> {}", dest.display());
        }
        SetupCommand::InstallPlugin(args) => {
            run_plugin_mutation("plugin.install", args, format).await?;
        }
        SetupCommand::UninstallPlugin(args) => {
            run_plugin_mutation("plugin.uninstall", args, format).await?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn google_oauth_probe_json(client: &reqwest::Client, name: &str, url: &str) -> Value {
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status();
            let body = response.json::<Value>().await;
            let ok = status.is_success() && body.is_ok();
            json!({
                "name": name,
                "url": url,
                "ok": ok,
                "status": status.as_u16(),
                "detail": if ok { "reachable JSON metadata" } else { "expected successful JSON metadata" },
            })
        }
        Err(error) => json!({
            "name": name,
            "url": url,
            "ok": false,
            "detail": error.to_string(),
        }),
    }
}

pub(crate) async fn google_oauth_public_check(public_url: &str) -> Result<Value> {
    let origin = public_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build OAuth public-check HTTP client")?;

    let authorization_metadata = google_oauth_probe_json(
        &client,
        "authorization-server metadata",
        &format!("{origin}/.well-known/oauth-authorization-server"),
    )
    .await;
    let protected_metadata = google_oauth_probe_json(
        &client,
        "protected-resource metadata",
        &format!("{origin}/.well-known/oauth-protected-resource"),
    )
    .await;

    let mcp_url = format!("{origin}/mcp");
    let mcp = match client.get(&mcp_url).send().await {
        Ok(response) => {
            let status = response.status();
            let challenge = response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let ok = status == reqwest::StatusCode::UNAUTHORIZED && !challenge.is_empty();
            json!({
                "name": "unauthenticated MCP challenge",
                "url": mcp_url,
                "ok": ok,
                "status": status.as_u16(),
                "detail": if ok { challenge } else { "expected HTTP 401 with WWW-Authenticate challenge".to_string() },
            })
        }
        Err(error) => json!({
            "name": "unauthenticated MCP challenge",
            "url": mcp_url,
            "ok": false,
            "detail": error.to_string(),
        }),
    };

    let login_url = format!("{origin}/auth/login?return_to=%2F");
    let login = match client.get(&login_url).send().await {
        Ok(response) => {
            let status = response.status();
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let ok = status.is_redirection()
                && reqwest::Url::parse(&location)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .is_some_and(|host| host == "accounts.google.com");
            json!({
                "name": "Google login redirect",
                "url": login_url,
                "ok": ok,
                "status": status.as_u16(),
                "detail": if ok { location } else { "expected redirect to accounts.google.com".to_string() },
            })
        }
        Err(error) => json!({
            "name": "Google login redirect",
            "url": login_url,
            "ok": false,
            "detail": error.to_string(),
        }),
    };

    let checks = vec![authorization_metadata, protected_metadata, mcp, login];
    let all_ok = checks
        .iter()
        .all(|check| check.get("ok").and_then(Value::as_bool) == Some(true));
    Ok(json!({ "all_ok": all_ok, "checks": checks }))
}

pub(crate) fn google_oauth_public_check_text(value: &Value) -> String {
    let mut lines = vec!["Google OAuth / MCP public check".to_string()];
    if let Some(checks) = value.get("checks").and_then(Value::as_array) {
        for check in checks {
            let ok = check.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let name = check.get("name").and_then(Value::as_str).unwrap_or("check");
            let detail = check
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status = check
                .get("status")
                .and_then(Value::as_u64)
                .map(|status| format!(" HTTP {status}"))
                .unwrap_or_default();
            lines.push(format!(
                "  {} {name}{status}: {detail}",
                if ok { "PASS" } else { "FAIL" }
            ));
        }
    }
    lines.join(
        "
",
    )
}

fn find_executable_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{name}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn claude_code_guide(args: &ClaudeCodeGuideArgs) -> Result<(Value, String)> {
    let name = args.name.trim();
    if name.is_empty() {
        anyhow::bail!("Claude Code upstream name must not be empty");
    }

    if let Some(target) = args.ssh_target.as_deref() {
        let target = target.trim();
        if target.is_empty() || !target.contains('@') {
            anyhow::bail!("--ssh-target must use user@host form");
        }
        let claude = args.claude_path.as_ref().ok_or_else(|| anyhow::anyhow!("remote Claude Code setup requires --claude-path with the exact executable path on the remote machine"))?;
        let identity = args.identity_file.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "remote Claude Code setup requires --identity-file for a dedicated Labby SSH key"
            )
        })?;
        let known_hosts = args.known_hosts_file.as_ref().ok_or_else(|| anyhow::anyhow!("remote Claude Code setup requires --known-hosts-file containing an independently verified host key"))?;
        let ssh = find_executable_on_path("ssh").unwrap_or_else(|| PathBuf::from("/usr/bin/ssh"));
        let claude_s = claude.to_string_lossy();
        let identity_s = identity.to_string_lossy();
        let known_hosts_s = known_hosts.to_string_lossy();
        let ssh_s = ssh.to_string_lossy();

        let preflight = format!(
            "{} -i {} -o IdentitiesOnly=yes -o UserKnownHostsFile={} -T -S none -o ControlMaster=no -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o StrictHostKeyChecking=yes {} {} --version",
            sh_quote(&ssh_s),
            sh_quote(&identity_s),
            sh_quote(&known_hosts_s),
            sh_quote(target),
            sh_quote(&claude_s)
        );
        let add = format!(
            "labby gateway add --name {} --command {} --arg=-i --arg={} --arg=-o --arg=IdentitiesOnly=yes --arg=-o --arg=UserKnownHostsFile={} --arg=-T --arg=-S --arg=none --arg=-o --arg=ControlMaster=no --arg=-o --arg=BatchMode=yes --arg=-o --arg=ConnectTimeout=10 --arg=-o --arg=ServerAliveInterval=30 --arg=-o --arg=ServerAliveCountMax=3 --arg=-o --arg=StrictHostKeyChecking=yes --arg={} --arg={} --arg=mcp --arg=serve",
            sh_quote(name),
            sh_quote(&ssh_s),
            sh_quote(&identity_s),
            sh_quote(&known_hosts_s),
            sh_quote(target),
            sh_quote(&claude_s)
        );
        let test = format!("labby gateway test --name {}", sh_quote(name));
        let value = json!({
            "mode": "remote_ssh",
            "name": name,
            "ssh_target": target,
            "claude_path": claude,
            "identity_file": identity,
            "known_hosts_file": known_hosts,
            "preflight_command": preflight,
            "add_command": add,
            "test_command": test,
        });
        let claude_login = format!("{} auth login", sh_quote(&claude_s));
        let claude_auth_status = format!("{} auth status --text", sh_quote(&claude_s));
        let claude_version = format!("{} --version", sh_quote(&claude_s));
        let claude_doctor = format!("{} doctor", sh_quote(&claude_s));
        let identity_quoted = sh_quote(&identity_s);
        let identity_pub = sh_quote(&format!("{identity_s}.pub"));
        let comment = sh_quote(&format!("labby-{name}"));
        let known_hosts_quoted = sh_quote(&known_hosts_s);
        let text = format!(
            r"Claude Code MCP -> Labby (remote over hardened SSH)

1. On the remote machine, install/update native Claude Code and authenticate the exact account that will serve MCP.
   Sign in and verify:
     {claude_login}
     {claude_auth_status}
     {claude_version}
     {claude_doctor}

2. On the Labby host, use a dedicated Ed25519 key for this one remote machine.
   If {identity} does not already exist:
     ssh-keygen -t ed25519 -f {identity} -C {comment}
   Add ONLY the public key ({identity_pub}) to the target account's ~/.ssh/authorized_keys.

3. Obtain the target SSH host-key fingerprint through a trusted channel.
   Store the verified key in: {known_hosts}
   Do not disable StrictHostKeyChecking and do not treat an unverified ssh-keyscan result as trust.

4. As the same service account that runs Labby, prove the exact non-interactive command:
   {preflight}

5. Persist the upstream:
   {add}

6. Test it:
   {test}

7. Invoke one safe Claude MCP tool and verify hostname, whoami, platform, and current working directory.

Keep Labby's spawn guard enabled. Both ssh and claude are normal supported commands; a global spawn-guard bypass is unnecessary.",
            claude_login = claude_login,
            claude_auth_status = claude_auth_status,
            claude_version = claude_version,
            claude_doctor = claude_doctor,
            identity = identity_quoted,
            identity_pub = identity_pub,
            comment = comment,
            known_hosts = known_hosts_quoted,
            preflight = preflight,
            add = add,
            test = test,
        );
        return Ok((value, text));
    }

    let claude = match args
        .claude_path
        .clone()
        .or_else(|| find_executable_on_path("claude"))
    {
        Some(path) => path,
        None => anyhow::bail!(
            "Claude Code was not found on PATH; install/authenticate Claude Code or pass --claude-path /absolute/path/to/claude"
        ),
    };
    let claude_s = claude.to_string_lossy();
    let add = format!(
        "labby gateway add --name {} --command {} --arg=mcp --arg=serve",
        sh_quote(name),
        sh_quote(&claude_s)
    );
    let test = format!("labby gateway test --name {}", sh_quote(name));
    let value = json!({
        "mode": "local_stdio",
        "name": name,
        "claude_path": claude,
        "doctor_command": format!("{} doctor", sh_quote(&claude_s)),
        "add_command": add,
        "test_command": test,
    });
    let login = format!("{} auth login", sh_quote(&claude_s));
    let auth_status = format!("{} auth status --text", sh_quote(&claude_s));
    let doctor = format!("{} doctor", sh_quote(&claude_s));
    let version = format!("{} --version", sh_quote(&claude_s));
    let text = format!(
        r"Claude Code MCP -> Labby (local stdio)

1. Authenticate and verify Claude Code:
   {login}
   {auth_status}
   {doctor}
   {version}

2. Persist Claude Code as a Labby upstream:
   {add}

3. Test the upstream:
   {test}

4. Invoke one safe Claude MCP tool and verify hostname, whoami, platform, and current working directory.

Labby launches Claude Code with `claude mcp serve`. Keep the spawn guard enabled; current Labby treats `claude` as a built-in allowed stdio command.",
        login = login,
        auth_status = auth_status,
        doctor = doctor,
        version = version,
        add = add,
        test = test,
    );
    Ok((value, text))
}

const CLAUDE_CODE_ROLLBACK_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct ClaudeCodeRollbackRecord {
    version: u32,
    name: String,
    previous: Option<crate::config::UpstreamConfig>,
    applied: crate::config::UpstreamConfig,
}

fn validate_claude_backup_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        anyhow::bail!(
            "Claude Code upstream name must contain only ASCII letters, digits, '-', '_', or '.'"
        );
    }
    Ok(())
}

fn claude_code_rollback_path(name: &str) -> Result<PathBuf> {
    validate_claude_backup_name(name)?;
    Ok(crate::dispatch::helpers::lab_home()
        .join("setup-backups")
        .join(format!("claude-code-{name}.json")))
}

fn existing_upstream_safe_to_snapshot(upstream: &crate::config::UpstreamConfig) -> bool {
    upstream.url.is_none()
        && upstream.socket_path.is_none()
        && upstream.headers.is_empty()
        && upstream.bearer_token_env.is_none()
        && upstream.env.is_empty()
        && upstream.oauth.is_none()
        && upstream.imported_from.is_none()
}

fn claude_code_upstream_spec(args: &ClaudeCodeGuideArgs) -> Result<crate::config::UpstreamConfig> {
    let name = args.name.trim();
    validate_claude_backup_name(name)?;

    let (command, command_args) = if let Some(target) = args.ssh_target.as_deref() {
        let target = target.trim();
        if target.is_empty() || !target.contains('@') {
            anyhow::bail!("--ssh-target must use user@host form");
        }
        let claude = args.claude_path.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "remote Claude Code setup requires --claude-path with the exact executable path on the remote machine"
            )
        })?;
        let identity = args.identity_file.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "remote Claude Code setup requires --identity-file for a dedicated Labby SSH key"
            )
        })?;
        let known_hosts = args.known_hosts_file.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "remote Claude Code setup requires --known-hosts-file containing an independently verified host key"
            )
        })?;
        let ssh = find_executable_on_path("ssh").unwrap_or_else(|| PathBuf::from("/usr/bin/ssh"));
        (
            ssh.to_string_lossy().to_string(),
            vec![
                "-i".to_string(),
                identity.to_string_lossy().to_string(),
                "-o".to_string(),
                "IdentitiesOnly=yes".to_string(),
                "-o".to_string(),
                format!("UserKnownHostsFile={}", known_hosts.to_string_lossy()),
                "-T".to_string(),
                "-S".to_string(),
                "none".to_string(),
                "-o".to_string(),
                "ControlMaster=no".to_string(),
                "-o".to_string(),
                "BatchMode=yes".to_string(),
                "-o".to_string(),
                "ConnectTimeout=10".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=30".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=3".to_string(),
                "-o".to_string(),
                "StrictHostKeyChecking=yes".to_string(),
                target.to_string(),
                claude.to_string_lossy().to_string(),
                "mcp".to_string(),
                "serve".to_string(),
            ],
        )
    } else {
        let claude = args
            .claude_path
            .clone()
            .or_else(|| find_executable_on_path("claude"))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Claude Code was not found on PATH; install/authenticate Claude Code or pass --claude-path /absolute/path/to/claude"
                )
            })?;
        (
            claude.to_string_lossy().to_string(),
            vec!["mcp".to_string(), "serve".to_string()],
        )
    };

    serde_json::from_value(json!({
        "name": name,
        "enabled": true,
        "priority": 1.0,
        "command": command,
        "args": command_args,
        "proxy_resources": true,
        "proxy_prompts": true,
        "proxy_skills": false
    }))
    .context("build Claude Code upstream definition")
}

#[cfg(not(feature = "gateway"))]
async fn run_claude_code_mutation(
    _args: &ClaudeCodeGuideArgs,
    _format: OutputFormat,
) -> Result<()> {
    anyhow::bail!("Claude Code --apply/--rollback requires the Labby gateway feature")
}

fn upstreams_equal(
    left: &crate::config::UpstreamConfig,
    right: &crate::config::UpstreamConfig,
) -> Result<bool> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}

fn write_claude_code_rollback(record: &ClaudeCodeRollbackRecord) -> Result<PathBuf> {
    let path = claude_code_rollback_path(&record.name)?;
    let directory = path
        .parent()
        .context("Claude Code rollback path has no parent")?;
    if std::fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        anyhow::bail!("refusing to write Claude rollback state through a symlinked directory");
    }
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }

    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer_pretty(&mut temporary, record)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&path)
        .map_err(|error| anyhow::anyhow!("persist Claude Code rollback record: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

fn read_claude_code_rollback(name: &str) -> Result<(PathBuf, ClaudeCodeRollbackRecord)> {
    let path = claude_code_rollback_path(name)?;
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("no Claude Code rollback record exists for `{name}`"))?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("refusing to read Claude rollback state through a symlink");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!(
                "Claude rollback record {} is too permissive; require mode 0600 before using it",
                path.display()
            );
        }
    }
    let record: ClaudeCodeRollbackRecord = serde_json::from_slice(&std::fs::read(&path)?)
        .context("parse Claude Code rollback record")?;
    if record.version != CLAUDE_CODE_ROLLBACK_VERSION || record.name != name {
        anyhow::bail!("Claude Code rollback record is incompatible with this request");
    }
    Ok((path, record))
}

#[cfg(feature = "gateway")]
async fn restore_claude_code_upstream(
    manager: &crate::dispatch::gateway::manager::GatewayManager,
    record: &ClaudeCodeRollbackRecord,
) -> Result<()> {
    if manager.upstream_config(&record.name).await.is_some() {
        manager
            .remove(&record.name, Some("setup.claude-code.rollback"), None)
            .await?;
    }
    if let Some(previous) = record.previous.clone() {
        manager
            .add(previous, None, Some("setup.claude-code.rollback"), None)
            .await?;
    }
    Ok(())
}

#[cfg(feature = "gateway")]
async fn run_claude_code_mutation(args: &ClaudeCodeGuideArgs, format: OutputFormat) -> Result<()> {
    let config_path = crate::config::config_toml_path()?;
    let config = crate::config::load_toml(&[config_path])?;
    let manager = crate::cli::gateway::build_manager(&config, true).await?;

    if args.rollback {
        let (path, record) = read_claude_code_rollback(args.name.trim())?;
        let current = manager.upstream_config(&record.name).await;
        let current_matches_applied = match current.as_ref() {
            Some(current) => upstreams_equal(current, &record.applied)?,
            None => false,
        };
        if !current_matches_applied {
            anyhow::bail!(
                "refusing Claude Code rollback because `{}` changed after Labby applied it; inspect {} before restoring manually",
                record.name,
                path.display()
            );
        }
        restore_claude_code_upstream(manager.as_ref(), &record).await?;
        std::fs::remove_file(&path)?;
        let result = json!({
            "status": "rolled_back",
            "name": record.name,
            "restored_previous": record.previous.is_some()
        });
        if format.is_json() {
            print(&result, format)?;
        } else {
            println!(
                "restored `{}` to its exact pre-Labby upstream state",
                args.name.trim()
            );
        }
        return Ok(());
    }

    let desired = claude_code_upstream_spec(args)?;
    let preflight = manager.test(Ok(&desired)).await?;
    if !preflight.connected {
        anyhow::bail!(
            "Claude Code MCP preflight failed before any config change: {}",
            preflight
                .last_error
                .as_deref()
                .unwrap_or("upstream did not complete MCP discovery")
        );
    }

    let existing = manager.upstream_config(&desired.name).await;
    if let Some(existing) = existing.as_ref() {
        if upstreams_equal(existing, &desired)? {
            let saved_test = manager.test(Err(&desired.name)).await?;
            if !saved_test.connected {
                anyhow::bail!(
                    "existing Claude Code upstream matches the requested config but is not healthy: {}",
                    saved_test
                        .last_error
                        .as_deref()
                        .unwrap_or("upstream did not complete MCP discovery")
                );
            }
            let result = json!({
                "status": "already_configured",
                "name": desired.name,
                "connected": true
            });
            if format.is_json() {
                print(&result, format)?;
            } else {
                println!(
                    "Claude Code upstream `{}` is already configured and healthy",
                    args.name.trim()
                );
            }
            return Ok(());
        }
        if !existing_upstream_safe_to_snapshot(existing) {
            anyhow::bail!(
                "refusing to replace `{}` automatically because the existing upstream contains transport, credential, imported, or environment state outside the plain Claude stdio profile",
                desired.name
            );
        }
        if !args.yes {
            anyhow::bail!(
                "an upstream named `{}` already exists and differs; no changes made. Review it, then re-run with --apply --yes to replace it transactionally",
                desired.name
            );
        }
    }

    let record = ClaudeCodeRollbackRecord {
        version: CLAUDE_CODE_ROLLBACK_VERSION,
        name: desired.name.clone(),
        previous: existing.clone(),
        applied: desired.clone(),
    };
    let rollback_path = write_claude_code_rollback(&record)?;

    let applied = async {
        if existing.is_some() {
            manager
                .remove(&desired.name, Some("setup.claude-code"), None)
                .await?;
        }
        manager
            .add(desired.clone(), None, Some("setup.claude-code"), None)
            .await?;
        let test = manager.test(Err(&desired.name)).await?;
        if !test.connected {
            anyhow::bail!(
                "saved Claude Code upstream failed its post-apply MCP handshake: {}",
                test.last_error
                    .as_deref()
                    .unwrap_or("upstream did not complete MCP discovery")
            );
        }
        Ok::<_, anyhow::Error>(test)
    }
    .await;

    let test = match applied {
        Ok(test) => test,
        Err(error) => match restore_claude_code_upstream(manager.as_ref(), &record).await {
            Ok(()) => {
                drop(std::fs::remove_file(&rollback_path));
                anyhow::bail!(
                    "Claude Code setup failed after mutation, so Labby restored the exact prior upstream state automatically: {error}"
                );
            }
            Err(rollback_error) => {
                anyhow::bail!(
                    "Claude Code setup failed and automatic rollback also failed. Rollback record retained at {}. setup error: {error}; rollback error: {rollback_error}",
                    rollback_path.display()
                );
            }
        },
    };

    let result = json!({
        "status": "configured",
        "name": desired.name,
        "connected": test.connected,
        "tool_count": test.tool_count,
        "resource_count": test.resource_count,
        "prompt_count": test.prompt_count,
        "rollback_record": rollback_path
    });
    if format.is_json() {
        print(&result, format)?;
    } else {
        println!(
            "Claude Code upstream `{}` is configured and healthy; rollback: `labby setup claude-code --name {} --rollback`",
            args.name.trim(),
            args.name.trim()
        );
    }
    Ok(())
}

async fn run_tailscale_funnel(args: SetupTailscaleFunnelArgs, format: OutputFormat) -> Result<()> {
    let (action, params) = if args.disable {
        (
            "tailscale_funnel.disable",
            json!({
                "backend_url": args.backend_url,
                "https_port": args.https_port,
            }),
        )
    } else if args.apply {
        (
            "tailscale_funnel.configure",
            json!({
                "backend_url": args.backend_url,
                "https_port": args.https_port,
            }),
        )
    } else {
        (
            "tailscale_funnel.inspect",
            json!({ "https_port": args.https_port }),
        )
    };
    let value = crate::dispatch::setup::dispatch(action, params).await?;
    if format.is_json() {
        print(&value, format)?;
        return Ok(());
    }

    if args.apply || args.disable {
        let outcome: crate::dispatch::setup::tailscale_funnel::TailscaleFunnelMutationOutcome =
            serde_json::from_value(value).context("decode Tailscale Funnel mutation result")?;
        if outcome.activation_required {
            println!("One-time Tailscale approval is required before Funnel can be enabled.");
            if let Some(url) = outcome.activation_url.as_deref() {
                println!("Approve: {url}");
            } else if let Some(message) = outcome.activation_message.as_deref() {
                println!("{message}");
            }
            println!("After approval, rerun: labby setup tailscale-funnel --apply");
            return Ok(());
        }
        println!(
            "Tailscale Funnel: {}{}",
            if outcome.configured {
                "configured"
            } else {
                "disabled"
            },
            if outcome.changed {
                ""
            } else {
                " (already converged)"
            }
        );
        println!("Public Labby: {}", outcome.public_origin);
        println!("Private backend: {}", outcome.backend_origin);
        println!("Google callback: {}", outcome.oauth_callback_url);
        println!("ChatGPT MCP URL: {}", outcome.mcp_url);
        println!("Verify:");
        for command in outcome.verification {
            println!("  {command}");
        }
        return Ok(());
    }

    let inspection: crate::dispatch::setup::tailscale_funnel::TailscaleFunnelInspection =
        serde_json::from_value(value).context("decode Tailscale Funnel inspection result")?;
    if !inspection.cli_available {
        println!("Tailscale Funnel is not available on this host.");
    } else {
        println!(
            "Tailscale: {}{}",
            inspection.version.as_deref().unwrap_or("detected"),
            if inspection.online {
                " · online"
            } else {
                " · offline"
            }
        );
    }
    if let Some(origin) = inspection.public_origin.as_deref() {
        println!("Public HTTPS candidate: {origin}");
    }
    if let Some(backend) = inspection.configured_backend.as_deref() {
        println!("Existing Funnel mapping: {backend}");
    }
    for blocker in &inspection.blockers {
        println!("Blocker: {blocker}");
    }
    if inspection.ready_to_configure {
        if inspection.configured_backend.as_deref() == Some(args.backend_url.as_str()) {
            println!(
                "Labby is already exposed through Funnel on port {}.",
                args.https_port
            );
        } else if inspection.activation_required {
            println!(
                "Ready for first-time approval. Run labby setup tailscale-funnel --apply; Labby will surface the Tailscale approval URL instead of waiting indefinitely."
            );
        } else {
            println!(
                "Ready. Run labby setup tailscale-funnel --apply to expose the local Labby backend."
            );
        }
    } else if !inspection.cli_available {
        println!(
            "Install the Tailscale CLI, or use labby setup public-proxy with an HTTPS public URL for Caddy, Nginx, or Traefik."
        );
    }
    Ok(())
}

async fn run_public_proxy(args: SetupPublicProxyArgs, format: OutputFormat) -> Result<()> {
    let value = crate::dispatch::setup::dispatch(
        "public_proxy.render",
        json!({
            "public_url": args.public_url,
            "backend_url": args.backend_url,
            "format": args.format.as_str(),
        }),
    )
    .await?;
    if format.is_json() {
        print(&value, format)?;
        return Ok(());
    }

    let outcome: crate::dispatch::setup::public_proxy::PublicProxyRenderOutcome =
        serde_json::from_value(value).context("decode public proxy render result")?;
    println!("Public Labby: {}", outcome.public_origin);
    println!("Private backend: {}", outcome.backend_origin);
    println!("Google callback: {}", outcome.oauth_callback_url);
    println!("ChatGPT MCP URL: {}", outcome.mcp_url);
    println!("Recommended: {}", outcome.recommended);
    println!();
    for (name, config) in outcome.configs {
        println!("{} configuration:\n{}", name, config.trim_end());
        println!();
    }
    println!("Verify after installing/reloading the proxy:");
    for command in outcome.verification {
        println!("  {command}");
    }
    Ok(())
}

async fn run_setup_proxy(args: SetupProxyArgs, format: OutputFormat) -> Result<()> {
    if !args.yes && !args.dry_run && !io::stdin().is_terminal() {
        anyhow::bail!("setup proxy requires --yes when stdin is not a TTY");
    }

    let home = crate::dispatch::helpers::lab_home();
    let config_path = home.join("config.toml");
    let mut preferences = crate::config::load_toml(&[config_path])?.proxy;
    apply_proxy_setup_overrides(&mut preferences, &args)?;
    if !args.yes && !args.dry_run {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let mut output = io::stderr().lock();
        preferences = prompt_proxy_preferences(preferences, &mut input, &mut output)?;
    }

    let bearer_token = if args.bearer_token_stdin {
        let mut token = String::new();
        io::stdin().read_line(&mut token)?;
        let token = token.trim().to_string();
        if token.is_empty() {
            anyhow::bail!("--bearer-token-stdin received an empty token");
        }
        Some(token)
    } else {
        None
    };
    let params = json!({
        "preferences": preferences,
        "bearer_token": bearer_token,
        "dry_run": args.dry_run,
    });
    let value = crate::dispatch::setup::dispatch("proxy.configure", params).await?;
    print(&value, format)?;
    Ok(())
}

fn apply_proxy_setup_overrides(
    preferences: &mut crate::proxy::config::ProxyPreferences,
    args: &SetupProxyArgs,
) -> Result<()> {
    if let Some(exposure) = args.exposure {
        preferences.exposure = exposure;
    }
    if let Some(auth) = args.auth {
        preferences.auth = auth;
    }
    if args.bearer_token_stdin {
        preferences.auth = crate::proxy::config::ProxyAuthMode::Bearer;
    }
    if let Some(path) = &args.path {
        preferences.path.clone_from(path);
    }
    if let Some(port) = &args.port {
        preferences.port = parse_proxy_setup_port(port)?;
    }
    if let Some(start) = args.port_range_start {
        preferences.port_range_start = start;
    }
    if let Some(end) = args.port_range_end {
        preferences.port_range_end = end;
    }
    if let Some(key) = &args.bearer_token_env {
        preferences.bearer_token_env.clone_from(key);
    }
    if !args.oauth_scopes.is_empty() {
        preferences.oauth_scopes.clone_from(&args.oauth_scopes);
    }
    if !args.inherit_env.is_empty() {
        preferences.inherit_env.clone_from(&args.inherit_env);
    }
    if let Some(grace) = args.shutdown_grace_ms {
        preferences.shutdown_grace_ms = grace;
    }
    preferences.validate().map_err(anyhow::Error::from)
}

fn parse_proxy_setup_port(value: &str) -> Result<crate::proxy::config::ProxyPortPreference> {
    if value.eq_ignore_ascii_case("random") {
        return Ok(crate::proxy::config::ProxyPortPreference::default());
    }
    let port = value
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("--port must be `random` or an integer from 1 to 65535"))?;
    if port == 0 {
        anyhow::bail!("--port must not be zero");
    }
    Ok(crate::proxy::config::ProxyPortPreference::Fixed(port))
}

fn prompt_proxy_preferences(
    mut preferences: crate::proxy::config::ProxyPreferences,
    input: &mut impl io::BufRead,
    output: &mut impl Write,
) -> Result<crate::proxy::config::ProxyPreferences> {
    use crate::proxy::config::{ProxyAuthMode, ProxyExposure};

    let exposure = prompt_value(
        input,
        output,
        "Exposure (tailscale/local)",
        match preferences.exposure {
            ProxyExposure::Tailscale => "tailscale",
            ProxyExposure::Local => "local",
        },
    )?;
    preferences.exposure = match exposure.to_ascii_lowercase().as_str() {
        "tailscale" => ProxyExposure::Tailscale,
        "local" => ProxyExposure::Local,
        _ => anyhow::bail!("exposure must be `tailscale` or `local`"),
    };
    let auth = prompt_value(
        input,
        output,
        "Auth (tailnet/bearer/oauth/none)",
        match preferences.auth {
            ProxyAuthMode::Tailnet => "tailnet",
            ProxyAuthMode::Bearer => "bearer",
            ProxyAuthMode::Oauth => "oauth",
            ProxyAuthMode::None => "none",
        },
    )?;
    preferences.auth = match auth.to_ascii_lowercase().as_str() {
        "tailnet" => ProxyAuthMode::Tailnet,
        "bearer" => ProxyAuthMode::Bearer,
        "oauth" => ProxyAuthMode::Oauth,
        "none" => ProxyAuthMode::None,
        _ => anyhow::bail!("auth must be `tailnet`, `bearer`, `oauth`, or `none`"),
    };
    preferences.path = prompt_value(input, output, "MCP path", &preferences.path)?;
    let default_port = preferences
        .port
        .fixed()
        .map_or_else(|| "random".to_string(), |port| port.to_string());
    preferences.port = parse_proxy_setup_port(&prompt_value(
        input,
        output,
        "External port",
        &default_port,
    )?)?;
    if preferences.port.fixed().is_none() {
        preferences.port_range_start = prompt_value(
            input,
            output,
            "Random port range start",
            &preferences.port_range_start.to_string(),
        )?
        .parse()?;
        preferences.port_range_end = prompt_value(
            input,
            output,
            "Random port range end",
            &preferences.port_range_end.to_string(),
        )?
        .parse()?;
    }
    preferences.validate().map_err(anyhow::Error::from)?;
    writeln!(
        output,
        "Proxy defaults selected; secrets will not be displayed."
    )?;
    Ok(preferences)
}

fn prompt_value(
    input: &mut impl io::BufRead,
    output: &mut impl Write,
    label: &str,
    default: &str,
) -> Result<String> {
    write!(output, "{label} [{default}]: ")?;
    output.flush()?;
    let mut answer = String::new();
    let read = input.read_line(&mut answer)?;
    if read == 0 {
        anyhow::bail!("setup proxy input ended before configuration was complete");
    }
    let answer = answer.trim();
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer.to_string()
    })
}

async fn run_incus_ssh_command(args: IncusSshArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        IncusSshCommand::Bootstrap {
            container,
            user,
            ssh_config,
            key_path,
            dry_run,
            include,
            exclude,
            fail_fast,
            continue_on_error: _,
            install_config,
            no_install_config,
            timeout_seconds,
            yes,
        } => {
            let options = incus_ssh_options(IncusSshOptionInput {
                container,
                user,
                ssh_config,
                key_path,
                fail_fast,
                include,
                exclude,
                install_config,
                no_install_config,
                dry_run,
                timeout_seconds,
            });
            let plan = crate::dispatch::setup::incus::incus_ssh_bootstrap_plan(&options)?;
            if format.is_json() {
                if dry_run {
                    print(&serde_json::to_value(plan)?, format)?;
                    return Ok(());
                }
            } else {
                println!(
                    "will bootstrap SSH key for container `{}` user `{}`:",
                    plan.container, plan.user
                );
                for step in &plan.steps {
                    println!("  - {step}");
                }
                print_incus_ssh_skips(
                    &plan.skipped_unsafe,
                    &plan.unsupported_include,
                    &plan.skipped_excluded,
                    &plan.skipped_not_included,
                );
            }
            if dry_run {
                return Ok(());
            }
            require_incus_ssh_confirmation(&plan.container, plan.targets.len(), yes)?;
            let outcome = crate::dispatch::setup::incus::incus_ssh_bootstrap(&options)?;
            if format.is_json() {
                print(&serde_json::to_value(outcome)?, format)?;
            } else {
                println!(
                    "bootstrapped {} SSH targets for container `{}` ({} failed)",
                    outcome.authorized.len(),
                    outcome.container,
                    outcome.failed.len()
                );
                if outcome.config_installed {
                    println!("installed sanitized SSH config in container");
                }
                print_incus_ssh_skips(
                    &outcome.skipped_unsafe,
                    &outcome.unsupported_include,
                    &outcome.skipped_excluded,
                    &outcome.skipped_not_included,
                );
                for failure in &outcome.failed {
                    println!("  failed: {} - {}", failure.target, failure.error);
                }
            }
        }
        IncusSshCommand::Verify {
            container,
            user,
            ssh_config,
            key_path,
            include,
            exclude,
            fail_fast,
            continue_on_error: _,
            install_config,
            no_install_config,
            timeout_seconds,
        } => {
            let options = incus_ssh_options(IncusSshOptionInput {
                container,
                user,
                ssh_config,
                key_path,
                fail_fast,
                include,
                exclude,
                install_config,
                no_install_config,
                dry_run: false,
                timeout_seconds,
            });
            let outcome = crate::dispatch::setup::incus::incus_ssh_verify(&options)?;
            if format.is_json() {
                print(&serde_json::to_value(outcome)?, format)?;
            } else {
                println!(
                    "verified {} SSH targets for container `{}` ({} failed)",
                    outcome.verified.len(),
                    outcome.container,
                    outcome.failed.len()
                );
                print_incus_ssh_skips(
                    &outcome.skipped_unsafe,
                    &outcome.unsupported_include,
                    &outcome.skipped_excluded,
                    &outcome.skipped_not_included,
                );
                for failure in &outcome.failed {
                    println!("  failed: {} - {}", failure.target, failure.error);
                }
            }
        }
    }
    Ok(())
}

struct IncusSshOptionInput {
    container: String,
    user: String,
    ssh_config: Option<PathBuf>,
    key_path: String,
    fail_fast: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    install_config: bool,
    no_install_config: bool,
    dry_run: bool,
    timeout_seconds: u64,
}

fn incus_ssh_options(
    input: IncusSshOptionInput,
) -> crate::dispatch::setup::incus::IncusSshBootstrapOptions {
    let IncusSshOptionInput {
        container,
        user,
        ssh_config,
        key_path,
        fail_fast,
        include,
        exclude,
        install_config,
        no_install_config,
        dry_run,
        timeout_seconds,
    } = input;
    let ssh_config = ssh_config.unwrap_or_else(default_ssh_config_path);
    let install_config =
        should_install_incus_ssh_config(install_config, no_install_config, &include, &exclude);
    crate::dispatch::setup::incus::IncusSshBootstrapOptions {
        container,
        user,
        ssh_config,
        key_path,
        dry_run,
        fail_fast,
        include,
        exclude,
        install_config,
        timeout_seconds,
    }
}

fn print_incus_ssh_skips(
    skipped_unsafe: &[String],
    unsupported_include: &[String],
    skipped_excluded: &[String],
    skipped_not_included: &[String],
) {
    for alias in skipped_unsafe {
        println!("  skipped unsafe SSH host alias: {alias}");
    }
    for include in unsupported_include {
        println!("  unsupported SSH Include ignored: {include}");
    }
    for alias in skipped_excluded {
        println!("  skipped excluded SSH host: {alias}");
    }
    for alias in skipped_not_included {
        println!("  skipped non-included SSH host: {alias}");
    }
}

async fn run_incus_backup_command(args: IncusBackupArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        IncusBackupCommand::Validate { config } => {
            let entries = crate::dispatch::setup::incus::parse_backup_config(&config)?;
            if format.is_json() {
                print(&serde_json::to_value(&entries)?, format)?;
            } else {
                println!("validated {} backup config entries", entries.len());
            }
        }
        IncusBackupCommand::Apply {
            name,
            config,
            dry_run,
            yes,
        } => {
            if !dry_run {
                require_incus_backup_confirmation(&name, yes)?;
            }
            let outcome =
                crate::dispatch::setup::incus::apply_backup_config(&name, &config, dry_run)?;
            if format.is_json() {
                print(&serde_json::to_value(&outcome)?, format)?;
            } else if dry_run {
                println!(
                    "dry-run: would apply {} backup config entries to {}",
                    outcome.applied.len(),
                    outcome.container
                );
            } else {
                println!(
                    "applied {} backup config entries to {}",
                    outcome.applied.len(),
                    outcome.container
                );
            }
        }
    }
    Ok(())
}

fn default_ssh_config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh")
        .join("config")
}

fn should_install_incus_ssh_config(
    install_config: bool,
    no_install_config: bool,
    include: &[String],
    exclude: &[String],
) -> bool {
    if install_config {
        return true;
    }
    if no_install_config {
        return false;
    }
    include.is_empty() && exclude.is_empty()
}

fn setup_skip_requested() -> bool {
    std::env::var("LABBY_SKIP_SETUP").as_deref() == Ok("1")
}

fn require_incus_ssh_confirmation(container: &str, target_count: usize, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!("setup incus-ssh bootstrap requires --yes when stdin is not a TTY");
    }
    eprintln!(
        "This will generate an SSH key in container `{container}` and update authorized_keys on {target_count} host(s)."
    );
    eprint!("Proceed? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        anyhow::bail!("setup incus-ssh bootstrap cancelled");
    }
}

fn require_incus_backup_confirmation(container: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!("setup incusbackup apply requires --yes when stdin is not a TTY");
    }
    eprintln!("This will apply Incus snapshot policy config to container `{container}`.");
    eprint!("Proceed? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        anyhow::bail!("setup incusbackup apply cancelled");
    }
}

async fn run_host_service_command(args: HostServiceArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        HostServiceCommand::Unit => {
            run_host_service_logged(
                "host_service.unit",
                format,
                crate::dispatch::setup::host_service::unit,
            )
            .await?;
        }
        HostServiceCommand::Install {
            install_self: install_self_flag,
            yes,
        } => {
            require_host_service_confirmation("install", yes)?;
            if install_self_flag {
                let source = std::env::current_exe()?;
                run_host_service_logged("host_service.install", format, || async move {
                    crate::dispatch::setup::host_service::install_self_transaction(&source).await
                })
                .await?;
                return Ok(());
            }
            run_host_service_logged(
                "host_service.install",
                format,
                crate::dispatch::setup::host_service::install,
            )
            .await?;
        }
        HostServiceCommand::Status => {
            run_host_service_logged(
                "host_service.status",
                format,
                crate::dispatch::setup::host_service::status,
            )
            .await?;
        }
        HostServiceCommand::Restart {
            install_self: install_self_flag,
            yes,
        } => {
            require_host_service_confirmation("restart", yes)?;
            if install_self_flag {
                let source = std::env::current_exe()?;
                run_host_service_logged("host_service.restart", format, || async move {
                    crate::dispatch::setup::host_service::install_self_transaction(&source).await
                })
                .await?;
                return Ok(());
            }
            run_host_service_logged(
                "host_service.restart",
                format,
                crate::dispatch::setup::host_service::restart,
            )
            .await?;
        }
        HostServiceCommand::Rollback { yes } => {
            require_host_service_confirmation("rollback", yes)?;
            run_host_service_logged(
                "host_service.rollback",
                format,
                crate::dispatch::setup::host_service::rollback_previous_release,
            )
            .await?;
        }
        HostServiceCommand::Uninstall { yes } => {
            require_host_service_confirmation("uninstall", yes)?;
            run_host_service_logged(
                "host_service.uninstall",
                format,
                crate::dispatch::setup::host_service::uninstall,
            )
            .await?;
        }
    }
    Ok(())
}

fn require_host_service_confirmation(action: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    let error = crate::dispatch::error::ToolError::ConfirmationRequired {
        message: format!("setup host-service {action} is destructive; pass -y/--yes to confirm"),
    };
    Err(anyhow::anyhow!(
        "{}",
        serde_json::to_string(&error).unwrap_or_else(|_| error.to_string())
    ))
}

async fn run_host_service_logged<F, Fut, T>(
    action: &'static str,
    format: OutputFormat,
    operation: F,
) -> Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, crate::dispatch::error::ToolError>>,
    T: Serialize,
{
    crate::cli::helpers::run_action_command(
        "setup",
        action.to_string(),
        json!({}),
        format,
        |_action, _params| async move {
            let value = operation().await?;
            serde_json::to_value(value).map_err(|err| crate::dispatch::error::ToolError::Sdk {
                sdk_kind: "internal_error".to_string(),
                message: err.to_string(),
            })
        },
    )
    .await?;
    Ok(())
}

async fn run_draft_command(args: DraftArgs, format: OutputFormat) -> Result<()> {
    match args.command {
        DraftCommand::Discard(args) => {
            let params = json!({});
            if args.dry_run {
                crate::cli::helpers::print_dry_run("setup", "draft.discard", &params, format);
                return Ok(());
            }
            crate::cli::helpers::run_confirmable_action_command(
                "setup",
                crate::dispatch::setup::ACTIONS,
                "draft.discard".to_string(),
                params,
                args.yes,
                format,
                |action, params| async move {
                    crate::dispatch::setup::dispatch(&action, params).await
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_plugin_mutation(
    action: &'static str,
    args: PluginMutationArgs,
    format: OutputFormat,
) -> Result<()> {
    let params = json!({
        "service": args.service,
        "confirm": true,
    });
    if args.dry_run {
        crate::cli::helpers::print_dry_run("setup", action, &params, format);
        return Ok(());
    }
    if !args.yes {
        anyhow::bail!("setup {action} is destructive; pass -y/--yes to confirm");
    }
    let value = crate::dispatch::setup::dispatch(action, params).await?;
    print(&value, format)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn interactive_proxy_setup_preserves_case_sensitive_path() {
        let mut input = io::Cursor::new("local\nnone\n/McpServer\nrandom\n50000\n51000\n");
        let mut output = Vec::new();
        let preferences = prompt_proxy_preferences(
            crate::proxy::config::ProxyPreferences::default(),
            &mut input,
            &mut output,
        )
        .expect("interactive proxy preferences");

        assert_eq!(preferences.path, "/McpServer");
        assert_eq!(preferences.port_range_start, 50_000);
        assert_eq!(preferences.port_range_end, 51_000);
    }

    #[tokio::test]
    async fn no_setup_flag_exits_cleanly() {
        let code = run(
            SetupArgs {
                no_setup: true,
                no_browser: true,
                ..Default::default()
            },
            OutputFormat::from_json_flag(
                true,
                crate::output::ColorPolicy::Plain,
                crate::output::RenderEnv::stdout(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn bare_setup_parses_as_default_incus_bootstrap() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "-y"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };

        assert!(args.command.is_none());
        assert!(!args.provision);
        assert!(!args.dry_run);
        assert!(args.yes);
    }

    #[test]
    fn public_proxy_cli_defaults_to_caddy_and_allows_advanced_formats() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "public-proxy",
            "--public-url",
            "https://labby.example.com",
        ])
        .expect("public proxy command");
        assert!(matches!(
            cli.command,
            crate::cli::Command::Setup(SetupArgs {
                command: Some(SetupCommand::PublicProxy(SetupPublicProxyArgs {
                    format: SetupPublicProxyFormat::Caddy,
                    ..
                })),
                ..
            })
        ));

        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "public-proxy",
            "--public-url",
            "https://labby.example.com",
            "--format",
            "traefik",
        ])
        .expect("advanced public proxy command");
        assert!(matches!(
            cli.command,
            crate::cli::Command::Setup(SetupArgs {
                command: Some(SetupCommand::PublicProxy(SetupPublicProxyArgs {
                    format: SetupPublicProxyFormat::Traefik,
                    ..
                })),
                ..
            })
        ));
    }

    #[test]
    fn owner_link_prepare_requires_an_explicit_approval_file() {
        assert!(crate::cli::Cli::try_parse_from(["labby", "setup", "owner-link-prepare"]).is_err());
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "owner-link-prepare",
            "--approval-file",
            "/private/owner-approval.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            crate::cli::Command::Setup(SetupArgs {
                command: Some(SetupCommand::OwnerLinkPrepare(OwnerLinkPrepareArgs { approval_file })),
                ..
            }) if approval_file.as_path() == std::path::Path::new("/private/owner-approval.json")
        ));
    }

    #[test]
    fn parses_setup_wizard_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby", "setup", "wizard", "--mode", "plugin", "--smoke",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Wizard(args)) = args.command else {
            panic!("expected setup wizard subcommand");
        };

        assert!(matches!(args.mode, SetupModeArg::Plugin));
        assert!(args.smoke);
    }

    #[test]
    fn parses_plugin_hook_no_repair_subcommand() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "plugin-hook", "--no-repair"])
            .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::PluginHook { no_repair }) = args.command else {
            panic!("expected plugin-hook subcommand");
        };
        assert!(no_repair);
    }

    #[test]
    fn parses_plugin_sync_export_connectivity_subcommands() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "plugin-sync"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup");
        };
        assert!(matches!(args.command, Some(SetupCommand::PluginSync(_))));

        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "plugin-export"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup");
        };
        assert!(matches!(args.command, Some(SetupCommand::PluginExport)));

        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "plugin-connectivity",
            "--server-url",
            "http://node-a:8765",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup");
        };
        let Some(SetupCommand::PluginConnectivity {
            server_url: Some(url),
        }) = args.command
        else {
            panic!("expected plugin-connectivity with url");
        };
        assert_eq!(url, "http://node-a:8765");
    }

    #[test]
    fn parses_google_oauth_guide_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "google-oauth",
            "--public-url",
            "https://labby.example.com",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::GoogleOauth(args)) = args.command else {
            panic!("expected google-oauth setup helper");
        };
        assert_eq!(args.public_url, "https://labby.example.com");
    }

    #[test]
    fn parses_remote_claude_code_guide_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "claude-code",
            "--name",
            "claude-remote",
            "--ssh-target",
            "operator@remote-host",
            "--claude-path",
            "/Users/operator/.local/bin/claude",
            "--identity-file",
            "/home/labby/.ssh/labby-claude-remote",
            "--known-hosts-file",
            "/home/labby/.ssh/known_hosts.claude-remote",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::ClaudeCode(args)) = args.command else {
            panic!("expected claude-code setup helper");
        };
        assert_eq!(args.name, "claude-remote");
        assert_eq!(args.ssh_target.as_deref(), Some("operator@remote-host"));
        assert_eq!(
            args.claude_path,
            Some(PathBuf::from("/Users/operator/.local/bin/claude"))
        );
    }

    #[test]
    fn claude_code_guide_uses_native_auth_and_hardened_ssh_commands() {
        let args = ClaudeCodeGuideArgs {
            name: "claude-remote".into(),
            claude_path: Some(PathBuf::from("/Users/operator/.local/bin/claude")),
            ssh_target: Some("operator@remote-host".into()),
            identity_file: Some(PathBuf::from("/home/labby/.ssh/labby-claude-remote")),
            known_hosts_file: Some(PathBuf::from("/home/labby/.ssh/known_hosts.claude-remote")),
            apply: false,
            rollback: false,
            yes: false,
        };
        let (value, text) = claude_code_guide(&args).unwrap();
        assert_eq!(value["mode"], "remote_ssh");
        assert!(text.contains("auth login"));
        assert!(text.contains("auth status --text"));
        assert!(text.contains("BatchMode=yes"));
        assert!(text.contains("StrictHostKeyChecking=yes"));
        assert!(text.contains("known_hosts.claude-remote"));
        assert!(text.contains("labby-claude-remote.pub"));
    }

    #[test]
    fn parses_setup_check_and_repair_subcommands() {
        for command in ["check", "repair"] {
            let cli = crate::cli::Cli::try_parse_from(["labby", "setup", command]).unwrap();
            let crate::cli::Command::Setup(args) = cli.command else {
                panic!("expected setup command");
            };
            match (command, args.command) {
                ("check", Some(SetupCommand::Check)) => {}
                ("repair", Some(SetupCommand::Repair)) => {}
                _ => panic!("unexpected setup subcommand for {command}"),
            }
        }
    }

    #[test]
    fn parses_setup_provision_flags() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "--provision",
            "--dry-run",
            "--skip-deps",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };

        assert!(args.provision);
        assert!(args.dry_run);
        assert!(args.skip_deps);
        assert!(args.command.is_none());
    }

    #[test]
    fn parses_incusbackup_apply_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "setup",
            "incusbackup",
            "apply",
            "--name",
            "labby",
            "--config",
            "config/incus/labby-backup.yaml",
            "--dry-run",
        ])
        .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Incusbackup(IncusBackupArgs {
            command:
                IncusBackupCommand::Apply {
                    name,
                    config,
                    dry_run,
                    yes,
                },
        })) = args.command
        else {
            panic!("expected setup incusbackup apply subcommand");
        };
        assert_eq!(name, "labby");
        assert_eq!(config, PathBuf::from("config/incus/labby-backup.yaml"));
        assert!(dry_run);
        assert!(!yes);
    }

    #[test]
    fn accepts_hidden_hyphenated_incus_backup_alias() {
        let cli = crate::cli::Cli::try_parse_from(["labby", "setup", "incus-backup", "validate"])
            .unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        assert!(matches!(
            args.command,
            Some(SetupCommand::Incusbackup(IncusBackupArgs {
                command: IncusBackupCommand::Validate { .. }
            }))
        ));
    }

    #[test]
    fn rejects_setup_incus_subcommand() {
        let err = crate::cli::Cli::try_parse_from(["labby", "setup", "incus"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn parses_top_level_incus_setup_subcommand() {
        let cli = crate::cli::Cli::try_parse_from([
            "labby",
            "incus",
            "setup",
            "--version",
            "v1.2.3",
            "--storage-driver",
            "dir",
            "--name",
            "labby-test",
            "--dry-run",
            "-y",
        ])
        .unwrap();
        let crate::cli::Command::Incus(args) = cli.command else {
            panic!("expected incus command");
        };
        let crate::cli::incus::IncusCommand::Setup(args) = args.command else {
            panic!("expected incus setup subcommand");
        };
        assert_eq!(args.version.as_deref(), Some("v1.2.3"));
        assert_eq!(args.storage_driver.as_deref(), Some("dir"));
        assert_eq!(args.name.as_deref(), Some("labby-test"));
        assert!(args.dry_run);
        assert!(args.yes);
    }

    #[test]
    fn incus_setup_defaults_to_latest_release() {
        let cli =
            crate::cli::Cli::try_parse_from(["labby", "incus", "setup", "--dry-run"]).unwrap();
        let crate::cli::Command::Incus(args) = cli.command else {
            panic!("expected incus command");
        };
        let crate::cli::incus::IncusCommand::Setup(args) = args.command else {
            panic!("expected incus setup subcommand");
        };
        assert_eq!(args.version.as_deref(), Some("latest"));
    }

    #[test]
    fn rejects_hyphenated_incus_bootstrap_subcommand() {
        let err =
            crate::cli::Cli::try_parse_from(["labby", "setup", "incus-bootstrap"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn parses_setup_draft_discard_subcommand() {
        let cli =
            crate::cli::Cli::try_parse_from(["labby", "setup", "draft", "discard", "-y"]).unwrap();
        let crate::cli::Command::Setup(args) = cli.command else {
            panic!("expected setup command");
        };
        let Some(SetupCommand::Draft(DraftArgs {
            command: DraftCommand::Discard(discard),
        })) = args.command
        else {
            panic!("expected setup draft discard subcommand");
        };
        assert!(discard.yes);
    }

    #[test]
    fn parses_host_service_subcommands() {
        for (command, flag, expected) in [
            ("unit", None, HostServiceCommand::Unit),
            (
                "install",
                Some("-y"),
                HostServiceCommand::Install {
                    install_self: false,
                    yes: true,
                },
            ),
            ("status", None, HostServiceCommand::Status),
            (
                "restart",
                Some("-y"),
                HostServiceCommand::Restart {
                    install_self: false,
                    yes: true,
                },
            ),
            (
                "uninstall",
                Some("-y"),
                HostServiceCommand::Uninstall { yes: true },
            ),
            (
                "rollback",
                Some("-y"),
                HostServiceCommand::Rollback { yes: true },
            ),
            (
                "restart",
                Some("--no-confirm"),
                HostServiceCommand::Restart {
                    install_self: false,
                    yes: true,
                },
            ),
        ] {
            let mut args = vec!["labby", "setup", "host-service", command];
            if let Some(flag) = flag {
                args.push(flag);
            }
            let cli = crate::cli::Cli::try_parse_from(args).unwrap();
            let crate::cli::Command::Setup(args) = cli.command else {
                panic!("expected setup command");
            };
            let Some(SetupCommand::HostService(HostServiceArgs { command: actual })) = args.command
            else {
                panic!("expected setup host-service subcommand");
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn parses_host_service_install_self_flag() {
        for (subcommand, expected) in [
            (
                "install",
                HostServiceCommand::Install {
                    install_self: true,
                    yes: true,
                },
            ),
            (
                "restart",
                HostServiceCommand::Restart {
                    install_self: true,
                    yes: true,
                },
            ),
        ] {
            let cli = crate::cli::Cli::try_parse_from([
                "labby",
                "setup",
                "host-service",
                subcommand,
                "--install-self",
                "-y",
            ])
            .unwrap();
            let crate::cli::Command::Setup(args) = cli.command else {
                panic!("expected setup command");
            };
            let Some(SetupCommand::HostService(HostServiceArgs { command })) = args.command else {
                panic!("expected setup host-service subcommand");
            };
            assert_eq!(command, expected);
        }
    }

    #[tokio::test]
    async fn host_service_destructive_commands_require_confirmation_envelope() {
        for command in [
            HostServiceCommand::Install {
                install_self: false,
                yes: false,
            },
            HostServiceCommand::Restart {
                install_self: false,
                yes: false,
            },
            HostServiceCommand::Uninstall { yes: false },
        ] {
            let err = run_host_service_command(
                HostServiceArgs { command },
                OutputFormat::from_json_flag(
                    true,
                    crate::output::ColorPolicy::Plain,
                    crate::output::RenderEnv::stdout(),
                ),
            )
            .await
            .unwrap_err();
            let envelope: Value = serde_json::from_str(&err.to_string()).unwrap();

            assert_eq!(envelope["kind"], "confirmation_required");
        }
    }
}
