//! Public CLI grammar and thin dispatch adapters.
//!
//! The Clap tree is the source of truth for parsing, help, documentation, and completion.

pub mod completion_cache;
pub mod completions;
pub mod config_inspect;
pub mod context;
#[cfg(feature = "gateway")]
pub mod create_server;
pub mod diagnostics;
pub mod docs;
pub mod doctor;
pub mod duration;
#[cfg(feature = "gateway")]
pub mod gateway;
pub mod health;
pub mod help;
pub mod helpers;
pub mod incus;
#[cfg(feature = "gateway")]
pub mod internal;
pub mod login;
pub mod logs;
pub mod migration;
pub mod oauth;
pub mod operator;
pub mod params;
pub mod proxy;
pub mod serve;
#[cfg(feature = "gateway")]
pub mod server;
#[cfg(feature = "gateway")]
pub mod server_lifecycle;
pub mod session;
pub mod setup;
#[cfg(any(feature = "skills", feature = "gateway"))]
pub mod skill;
#[cfg(feature = "skills")]
pub mod skills;
#[cfg(feature = "gateway")]
pub mod snippets;
pub mod state;
pub mod style;
pub mod update;
// [lab-scaffold: cli-modules]

use crate::config::LabConfig;
use crate::output::{ColorPolicy, OutputFormat, RenderEnv};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

/// Manage Labby gateways, upstream servers, and local installations.
#[derive(Debug, Parser)]
#[command(name = "labby", version, about, long_about = None, styles = style::AURORA_STYLES, disable_help_subcommand = true, arg_required_else_help = true, after_help = "Examples:\n  labby help --all\n  labby help --search oauth\n  labby host service status\n\nHelp is offline and never starts services. Use labby help <resource> --all for a complete subtree.")]
pub struct Cli {
    /// Emit machine-readable JSON. Diagnostics never enter stdout.
    #[arg(long, global = true)]
    pub json: bool,
    /// Control human-readable CLI styling.
    #[arg(long, global = true, value_enum, default_value_t = ColorPolicy::Auto)]
    pub color: ColorPolicy,
    /// Include diagnostic events on stderr. Repeat for trace-level detail.
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count, conflicts_with = "quiet")]
    pub verbose: u8,
    /// Suppress console logs, but always report command errors.
    #[arg(short = 'q', long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    /// Never prompt for missing input or confirmation.
    #[arg(long, global = true)]
    pub no_input: bool,
    /// Select a saved destination for a daemon-backed command. Never falls back locally.
    #[arg(long, global = true, conflicts_with = "server")]
    pub context: Option<String>,
    /// Explicit Labby server URL; uses credentials bound to that destination.
    #[arg(long, global = true, conflicts_with = "context")]
    pub server: Option<String>,
    /// Select the Team authority context for team-scoped actions (sent as the x-labby-team-id header to the Labby daemon)
    #[arg(long, global = true, value_name = "TEAM_ID", value_parser = parse_team_id)]
    #[cfg_attr(
        feature = "proxy-testkit",
        arg(env = "LABBY_E2E_TEAM_ID", hide_env = true)
    )]
    pub team_id: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    #[must_use]
    pub fn format(&self) -> OutputFormat {
        OutputFormat::from_json_flag(self.json, self.color, RenderEnv::stdout())
    }
}

fn parse_team_id(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err("team id must not be empty".to_owned());
    }
    if !value.is_ascii() || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("team id must be ASCII without control characters".to_owned());
    }
    Ok(value.to_owned())
}

/// Public resource groups. Skipped variants are internal adapter targets, not aliases.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Discover commands, expand an entire subtree, or search offline help.
    Help(help::HelpArgs),
    /// Save and select non-secret destinations in the existing host configuration.
    Context(context::ContextArgs),
    /// Authenticate to Labby and manage credential bootstrap or OAuth relays.
    Auth(operator::AuthArgs),
    /// Inspect the selected Labby gateway, its sessions, URLs, and usage.
    #[cfg(feature = "gateway")]
    Gateway(gateway::GatewayArgs),
    /// Manage upstream MCP servers: configuration, testing, lifecycle, and authentication.
    #[cfg(feature = "gateway")]
    Server(server::ServerArgs),
    /// Manage public protected MCP routes. Use replace for full configuration replacement.
    #[cfg(feature = "gateway")]
    Route(gateway::GatewayProtectedRouteArgs),
    /// Manage reusable capability loadouts. set patches supplied fields only.
    #[cfg(feature = "gateway")]
    Loadout(gateway::GatewayLoadoutArgs),
    /// Execute Code Mode and manage its settings, UI, and upstream hints.
    #[cfg(feature = "gateway")]
    Code(server::CodeArgs),
    /// Manage saved executable snippets in the local installation.
    #[cfg(feature = "gateway")]
    #[command(name = "snippet")]
    Snippets(snippets::SnippetsArgs),
    /// Read locally visible skills and manage daemon-backed upstream skill policy.
    #[cfg(any(feature = "skills", feature = "gateway"))]
    #[command(
        long_about = "Read locally visible skills and manage daemon-backed upstream skill policy. Local reads do not grant access to shared or private artifact-backed skills; use an authenticated HTTP or MCP client for those."
    )]
    Skill(skill::SkillArgs),
    /// Audit configuration and dependencies without automatically repairing them.
    Doctor(doctor::DoctorArgs),
    /// Query bounded local process logs, or explicitly select the deployment journal.
    Logs(logs::LogsArgs),
    /// Guide onboarding, check prerequisites, or explicitly repair local setup.
    Setup(setup::SetupArgs),
    /// Install, update, or operate the host service and its Incus deployment.
    Host(operator::HostArgs),
    /// Inspect setup state and manage drafts or proxy defaults.
    Config(operator::ConfigArgs),
    /// Migrate, export, verify, or restore durable installation state offline.
    State(state::StateArgs),
    /// Run the Labby HTTP runtime in the foreground.
    Serve(serve::ServeArgs),
    /// Run the Labby stdio MCP transport. stdout contains protocol bytes only.
    Mcp(serve::McpServeArgs),
    /// Proxy a stdio upstream to Streamable HTTP.
    Proxy(proxy::ProxyArgs),
    /// Generate shell completions offline from the same command tree.
    Completions(completions::CompletionsArgs),
    /// Repository documentation generator. Not an operator command.
    #[command(hide = true)]
    Docs(docs::DocsArgs),
    #[command(skip)]
    Session(session::Operation),
    #[command(skip)]
    ConfigInspect(config_inspect::Operation),
    #[command(skip)]
    Login(login::LoginArgs),
    #[command(skip)]
    Health,
    #[command(skip)]
    Incus(incus::IncusArgs),
    #[command(skip)]
    Update(update::UpdateArgs),
    #[command(skip)]
    Oauth(oauth::OauthArgs),
    #[cfg(feature = "skills")]
    #[command(skip)]
    Skills(skills::SkillsArgs),
    #[cfg(feature = "gateway")]
    #[command(hide = true)]
    Internal(internal::InternalArgs),
    // [lab-scaffold: cli-variants]
}

impl Command {
    /// Lower the public grammar into the established typed operations. No I/O occurs here.
    #[must_use]
    pub fn into_operation(self) -> Self {
        match self {
            Self::Auth(args) => args.operation(),
            Self::Host(args) => args.operation(),
            Self::Config(args) => args.operation(),
            #[cfg(feature = "gateway")]
            Self::Server(args) => Self::Gateway(gateway::GatewayArgs {
                command: args.operation(),
            }),
            #[cfg(feature = "gateway")]
            Self::Route(args) => Self::Gateway(gateway::GatewayArgs {
                command: gateway::GatewayCommand::ProtectedRoute(args),
            }),
            #[cfg(feature = "gateway")]
            Self::Loadout(args) => Self::Gateway(gateway::GatewayArgs {
                command: gateway::GatewayCommand::Loadout(args),
            }),
            #[cfg(feature = "gateway")]
            Self::Code(args) => Self::Gateway(gateway::GatewayArgs {
                command: args.operation(),
            }),
            #[cfg(any(feature = "skills", feature = "gateway"))]
            Self::Skill(args) => args.operation(),
            Self::State(state::StateArgs {
                command:
                    state::StateCommand::Access(state::StateAccessArgs {
                        command: state::StateAccessCommand::Migrate,
                    }),
            }) => Self::State(state::StateArgs {
                command: state::StateCommand::MigrateAccess,
            }),
            operation => operation,
        }
    }

    /// Resource label for contexts that have no parsed full command path.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Session(_) => "auth",
            Self::ConfigInspect(_) => "config",
            Self::Help(_) => "help",
            Self::Context(_) => "context",
            Self::Auth(_) => "auth",
            Self::Host(_) => "host",
            Self::Config(_) => "config",
            Self::Serve(_) => "serve",
            Self::Mcp(_) => "mcp",
            Self::Doctor(_) => "doctor",
            Self::Docs(_) => "docs",
            Self::Health => "health",
            Self::Login(_) => "login",
            Self::Logs(_) => "logs",
            Self::Setup(_) => "setup",
            Self::Incus(_) => "incus",
            Self::Update(_) => "update",
            Self::State(_) => "state",
            Self::Completions(_) => "completions",
            #[cfg(feature = "gateway")]
            Self::Gateway(_) => "gateway",
            #[cfg(feature = "gateway")]
            Self::Server(_) => "server",
            #[cfg(feature = "gateway")]
            Self::Route(_) => "route",
            #[cfg(feature = "gateway")]
            Self::Loadout(_) => "loadout",
            #[cfg(feature = "gateway")]
            Self::Code(_) => "code",
            #[cfg(feature = "gateway")]
            Self::Snippets(_) => "snippet",
            #[cfg(any(feature = "skills", feature = "gateway"))]
            Self::Skill(_) => "skill",
            #[cfg(feature = "skills")]
            Self::Skills(_) => "skills",
            Self::Oauth(_) => "oauth",
            Self::Proxy(_) => "proxy",
            #[cfg(feature = "gateway")]
            Self::Internal(_) => "internal",
        }
    }
}

/// Route parsed inputs into the established operations without duplicating policy.
pub fn dispatch(cli: Cli, config: LabConfig) -> impl Future<Output = Result<ExitCode>> {
    Box::pin(helpers::INTERACTIVE.scope(!cli.no_input && !cli.json, dispatch_inner(cli, config)))
}

fn dispatch_inner(mut cli: Cli, mut config: LabConfig) -> impl Future<Output = Result<ExitCode>> {
    Box::pin(async move {
        let format = cli.format();
        context::prepare(&mut cli, &mut config)?;
        #[cfg(feature = "gateway")]
        if let Command::Server(server::ServerArgs {
            command: server::ServerCommand::Add(args),
        }) = &mut cli.command
        {
            create_server::prepare(
                args,
                helpers::interactive_allowed(),
                config.cli_target.as_ref(),
                cli.team_id.as_deref(),
            )?;
        }
        let team_id = cli.team_id;
        let server = cli.server;
        let context = cli.context;
        match cli.command.into_operation() {
            Command::Session(operation) => session::run(operation, &config, format).await,
            Command::ConfigInspect(operation) => config_inspect::run(operation, format),
            Command::Help(args) => help::run(args, format),
            Command::Context(args) => context::run(args, server, team_id, format).await,
            Command::Auth(_) | Command::Host(_) | Command::Config(_) => Err(anyhow::anyhow!(
                "internal CLI lowering failure; no operation was dispatched"
            )),
            #[cfg(feature = "gateway")]
            Command::Server(_) | Command::Route(_) | Command::Loadout(_) | Command::Code(_) => {
                Err(anyhow::anyhow!(
                    "internal gateway CLI lowering failure; no operation was dispatched"
                ))
            }
            #[cfg(any(feature = "skills", feature = "gateway"))]
            Command::Skill(_) => Err(anyhow::anyhow!(
                "internal skill CLI lowering failure; no operation was dispatched"
            )),
            Command::Serve(args) => serve::run(args, &config).await,
            Command::Mcp(args) => serve::run_mcp(args, &config).await,
            Command::Doctor(args) => doctor::run(args, format, &config).await,
            Command::Docs(args) => docs::run(args, format),
            Command::Health => health::run(format).await,
            Command::Logs(args) => logs::run(args, format).await,
            Command::Login(mut args) => {
                if args.server.is_none() {
                    args.server = Some(session::selected_server(&config)?);
                }
                login::run(args, format).await
            }
            Command::Setup(args) => setup::run(args, format).await,
            Command::Incus(args) => incus::run(args, format).await,
            Command::Update(args) => update::run(args, format).await,
            Command::State(args) => state::run(args, format).await,
            Command::Completions(args) => {
                completions::run(args, &config, server, context, team_id, format).await
            }
            #[cfg(feature = "gateway")]
            Command::Gateway(args) => gateway::run(args, format, &config, team_id.as_deref()).await,
            #[cfg(feature = "gateway")]
            Command::Snippets(args) => snippets::run(args, format, &config).await,
            #[cfg(feature = "skills")]
            Command::Skills(args) => skills::run(args, format, &config).await,
            Command::Oauth(args) => oauth::run(args, format, &config).await,
            Command::Proxy(args) => proxy::run(args, &config, format).await,
            #[cfg(feature = "gateway")]
            Command::Internal(args) => internal::run(args),
            // [lab-scaffold: cli-dispatch]
        }
    })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn cli_accepts_operator_login_for_an_explicit_server() {
        assert!(
            Cli::try_parse_from(["labby", "auth", "login", "--server", "https://lab.example"])
                .is_ok()
        );
    }

    #[test]
    fn cli_login_accepts_each_registration_method_and_rejects_ambiguity() {
        for flags in [
            vec!["--dynamic-registration"],
            vec![
                "--client-metadata-url",
                "https://client.example/metadata.json",
            ],
            vec!["--client-id", "registered-cli"],
            vec![
                "--client-id",
                "registered-cli",
                "--client-secret-env",
                "CLI_SECRET",
            ],
        ] {
            assert!(Cli::try_parse_from([vec!["labby", "auth", "login"], flags].concat()).is_ok());
        }
        for flags in [
            vec!["--client-secret-env", "CLI_SECRET"],
            vec!["--dynamic-registration", "--client-id", "registered-cli"],
            vec![
                "--dynamic-registration",
                "--client-metadata-url",
                "https://client.example/id",
            ],
            vec![
                "--client-id",
                "registered-cli",
                "--client-metadata-url",
                "https://client.example/id",
            ],
        ] {
            assert!(Cli::try_parse_from([vec!["labby", "auth", "login"], flags].concat()).is_err());
        }
    }

    #[test]
    fn cli_parses_global_color_flag() {
        let cli = Cli::parse_from(["lab", "--color", "plain", "doctor"]);
        assert_eq!(cli.color, ColorPolicy::Plain);
        assert!(matches!(cli.command.into_operation(), Command::Doctor(_)));
    }

    #[test]
    fn cli_defaults_color_policy_to_auto() {
        let cli = Cli::parse_from(["lab", "doctor"]);
        assert_eq!(cli.color, ColorPolicy::Auto);
        assert!(matches!(cli.command.into_operation(), Command::Doctor(_)));
    }

    #[test]
    fn cli_parses_global_team_id_flag() {
        let cli = Cli::parse_from(["labby", "--team-id", "team-alpha", "doctor"]);
        assert_eq!(cli.team_id.as_deref(), Some("team-alpha"));
        assert!(matches!(cli.command.into_operation(), Command::Doctor(_)));

        // `global = true` means the flag is accepted after the subcommand too.
        let cli = Cli::parse_from(["labby", "doctor", "--team-id", "team-beta"]);
        assert_eq!(cli.team_id.as_deref(), Some("team-beta"));
    }

    #[test]
    fn cli_rejects_empty_team_id() {
        let error = Cli::try_parse_from(["labby", "--team-id", "", "doctor"])
            .expect_err("an empty team id must be rejected at parse time");
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error.to_string().contains("team id must not be empty"),
            "{error}"
        );
    }

    #[test]
    fn cli_rejects_non_ascii_or_control_team_id() {
        for value in ["équipe", "team\u{1}id", "team\nid"] {
            let error = Cli::try_parse_from(["labby", "--team-id", value, "doctor"])
                .expect_err("non-ASCII and control characters must be rejected");
            assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
            assert!(
                error
                    .to_string()
                    .contains("team id must be ASCII without control characters"),
                "{error}"
            );
        }
    }

    #[test]
    fn team_id_help_names_the_daemon_header() {
        use clap::CommandFactory;

        let command = Cli::command();
        let arg = command
            .get_arguments()
            .find(|arg| arg.get_id() == "team_id")
            .expect("--team-id is a top-level argument");
        assert!(arg.is_global_set(), "--team-id must be a global flag");
        assert_eq!(
            arg.get_help().map(ToString::to_string).as_deref(),
            Some(
                "Select the Team authority context for team-scoped actions (sent as the \
                 x-labby-team-id header to the Labby daemon)"
            )
        );
    }

    /// Product builds must never consult the `LABBY_E2E_TEAM_ID` hook that the
    /// live test harness still exports; only `--team-id` selects a Team.
    #[cfg(not(feature = "proxy-testkit"))]
    #[test]
    fn team_id_env_fallback_is_compiled_out_of_product_builds() {
        use clap::CommandFactory;

        let command = Cli::command();
        let arg = command
            .get_arguments()
            .find(|arg| arg.get_id() == "team_id")
            .expect("--team-id is a top-level argument");
        assert!(
            arg.get_env().is_none(),
            "product builds must not bind --team-id to any environment variable"
        );
        assert!(
            Cli::command()
                .get_arguments()
                .all(|arg| arg.get_env() != Some(std::ffi::OsStr::new("LABBY_E2E_TEAM_ID"))),
            "no top-level argument may read LABBY_E2E_TEAM_ID in product builds"
        );
    }

    /// Test-support builds keep the env fallback only until the live harness
    /// migrates to `--team-id`.
    #[cfg(feature = "proxy-testkit")]
    #[test]
    fn team_id_env_fallback_is_limited_to_test_support_builds() {
        use clap::CommandFactory;

        let command = Cli::command();
        let arg = command
            .get_arguments()
            .find(|arg| arg.get_id() == "team_id")
            .expect("--team-id is a top-level argument");
        assert_eq!(
            arg.get_env(),
            Some(std::ffi::OsStr::new("LABBY_E2E_TEAM_ID")),
            "proxy-testkit builds bind --team-id to the transitional harness variable"
        );
        let help = Cli::command().render_long_help().to_string();
        assert!(
            !help.contains("LABBY_E2E_TEAM_ID"),
            "the transitional env hook must stay out of rendered help and generated docs"
        );
    }

    #[test]
    fn cli_doctor_accepts_auth_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "auth"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::Auth(_))
            })
        ));
    }

    #[test]
    fn cli_doctor_accepts_system_subcommand() {
        let cli = Cli::parse_from(["lab", "doctor", "system"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Doctor(doctor::DoctorArgs {
                check: Some(doctor::DoctorCheck::System)
            })
        ));
    }

    #[test]
    fn cli_accepts_top_level_incus_sync() {
        let cli = Cli::parse_from(["labby", "host", "incus", "sync"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Incus(incus::IncusArgs {
                command: incus::IncusCommand::Sync(_)
            })
        ));
    }

    #[test]
    fn cli_accepts_server_owned_updates() {
        let cli = Cli::try_parse_from(["labby", "serve", "--auto-update"]).unwrap();
        let Command::Serve(args) = cli.command.into_operation() else {
            panic!("expected serve");
        };
        assert!(args.auto_update);
    }

    #[test]
    fn cli_accepts_native_automatic_update_modes() {
        for args in [
            vec!["labby", "host", "update", "--automatic"],
            vec!["labby", "host", "update", "--automatic", "--dry-run"],
            vec!["labby", "host", "update", "--auto-update", "enable"],
            vec!["labby", "host", "update", "--auto-update", "disable"],
            vec!["labby", "host", "update", "--auto-update", "status"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        for args in [
            vec![
                "labby",
                "host",
                "update",
                "--automatic",
                "--version",
                "v1.0.0",
            ],
            vec![
                "labby",
                "host",
                "update",
                "--automatic",
                "--auto-update",
                "enable",
            ],
            vec![
                "labby",
                "host",
                "update",
                "--auto-update",
                "enable",
                "--install-dir",
                "/tmp/bin",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn cli_accepts_update_default() {
        let cli = Cli::parse_from(["labby", "host", "update"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Update(update::UpdateArgs { .. })
        ));
    }

    #[test]
    fn cli_doctor_rejects_removed_services_subcommand() {
        let error = Cli::try_parse_from(["lab", "doctor", "services"])
            .expect_err("the synthetic doctor services surface must stay removed");
        assert!(error.to_string().contains("unrecognized subcommand"));
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
    fn cli_accepts_proxy_command_with_js_file() {
        let cli = Cli::parse_from(["labby", "proxy", "/path/to/dist.js"]);
        assert!(matches!(cli.command.into_operation(), Command::Proxy(_)));
    }

    #[test]
    fn cli_proxy_accepts_child_arguments() {
        let cli = Cli::parse_from([
            "labby",
            "proxy",
            "/path/to/dist.js",
            "--workspace",
            "/srv/data",
        ]);
        assert!(
            matches!(cli.command.into_operation(), Command::Proxy(args) if args.command.len() == 3)
        );
    }

    #[test]
    fn cli_proxy_accepts_explicit_separator() {
        let cli = Cli::parse_from([
            "labby",
            "proxy",
            "--",
            "npx",
            "-y",
            "@modelcontextprotocol/server-filesystem",
        ]);
        assert!(matches!(cli.command.into_operation(), Command::Proxy(_)));
    }

    #[test]
    fn cli_proxy_accepts_port_override() {
        let cli = Cli::parse_from(["labby", "proxy", "--port", "52177", "server"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Proxy(args) if args.port == Some(52177)
        ));
    }

    #[test]
    fn cli_proxy_accepts_bearer_token() {
        let cli = Cli::parse_from(["labby", "proxy", "--bearer-token", "secret", "server"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Proxy(args) if args.bearer_token == Some("secret".to_string())
        ));
    }

    #[test]
    fn replacement_setup_commands_parse_and_retired_plugin_install_is_rejected() {
        let cli = Cli::try_parse_from(["labby", "setup"]).expect("setup parses");
        assert!(matches!(cli.command.into_operation(), Command::Setup(_)));

        for command in ["check", "repair"] {
            let cli = Cli::try_parse_from(["labby", "setup", command])
                .unwrap_or_else(|error| panic!("setup {command} must parse: {error}"));
            assert!(matches!(cli.command.into_operation(), Command::Setup(_)));
        }

        let error = Cli::try_parse_from(["labby", "setup", "install-plugin", "gateway", "-y"])
            .expect_err("retired setup install-plugin must stay unavailable");
        assert!(error.to_string().contains("unrecognized subcommand"));
    }

    #[test]
    fn cli_parses_completions_subcommand() {
        let cli = Cli::parse_from(["labby", "completions", "bash"]);
        assert!(matches!(
            cli.command.into_operation(),
            Command::Completions(_)
        ));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn cli_parses_snippets_subcommands() {
        let cli = Cli::parse_from(["labby", "snippet", "list"]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));

        let cli = Cli::parse_from([
            "labby",
            "snippet",
            "run",
            "homelab-readonly-pulse",
            "--param",
            "host=node-a",
        ]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));

        let cli = Cli::parse_from([
            "labby",
            "snippet",
            "add",
            "daily",
            "--file",
            "daily.md",
            "--description",
            "Daily check",
        ]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));

        let cli = Cli::parse_from(["labby", "snippet", "remove", "daily", "-y"]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));

        let cli = Cli::parse_from([
            "labby", "snippet", "validate", "daily", "--file", "daily.md",
        ]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));

        let cli = Cli::parse_from(["labby", "snippet", "test", "daily", "--param", "limit=3"]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));

        let cli = Cli::parse_from(["labby", "snippet", "test", "--all"]);
        assert!(matches!(cli.command.into_operation(), Command::Snippets(_)));
    }
}
