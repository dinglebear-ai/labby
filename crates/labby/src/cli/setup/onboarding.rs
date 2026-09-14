//! Typed first-run onboarding shared by the one-line installer and direct CLI use.

#[cfg(target_os = "linux")]
use std::io::Read as _;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context as _, Result, bail};
use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(target_os = "linux")]
use sha2::{Digest as _, Sha256};

use super::{SetupArgs, SetupDeploymentArg, SetupOauthArg, SetupRoleArg};
use crate::config::env_merge::{self, EnvEntry, MergeRequest};
use crate::output::{OutputFormat, print};

#[cfg(target_os = "linux")]
const SERVER_ENV: &str = "/home/labby/.labby/.env";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8765;
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
    role: SetupRoleArg,
    deployment: Option<SetupDeploymentArg>,
    host: String,
    port: u16,
    server_url: Option<String>,
    public_url: Option<String>,
    oauth: Option<OAuthConfig>,
    client_auth: Option<ClientAuth>,
    client_bearer_token: Option<String>,
    install_desktop: bool,
    no_browser: bool,
    invoking_home: PathBuf,
    invoking_user: Option<String>,
}

pub(super) async fn run(args: SetupArgs, format: OutputFormat) -> Result<ExitCode> {
    let interactive = std::io::stdin().is_terminal() && !args.yes;
    if interactive {
        print_banner();
    }
    let plan = collect_plan(&args, interactive)?;
    if args.dry_run {
        print(&redacted_plan(&plan), format)?;
        return Ok(ExitCode::SUCCESS);
    }

    if requires_root(&plan) && !nix::unistd::Uid::effective().is_root() {
        return elevate_and_apply(&plan);
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
    if requires_root(&plan) && !nix::unistd::Uid::effective().is_root() {
        bail!("native Linux server setup requires root privileges");
    }
    apply(plan, format).await
}

fn collect_plan(args: &SetupArgs, interactive: bool) -> Result<SetupPlan> {
    let theme = ColorfulTheme::default();
    let role = match args.role {
        Some(role) => role,
        None if interactive => match Select::with_theme(&theme)
            .with_prompt("What are we setting up?")
            .items(&[
                "Server — run Labby here",
                "Client — connect to a Labby server",
            ])
            .default(0)
            .interact()?
        {
            0 => SetupRoleArg::Server,
            _ => SetupRoleArg::Client,
        },
        None => bail!("non-interactive setup requires --role server|client"),
    };

    let invoking_home =
        dirs::home_dir().context("could not determine the invoking user's home directory")?;
    let invoking_user = std::env::var("SUDO_USER")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .filter(|value| !value.trim().is_empty() && value != "root");

    match role {
        SetupRoleArg::Server => {
            collect_server_plan(args, interactive, &theme, invoking_home, invoking_user)
        }
        SetupRoleArg::Client => {
            collect_client_plan(args, interactive, &theme, invoking_home, invoking_user)
        }
    }
}

fn collect_server_plan(
    args: &SetupArgs,
    interactive: bool,
    theme: &ColorfulTheme,
    invoking_home: PathBuf,
    invoking_user: Option<String>,
) -> Result<SetupPlan> {
    let incus_ready =
        cfg!(all(target_os = "linux", target_arch = "x86_64")) && command_ok("incus", &["version"]);
    let deployment = match args.deployment {
        Some(value) => value,
        None if interactive && incus_ready => match Select::with_theme(theme)
            .with_prompt("Deployment")
            .items(&["Native service — fastest", "Incus container — isolated"])
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
        None if interactive => validate_host(
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
        None if interactive => Input::<u16>::with_theme(theme)
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
        None => DEFAULT_PORT,
    };

    let provider = match args.oauth {
        Some(value) => value,
        None if interactive => match Select::with_theme(theme)
            .with_prompt("Authentication")
            .items(&[
                "Bearer token — generated automatically",
                "Google OAuth + bearer break-glass",
                "Authelia OAuth + bearer break-glass",
            ])
            .default(0)
            .interact()?
        {
            1 => SetupOauthArg::Google,
            2 => SetupOauthArg::Authelia,
            _ => SetupOauthArg::None,
        },
        None => SetupOauthArg::None,
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
        role: SetupRoleArg::Server,
        deployment: Some(deployment),
        host,
        port,
        server_url: None,
        public_url,
        oauth,
        client_auth: None,
        client_bearer_token: None,
        install_desktop,
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
            .items(&["Browser sign-in (OAuth)", "Bearer token"])
            .default(0)
            .interact()?
        {
            1 => ClientAuth::Bearer,
            _ => ClientAuth::OAuth,
        }
    } else if std::env::var("LABBY_MCP_HTTP_TOKEN")
        .ok()
        .is_some_and(|v| !v.trim().is_empty())
    {
        ClientAuth::Bearer
    } else {
        ClientAuth::OAuth
    };
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
        role: SetupRoleArg::Client,
        deployment: None,
        host: DEFAULT_HOST.to_string(),
        port: DEFAULT_PORT,
        server_url: Some(server_url),
        public_url: None,
        oauth: None,
        client_auth: Some(client_auth),
        client_bearer_token,
        install_desktop: desktop_choice(args, interactive, theme)?,
        no_browser: args.no_browser,
        invoking_home,
        invoking_user,
    })
}

fn desktop_choice(args: &SetupArgs, interactive: bool, theme: &ColorfulTheme) -> Result<bool> {
    if args.desktop {
        return Ok(true);
    }
    if args.no_desktop {
        return Ok(false);
    }
    if !desktop_supported() || !interactive {
        return Ok(false);
    }
    Ok(Confirm::with_theme(theme)
        .with_prompt("Install the Labby desktop app?")
        .default(true)
        .interact()?)
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

fn elevate_and_apply(plan: &SetupPlan) -> Result<ExitCode> {
    let plan_dir = plan.invoking_home.join(".labby");
    std::fs::create_dir_all(&plan_dir)?;
    let mut plan_file = tempfile::NamedTempFile::new_in(&plan_dir)?;
    serde_json::to_writer(&mut plan_file, plan)?;
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

async fn apply(plan: SetupPlan, format: OutputFormat) -> Result<ExitCode> {
    let result = match plan.role {
        SetupRoleArg::Server => apply_server(&plan, format).await?,
        SetupRoleArg::Client => apply_client(&plan).await?,
    };
    print(&result, format)?;
    Ok(ExitCode::SUCCESS)
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

async fn apply_native_server(plan: &SetupPlan) -> Result<serde_json::Value> {
    #[cfg(target_os = "linux")]
    {
        if !nix::unistd::Uid::effective().is_root() {
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
        configure_local_client(plan, &token)?;
        if plan.install_desktop {
            install_desktop(plan)?;
        }
        return Ok(json!({
            "ok": true,
            "role": "server",
            "deployment": "native",
            "service": outcome.message,
            "web": advertised_url(plan),
            "mcp": format!("{}/mcp", advertised_url(plan).trim_end_matches('/')),
            "client_configured": true,
            "desktop_requested": plan.install_desktop,
            "features": "full",
        }));
    }
    #[cfg(target_os = "macos")]
    {
        let home_env = plan.invoking_home.join(".labby/.env");
        let token = configure_server_env(&home_env, plan)?;
        install_macos_service(plan)?;
        configure_local_client(plan, &token)?;
        if plan.install_desktop {
            install_desktop(plan)?;
        }
        return Ok(json!({
            "ok": true,
            "role": "server",
            "deployment": "native",
            "persistence": "launch_agent_at_login",
            "web": advertised_url(plan),
            "mcp": format!("{}/mcp", advertised_url(plan).trim_end_matches('/')),
            "desktop_requested": plan.install_desktop,
            "features": "full",
        }));
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    bail!(
        "native persistent server installation is not yet supported on this operating system; use client mode"
    )
}

#[cfg(target_os = "linux")]
async fn apply_incus_server(plan: &SetupPlan, format: OutputFormat) -> Result<serde_json::Value> {
    let image = ensure_release_incus_image()?;
    let args = crate::cli::incus::IncusSetupArgs {
        image: Some(image),
        skip_install: true,
        yes: true,
        dry_run: false,
        ..crate::cli::incus::IncusSetupArgs::default()
    };
    crate::cli::incus::run_setup(args, format).await?;
    // The release image owns the guest's internal :8765 listener. Host
    // publishing and operator-specific authentication are applied afterwards,
    // keeping immutable image bytes free of user secrets.
    let token = configure_incus_server(plan)?;
    converge_incus_publish("labby", &plan.host, plan.port)?;
    configure_local_client(plan, &token)?;
    if plan.install_desktop {
        install_desktop(plan)?;
    }
    Ok(json!({
        "ok": true,
        "role": "server",
        "deployment": "incus",
        "web": advertised_url(plan),
        "mcp": format!("{}/mcp", advertised_url(plan).trim_end_matches('/')),
        "desktop_requested": plan.install_desktop,
        "features": "full",
    }))
}

#[cfg(not(target_os = "linux"))]
async fn apply_incus_server(_plan: &SetupPlan, _format: OutputFormat) -> Result<serde_json::Value> {
    bail!("Incus server deployment is currently supported only on Linux hosts")
}

#[cfg(target_os = "linux")]
fn ensure_release_incus_image() -> Result<String> {
    const IMAGE_ASSET: &str = "labby-incus-x86_64-unknown-linux-gnu.tar.xz";
    const REPO: &str = "dinglebear-ai/labby";
    const SIGNER_WORKFLOW: &str = "dinglebear-ai/labby/.github/workflows/release.yml";

    if !cfg!(target_arch = "x86_64") {
        bail!("the prebuilt Labby Incus image is currently published only for x86_64 Linux hosts");
    }
    if !command_ok("gh", &["--version"]) {
        bail!("GitHub CLI (gh) is required to verify the Labby release manifest and Incus image");
    }

    let version = env!("CARGO_PKG_VERSION");
    let tag = format!("v{version}");
    let source_ref = format!("refs/tags/{tag}");
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
            "release-manifest.json",
            "--pattern",
            IMAGE_ASSET,
            "--dir",
            temp_path,
        ],
        "download exact Labby Incus release",
    )?;
    let manifest = temp.path().join("release-manifest.json");
    let image = temp.path().join(IMAGE_ASSET);
    if !manifest.is_file() || !image.is_file() {
        bail!("release {tag} is missing its immutable manifest or prebuilt Incus image");
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
            &source_ref,
            "--deny-self-hosted-runners",
        ],
        "verify Labby release manifest provenance",
    )?;

    let manifest_value: serde_json::Value = serde_json::from_reader(
        std::fs::File::open(&manifest).context("open verified release manifest")?,
    )
    .context("parse verified release manifest")?;
    let incus = manifest_value
        .pointer("/distributions/incus")
        .and_then(serde_json::Value::as_object)
        .context("release manifest is missing the Incus distribution")?;
    let manifest_asset = incus
        .get("asset")
        .and_then(serde_json::Value::as_str)
        .context("release manifest is missing the Incus asset name")?;
    let expected_sha256 = incus
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .context("release manifest is missing the Incus SHA-256")?;
    if manifest_asset != IMAGE_ASSET {
        bail!(
            "release manifest Incus asset {manifest_asset:?} does not match the supported asset {IMAGE_ASSET:?}"
        );
    }
    if expected_sha256.len() != 64 || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("release manifest contains an invalid Incus SHA-256");
    }
    let actual_sha256 = sha256_file(&image)?;
    if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "prebuilt Incus image digest mismatch: expected {expected_sha256}, got {actual_sha256}"
        );
    }

    let alias = format!("labby-release-{version}-{}", &actual_sha256[..12]);
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
fn configure_incus_server(plan: &SetupPlan) -> Result<String> {
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
    let server_url = plan
        .server_url
        .as_deref()
        .context("client setup requires a server URL")?;
    let env_path = plan.invoking_home.join(".labby/.env");
    let mut entries = vec![EnvEntry::new("LABBY_SERVER_URL", server_url).force()];
    if let Some(token) = plan.client_bearer_token.as_deref() {
        entries.push(EnvEntry::new("LABBY_MCP_HTTP_TOKEN", token).force());
    }
    merge_env(&env_path, entries)?;
    if plan.install_desktop {
        install_desktop(plan)?;
    }
    if matches!(plan.client_auth, Some(ClientAuth::OAuth)) {
        let server = crate::oauth::cli_session::server_url(server_url)?;
        crate::oauth::cli_session::login(&server, None).await?;
    }
    Ok(json!({
        "ok": true,
        "role": "client",
        "server": server_url,
        "auth": match plan.client_auth { Some(ClientAuth::Bearer) => "bearer", _ => "oauth" },
        "desktop_requested": plan.install_desktop,
    }))
}

fn configure_server_env(path: &Path, plan: &SetupPlan) -> Result<String> {
    if !path.exists() {
        crate::dispatch::setup::bootstrap_at(path)
            .map_err(|e| anyhow::anyhow!("bootstrap server credentials: {e}"))?;
    }
    let token = read_env(path, "LABBY_MCP_HTTP_TOKEN")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(crate::dispatch::setup::generate_mcp_token);
    let mut entries = vec![
        EnvEntry::new("LABBY_MCP_HTTP_TOKEN", token.clone()).force(),
        EnvEntry::new("LABBY_MCP_TRANSPORT", "http").force(),
        EnvEntry::new("LABBY_MCP_HTTP_HOST", plan.host.clone()).force(),
        EnvEntry::new("LABBY_MCP_HTTP_PORT", plan.port.to_string()).force(),
    ];
    if let Some(public_url) = plan.public_url.as_deref() {
        entries.push(EnvEntry::new("LABBY_PUBLIC_URL", public_url).force());
    }
    match plan.oauth.as_ref() {
        None => entries.push(EnvEntry::new("LABBY_AUTH_MODE", "bearer").force()),
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

fn configure_local_client(plan: &SetupPlan, token: &str) -> Result<()> {
    let env = plan.invoking_home.join(".labby/.env");
    let server = format!("http://127.0.0.1:{}", plan.port);
    merge_env(
        &env,
        vec![
            EnvEntry::new("LABBY_SERVER_URL", server).force(),
            EnvEntry::new("LABBY_MCP_HTTP_TOKEN", token).force(),
        ],
    )?;
    if nix::unistd::Uid::effective().is_root()
        && let Some(user) = plan.invoking_user.as_deref()
    {
        chown_tree(&plan.invoking_home.join(".labby"), user)?;
    }
    Ok(())
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
    dotenvy::from_path_iter(path)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|(entry_key, value)| (entry_key == key).then_some(value))
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
        .env("LABBY_HOME", plan.invoking_home.join(".labby"))
        .status()?;
    if !status.success() {
        bail!("macOS service installer exited with {status}");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn install_macos_service(_plan: &SetupPlan) -> Result<()> {
    bail!("macOS service installer is unavailable")
}

#[cfg(target_os = "linux")]
fn converge_incus_publish(name: &str, host: &str, port: u16) -> Result<()> {
    let listen = format!("tcp:{host}:{port}");
    let exists = Command::new("incus")
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
        Command::new("incus")
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
        Command::new("incus")
            .args([
                "config",
                "device",
                "add",
                name,
                "labby-http",
                "proxy",
                "listen",
                &listen,
                "connect=tcp:127.0.0.1:8765",
            ])
            .status()?
    };
    if !status.success() {
        bail!("failed to publish Incus Labby endpoint on {host}:{port}");
    }
    let autostart = Command::new("incus")
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
        "oauth": plan.oauth.as_ref().map(|provider| match provider { OAuthConfig::Google { .. } => "google", OAuthConfig::Authelia { .. } => "authelia" }),
        "client_auth": plan.client_auth,
        "install_desktop": plan.install_desktop,
        "no_browser": plan.no_browser,
    })
}

fn print_banner() {
    eprintln!(
        r#"
  _          _     _
 | |    __ _| |__ | |__  _   _
 | |   / _` | '_ \| '_ \| | | |
 | |__| (_| | |_) | |_) | |_| |
 |_____\__,_|_.__/|_.__/ \__, |
                         |___/

  One setup. Every interface.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertised_url_never_uses_unspecified_address() {
        let plan = SetupPlan {
            role: SetupRoleArg::Server,
            deployment: Some(SetupDeploymentArg::Native),
            host: "0.0.0.0".into(),
            port: 9123,
            server_url: None,
            public_url: None,
            oauth: None,
            client_auth: None,
            client_bearer_token: None,
            install_desktop: false,
            no_browser: false,
            invoking_home: PathBuf::from("/tmp/user"),
            invoking_user: Some("user".into()),
        };
        assert_eq!(advertised_url(&plan), "http://127.0.0.1:9123");
    }

    #[test]
    fn public_oauth_url_allows_loopback_http_but_not_remote_http() {
        assert!(validate_public_url("http://127.0.0.1:8765").is_ok());
        assert!(validate_public_url("https://labby.example.com").is_ok());
        assert!(validate_public_url("http://192.168.1.50:8765").is_err());
    }
}
