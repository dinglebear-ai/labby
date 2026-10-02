//! Guided adapter over the existing OAuth setup and stdio proxy runtimes.

use std::process::{Command, ExitCode};

use anyhow::{Context as _, Result, bail};
use dialoguer::{Confirm, theme::ColorfulTheme};

use super::{SetupArgs, SetupAuthArg, SetupOauthArg, SetupRoleArg};
use crate::output::OutputFormat;
use crate::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPortPreference, ProxyPreferences};

const GOOGLE_CONSOLE: &str = "https://console.cloud.google.com/auth/clients";
const TAILSCALE_DOWNLOAD: &str = "https://tailscale.com/download";

pub(super) async fn run(
    mut args: SetupArgs,
    interactive: bool,
    format: OutputFormat,
) -> Result<ExitCode> {
    if args.host.is_some()
        || args.server_url.is_some()
        || args.auth.is_some()
        || args.oauth.is_some()
    {
        bail!(
            "ChatGPT setup selects Google OAuth and a loopback proxy; omit --host, --server-url, --auth, and --oauth"
        );
    }
    let public_port = args.port.unwrap_or(443);
    if !matches!(public_port, 443 | 8443 | 10000) {
        bail!("Tailscale Funnel supports public HTTPS ports 443, 8443, and 10000");
    }
    if !args.dry_run {
        ensure_dependencies(interactive, args.skip_deps, args.no_browser)?;
    }
    let mut options = crate::proxy::tailscale::TailscaleServeOptions::for_proxy(
        ([127, 0, 0, 1], 1).into(),
        "/mcp".into(),
        ProxyPortPreference::Fixed(public_port),
        443,
        10000,
    );
    options.exposure = ProxyExposure::Funnel;
    options.auth = ProxyAuthMode::Oauth;
    let publication = crate::proxy::tailscale::TailscaleServePlan::prepare(options)
        .await
        .context("Cannot prepare a public Funnel endpoint. If public 443 is already configured, keep that mapping and use another Tailscale node, or explicitly try --port 8443/10000. Nonstandard ports have not passed our ChatGPT qualification. Labby's local port is separate from this public port.")?;
    let mut origin = publication.public_url().clone();
    origin.set_path("");
    let origin = origin.as_str().trim_end_matches('/').to_string();
    if args
        .public_url
        .as_deref()
        .is_some_and(|value| value.trim_end_matches('/') != origin)
    {
        bail!("--public-url must match the derived Funnel origin {origin}");
    }
    eprintln!(
        "\nGoogle callback: {origin}/auth/google/callback\nGoogle Cloud: {GOOGLE_CONSOLE}\n\nComplete Branding and Audience; add your account as a test user for an External app in Testing. Create a Web application client and paste the callback into Authorized redirect URIs. Use only openid, email, and profile.\n\nPublic HTTPS port: {public_port}. Labby uses a separate local loopback port; no local privileged port bind is required.\n"
    );
    let paths = crate::installation::InstallationPaths::resolve()?;
    // Reuse the service-free OAuth configuration flow. It opens Google Cloud
    // before requesting credentials, stores secrets privately, and preserves keys.
    args.chatgpt = false;
    args.role = Some(SetupRoleArg::Server);
    args.config_only = true;
    args.auth = Some(SetupAuthArg::OAuth);
    args.oauth = Some(SetupOauthArg::Google);
    args.public_url = Some(origin);
    args.port = None;
    let dry_run = args.dry_run;
    let mut outcome = super::onboarding::configure_only(&args, interactive)?;
    let preferences = ProxyPreferences {
        exposure: ProxyExposure::Funnel,
        auth: ProxyAuthMode::Oauth,
        port: ProxyPortPreference::Fixed(public_port),
        ..ProxyPreferences::default()
    };
    crate::dispatch::setup::proxy::configure_at(
        &paths.root().join("config.toml"),
        &paths.root().join(".env"),
        crate::dispatch::setup::proxy::ProxySetupRequest {
            preferences,
            bearer_token: None,
            dry_run,
        },
    )
    .map_err(|error| anyhow::anyhow!("proxy preferences could not be saved: {error}"))?;
    let mcp_path = paths.root().join(".mcp.json");
    if !dry_run {
        crate::dispatch::setup::proxy::configure_microsandbox_at(&mcp_path)?;
    }
    outcome["mcp_url"] = serde_json::json!(publication.public_url());
    outcome["mcp_json_path"] = serde_json::json!(mcp_path);
    outcome["funnel_port"] = serde_json::json!(public_port);
    crate::output::print(&outcome, format)?;
    eprintln!(
        "\nNext: run npx -y @dinglebear/labby proxy\nIn ChatGPT, enable Developer mode and create an OAuth app with URL {}. Then ask it to run runtime_check before creating a sandbox.\nMicrosandbox connects to Labby through {}. Existing servers and any customized Microsandbox definition are preserved.\n",
        publication.public_url(),
        mcp_path.display()
    );
    if interactive
        && !dry_run
        && Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Start the proxy now? Keep this terminal open while using ChatGPT")
            .default(true)
            .interact()?
    {
        // A fresh process loads the saved env/config without mutating this
        // process's environment or accidentally retaining stale OAuth settings.
        let status = Command::new(std::env::current_exe()?)
            .arg("proxy")
            .status()?;
        if !status.success() {
            bail!("proxy exited unsuccessfully; configuration is retained, retry labby proxy");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn ensure_dependencies(interactive: bool, skip_deps: bool, no_browser: bool) -> Result<()> {
    let node_major = Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| node_major(&String::from_utf8_lossy(&output.stdout)));
    if !node_major.is_some_and(|major| major >= 22) {
        #[cfg(target_os = "macos")]
        if install_offer(
            "Node.js 22",
            "brew",
            &["install", "node@22"],
            interactive,
            skip_deps,
        )? {
            bail!("Node.js 22 installed. Add it to PATH as shown by Homebrew, then rerun setup");
        }
        #[cfg(target_os = "linux")]
        if install_offer(
            "Node.js 22",
            "mise",
            &["install", "node@22"],
            interactive,
            skip_deps,
        )? {
            bail!("Node.js 22 installed. Activate it with mise, then rerun setup");
        }
        bail!(
            "Node.js 22+ is required for npx Microsandbox. Install it from https://nodejs.org/en/download and rerun setup"
        );
    }
    if !super::onboarding::installed_client_program("npx") {
        bail!("npx is missing; install the npm component of Node.js and rerun setup");
    }
    if !super::onboarding::installed_client_program("tailscale") {
        #[cfg(target_os = "macos")]
        if std::path::Path::new("/Applications/Tailscale.app").exists() {
            if !no_browser && interactive {
                super::browser_handoff::open_url("tailscale://");
            }
            bail!(
                "Tailscale is already installed, but its CLI is not on PATH. Open Tailscale, sign in, enable its CLI, and rerun setup"
            );
        }
        #[cfg(target_os = "macos")]
        if install_offer(
            "Tailscale",
            "brew",
            &["install", "--cask", "tailscale"],
            interactive,
            skip_deps,
        )? {
            if !no_browser {
                super::browser_handoff::open_url("tailscale://");
            }
            bail!(
                "Tailscale installed. Open it, approve the VPN configuration, sign in, and ensure its CLI is on PATH; then rerun setup"
            );
        }
        #[cfg(target_os = "linux")]
        if !skip_deps && interactive && Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Download Tailscale's official installer from https://tailscale.com/install.sh and run it with sudo?")
            .default(true).interact()?
        {
            let installer = tempfile::NamedTempFile::new()?;
            if !Command::new("curl").args(["--fail", "--location", "--proto", "=https", "--tlsv1.2", "--max-time", "60", "--output"])
                .arg(installer.path()).arg("https://tailscale.com/install.sh").status()?.success()
            {
                bail!("Tailscale installer download failed; install from {TAILSCALE_DOWNLOAD} and rerun setup");
            }
            if !Command::new("sudo").arg("sh").arg(installer.path()).status()?.success() {
                bail!("Tailscale installation failed; resolve the installer error and rerun setup");
            }
            bail!("Tailscale installed. Run tailscale up to sign in, then rerun setup");
        }
        if !no_browser && interactive {
            super::browser_handoff::open_url(TAILSCALE_DOWNLOAD);
        }
        bail!(
            "Tailscale is required. Install it from {TAILSCALE_DOWNLOAD}, sign in, enable its CLI, and rerun setup"
        );
    }
    #[cfg(target_os = "macos")]
    if !cfg!(target_arch = "aarch64")
        || !Command::new("/usr/sbin/sysctl")
            .args(["-n", "kern.hv_support"])
            .output()
            .is_ok_and(|output| output.status.success() && output.stdout == b"1\n")
    {
        bail!("This Microsandbox setup requires an Apple Silicon Mac or Linux with KVM");
    }
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
        {
            bail!(
                "KVM is unavailable: {error}. Enable virtualization in firmware and grant this user access to /dev/kvm, then log in again and rerun setup. Hardware virtualization cannot be installed by Labby."
            );
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    bail!("Guided sandbox setup currently supports macOS Apple Silicon and Linux with KVM");
    let _ = (interactive, skip_deps, no_browser);
    Ok(())
}

fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn install_offer(
    label: &str,
    installer: &str,
    arguments: &[&str],
    interactive: bool,
    skip_deps: bool,
) -> Result<bool> {
    if skip_deps || !interactive || !super::onboarding::installed_client_program(installer) {
        return Ok(false);
    }
    if !Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Install {label} with {installer} {}?",
            arguments.join(" ")
        ))
        .default(true)
        .interact()?
    {
        return Ok(false);
    }
    if !Command::new(installer).args(arguments).status()?.success() {
        bail!("{label} installation failed; resolve the installer error and rerun setup");
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_versions_require_a_parseable_major() {
        assert_eq!(node_major("v22.18.0\n"), Some(22));
        assert_eq!(node_major("v24.0.0"), Some(24));
        assert_eq!(node_major("garbage"), None);
        assert_eq!(node_major("v"), None);
    }
}
