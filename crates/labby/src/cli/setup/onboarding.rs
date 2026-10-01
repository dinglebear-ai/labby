//! Typed first-run onboarding shared by the one-line installer and direct CLI use.

#[cfg(target_os = "linux")]
use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context as _, Result, bail};
use dialoguer::{Confirm, Input, MultiSelect, Password, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(target_os = "linux")]
use sha2::{Digest as _, Sha256};

use super::{SetupArgs, SetupAuthArg, SetupDeploymentArg, SetupOauthArg, SetupRoleArg};
use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::output::{OutputFormat, print};

#[cfg(target_os = "linux")]
const SERVER_ENV: &str = "/home/labby/.labby/.env";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8765;
#[cfg(target_os = "macos")]
const MACOS_SERVICE_INSTALLER: &str =
    include_str!("../../../../../scripts/install-macos-service.sh");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClientAuth {
    OAuth,
    Bearer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
enum OAuthConfig {
    Google {
        client_id: String,
        client_secret: String,
        admin_email: String,
    },
    Authelia {
        issuer_url: String,
        client_id: String,
        client_secret: String,
        admin_email: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SetupPlan {
    #[serde(default)]
    preserve: PreserveDefaults,
    role: SetupRoleArg,
    deployment: Option<SetupDeploymentArg>,
    host: String,
    port: u16,
    server_url: Option<String>,
    public_url: Option<String>,
    server_auth: Option<SetupAuthArg>,
    oauth: Option<OAuthConfig>,
    client_auth: Option<ClientAuth>,
    client_bearer_token: Option<String>,
    #[serde(default)]
    selected_clients: Vec<crate::dispatch::setup::client_registration::ExternalClient>,
    install_desktop: bool,
    no_browser: bool,
    invoking_home: PathBuf,
    invoking_user: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PreserveDefaults {
    host: bool,
    port: bool,
    auth: bool,
    public_url: bool,
}

pub(super) async fn run(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    let interactive = crate::cli::helpers::interactive_allowed() && !args.yes;
    if interactive {
        print_banner();
    }
    let plan = collect_plan(&args, interactive)?;
    if args.dry_run {
        print(&redacted_plan(&plan), format)?;
        return Ok(ExitCode::SUCCESS);
    }

    reject_unsupported_privileged_root(&plan)?;

    if plan.install_desktop && is_unix_root() {
        bail!(
            "desktop installation must run as the desktop user; run labby setup without sudo so only native service installation elevates"
        );
    }

    if requires_root(&plan) && !is_unix_root() {
        elevate_and_apply(&plan)?;
        if plan.install_desktop {
            // The privileged child already printed the server summary; the
            // desktop app is the invoking user's, so its outcome is reported
            // here and never undoes the live server.
            let desktop = report_desktop_install(&plan, &install_desktop);
            print(&desktop.summary(&plan), format)?;
        }
        let handoff = native_browser_handoff(&plan).await;
        let clients = register_selected_clients(&plan).await;
        print(
            &json!({"browser_handoff":handoff,"selected_clients":clients}),
            format,
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    apply(plan, format).await
}

pub(super) async fn apply_plan_file(path: &Path, format: OutputFormat) -> Result<ExitCode> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read setup plan {}", path.display()))?;
    let plan: SetupPlan = serde_json::from_slice(&bytes).context("parse setup plan")?;
    // The plan may contain provider/client secrets. Remove it as soon as the
    // privileged process has a private in-memory copy.
    drop(std::fs::remove_file(path));
    // The plan file is user-writable; the identity this process will chown
    // for comes from sudo's own record, and a plan that disagrees is refused.
    #[cfg(unix)]
    let plan = rederive_invoking_identity(
        plan,
        std::env::var("SUDO_UID").ok().as_deref(),
        std::env::var("SUDO_USER").ok().as_deref(),
        dirs::home_dir(),
    )?;
    reject_unsupported_privileged_root(&plan)?;
    if requires_root(&plan) && !is_unix_root() {
        bail!("native Linux server setup requires root privileges");
    }
    apply(plan, format).await
}

/// Replace the plan's invoking identity with the one sudo reports and refuse
/// a plan that disagrees with it.
///
/// The privileged child restores ownership of `<invoking_home>/.labby` to
/// `invoking_user`, so those values must not come from the user-writable plan
/// file. `SUDO_UID` names the invoking account authoritatively and `SUDO_USER`
/// must agree with it; without sudo's record nothing was delegated.
#[cfg(unix)]
fn rederive_invoking_identity(
    plan: SetupPlan,
    sudo_uid: Option<&str>,
    sudo_user: Option<&str>,
    ambient_home: Option<PathBuf>,
) -> Result<SetupPlan> {
    let (invoking_user, invoking_home) = sudo_invoking_identity(sudo_uid, sudo_user, ambient_home)?;
    if plan.invoking_user != invoking_user || plan.invoking_home != invoking_home {
        bail!(
            "setup plan records invoking user {:?} with home {}, but sudo reports {:?} with home {}; refusing a plan that disagrees with the invoking identity",
            plan.invoking_user,
            plan.invoking_home.display(),
            invoking_user,
            invoking_home.display()
        );
    }
    Ok(SetupPlan {
        invoking_home,
        invoking_user,
        ..plan
    })
}

/// The invoking account as sudo recorded it: `None` plus the process's own
/// home when nothing was delegated (no sudo, or root invoking sudo).
#[cfg(unix)]
fn sudo_invoking_identity(
    sudo_uid: Option<&str>,
    sudo_user: Option<&str>,
    ambient_home: Option<PathBuf>,
) -> Result<(Option<String>, PathBuf)> {
    let own_home = |ambient_home: Option<PathBuf>| {
        ambient_home.context("could not determine the invoking user's home directory")
    };
    let Some(uid) = sudo_uid else {
        if sudo_user.is_some() {
            bail!("SUDO_USER is set without SUDO_UID; cannot corroborate the invoking identity");
        }
        return Ok((None, own_home(ambient_home)?));
    };
    let uid: u32 = uid
        .trim()
        .parse()
        .context("SUDO_UID is not a numeric user id")?;
    let account = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .context("resolve the sudo invoking account")?
        .context("the sudo invoking account does not exist")?;
    if let Some(name) = sudo_user
        && name != account.name
    {
        bail!(
            "SUDO_USER {name:?} does not name the account for SUDO_UID {uid} ({})",
            account.name
        );
    }
    if account.uid.is_root() {
        return Ok((None, own_home(ambient_home)?));
    }
    Ok((Some(account.name), account.dir))
}

fn collect_plan(args: &SetupArgs, interactive: bool) -> Result<SetupPlan> {
    let theme = ColorfulTheme::default();
    let role = match args.role {
        Some(role) => role,
        None if interactive => match Select::with_theme(&theme)
            .with_prompt("How would you like to use Labby?")
            .items(["Use Labby on this computer", "Connect to an existing Labby"])
            .default(0)
            .interact()?
        {
            0 => SetupRoleArg::Server,
            _ => SetupRoleArg::Client,
        },
        None => bail!("non-interactive setup requires --role server|client"),
    };

    let invoking_user = std::env::var("SUDO_USER")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .filter(|value| !value.trim().is_empty() && value != "root");
    let invoking_home =
        invoking_home_for(is_unix_root(), invoking_user.as_deref(), dirs::home_dir())?;

    match role {
        SetupRoleArg::Server => {
            collect_server_plan(args, interactive, &theme, invoking_home, invoking_user)
        }
        SetupRoleArg::Client => {
            collect_client_plan(args, interactive, &theme, invoking_home, invoking_user)
        }
    }
}

#[cfg(unix)]
fn is_unix_root() -> bool {
    nix::unistd::Uid::effective().is_root()
}

#[cfg(not(unix))]
const fn is_unix_root() -> bool {
    false
}

fn invoking_home_for(
    is_root: bool,
    user: Option<&str>,
    ambient_home: Option<PathBuf>,
) -> Result<PathBuf> {
    #[cfg(unix)]
    if is_root && let Some(user) = user {
        return nix::unistd::User::from_name(user)
            .context("resolve invoking user account")?
            .map(|account| account.dir)
            .context("invoking user account does not exist");
    }
    #[cfg(not(unix))]
    let _ = (is_root, user);
    ambient_home.context("could not determine the invoking user's home directory")
}

fn installed_client_program(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            let candidate = directory.join(program);
            let Ok(metadata) = candidate.metadata() else {
                return false;
            };
            if !metadata.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
    })
}

fn collect_selected_clients(
    args: &SetupArgs,
    interactive: bool,
    theme: &ColorfulTheme,
) -> Result<Vec<crate::dispatch::setup::client_registration::ExternalClient>> {
    use crate::dispatch::setup::client_registration::ExternalClient;
    if !args.clients.is_empty() {
        return Ok(args.clients.iter().copied().map(Into::into).collect());
    }
    if !interactive {
        return Ok(Vec::new());
    }
    let detected = [
        ("Codex", "codex", ExternalClient::Codex),
        ("Claude Code", "claude", ExternalClient::ClaudeCode),
    ]
    .into_iter()
    .filter(|(_, program, _)| installed_client_program(program))
    .collect::<Vec<_>>();
    if detected.is_empty() {
        return Ok(Vec::new());
    }
    let selected = MultiSelect::with_theme(theme)
        .with_prompt("Where would you like to use Labby? Select installed clients, or leave empty to use Labby only")
        .items(detected.iter().map(|(label, _, _)| *label).collect::<Vec<_>>())
        .interact()?;
    Ok(selected
        .into_iter()
        .map(|index| detected[index].2)
        .collect())
}

fn fresh_local_port(preferred: u16, root: &Path) -> Result<u16> {
    // Existing installations retain their configured setup target. An occupied
    // port on a fresh loopback installation can be replaced with an OS-selected
    // port; the service still performs the final bind and reports any race.
    if root.join(".env").exists() || root.join("config.toml").exists() {
        return Ok(preferred);
    }
    match std::net::TcpListener::bind((DEFAULT_HOST, preferred)) {
        Ok(listener) => Ok(listener.local_addr()?.port()),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            let listener = std::net::TcpListener::bind((DEFAULT_HOST, 0))
                .context("select an available loopback port for Labby")?;
            Ok(listener.local_addr()?.port())
        }
        Err(error) => Err(error).context("check the local Labby listener"),
    }
}

fn collect_server_plan(
    args: &SetupArgs,
    interactive: bool,
    theme: &ColorfulTheme,
    invoking_home: PathBuf,
    invoking_user: Option<String>,
) -> Result<SetupPlan> {
    let advanced = interactive && args.advanced;
    let incus_ready =
        cfg!(all(target_os = "linux", target_arch = "x86_64")) && command_ok("incus", &["version"]);
    let deployment = match args.deployment {
        Some(value) => value,
        None if advanced && incus_ready => match Select::with_theme(theme)
            .with_prompt("Deployment")
            .items(["Native service — fastest", "Incus container — isolated"])
            .default(0)
            .interact()?
        {
            0 => SetupDeploymentArg::Native,
            _ => SetupDeploymentArg::Incus,
        },
        None => SetupDeploymentArg::Native,
    };
    if matches!(deployment, SetupDeploymentArg::Incus) && !incus_ready {
        bail!("Incus deployment was selected, but a usable local Incus daemon was not detected");
    }

    let host = match args.host.as_deref() {
        Some(value) => validate_host(value)?,
        None if advanced => validate_host(
            &Input::<String>::with_theme(theme)
                .with_prompt("Listen address")
                .default(DEFAULT_HOST.to_string())
                .interact_text()?,
        )?,
        None => DEFAULT_HOST.to_string(),
    };
    let port = match args.port {
        Some(port) if port > 0 => port,
        Some(_) => bail!("port must be between 1 and 65535"),
        None if advanced => Input::<u16>::with_theme(theme)
            .with_prompt("Port")
            .default(DEFAULT_PORT)
            .validate_with(|value: &u16| {
                if *value == 0 {
                    Err("port must be non-zero")
                } else {
                    Ok(())
                }
            })
            .interact_text()?,
        None if !advanced
            && matches!(deployment, SetupDeploymentArg::Native)
            && host == DEFAULT_HOST =>
        {
            let root = std::env::var_os("LABBY_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| invoking_home.join(".labby"));
            let root = crate::installation::InstallationPaths::from_root(root)?
                .root()
                .to_path_buf();
            fresh_local_port(DEFAULT_PORT, &root)?
        }
        None => DEFAULT_PORT,
    };

    let server_auth = match args.auth {
        Some(value) => value,
        // Backwards compatibility: before --auth existed, selecting an OAuth
        // provider also retained Labby's static bearer break-glass credential.
        None if matches!(
            args.oauth,
            Some(SetupOauthArg::Google | SetupOauthArg::Authelia)
        ) =>
        {
            SetupAuthArg::Both
        }
        None if advanced => match Select::with_theme(theme)
            .with_prompt("Authentication")
            .items(["Bearer token", "OAuth only", "OAuth + bearer break-glass"])
            .default(0)
            .interact()?
        {
            1 => SetupAuthArg::OAuth,
            2 => SetupAuthArg::Both,
            _ => SetupAuthArg::Bearer,
        },
        None => SetupAuthArg::Bearer,
    };

    let provider = match server_auth {
        SetupAuthArg::Bearer => {
            if matches!(
                args.oauth,
                Some(SetupOauthArg::Google | SetupOauthArg::Authelia)
            ) {
                bail!("--auth bearer cannot be combined with --oauth google|authelia");
            }
            SetupOauthArg::None
        }
        SetupAuthArg::OAuth | SetupAuthArg::Both => match args.oauth {
            Some(SetupOauthArg::Google) => SetupOauthArg::Google,
            Some(SetupOauthArg::Authelia) => SetupOauthArg::Authelia,
            Some(SetupOauthArg::None) => {
                bail!("--auth oauth|both requires --oauth google|authelia")
            }
            None if interactive => match Select::with_theme(theme)
                .with_prompt("OAuth provider")
                .items(["Google", "Authelia"])
                .default(0)
                .interact()?
            {
                1 => SetupOauthArg::Authelia,
                _ => SetupOauthArg::Google,
            },
            None => bail!("OAuth authentication requires --oauth google|authelia"),
        },
    };

    let (public_url, oauth) = match provider {
        SetupOauthArg::None => (args.public_url.clone(), None),
        SetupOauthArg::Google => {
            let public_url = required_public_url(
                args.public_url.as_deref(),
                interactive,
                theme,
                host.as_str(),
                port,
            )?;
            if interactive {
                print_google_oauth_setup_guidance(&public_url);
            }
            let client_id = prompt_required(
                "Google client ID",
                "LABBY_GOOGLE_CLIENT_ID",
                interactive,
                theme,
            )?;
            let client_secret = prompt_secret(
                "Google client secret",
                "LABBY_GOOGLE_CLIENT_SECRET",
                interactive,
                theme,
            )?;
            let admin_email = prompt_required(
                "Bootstrap admin email",
                "LABBY_AUTH_ADMIN_EMAIL",
                interactive,
                theme,
            )?;
            (
                Some(public_url),
                Some(OAuthConfig::Google {
                    client_id,
                    client_secret,
                    admin_email,
                }),
            )
        }
        SetupOauthArg::Authelia => {
            let public_url = required_public_url(
                args.public_url.as_deref(),
                interactive,
                theme,
                host.as_str(),
                port,
            )?;
            let issuer_url = prompt_required(
                "Authelia issuer URL",
                "LABBY_AUTHELIA_ISSUER_URL",
                interactive,
                theme,
            )?;
            validate_https_url(&issuer_url, "Authelia issuer URL")?;
            let client_id = prompt_required(
                "Authelia client ID",
                "LABBY_AUTHELIA_CLIENT_ID",
                interactive,
                theme,
            )?;
            let client_secret = prompt_secret(
                "Authelia client secret",
                "LABBY_AUTHELIA_CLIENT_SECRET",
                interactive,
                theme,
            )?;
            let admin_email = prompt_required(
                "Bootstrap admin email",
                "LABBY_AUTH_ADMIN_EMAIL",
                interactive,
                theme,
            )?;
            (
                Some(public_url),
                Some(OAuthConfig::Authelia {
                    issuer_url,
                    client_id,
                    client_secret,
                    admin_email,
                }),
            )
        }
    };

    let install_desktop = desktop_choice(args, interactive, theme)?;
    Ok(SetupPlan {
        preserve: PreserveDefaults {
            host: !advanced && args.host.is_none(),
            port: !advanced && args.port.is_none(),
            auth: !advanced && args.auth.is_none() && args.oauth.is_none(),
            public_url: args.public_url.is_none(),
        },
        role: SetupRoleArg::Server,
        deployment: Some(deployment),
        host,
        port,
        server_url: None,
        public_url,
        server_auth: Some(server_auth),
        oauth,
        client_auth: None,
        client_bearer_token: None,
        install_desktop,
        selected_clients: collect_selected_clients(args, interactive, theme)?,
        no_browser: args.no_browser,
        invoking_home,
        invoking_user,
    })
}

fn collect_client_plan(
    args: &SetupArgs,
    interactive: bool,
    theme: &ColorfulTheme,
    invoking_home: PathBuf,
    invoking_user: Option<String>,
) -> Result<SetupPlan> {
    if args.auth.is_some() {
        bail!(
            "--auth configures server authentication topology; client mode selects browser OAuth or bearer with --oauth"
        )
    }
    let raw_url = match args.server_url.as_deref() {
        Some(value) => value.to_string(),
        None if interactive => Input::<String>::with_theme(theme)
            .with_prompt("Labby server URL")
            .interact_text()?,
        None => bail!("client setup requires --server-url in non-interactive mode"),
    };
    let server = crate::oauth::cli_session::server_url(&raw_url)?;
    let server_url = server.as_str().trim_end_matches('/').to_string();

    let client_auth = if let Some(provider) = args.oauth {
        if matches!(provider, SetupOauthArg::None) {
            ClientAuth::Bearer
        } else {
            ClientAuth::OAuth
        }
    } else if interactive {
        match Select::with_theme(theme)
            .with_prompt("Client authentication")
            .items(["Browser sign-in (OAuth)", "Bearer token"])
            .default(0)
            .interact()?
        {
            1 => ClientAuth::Bearer,
            _ => ClientAuth::OAuth,
        }
    } else if std::env::var("LABBY_MCP_HTTP_TOKEN").is_ok_and(|v| !v.trim().is_empty()) {
        ClientAuth::Bearer
    } else {
        ClientAuth::OAuth
    };
    validate_client_browser_mode(client_auth, args.no_browser)?;
    let client_bearer_token = if matches!(client_auth, ClientAuth::Bearer) {
        Some(
            match std::env::var("LABBY_MCP_HTTP_TOKEN")
                .ok()
                .filter(|v| !v.trim().is_empty())
            {
                Some(value) => value,
                None if interactive => Password::with_theme(theme)
                    .with_prompt("Bearer token")
                    .allow_empty_password(false)
                    .interact()?,
                None => bail!(
                    "bearer client setup requires LABBY_MCP_HTTP_TOKEN in non-interactive mode"
                ),
            },
        )
    } else {
        None
    };

    Ok(SetupPlan {
        preserve: PreserveDefaults::default(),
        role: SetupRoleArg::Client,
        deployment: None,
        host: DEFAULT_HOST.to_string(),
        port: DEFAULT_PORT,
        server_url: Some(server_url),
        public_url: None,
        server_auth: None,
        oauth: None,
        client_auth: Some(client_auth),
        client_bearer_token,
        selected_clients: collect_selected_clients(args, interactive, theme)?,
        install_desktop: desktop_choice(args, interactive, theme)?,
        no_browser: args.no_browser,
        invoking_home,
        invoking_user,
    })
}

fn validate_client_browser_mode(auth: ClientAuth, no_browser: bool) -> Result<()> {
    if no_browser && matches!(auth, ClientAuth::OAuth) {
        bail!(
            "OAuth client setup requires browser authorization; omit --no-browser or use --oauth none with LABBY_MCP_HTTP_TOKEN for bearer authentication"
        );
    }
    Ok(())
}

fn desktop_choice(args: &SetupArgs, interactive: bool, theme: &ColorfulTheme) -> Result<bool> {
    resolve_desktop_choice(args, interactive, desktop_supported(), |default| {
        Ok(Confirm::with_theme(theme)
            .with_prompt(
                "Install the Labby desktop app? (optional; requires a published desktop package)",
            )
            .default(default)
            .interact()?)
    })
}

/// Desktop installation is opt-in. The interactive prompt defaults to No
/// because the desktop package is additive to a release and may not be
/// published for this platform or version; explicit flags win, and
/// non-interactive or unsupported hosts never prompt.
fn resolve_desktop_choice(
    args: &SetupArgs,
    interactive: bool,
    supported: bool,
    prompt: impl FnOnce(bool) -> Result<bool>,
) -> Result<bool> {
    if args.desktop {
        return Ok(true);
    }
    if args.no_desktop {
        return Ok(false);
    }
    if !supported || !interactive {
        return Ok(false);
    }
    prompt(false)
}

fn desktop_supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
        || (cfg!(all(target_os = "linux", target_arch = "x86_64"))
            && (std::env::var_os("DISPLAY").is_some()
                || std::env::var_os("WAYLAND_DISPLAY").is_some()))
}

fn required_public_url(
    configured: Option<&str>,
    interactive: bool,
    theme: &ColorfulTheme,
    host: &str,
    port: u16,
) -> Result<String> {
    let default = if host == "127.0.0.1" || host == "localhost" {
        format!("http://127.0.0.1:{port}")
    } else {
        String::new()
    };
    let value = match configured {
        Some(value) => value.to_string(),
        None if interactive => Input::<String>::with_theme(theme)
            .with_prompt("Public browser / OAuth URL")
            .with_initial_text(default)
            .interact_text()?,
        None => bail!("OAuth setup requires --public-url in non-interactive mode"),
    };
    validate_public_url(&value)?;
    Ok(value.trim_end_matches('/').to_string())
}

fn google_callback_url(public_url: &str) -> String {
    format!("{}/auth/google/callback", public_url.trim_end_matches('/'))
}

fn print_google_oauth_setup_guidance(public_url: &str) {
    let callback_url = google_callback_url(public_url);
    eprintln!(
        "\nGoogle OAuth setup\n  1. In Google Auth Platform, create an OAuth client of type Web application.\n  2. Add this exact Authorized redirect URI:\n     {callback_url}\n  3. Copy the Client ID and Client secret back into this setup flow.\n\nChatGPT web: Labby must use OAuth and a publicly reachable HTTPS public URL. Bearer-only mode cannot be used for the Labby ChatGPT web connection.\n"
    );
}

fn validate_public_url(raw: &str) -> Result<()> {
    let url = url::Url::parse(raw).context("public URL is not a valid URL")?;
    let loopback = matches!(url.host(), Some(url::Host::Ipv4(v)) if v.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(v)) if v.is_loopback())
        || matches!(url.host(), Some(url::Host::Domain(v)) if v.eq_ignore_ascii_case("localhost"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("public OAuth URL must use HTTPS except for loopback development origins");
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("public OAuth URL must not contain credentials, query, or fragment");
    }
    Ok(())
}

fn validate_https_url(raw: &str, label: &str) -> Result<()> {
    let url = url::Url::parse(raw).with_context(|| format!("{label} is not a valid URL"))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("{label} must be an HTTPS URL without credentials, query, or fragment");
    }
    Ok(())
}

fn validate_host(raw: &str) -> Result<String> {
    let host = raw.trim();
    if host.is_empty()
        || host.contains('/')
        || host.contains("http://")
        || host.contains("https://")
    {
        bail!("listen address must be a host or IP, not a URL");
    }
    Ok(host.to_string())
}

fn prompt_required(
    label: &str,
    env_key: &str,
    interactive: bool,
    theme: &ColorfulTheme,
) -> Result<String> {
    if let Some(value) = std::env::var(env_key).ok().filter(|v| !v.trim().is_empty()) {
        return Ok(value);
    }
    if !interactive {
        bail!("{label} is required; set {env_key}");
    }
    Ok(Input::<String>::with_theme(theme)
        .with_prompt(label)
        .validate_with(|value: &String| {
            if value.trim().is_empty() {
                Err("value is required")
            } else {
                Ok(())
            }
        })
        .interact_text()?
        .trim()
        .to_string())
}

fn prompt_secret(
    label: &str,
    env_key: &str,
    interactive: bool,
    theme: &ColorfulTheme,
) -> Result<String> {
    if let Some(value) = std::env::var(env_key).ok().filter(|v| !v.trim().is_empty()) {
        return Ok(value);
    }
    if !interactive {
        bail!("{label} is required; set {env_key}");
    }
    Ok(Password::with_theme(theme)
        .with_prompt(label)
        .allow_empty_password(false)
        .interact()?)
}

fn requires_root(plan: &SetupPlan) -> bool {
    cfg!(target_os = "linux")
        && matches!(plan.role, SetupRoleArg::Server)
        && matches!(plan.deployment, Some(SetupDeploymentArg::Native))
}

fn reject_unsupported_privileged_root(plan: &SetupPlan) -> Result<()> {
    if requires_root(plan) && std::env::var_os("LABBY_HOME").is_some_and(|value| !value.is_empty())
    {
        bail!(
            "native Linux service setup uses /home/labby/.labby for server state and cannot safely write an invoking-user custom LABBY_HOME while elevated; use the default invoking-user state root or configure the client separately as that user"
        );
    }
    Ok(())
}

fn privileged_plan(plan: &SetupPlan) -> SetupPlan {
    let mut privileged = plan.clone();
    privileged.install_desktop = false;
    privileged
}

fn elevate_and_apply(plan: &SetupPlan) -> Result<ExitCode> {
    let plan_dir = plan.invoking_home.join(".labby");
    std::fs::create_dir_all(&plan_dir)?;
    let mut plan_file = tempfile::NamedTempFile::new_in(&plan_dir)?;
    // User application files and gh authentication belong to the original process.
    serde_json::to_writer(&mut plan_file, &privileged_plan(plan))?;
    plan_file.write_all(
        b"
",
    )?;
    plan_file.as_file().sync_all()?;
    let executable = std::env::current_exe().context("resolve current Labby executable")?;
    eprintln!(
        "
Labby needs administrator access to install the native system service."
    );
    let status = Command::new("sudo")
        .arg(executable)
        .arg("setup")
        .arg("--apply-plan")
        .arg(plan_file.path())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("launch sudo for native server installation")?;
    if !status.success() {
        bail!("privileged Labby setup exited with {status}");
    }
    Ok(ExitCode::SUCCESS)
}

async fn apply(mut plan: SetupPlan, format: OutputFormat) -> Result<ExitCode> {
    if matches!(plan.role, SetupRoleArg::Server)
        && matches!(plan.deployment, Some(SetupDeploymentArg::Native))
    {
        #[cfg(target_os = "linux")]
        let root = Path::new(SERVER_ENV)
            .parent()
            .context("server environment has no parent")?
            .to_path_buf();
        #[cfg(not(target_os = "linux"))]
        let root = invoking_installation_root(&plan)?;
        resolve_existing_native_defaults(&mut plan, &root)?;
    }
    let mut result = match plan.role {
        SetupRoleArg::Server => apply_server(&plan, format).await?,
        SetupRoleArg::Client => apply_client(&plan).await?,
    };
    let handoff = native_browser_handoff(&plan).await;
    let clients = register_selected_clients(&plan).await;
    if let Some(object) = result.as_object_mut() {
        object.insert("browser_handoff".into(), json!(handoff));
        object.insert("selected_clients".into(), clients);
    }
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
}

async fn register_selected_clients(plan: &SetupPlan) -> serde_json::Value {
    if plan.selected_clients.is_empty() {
        return json!({"status":"not_selected","connected":false});
    }
    if is_unix_root() {
        return json!({"status":"deferred_to_invoking_user","connected":false});
    }
    let root = match invoking_installation_root(plan) {
        Ok(root) => root,
        Err(error) => {
            return json!({"status":"needs_attention","connected":false,"message":error.to_string()});
        }
    };
    let (gateway, bearer) = match crate::dispatch::setup::client_bridge::saved_connection_identity(
        &root,
    ) {
        Ok(identity) => identity,
        Err(error) => {
            return json!({"status":"needs_attention","connected":false,"message":error.to_string()});
        }
    };
    if let Some(requested) = plan.server_url.as_ref() {
        if crate::config::cli::normalize_server(requested)
            .ok()
            .as_deref()
            != Some(gateway.as_str())
        {
            return json!({"status":"needs_attention","connected":false,"message":"Saved client connection does not match the selected gateway"});
        }
    }
    let oauth_only = !bearer;
    let results = if oauth_only {
        let gateway = format!(
            "{}/mcp",
            gateway.trim_end_matches('/').trim_end_matches("/mcp")
        );
        crate::dispatch::setup::client_registration::register_oauth_clients(
            plan.invoking_home.clone(),
            root,
            gateway,
            plan.selected_clients.clone(),
        )
        .await
    } else {
        let binary = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                return json!({"status":"needs_attention","connected":false,"message":error.to_string()});
            }
        };
        crate::dispatch::setup::client_registration::register_saved_clients(
            plan.invoking_home.clone(),
            root,
            gateway,
            plan.selected_clients.clone(),
            binary,
        )
        .await
    };
    json!({"status":"client_use_not_verified","connected":false,"clients":results})
}

async fn native_browser_handoff(plan: &SetupPlan) -> &'static str {
    if !matches!(plan.role, SetupRoleArg::Server)
        || !matches!(plan.deployment, Some(SetupDeploymentArg::Native))
        || plan.no_browser
    {
        return "not_requested";
    }
    if is_unix_root() {
        return "deferred_to_invoking_user";
    }
    let Ok(root) = invoking_installation_root(plan) else {
        return "needs_attention";
    };
    let Some(token) = read_env(&root.join(".env"), "LABBY_MCP_HTTP_TOKEN") else {
        return "needs_attention";
    };
    if token.trim().is_empty() {
        return "not_applicable_to_saved_auth";
    }
    let Some(server) = read_env(&root.join(".env"), "LABBY_SERVER_URL") else {
        return "needs_attention";
    };
    match super::browser_handoff::open(&server, &token).await {
        Ok(()) => "opened",
        Err(_) => {
            eprintln!(
                "Labby is installed. The local browser handoff needs attention; rerun setup to obtain a fresh link."
            );
            "needs_attention"
        }
    }
}

async fn apply_server(plan: &SetupPlan, format: OutputFormat) -> Result<serde_json::Value> {
    let deployment = plan
        .deployment
        .context("server setup requires a deployment backend")?;
    match deployment {
        SetupDeploymentArg::Native => apply_native_server(plan).await,
        SetupDeploymentArg::Incus => apply_incus_server(plan, format).await,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn apply_native_server(plan: &SetupPlan) -> Result<serde_json::Value> {
    #[cfg(target_os = "linux")]
    {
        if !is_unix_root() {
            bail!("native Linux server setup requires root privileges");
        }
        crate::dispatch::setup::provision::ensure_lab_user()
            .await
            .map_err(|e| anyhow::anyhow!("create Labby service account: {e}"))?;
        let env_path = Path::new(SERVER_ENV);
        let token = configure_server_env(env_path, plan)?;
        chown_tree(Path::new("/home/labby/.labby"), "labby")?;
        let executable = std::env::current_exe().context("resolve current Labby executable")?;
        let outcome = crate::dispatch::setup::host_service::install_self_transaction(&executable)
            .await
            .map_err(|e| anyhow::anyhow!("install Labby system service: {e}"))?;
        if matches!(resolved_server_auth(plan), SetupAuthArg::Bearer) {
            // Access stores enforce same-user ownership, including during health
            // inspection. Bootstrap with the installed executable as the service
            // account, then refresh the daemon's cached admission state.
            run_status(
                "runuser",
                &[
                    "-u",
                    "labby",
                    "--",
                    "env",
                    "LABBY_HOME=/home/labby/.labby",
                    "/usr/local/bin/labby",
                    "setup",
                    "--bootstrap-static-owner",
                ],
                "bootstrap native static bearer owner",
            )?;
            crate::dispatch::setup::host_service::restart()
                .await
                .map_err(|e| anyhow::anyhow!("refresh native owner admission: {e}"))?;
        }
        configure_local_client(plan, token.as_deref())?;
        let desktop = report_desktop_install(plan, &install_desktop);
        return Ok(with_desktop_summary(
            json!({
                "ok": true,
                "role": "server",
                "deployment": "native",
                "service": outcome.message,
                "web": advertised_url(plan),
                "mcp": format!("{}/mcp", advertised_url(plan).trim_end_matches('/')),
                "client_configured": true,
                "local_cli_configured": true,
                "features": "full",
                "installation_complete": true,
                "first_use_status": "not_verified",
                "first_use_checks_required": ["agent_provider", "starter_agent_run", "selected_clients", "discover", "mcp_tool_call"],
            }),
            plan,
            &desktop,
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let home_env = invoking_installation_root(plan)?.join(".env");
        let token = configure_server_env(&home_env, plan)?;
        if matches!(resolved_server_auth(plan), SetupAuthArg::Bearer) {
            bootstrap_static_owner_at(
                home_env
                    .parent()
                    .context("server environment has no parent")?,
            )
            .await?;
        }
        install_macos_service(plan)?;
        configure_local_client(plan, token.as_deref())?;
        let desktop = report_desktop_install(plan, &install_desktop);
        return Ok(with_desktop_summary(
            json!({
                "ok": true,
                "role": "server",
                "deployment": "native",
                "persistence": "launch_agent_at_login",
                "web": advertised_url(plan),
                "mcp": format!("{}/mcp", advertised_url(plan).trim_end_matches('/')),
                "features": "full",
                "local_cli_configured": true,
                "installation_complete": true,
                "first_use_status": "not_verified",
                "first_use_checks_required": ["agent_provider", "starter_agent_run", "selected_clients", "discover", "mcp_tool_call"],
            }),
            plan,
            &desktop,
        ));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn apply_native_server(_plan: &SetupPlan) -> Result<serde_json::Value> {
    bail!(
        "native persistent server installation is not yet supported on this operating system; use client mode"
    )
}

#[cfg(target_os = "linux")]
async fn apply_incus_server(plan: &SetupPlan, format: OutputFormat) -> Result<serde_json::Value> {
    let image = ensure_release_incus_image()?;
    let args = crate::cli::incus::IncusSetupArgs {
        image: Some(image),
        version: Some(format!("v{}", env!("CARGO_PKG_VERSION"))),
        yes: true,
        dry_run: false,
        ..crate::cli::incus::IncusSetupArgs::default()
    };
    crate::cli::incus::run_setup(args, format).await?;
    // The installed release owns the guest's internal :8765 listener. Host
    // publishing and operator-specific authentication are applied afterwards,
    // keeping immutable image bytes free of user secrets.
    let token = configure_incus_server(plan)?;
    converge_incus_publish("labby", &plan.host, plan.port)?;
    configure_local_client(plan, token.as_deref())?;
    let desktop = report_desktop_install(plan, &install_desktop);
    Ok(with_desktop_summary(
        json!({
            "ok": true,
            "role": "server",
            "deployment": "incus",
            "web": advertised_url(plan),
            "mcp": format!("{}/mcp", advertised_url(plan).trim_end_matches('/')),
            "features": "full",
        }),
        plan,
        &desktop,
    ))
}

#[cfg(not(target_os = "linux"))]
async fn apply_incus_server(_plan: &SetupPlan, _format: OutputFormat) -> Result<serde_json::Value> {
    bail!("Incus server deployment is currently supported only on Linux hosts")
}

#[cfg(target_os = "linux")]
fn ensure_release_incus_image() -> Result<String> {
    const IMAGE_ASSET: &str = "labby-incus-x86_64-unknown-linux-gnu.tar.xz";
    const REPO: &str = "dinglebear-ai/labby";
    const SIGNER_WORKFLOW: &str = "dinglebear-ai/labby/.github/workflows/build-incus-image.yml";

    if !cfg!(target_arch = "x86_64") {
        bail!("the prebuilt Labby Incus image is currently published only for x86_64 Linux hosts");
    }
    if !command_ok("gh", &["--version"]) {
        bail!("GitHub CLI (gh) is required to verify the Labby release manifest and Incus image");
    }

    let pointer = Command::new("gh")
        .args([
            "api",
            "repos/dinglebear-ai/labby/git/ref/tags/labby-incus-latest",
            "--jq",
            ".object.sha",
        ])
        .output()
        .context("resolve latest qualified Incus image")?;
    if !pointer.status.success() {
        bail!("could not resolve the qualified Incus image pointer");
    }
    let commit = String::from_utf8(pointer.stdout)
        .context("Incus image pointer is not UTF-8")?
        .trim()
        .to_string();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Incus image pointer is not a commit SHA");
    }
    let tag = format!("incus-{commit}");
    let temp = tempfile::tempdir().context("create Incus release download directory")?;
    let temp_path = temp
        .path()
        .to_str()
        .context("Incus release temp path is not UTF-8")?;

    run_status(
        "gh",
        &[
            "release",
            "download",
            &tag,
            "--repo",
            REPO,
            "--pattern",
            "incus-image-manifest.json",
            "--pattern",
            IMAGE_ASSET,
            "--dir",
            temp_path,
        ],
        "download qualified Incus image release",
    )?;
    let manifest = temp.path().join("incus-image-manifest.json");
    let image = temp.path().join(IMAGE_ASSET);
    if !manifest.is_file() || !image.is_file() {
        bail!("image release {tag} is missing its immutable manifest or prebuilt Incus image");
    }

    run_status(
        "gh",
        &[
            "attestation",
            "verify",
            manifest
                .to_str()
                .context("release manifest path is not UTF-8")?,
            "--repo",
            REPO,
            "--signer-workflow",
            SIGNER_WORKFLOW,
            "--source-ref",
            "refs/heads/main",
            "--source-digest",
            &commit,
            "--deny-self-hosted-runners",
        ],
        "verify Incus image manifest provenance",
    )?;
    run_status(
        "gh",
        &[
            "attestation",
            "verify",
            image.to_str().context("Incus image path is not UTF-8")?,
            "--repo",
            REPO,
            "--signer-workflow",
            SIGNER_WORKFLOW,
            "--source-ref",
            "refs/heads/main",
            "--source-digest",
            &commit,
            "--deny-self-hosted-runners",
        ],
        "verify Incus image provenance",
    )?;

    let manifest_value: serde_json::Value = serde_json::from_reader(
        std::fs::File::open(&manifest).context("open verified release manifest")?,
    )
    .context("parse verified release manifest")?;
    if manifest_value
        .get("schema")
        .and_then(serde_json::Value::as_str)
        != Some("ai.dinglebear.labby/incus-image-manifest/v1")
        || manifest_value
            .get("commit")
            .and_then(serde_json::Value::as_str)
            != Some(commit.as_str())
        || manifest_value
            .get("tag")
            .and_then(serde_json::Value::as_str)
            != Some(tag.as_str())
    {
        bail!("Incus image manifest does not match the qualified pointer");
    }
    let incus = manifest_value
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .and_then(|assets| {
            assets.iter().find(|asset| {
                asset.get("name").and_then(serde_json::Value::as_str) == Some(IMAGE_ASSET)
            })
        })
        .and_then(serde_json::Value::as_object)
        .context("image manifest is missing the Incus asset")?;
    let manifest_asset = incus
        .get("asset")
        .and_then(serde_json::Value::as_str)
        .context("image manifest is missing the Incus asset name")?;
    let expected_sha256 = incus
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .context("image manifest is missing the Incus SHA-256")?;
    if manifest_asset != IMAGE_ASSET {
        bail!(
            "image manifest Incus asset {manifest_asset:?} does not match the supported asset {IMAGE_ASSET:?}"
        );
    }
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("image manifest contains an invalid Incus SHA-256");
    }
    let actual_sha256 = sha256_file(&image)?;
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "prebuilt Incus image digest mismatch: expected {expected_sha256}, got {actual_sha256}"
        );
    }

    let alias = format!("labby-image-{}", &actual_sha256[..12]);
    if command_ok("incus", &["image", "info", &alias]) {
        return Ok(alias);
    }
    run_status(
        "incus",
        &[
            "image",
            "import",
            image.to_str().context("Incus image path is not UTF-8")?,
            "--alias",
            &alias,
        ],
        "import verified Labby Incus image",
    )?;
    Ok(alias)
}

#[cfg(target_os = "linux")]
fn configure_incus_server(plan: &SetupPlan) -> Result<Option<String>> {
    const CONTAINER: &str = "labby";
    const REMOTE_ENV: &str = "/home/labby/.labby/.env";

    let temp = tempfile::tempdir().context("create Incus configuration staging directory")?;
    let staged_env = temp.path().join("labby.env");
    let remote = format!("{CONTAINER}{REMOTE_ENV}");
    let pulled = Command::new("incus")
        .args(["file", "pull", &remote])
        .arg(&staged_env)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !pulled {
        crate::dispatch::setup::bootstrap_at(&staged_env)
            .map_err(|error| anyhow::anyhow!("bootstrap Incus server credentials: {error}"))?;
    }

    let mut guest_plan = plan.clone();
    guest_plan.host = DEFAULT_HOST.to_string();
    guest_plan.port = DEFAULT_PORT;
    let token = configure_server_env(&staged_env, &guest_plan)?;

    run_status_path(
        "incus",
        &["file", "push"],
        &staged_env,
        &remote,
        "install Incus server configuration",
    )?;
    run_status(
        "incus",
        &["exec", CONTAINER, "--", "chown", "labby:labby", REMOTE_ENV],
        "set Incus server configuration ownership",
    )?;
    run_status(
        "incus",
        &["exec", CONTAINER, "--", "chmod", "0600", REMOTE_ENV],
        "set Incus server configuration permissions",
    )?;
    run_status(
        "incus",
        &[
            "exec",
            CONTAINER,
            "--",
            "chmod",
            "0700",
            "/home/labby/.labby",
        ],
        "protect Incus access state directory",
    )?;
    if matches!(resolved_server_auth(plan), SetupAuthArg::Bearer) {
        run_status(
            "incus",
            &[
                "exec",
                CONTAINER,
                "--",
                "runuser",
                "-u",
                "labby",
                "--",
                "env",
                "LABBY_HOME=/home/labby/.labby",
                "/usr/local/bin/labby",
                "setup",
                "--bootstrap-static-owner",
            ],
            "bootstrap Incus static bearer owner",
        )?;
    }
    run_status(
        "incus",
        &["exec", CONTAINER, "--", "systemctl", "restart", "labby"],
        "restart Labby inside Incus",
    )?;
    run_status(
        "incus",
        &[
            "exec",
            CONTAINER,
            "--",
            "curl",
            "--fail",
            "--silent",
            "--show-error",
            "--retry",
            "10",
            "--retry-connrefused",
            "--retry-delay",
            "1",
            "--retry-max-time",
            "30",
            "--max-time",
            "3",
            "http://127.0.0.1:8765/ready",
        ],
        "verify Labby readiness inside Incus",
    )?;
    Ok(token)
}

#[cfg(target_os = "linux")]
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open {} for SHA-256", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(target_os = "linux")]
fn run_status(program: &str, args: &[&str], action: &str) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("{action}: launch {program}"))?;
    if !status.success() {
        bail!("{action} failed with {status}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_status_path(
    program: &str,
    prefix: &[&str],
    path: &Path,
    suffix: &str,
    action: &str,
) -> Result<()> {
    let mut command = Command::new(program);
    command.args(prefix).arg(path).arg(suffix);
    let status = command
        .status()
        .with_context(|| format!("{action}: launch {program}"))?;
    if !status.success() {
        bail!("{action} failed with {status}");
    }
    Ok(())
}

async fn apply_client(plan: &SetupPlan) -> Result<serde_json::Value> {
    apply_client_with(plan, &install_desktop).await
}

async fn apply_client_with(
    plan: &SetupPlan,
    installer: DesktopInstaller<'_>,
) -> Result<serde_json::Value> {
    let server_url = plan
        .server_url
        .as_deref()
        .context("client setup requires a server URL")?;
    let env_path = invoking_installation_root(plan)?.join(".env");
    configure_client_env(&env_path, server_url, plan.client_bearer_token.as_deref())?;
    if matches!(plan.client_auth, Some(ClientAuth::OAuth)) {
        let server = crate::oauth::cli_session::server_url(server_url)?;
        crate::oauth::cli_session::login(&server, None).await?;
    }
    let desktop = report_desktop_install(plan, installer);
    Ok(with_desktop_summary(
        json!({
            "ok": true,
            "role": "client",
            "server": server_url,
            "auth": match plan.client_auth { Some(ClientAuth::Bearer) => "bearer", _ => "oauth" },
        }),
        plan,
        &desktop,
    ))
}

fn configure_client_env(path: &Path, server_url: &str, bearer: Option<&str>) -> Result<()> {
    // Credentials are bound to the selected server and auth mode. An old token
    // must never override OAuth or be forwarded to a newly selected origin.
    merge_env(
        path,
        vec![
            EnvEntry::new("LABBY_SERVER_URL", server_url).force(),
            EnvEntry::new("LABBY_MCP_HTTP_TOKEN", bearer.unwrap_or("")).force(),
        ],
    )
}

pub(super) async fn bootstrap_static_owner_at(root: &Path) -> Result<()> {
    use crate::access::{AccessRuntime, AccessRuntimeStatus, BootstrapOwnerInput};

    let paths = crate::installation::InstallationPaths::from_root(root)?;
    let env = paths.dotenv();
    if read_env(&env, "LABBY_AUTH_MODE").as_deref() != Some("bearer")
        || read_env(&env, "LABBY_MCP_HTTP_TOKEN").is_none_or(|token| token.trim().is_empty())
    {
        bail!(
            "static owner setup requires a configured bearer token and bearer authentication mode"
        );
    }
    let runtime = AccessRuntime::initialize(paths.access_db()).await;
    match runtime.status().await {
        AccessRuntimeStatus::Ready => Ok(()),
        AccessRuntimeStatus::Blocked(reason) => bail!(
            "owner setup is blocked ({reason:?}); repair the existing access store before retrying setup"
        ),
        AccessRuntimeStatus::SetupRequired(_) => {
            let identity = labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .context("construct static bearer owner identity")?;
            runtime
                .bootstrap_owner(
                    BootstrapOwnerInput::new(identity, "Local", "Default")
                        .context("construct local owner bootstrap")?,
                )
                .await
                .context("bootstrap durable static bearer owner")?;
            Ok(())
        }
    }
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn resolved_server_auth(plan: &SetupPlan) -> SetupAuthArg {
    plan.server_auth.unwrap_or_else(|| {
        if plan.oauth.is_some() {
            SetupAuthArg::Both
        } else {
            SetupAuthArg::Bearer
        }
    })
}

/// Resolve omitted choices from the service's actual state before installing,
/// connecting the local client, or offering a browser handoff.
fn resolve_existing_native_defaults(plan: &mut SetupPlan, root: &Path) -> Result<()> {
    let raw = crate::config::host_write::read_config_snapshot(&root.join(".env"))
        .map_err(|_| anyhow::anyhow!("cannot safely read existing server environment"))?;
    let mut env = std::collections::BTreeMap::new();
    for entry in dotenvy::from_read_iter(raw.as_bytes()) {
        let (key, value) = entry.map_err(|_| {
            anyhow::anyhow!("existing server environment is invalid; repair it before setup")
        })?;
        env.insert(key, value);
    }
    let config =
        crate::config::load_toml_from_fixed_root(&[root.join("config.toml")]).map_err(|_| {
            anyhow::anyhow!(
                "existing server configuration is invalid or unreadable; repair it before setup"
            )
        })?;
    let value = |key: &str, fallback: Option<String>| env.get(key).cloned().or(fallback);
    if (plan.preserve.host || plan.preserve.port)
        && value("LABBY_MCP_TRANSPORT", config.mcp.transport.clone())
            .is_some_and(|transport| transport != "http")
    {
        bail!(
            "existing server uses another transport; choose explicit HTTP listener settings with --advanced"
        );
    }

    if plan.preserve.host {
        if let Some(host) = value("LABBY_MCP_HTTP_HOST", config.mcp.host.clone()) {
            plan.host = validate_host(&host)?;
        }
    }
    if plan.preserve.port {
        if let Some(port) = value(
            "LABBY_MCP_HTTP_PORT",
            config.mcp.port.map(|port| port.to_string()),
        ) {
            plan.port = port
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .context("existing server port must be between 1 and 65535")?;
        }
    }
    let auth = config.auth.unwrap_or_default();
    if plan.preserve.public_url {
        if let Some(url) =
            value("LABBY_PUBLIC_URL", auth.public_url.clone()).filter(|url| !url.is_empty())
        {
            validate_public_url(&url)?;
            plan.public_url = Some(url);
        }
    }
    if plan.preserve.auth {
        match value("LABBY_AUTH_MODE", auth.mode.clone()).as_deref() {
            None | Some("bearer") => {}
            Some("oauth") => {
                let required = |key: &str, fallback: Option<String>| {
                    value(key, fallback)
                    .filter(|value| !value.trim().is_empty())
                    .with_context(|| format!("existing OAuth configuration is missing {key}; repair it before setup"))
                };
                let admin_email = required("LABBY_AUTH_ADMIN_EMAIL", auth.admin_email.clone())?;
                plan.oauth = Some(
                    match required("LABBY_AUTH_PROVIDER", auth.provider.clone())?.as_str() {
                        "google" => OAuthConfig::Google {
                            client_id: required(
                                "LABBY_GOOGLE_CLIENT_ID",
                                auth.google_client_id.clone(),
                            )?,
                            client_secret: required(
                                "LABBY_GOOGLE_CLIENT_SECRET",
                                auth.google_client_secret.clone(),
                            )?,
                            admin_email,
                        },
                        "authelia" => OAuthConfig::Authelia {
                            issuer_url: required(
                                "LABBY_AUTHELIA_ISSUER_URL",
                                auth.authelia_issuer_url.clone(),
                            )?,
                            client_id: required(
                                "LABBY_AUTHELIA_CLIENT_ID",
                                auth.authelia_client_id.clone(),
                            )?,
                            client_secret: required(
                                "LABBY_AUTHELIA_CLIENT_SECRET",
                                auth.authelia_client_secret.clone(),
                            )?,
                            admin_email,
                        },
                        _ => {
                            bail!("existing OAuth provider is unsupported; repair it before setup")
                        }
                    },
                );
                plan.server_auth = Some(
                    if env
                        .get("LABBY_MCP_HTTP_TOKEN")
                        .is_some_and(|token| !token.trim().is_empty())
                    {
                        SetupAuthArg::Both
                    } else {
                        SetupAuthArg::OAuth
                    },
                );
                if plan.public_url.is_none() {
                    bail!("existing OAuth configuration needs a public URL");
                }
            }
            Some(_) => bail!("existing authentication mode is unsupported; repair it before setup"),
        }
    }
    Ok(())
}

#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn configure_server_env(path: &Path, plan: &SetupPlan) -> Result<Option<String>> {
    // The access store requires an owner-only state directory. Environment
    // merges protect individual files but create new parents with the umask.
    let root = path.parent().context("server environment has no parent")?;
    let paths = crate::installation::InstallationPaths::from_root(root)?;
    std::fs::create_dir_all(paths.root())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(paths.root(), std::fs::Permissions::from_mode(0o700))?;
    }
    // Do not run the generic bearer bootstrap here. This function already
    // owns the complete server environment plan, and OAuth-only setup must
    // never mint a throwaway static credential just to clear it immediately.
    // env_merge creates a missing .env with the same owner-only guarantees.
    let auth = resolved_server_auth(plan);
    match (auth, plan.oauth.as_ref()) {
        (SetupAuthArg::Bearer, None) | (SetupAuthArg::OAuth | SetupAuthArg::Both, Some(_)) => {}
        (SetupAuthArg::Bearer, Some(_)) => {
            bail!("bearer-only server plan cannot contain an OAuth provider")
        }
        (SetupAuthArg::OAuth | SetupAuthArg::Both, None) => {
            bail!("OAuth server plan requires exactly one OAuth provider")
        }
    }

    let token = matches!(auth, SetupAuthArg::Bearer | SetupAuthArg::Both).then(|| {
        read_env(path, "LABBY_MCP_HTTP_TOKEN")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(crate::dispatch::setup::generate_mcp_token)
    });
    let mut entries = vec![
        EnvEntry::new("LABBY_MCP_HTTP_TOKEN", token.as_deref().unwrap_or("")).force(),
        EnvEntry::new("LABBY_MCP_TRANSPORT", "http").force(),
        EnvEntry::new("LABBY_MCP_HTTP_HOST", plan.host.clone()).force(),
        EnvEntry::new("LABBY_MCP_HTTP_PORT", plan.port.to_string()).force(),
    ];
    if let Some(public_url) = plan.public_url.as_deref() {
        entries.push(EnvEntry::new("LABBY_PUBLIC_URL", public_url).force());
    }
    match plan.oauth.as_ref() {
        None => entries.extend([
            EnvEntry::new("LABBY_AUTH_MODE", "bearer").force(),
            EnvEntry::new("LABBY_AUTH_PROVIDER", "").force(),
            EnvEntry::new("LABBY_AUTH_ADMIN_EMAIL", "").force(),
            EnvEntry::new("LABBY_GOOGLE_CLIENT_ID", "").force(),
            EnvEntry::new("LABBY_GOOGLE_CLIENT_SECRET", "").force(),
            EnvEntry::new("LABBY_AUTHELIA_ISSUER_URL", "").force(),
            EnvEntry::new("LABBY_AUTHELIA_CLIENT_ID", "").force(),
            EnvEntry::new("LABBY_AUTHELIA_CLIENT_SECRET", "").force(),
        ]),
        Some(OAuthConfig::Google {
            client_id,
            client_secret,
            admin_email,
        }) => {
            entries.extend([
                EnvEntry::new("LABBY_AUTH_MODE", "oauth").force(),
                EnvEntry::new("LABBY_AUTH_PROVIDER", "google").force(),
                EnvEntry::new("LABBY_AUTH_ADMIN_EMAIL", admin_email).force(),
                EnvEntry::new("LABBY_GOOGLE_CLIENT_ID", client_id).force(),
                EnvEntry::new("LABBY_GOOGLE_CLIENT_SECRET", client_secret).force(),
                EnvEntry::new("LABBY_AUTHELIA_ISSUER_URL", "").force(),
                EnvEntry::new("LABBY_AUTHELIA_CLIENT_ID", "").force(),
                EnvEntry::new("LABBY_AUTHELIA_CLIENT_SECRET", "").force(),
            ]);
        }
        Some(OAuthConfig::Authelia {
            issuer_url,
            client_id,
            client_secret,
            admin_email,
        }) => {
            entries.extend([
                EnvEntry::new("LABBY_AUTH_MODE", "oauth").force(),
                EnvEntry::new("LABBY_AUTH_PROVIDER", "authelia").force(),
                EnvEntry::new("LABBY_AUTH_ADMIN_EMAIL", admin_email).force(),
                EnvEntry::new("LABBY_AUTHELIA_ISSUER_URL", issuer_url).force(),
                EnvEntry::new("LABBY_AUTHELIA_CLIENT_ID", client_id).force(),
                EnvEntry::new("LABBY_AUTHELIA_CLIENT_SECRET", client_secret).force(),
                EnvEntry::new("LABBY_GOOGLE_CLIENT_ID", "").force(),
                EnvEntry::new("LABBY_GOOGLE_CLIENT_SECRET", "").force(),
            ]);
        }
    }
    merge_env(path, entries)?;
    crate::dispatch::setup::ensure_oauth_encryption_key_at(path)
        .map_err(|e| anyhow::anyhow!("provision OAuth encryption key: {e}"))?;
    Ok(token)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn configure_local_client(plan: &SetupPlan, token: Option<&str>) -> Result<()> {
    let root = invoking_installation_root(plan)?;
    if is_unix_root()
        && plan.invoking_user.is_some()
        && std::env::var_os("LABBY_HOME").is_some_and(|value| !value.is_empty())
    {
        bail!(
            "configure a custom LABBY_HOME client as the invoking user, not through elevated setup"
        );
    }
    let env = root.join(".env");
    let server = advertised_url(plan);
    merge_env(
        &env,
        vec![
            EnvEntry::new("LABBY_SERVER_URL", server).force(),
            EnvEntry::new("LABBY_MCP_HTTP_TOKEN", token.unwrap_or("")).force(),
        ],
    )?;
    if is_unix_root()
        && let Some(user) = plan.invoking_user.as_deref()
    {
        chown_tree(&root, user)?;
    }
    Ok(())
}

/// Resolve the invoking account's state separately from the Linux service
/// account. An explicit LABBY_HOME must govern every invoking-user write.
fn invoking_installation_root(plan: &SetupPlan) -> Result<PathBuf> {
    invoking_installation_root_with(plan, std::env::var_os("LABBY_HOME"))
}

fn invoking_installation_root_with(
    plan: &SetupPlan,
    selected_root: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    let root = selected_root
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| plan.invoking_home.join(".labby"));
    Ok(crate::installation::InstallationPaths::from_root(root)
        .context("resolve invoking user's Labby installation root")?
        .root()
        .to_path_buf())
}

fn merge_env(path: &Path, entries: Vec<EnvEntry>) -> Result<()> {
    env_merge::merge(
        path,
        MergeRequest {
            entries,
            force: false,
            expected_mtime: env_merge::snapshot_mtime(path),
        },
    )
    .map_err(|e| anyhow::anyhow!("update {}: {e}", path.display()))?;
    Ok(())
}

fn read_env(path: &Path, key: &str) -> Option<String> {
    let raw = crate::config::host_write::read_config_snapshot(path).ok()?;
    let entries = dotenvy::from_read_iter(raw.as_bytes())
        .collect::<std::result::Result<std::collections::BTreeMap<_, _>, _>>()
        .ok()?;
    entries.get(key).cloned()
}

#[cfg(target_os = "macos")]
fn install_macos_service(plan: &SetupPlan) -> Result<()> {
    let mut script = tempfile::NamedTempFile::new()?;
    script.write_all(MACOS_SERVICE_INSTALLER.as_bytes())?;
    script.as_file().sync_all()?;
    let executable = std::env::current_exe()?;
    let status = Command::new("bash")
        .arg(script.path())
        .arg("install")
        .env("LABBY_SERVICE_BIN", &executable)
        .env("LABBY_SERVICE_HOST", &plan.host)
        .env("LABBY_SERVICE_PORT", plan.port.to_string())
        .env("LABBY_HOME", invoking_installation_root(plan)?)
        .status()?;
    if !status.success() {
        bail!("macOS service installer exited with {status}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn converge_incus_publish(name: &str, host: &str, port: u16) -> Result<()> {
    converge_incus_publish_with(Path::new("incus"), name, host, port)
}

#[cfg(target_os = "linux")]
fn converge_incus_publish_with(program: &Path, name: &str, host: &str, port: u16) -> Result<()> {
    let listen = format!("tcp:{host}:{port}");
    let exists = Command::new(program)
        .args(["config", "device", "show", name])
        .output()
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.trim() == "labby-http:")
        })
        .unwrap_or(false);
    let status = if exists {
        Command::new(program)
            .args([
                "config",
                "device",
                "set",
                name,
                "labby-http",
                "listen",
                &listen,
            ])
            .status()?
    } else {
        Command::new(program)
            .args([
                "config",
                "device",
                "add",
                name,
                "labby-http",
                "proxy",
                &format!("listen={listen}"),
                "connect=tcp:127.0.0.1:8765",
            ])
            .status()?
    };
    if !status.success() {
        bail!("failed to publish Incus Labby endpoint on {host}:{port}");
    }
    let autostart = Command::new(program)
        .args(["config", "set", name, "boot.autostart", "true"])
        .status()?;
    if !autostart.success() {
        bail!("failed to enable Incus container autostart");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn converge_incus_publish(_name: &str, _host: &str, _port: u16) -> Result<()> {
    bail!("Incus publishing is unavailable")
}

/// Installs the release-owned desktop app for a completed plan.
type DesktopInstaller<'a> = &'a (dyn Fn(&SetupPlan) -> Result<()> + Sync);

/// Outcome of the optional desktop installation, reported in every setup
/// summary as `desktop_requested`, `desktop_installed`, and `desktop_error`.
struct DesktopOutcome {
    installed: bool,
    error: Option<String>,
}

impl DesktopOutcome {
    fn summary(&self, plan: &SetupPlan) -> serde_json::Value {
        with_desktop_summary(json!({ "ok": true }), plan, self)
    }
}

fn with_desktop_summary(
    mut summary: serde_json::Value,
    plan: &SetupPlan,
    desktop: &DesktopOutcome,
) -> serde_json::Value {
    if let Some(object) = summary.as_object_mut() {
        object.insert("desktop_requested".into(), json!(plan.install_desktop));
        object.insert("desktop_installed".into(), json!(desktop.installed));
        object.insert("desktop_error".into(), json!(desktop.error));
    }
    summary
}

/// Install the optional desktop app once the server or client configuration
/// is complete. A failure is reported in the summary and on stderr but never
/// fails setup: the release-owned desktop package is additive and may not be
/// published for this platform or version, and the configuration it would sit
/// on top of is already live.
fn report_desktop_install(plan: &SetupPlan, installer: DesktopInstaller<'_>) -> DesktopOutcome {
    if !plan.install_desktop {
        return DesktopOutcome {
            installed: false,
            error: None,
        };
    }
    match installer(plan) {
        Ok(()) => DesktopOutcome {
            installed: true,
            error: None,
        },
        Err(error) => {
            let reason = format!("{error:#}");
            eprintln!(
                "warning: the Labby desktop app was not installed: {reason}. Labby installation succeeded; first-use checks are still required."
            );
            DesktopOutcome {
                installed: false,
                error: Some(reason),
            }
        }
    }
}

fn install_desktop(plan: &SetupPlan) -> Result<()> {
    // Desktop packaging is release-owned. Setup deliberately refuses to build
    // Tauri from source; it installs only an already-published package. The
    // release workflow is responsible for making that package available.
    let target = plan
        .server_url
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| advertised_url(plan));
    crate::desktop_install::install_release_app(&target)
}

fn advertised_url(plan: &SetupPlan) -> String {
    if let Some(url) = plan.public_url.as_deref() {
        return url.trim_end_matches('/').to_string();
    }
    let host = if plan.host == "0.0.0.0" || plan.host == "::" {
        "127.0.0.1"
    } else {
        plan.host.as_str()
    };
    format!("http://{host}:{}", plan.port)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn chown_tree(path: &Path, user: &str) -> Result<()> {
    let status = Command::new("chown")
        .arg("-R")
        .arg(user)
        .arg(path)
        .status()?;
    if !status.success() {
        bail!(
            "failed to restore ownership of {} to {user}",
            path.display()
        );
    }
    Ok(())
}

fn command_ok(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn redacted_plan(plan: &SetupPlan) -> serde_json::Value {
    json!({
        "role": plan.role,
        "deployment": plan.deployment,
        "host": plan.host,
        "port": plan.port,
        "server_url": plan.server_url,
        "public_url": plan.public_url,
        "server_auth": plan.server_auth,
        "oauth": plan.oauth.as_ref().map(|provider| match provider { OAuthConfig::Google { .. } => "google", OAuthConfig::Authelia { .. } => "authelia" }),
        "client_auth": plan.client_auth,
        "selected_clients": plan.selected_clients,
        "install_desktop": plan.install_desktop,
        "no_browser": plan.no_browser,
    })
}

fn print_banner() {
    eprintln!(
        r"
  _          _     _
 | |    __ _| |__ | |__  _   _
 | |   / _` | '_ \| '_ \| | | |
 | |__| (_| | |_) | |_) | |_| |
 |_____\__,_|_.__/|_.__/ \__, |
                         |___/

  One setup. Every interface.
"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_plan(home: PathBuf) -> SetupPlan {
        SetupPlan {
            preserve: PreserveDefaults::default(),
            role: SetupRoleArg::Server,
            deployment: Some(SetupDeploymentArg::Native),
            host: DEFAULT_HOST.into(),
            port: DEFAULT_PORT,
            server_url: None,
            public_url: None,
            server_auth: Some(SetupAuthArg::Bearer),
            oauth: None,
            client_auth: None,
            client_bearer_token: None,
            selected_clients: Vec::new(),
            install_desktop: true,
            no_browser: true,
            invoking_home: home,
            invoking_user: Some("operator".into()),
        }
    }

    #[test]
    fn omitted_native_choices_preserve_existing_oauth_and_listener() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".env"), "LABBY_MCP_HTTP_HOST=192.0.2.20\nLABBY_MCP_HTTP_PORT=9321\nLABBY_AUTH_MODE=oauth\nLABBY_AUTH_PROVIDER=google\nLABBY_PUBLIC_URL=https://labby.example.com\nLABBY_GOOGLE_CLIENT_ID=existing-id\nLABBY_GOOGLE_CLIENT_SECRET=existing-secret\nLABBY_AUTH_ADMIN_EMAIL=admin@example.com\n").unwrap();
        let mut plan = server_plan(root.to_path_buf());
        plan.preserve = PreserveDefaults {
            host: true,
            port: true,
            auth: true,
            public_url: true,
        };
        resolve_existing_native_defaults(&mut plan, root).unwrap();
        assert_eq!(plan.host, "192.0.2.20");
        assert_eq!(plan.port, 9321);
        assert!(matches!(plan.server_auth, Some(SetupAuthArg::OAuth)));
        assert_eq!(
            plan.public_url.as_deref(),
            Some("https://labby.example.com")
        );
        assert!(
            configure_server_env(&root.join(".env"), &plan)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            read_env(&root.join(".env"), "LABBY_GOOGLE_CLIENT_SECRET").as_deref(),
            Some("existing-secret")
        );
    }

    #[test]
    fn saved_toml_listener_and_environment_precedence_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.toml"),
            "[mcp]\nhost = '192.0.2.30'\nport = 9345\n",
        )
        .unwrap();
        std::fs::write(temp.path().join(".env"), "LABBY_MCP_HTTP_PORT=9456\n").unwrap();
        let mut plan = server_plan(temp.path().to_path_buf());
        plan.preserve = PreserveDefaults {
            host: true,
            port: true,
            auth: true,
            public_url: true,
        };
        resolve_existing_native_defaults(&mut plan, temp.path()).unwrap();
        assert_eq!(plan.host, "192.0.2.30");
        assert_eq!(plan.port, 9456);
    }

    #[test]
    fn explicit_native_choices_override_existing_configuration() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join(".env"),
            "LABBY_MCP_HTTP_HOST=192.0.2.20\nLABBY_MCP_HTTP_PORT=9321\nLABBY_AUTH_MODE=oauth\n",
        )
        .unwrap();
        let mut plan = server_plan(temp.path().to_path_buf());
        resolve_existing_native_defaults(&mut plan, temp.path()).unwrap();
        assert_eq!(plan.host, DEFAULT_HOST);
        assert_eq!(plan.port, DEFAULT_PORT);
        assert!(matches!(plan.server_auth, Some(SetupAuthArg::Bearer)));
    }

    #[test]
    fn invalid_existing_environment_fails_before_reconfiguration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".env");
        std::fs::write(&path, "LABBY_AUTH_MODE='unterminated").unwrap();
        let mut plan = server_plan(temp.path().to_path_buf());
        assert!(resolve_existing_native_defaults(&mut plan, temp.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "LABBY_AUTH_MODE='unterminated"
        );
    }

    #[test]
    fn fresh_local_setup_avoids_occupied_ports_and_retains_existing_target() {
        let directory = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind((DEFAULT_HOST, 0)).unwrap();
        let preferred = listener.local_addr().unwrap().port();
        let selected = fresh_local_port(preferred, directory.path()).unwrap();
        assert_ne!(selected, preferred);
        assert_ne!(selected, 0);
        std::fs::write(directory.path().join("config.toml"), "").unwrap();
        assert_eq!(
            fresh_local_port(preferred, directory.path()).unwrap(),
            preferred
        );
    }

    #[test]
    fn elevated_plan_defers_desktop_installation_to_invoking_process() {
        let plan = server_plan(PathBuf::from("/home/operator"));
        let elevated = privileged_plan(&plan);
        assert!(!elevated.install_desktop);
        assert!(plan.install_desktop);
        assert_eq!(elevated.invoking_home, plan.invoking_home);
    }

    #[test]
    fn desktop_choice_defaults_to_no() {
        // No release publishes a desktop package for every platform, so the
        // interactive prompt must default to No; explicit flags, non-interactive
        // runs, and unsupported hosts never prompt at all.
        let args = SetupArgs::default();
        let mut asked = None;
        let chosen = resolve_desktop_choice(&args, true, true, |default| {
            asked = Some(default);
            Ok(default)
        })
        .unwrap();
        assert_eq!(asked, Some(false), "the desktop prompt must default to No");
        assert!(!chosen);
        let never = |_: bool| -> Result<bool> { bail!("the desktop prompt must not run") };
        let explicit = SetupArgs {
            desktop: true,
            ..SetupArgs::default()
        };
        assert!(resolve_desktop_choice(&explicit, true, true, never).unwrap());
        let declined = SetupArgs {
            no_desktop: true,
            ..SetupArgs::default()
        };
        assert!(!resolve_desktop_choice(&declined, true, true, never).unwrap());
        assert!(!resolve_desktop_choice(&args, false, true, never).unwrap());
        assert!(!resolve_desktop_choice(&args, true, false, never).unwrap());
    }

    #[tokio::test]
    async fn desktop_install_failure_is_reported_not_fatal() {
        let home = tempfile::tempdir().unwrap();
        let plan = SetupPlan {
            preserve: PreserveDefaults::default(),
            role: SetupRoleArg::Client,
            deployment: None,
            host: DEFAULT_HOST.into(),
            port: DEFAULT_PORT,
            server_url: Some("http://127.0.0.1:8765".into()),
            public_url: None,
            server_auth: None,
            oauth: None,
            client_auth: Some(ClientAuth::Bearer),
            client_bearer_token: Some("client-token".into()),
            selected_clients: Vec::new(),
            install_desktop: true,
            no_browser: true,
            invoking_home: home.path().to_path_buf(),
            invoking_user: None,
        };
        let failing = |_: &SetupPlan| -> Result<()> {
            bail!("release v9.9.9 does not contain the expected desktop asset")
        };
        let result = apply_client_with(&plan, &failing)
            .await
            .expect("client setup completes even though the desktop app did not install");
        assert_eq!(result["ok"], true);
        assert_eq!(result["desktop_requested"], true);
        assert_eq!(result["desktop_installed"], false);
        assert!(
            result["desktop_error"]
                .as_str()
                .is_some_and(|reason| reason.contains("expected desktop asset")),
            "desktop_error must carry the installer's reason; got {:?}",
            result["desktop_error"]
        );
        // The client configuration itself is complete.
        assert_eq!(
            read_env(&home.path().join(".labby/.env"), "LABBY_SERVER_URL").as_deref(),
            Some("http://127.0.0.1:8765")
        );
        let installed = apply_client_with(&plan, &|_: &SetupPlan| Ok(()))
            .await
            .unwrap();
        assert_eq!(installed["desktop_installed"], true);
        assert!(installed["desktop_error"].is_null());
    }

    /// The privileged `--apply-plan` child runs `chown -R` over the invoking
    /// user's `.labby`. Those values come from a plan file the unprivileged
    /// parent wrote, so the child must take the identity from sudo's own
    /// record instead and refuse a plan that disagrees with it.
    #[cfg(unix)]
    #[test]
    fn apply_plan_rederives_invoking_identity_from_sudo_user() {
        let account = nix::unistd::User::from_uid(nix::unistd::Uid::current())
            .unwrap()
            .unwrap();
        if account.uid.is_root() {
            // A root test process cannot model a delegated invoking account.
            return;
        }
        let uid = account.uid.as_raw().to_string();
        let mut plan = server_plan(account.dir.clone());
        plan.invoking_user = Some(account.name.clone());

        // The plan agrees with sudo: the applied identity is sudo's.
        let verified = rederive_invoking_identity(
            plan.clone(),
            Some(&uid),
            Some(&account.name),
            Some(PathBuf::from("/root")),
        )
        .unwrap();
        assert_eq!(
            verified.invoking_user.as_deref(),
            Some(account.name.as_str())
        );
        assert_eq!(verified.invoking_home, account.dir);

        // A plan that points the privileged chown elsewhere is refused.
        let mut elsewhere = plan.clone();
        elsewhere.invoking_home = PathBuf::from("/srv/elsewhere");
        let error = rederive_invoking_identity(elsewhere, Some(&uid), Some(&account.name), None)
            .unwrap_err();
        assert!(error.to_string().contains("disagrees"), "{error:#}");
        let mut other_user = plan.clone();
        other_user.invoking_user = Some("someone-else".into());
        assert!(
            rederive_invoking_identity(other_user, Some(&uid), Some(&account.name), None).is_err()
        );

        // SUDO_USER must name the SUDO_UID account.
        assert!(
            rederive_invoking_identity(plan.clone(), Some(&uid), Some("someone-else"), None)
                .is_err()
        );

        // Without sudo's record there is no delegated identity to apply.
        assert!(
            rederive_invoking_identity(plan, None, None, Some(PathBuf::from("/root"))).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn externally_elevated_setup_uses_account_home_instead_of_root_home() {
        let account = nix::unistd::User::from_uid(nix::unistd::Uid::current())
            .unwrap()
            .unwrap();
        assert_eq!(
            invoking_home_for(true, Some(&account.name), Some(PathBuf::from("/root"))).unwrap(),
            account.dir
        );
        assert_eq!(
            invoking_home_for(
                false,
                Some(&account.name),
                Some(PathBuf::from("/custom-home"))
            )
            .unwrap(),
            PathBuf::from("/custom-home")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn non_unix_client_setup_uses_the_current_user_home() {
        let home = PathBuf::from(r"C:\Users\operator");
        assert!(!is_unix_root());
        assert_eq!(
            invoking_home_for(false, None, Some(home.clone())).unwrap(),
            home
        );
        assert!(invoking_home_for(false, None, None).is_err());
    }

    #[test]
    fn no_browser_refuses_oauth_before_client_configuration() {
        assert!(validate_client_browser_mode(ClientAuth::OAuth, true).is_err());
        assert!(validate_client_browser_mode(ClientAuth::OAuth, false).is_ok());
        assert!(validate_client_browser_mode(ClientAuth::Bearer, true).is_ok());
    }

    #[test]
    fn switching_to_oauth_clears_bearer_from_previous_origin() {
        let directory = tempfile::tempdir().unwrap();
        let env = directory.path().join(".env");
        configure_client_env(&env, "https://a.example", Some("server-a-secret")).unwrap();
        configure_client_env(&env, "https://b.example", None).unwrap();
        assert_eq!(
            read_env(&env, "LABBY_SERVER_URL").as_deref(),
            Some("https://b.example")
        );
        assert_eq!(read_env(&env, "LABBY_MCP_HTTP_TOKEN").as_deref(), Some(""));
        configure_client_env(&env, "https://c.example", Some("server-c-secret")).unwrap();
        assert_eq!(
            read_env(&env, "LABBY_MCP_HTTP_TOKEN").as_deref(),
            Some("server-c-secret")
        );
    }

    #[test]
    fn oauth_only_server_env_clears_static_bearer() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut plan = server_plan(root.clone());
        plan.server_auth = Some(SetupAuthArg::OAuth);
        plan.public_url = Some("https://labby.example.com".into());
        plan.oauth = Some(OAuthConfig::Google {
            client_id: "client-id".into(),
            client_secret: "client-secret".into(),
            admin_email: "admin@example.com".into(),
        });

        let token = configure_server_env(&root.join(".env"), &plan).unwrap();
        assert!(token.is_none());
        assert_eq!(
            read_env(&root.join(".env"), "LABBY_AUTH_MODE").as_deref(),
            Some("oauth")
        );
        assert_eq!(
            read_env(&root.join(".env"), "LABBY_AUTH_PROVIDER").as_deref(),
            Some("google")
        );
        assert_eq!(
            read_env(&root.join(".env"), "LABBY_MCP_HTTP_TOKEN").as_deref(),
            Some("")
        );
        for entry in std::fs::read_dir(&root).unwrap().filter_map(Result::ok) {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(".env.bak.") {
                assert_eq!(
                    read_env(&entry.path(), "LABBY_MCP_HTTP_TOKEN").as_deref(),
                    Some(""),
                    "fresh OAuth-only setup must never retain a generated bearer in its backup"
                );
            }
        }
    }

    #[test]
    fn client_plan_refuses_server_auth_topology() {
        let args = SetupArgs {
            role: Some(SetupRoleArg::Client),
            auth: Some(SetupAuthArg::OAuth),
            server_url: Some("https://labby.example.com".into()),
            yes: true,
            ..SetupArgs::default()
        };
        let error = collect_client_plan(
            &args,
            false,
            &ColorfulTheme::default(),
            PathBuf::from("/tmp/operator"),
            Some("operator".into()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("server authentication topology"));
    }

    #[test]
    fn oauth_plus_bearer_server_env_generates_break_glass_token() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut plan = server_plan(root.clone());
        plan.server_auth = Some(SetupAuthArg::Both);
        plan.public_url = Some("https://labby.example.com".into());
        plan.oauth = Some(OAuthConfig::Google {
            client_id: "client-id".into(),
            client_secret: "client-secret".into(),
            admin_email: "admin@example.com".into(),
        });

        let token = configure_server_env(&root.join(".env"), &plan)
            .unwrap()
            .expect("break-glass token");
        assert_eq!(token.len(), 64);
        assert_eq!(
            read_env(&root.join(".env"), "LABBY_MCP_HTTP_TOKEN").as_deref(),
            Some(token.as_str())
        );
        assert_eq!(
            read_env(&root.join(".env"), "LABBY_AUTH_MODE").as_deref(),
            Some("oauth")
        );
    }

    #[test]
    fn switching_server_from_oauth_to_bearer_clears_provider_credentials() {
        for (provider, oauth) in [
            (
                "google",
                OAuthConfig::Google {
                    client_id: "google-client-id".into(),
                    client_secret: "google-client-secret".into(),
                    admin_email: "google-admin@example.com".into(),
                },
            ),
            (
                "authelia",
                OAuthConfig::Authelia {
                    issuer_url: "https://auth.example.com".into(),
                    client_id: "authelia-client-id".into(),
                    client_secret: "authelia-client-secret".into(),
                    admin_email: "authelia-admin@example.com".into(),
                },
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let env = root.join(".env");
            let mut oauth_plan = server_plan(root.clone());
            oauth_plan.server_auth = Some(SetupAuthArg::OAuth);
            oauth_plan.public_url = Some("https://labby.example.com".into());
            oauth_plan.oauth = Some(oauth);
            configure_server_env(&env, &oauth_plan).unwrap();
            assert_eq!(
                read_env(&env, "LABBY_AUTH_PROVIDER").as_deref(),
                Some(provider)
            );

            let token = configure_server_env(&env, &server_plan(root.clone()))
                .unwrap()
                .expect("bearer token");
            assert_eq!(token.len(), 64);
            assert_eq!(read_env(&env, "LABBY_AUTH_MODE").as_deref(), Some("bearer"));
            for key in [
                "LABBY_AUTH_PROVIDER",
                "LABBY_AUTH_ADMIN_EMAIL",
                "LABBY_GOOGLE_CLIENT_ID",
                "LABBY_GOOGLE_CLIENT_SECRET",
                "LABBY_AUTHELIA_ISSUER_URL",
                "LABBY_AUTHELIA_CLIENT_ID",
                "LABBY_AUTHELIA_CLIENT_SECRET",
            ] {
                assert_eq!(
                    read_env(&env, key).as_deref(),
                    Some(""),
                    "{key} must be cleared when OAuth is disabled"
                );
            }
        }
    }

    #[test]
    fn legacy_oauth_plan_resolves_to_both_for_compatibility() {
        let directory = tempfile::tempdir().unwrap();
        let mut plan = server_plan(directory.path().to_path_buf());
        plan.server_auth = None;
        plan.oauth = Some(OAuthConfig::Google {
            client_id: "id".into(),
            client_secret: "secret".into(),
            admin_email: "admin@example.com".into(),
        });
        assert_eq!(resolved_server_auth(&plan), SetupAuthArg::Both);
    }

    #[tokio::test]
    async fn fresh_nested_server_root_supports_durable_owner_bootstrap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("new-user/.labby");
        assert!(!root.exists());
        configure_server_env(&root.join(".env"), &server_plan(root.clone())).unwrap();
        bootstrap_static_owner_at(&root).await.unwrap();
        let runtime = crate::access::AccessRuntime::initialize(root.join("access.db")).await;
        assert_eq!(
            runtime.status().await,
            crate::access::AccessRuntimeStatus::Ready
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[tokio::test]
    async fn static_owner_setup_is_durable_and_preserves_existing_owner() {
        use crate::access::{AccessRuntime, AccessRuntimeStatus, BootstrapOwnerInput};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap().join(".labby");
        assert!(!root.exists());
        configure_server_env(&root.join(".env"), &server_plan(root.clone())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        bootstrap_static_owner_at(&root).await.unwrap();
        let reopened = AccessRuntime::initialize(root.join("access.db")).await;
        assert_eq!(reopened.status().await, AccessRuntimeStatus::Ready);
        drop(reopened);
        bootstrap_static_owner_at(&root).await.unwrap();
        let connection = rusqlite::Connection::open(root.join("access.db")).unwrap();
        let credential: String = connection
            .query_row(
                "SELECT credential_id FROM principal_links WHERE link_id='bootstrap-owner-link'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(credential, "static-bearer:primary");
        drop(connection);

        let other_directory = tempfile::tempdir().unwrap();
        let other = other_directory.path().canonicalize().unwrap();
        configure_server_env(&other.join(".env"), &server_plan(other.clone())).unwrap();
        let runtime = AccessRuntime::initialize(other.join("access.db")).await;
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "existing-owner",
        )
        .unwrap();
        runtime
            .bootstrap_owner(BootstrapOwnerInput::new(identity, "Existing", "Project").unwrap())
            .await
            .unwrap();
        drop(runtime);
        bootstrap_static_owner_at(&other).await.unwrap();
        let connection = rusqlite::Connection::open(other.join("access.db")).unwrap();
        let credential: String = connection
            .query_row(
                "SELECT credential_id FROM principal_links WHERE link_id='bootstrap-owner-link'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(credential, "existing-owner");
    }

    #[tokio::test]
    async fn static_owner_setup_refuses_corrupt_store_and_oauth_mode() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        configure_server_env(&root.join(".env"), &server_plan(root.clone())).unwrap();
        std::fs::write(root.join("access.db"), b"not a database").unwrap();
        assert!(bootstrap_static_owner_at(&root).await.is_err());
        assert_eq!(
            std::fs::read(root.join("access.db")).unwrap(),
            b"not a database"
        );
        merge_env(
            &root.join(".env"),
            vec![EnvEntry::new("LABBY_AUTH_MODE", "oauth").force()],
        )
        .unwrap();
        assert!(bootstrap_static_owner_at(&root).await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn incus_publish_accepts_fresh_device_key_value_arguments() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("incus");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
set -eu
case "$1 $2 $3" in
  'config device show') exit 0 ;;
  'config device add')
    [ "$4" = labby ] && [ "$5" = labby-http ] && [ "$6" = proxy ] &&
    [ "$7" = listen=tcp:127.0.0.1:9123 ] && [ "$8" = connect=tcp:127.0.0.1:8765 ] ;;
  'config set labby') [ "$4" = boot.autostart ] && [ "$5" = true ] ;;
  *) exit 99 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        converge_incus_publish_with(&executable, "labby", "127.0.0.1", 9123).unwrap();
    }

    #[test]
    fn advertised_url_never_uses_unspecified_address() {
        let plan = SetupPlan {
            preserve: PreserveDefaults::default(),
            role: SetupRoleArg::Server,
            deployment: Some(SetupDeploymentArg::Native),
            host: "0.0.0.0".into(),
            port: 9123,
            server_url: None,
            public_url: None,
            server_auth: Some(SetupAuthArg::Bearer),
            oauth: None,
            client_auth: None,
            client_bearer_token: None,
            selected_clients: Vec::new(),
            install_desktop: false,
            no_browser: false,
            invoking_home: PathBuf::from("/tmp/user"),
            invoking_user: Some("user".into()),
        };
        assert_eq!(advertised_url(&plan), "http://127.0.0.1:9123");
        let selected = invoking_installation_root_with(
            &plan,
            Some(std::ffi::OsString::from("/private/tmp/labby-preview")),
        )
        .unwrap();
        assert_eq!(selected, PathBuf::from("/private/tmp/labby-preview"));
        assert!(invoking_installation_root_with(
            &plan,
            Some(std::ffi::OsString::from("relative-root")),
        )
        .is_err());

        let bound_to_other_interface = SetupPlan {
            host: "192.0.2.10".into(),
            ..plan
        };
        assert_eq!(
            advertised_url(&bound_to_other_interface),
            "http://192.0.2.10:9123"
        );
    }

    #[test]
    fn google_callback_uses_public_origin_without_duplicate_slash() {
        assert_eq!(
            google_callback_url("https://labby.example.com/"),
            "https://labby.example.com/auth/google/callback"
        );
    }

    #[test]
    fn public_oauth_url_allows_loopback_http_but_not_remote_http() {
        assert!(validate_public_url("http://127.0.0.1:8765").is_ok());
        assert!(validate_public_url("https://labby.example.com").is_ok());
        assert!(validate_public_url("http://192.168.1.50:8765").is_err());
    }
}
