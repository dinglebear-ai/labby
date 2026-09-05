//! `labby doctor` — focused health checks and full audit.
//!
//! Subcommands:
//!   labby doctor              — full audit (system + auth + gateway + relay)
//!   labby doctor system       — local system checks only
//!   labby doctor auth         — auth/OAuth configuration checks
//!   labby doctor oauth-relay  — public OAuth callback relay registry checks
//!
//! Exit codes: 0 = ok, 1 = warnings, 2 = failures.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::dispatch::clients::ServiceClients;
use crate::dispatch::doctor::{
    Finding, Report, Severity, run_auth_checks_with_config, run_system_checks,
};
use crate::output::OutputFormat;
use crate::output::theme::CliTheme;

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[command(subcommand)]
    pub check: Option<DoctorCheck>,
}

#[derive(Debug, Subcommand)]
pub enum DoctorCheck {
    /// Check auth/OAuth configuration (env vars, files, permissions)
    Auth(DoctorAuthArgs),
    /// Check public OAuth callback relay registry and optionally target sockets
    OauthRelay(DoctorOauthRelayArgs),
    /// Check public Lab and protected MCP proxy endpoints from caller-visible URLs
    Proxy(DoctorProxyArgs),
    /// Run local system checks (env vars, Docker, disk, toolchain)
    System,
}

#[derive(Debug, Args)]
pub struct DoctorAuthArgs {
    /// Explicitly probe the configured provider's discovery and JWKS endpoints
    #[arg(long)]
    pub live: bool,
}

#[derive(Debug, Args)]
pub struct DoctorProxyArgs {
    /// Public Lab app URL, e.g. <https://lab.example.com> (default: LABBY_PUBLIC_URL)
    #[arg(long)]
    pub app_url: Option<String>,
    /// Public MCP gateway URL, e.g. <https://mcp.example.com> (default: LABBY_MCP_GATEWAY_URL)
    #[arg(long)]
    pub mcp_url: Option<String>,
    /// Protected MCP public route path, e.g. /telemetry
    #[arg(long)]
    pub route: Option<String>,
    /// Optional private backend origin for backend-leak probe, e.g. `http://mcp-backend:3100`
    #[arg(long)]
    pub backend_url: Option<String>,
}

#[derive(Debug, Args)]
pub struct DoctorOauthRelayArgs {
    /// Probe registered target sockets in addition to registry readiness
    #[arg(long)]
    pub probe_targets: bool,
}

/// Run the doctor subcommand.
pub async fn run(
    args: DoctorArgs,
    format: OutputFormat,
    config: &crate::config::LabConfig,
) -> Result<ExitCode> {
    match args.check {
        None => run_full_audit(format, config).await,
        Some(DoctorCheck::Auth(args)) => run_auth(args, format, config).await,
        Some(DoctorCheck::OauthRelay(args)) => run_oauth_relay(args, format).await,
        Some(DoctorCheck::Proxy(args)) => run_proxy(args, format).await,
        Some(DoctorCheck::System) => run_system(format).await,
    }
}

// ---------------------------------------------------------------------------
// Full audit (existing default behaviour)
// ---------------------------------------------------------------------------

async fn run_full_audit(
    format: OutputFormat,
    config: &crate::config::LabConfig,
) -> Result<ExitCode> {
    use tokio::sync::mpsc;
    let clients = Arc::new(ServiceClients::from_env());
    let (tx, mut rx) = mpsc::channel(64);
    let public_relay = load_optional_public_relay_manager().await;
    let resolved_auth = crate::config::resolve_auth_for_config(config).ok();

    tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_audit_full_with_relay_and_auth(
            clients,
            public_relay,
            resolved_auth,
            tx,
        )
        .await;
    });

    let mut findings: Vec<Finding> = Vec::new();

    if format.is_json() {
        while let Some(f) = rx.recv().await {
            findings.push(f);
        }
        let report = Report { findings };
        println!("{}", serde_json::to_string_pretty(&report)?);
        Ok(exit_code(&report))
    } else {
        let theme = CliTheme::from_context(format.render_context());
        while let Some(f) = rx.recv().await {
            print_finding(theme, &f);
            findings.push(f);
        }
        Ok(exit_code(&Report { findings }))
    }
}

// ---------------------------------------------------------------------------
// auth subcommand
// ---------------------------------------------------------------------------

async fn run_auth(
    args: DoctorAuthArgs,
    format: OutputFormat,
    config: &crate::config::LabConfig,
) -> Result<ExitCode> {
    let resolved = crate::config::resolve_auth_for_config(config)?;
    let resolved_for_checks = resolved.clone();
    let mut findings = tokio::task::spawn_blocking(move || {
        run_auth_checks_with_config(Some(&resolved_for_checks))
    })
    .await
    .map_err(|e| anyhow::anyhow!("auth.check panicked: {e}"))?;
    if args.live {
        findings.push(run_auth_live_probe(&resolved).await);
    }

    let report = Report { findings };

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "Auth / OAuth configuration");

    // Group and label findings by check category
    let groups: &[(&str, &str)] = &[
        ("auth:mode", "Mode"),
        ("auth:provider", "Provider"),
        ("auth:provider-config-fingerprint", "Provider"),
        ("auth:provider-generation", "Provider"),
        ("auth:access-token-window", "Provider"),
        ("auth:web-ui-auth-disabled", "Safety gate"),
        ("auth:bearer-token", "Bearer token"),
        ("auth:public-url", "Public URL"),
        ("auth:google-client-id", "Google credentials"),
        ("auth:google-client-secret", "Google credentials"),
        ("auth:authelia-issuer", "Authelia credentials"),
        ("auth:authelia-client-id", "Authelia credentials"),
        ("auth:authelia-client-secret", "Authelia credentials"),
        ("auth:token-encryption-key", "Credential encryption"),
        ("auth:sqlite-path", "Auth store"),
        ("auth:key-path", "Auth store"),
        ("auth:sqlite-perms", "Auth store"),
        ("auth:key-perms", "Auth store"),
    ];

    let mut last_group = "";
    for f in &report.findings {
        // Print section header when the group label changes
        let group_label = groups
            .iter()
            .find(|(check, _)| f.check == *check)
            .map(|(_, label)| *label)
            .unwrap_or("Other");
        if group_label != last_group {
            if !last_group.is_empty() {
                println!();
            }
            println!("  {}", theme.primary(&format!("{group_label}:")));
            last_group = group_label;
        }
        print_finding_indented(theme, f);
    }
    println!();

    Ok(exit_code(&report))
}

async fn run_auth_live_probe(config: &labby_auth::config::AuthConfig) -> Finding {
    let Some(authelia) = config.authelia.clone() else {
        return Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Warn,
            message: "live provider probe is currently available for Authelia".into(),
        };
    };
    let Some(public_url) = config.public_url.as_ref() else {
        return Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "LABBY_PUBLIC_URL is required for the live provider probe".into(),
        };
    };
    let redirect =
        match public_url.join(labby_auth::config::AUTHELIA_CALLBACK_PATH.trim_start_matches('/')) {
            Ok(url) => url,
            Err(_) => {
                return Finding {
                    service: "auth".into(),
                    check: "auth:live-provider-probe".into(),
                    severity: Severity::Fail,
                    message: "public URL cannot form the provider callback".into(),
                };
            }
        };
    match tokio::time::timeout(
        std::time::Duration::from_secs(35),
        labby_auth::authelia::AutheliaProvider::live_probe(authelia, redirect),
    )
    .await
    {
        Ok(Ok(())) => Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Ok,
            message: "Authelia discovery and JWKS probe passed".into(),
        },
        Ok(Err(_)) => Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "Authelia discovery/JWKS probe failed; inspect redacted server debug logs"
                .into(),
        },
        Err(_) => Finding {
            service: "auth".into(),
            check: "auth:live-provider-probe".into(),
            severity: Severity::Fail,
            message: "Authelia discovery/JWKS probe exceeded 35 seconds".into(),
        },
    }
}

async fn run_oauth_relay(args: DoctorOauthRelayArgs, format: OutputFormat) -> Result<ExitCode> {
    let manager = load_optional_public_relay_manager().await;
    let report = crate::dispatch::doctor::check_public_relay(manager, args.probe_targets).await;

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "OAuth callback relay");
    for finding in &report.findings {
        print_finding_indented(theme, finding);
    }
    println!();

    Ok(exit_code(&report))
}

async fn load_optional_public_relay_manager()
-> Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>> {
    let store = crate::oauth::public_relay::PublicRelayRegistryStore::new(
        crate::oauth::public_relay::PublicRelayRegistryStore::default_path(),
    );
    if !store.path().exists() {
        return None;
    }
    let registry_path = store.path().to_path_buf();
    match crate::oauth::public_relay::PublicRelayRegistryManager::load(store).await {
        Ok(manager) => Some(Arc::new(manager)),
        Err(error) => {
            // This path is a best-effort optimization: `check_public_relay`'s
            // `None` branch independently reloads the same file and
            // re-surfaces the error as a finding, so a silent `None` here
            // is not fatal. Still log it -- matches the pattern in
            // `cli/serve.rs::run` for the identical load-at-startup case --
            // so a load failure is visible even if a future caller of this
            // helper doesn't have that fallback.
            tracing::warn!(
                subsystem = "doctor",
                phase = "oauth.public_relay.load_failed",
                registry_path = %registry_path.display(),
                kind = error.kind(),
                error = %error,
                "doctor failed to load public oauth callback relay registry"
            );
            None
        }
    }
}

async fn run_proxy(args: DoctorProxyArgs, format: OutputFormat) -> Result<ExitCode> {
    let Some(route) = args.route else {
        let value = crate::dispatch::doctor::dispatch_with_surface(
            "proxy.preflight",
            serde_json::json!({}),
            "cli",
        )
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let report: Report = serde_json::from_value(value)?;

        if format.is_json() {
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(exit_code(&report));
        }

        let theme = CliTheme::from_context(format.render_context());
        print_section(theme, "Stdio proxy preflight");
        for finding in &report.findings {
            print_finding_indented(theme, finding);
        }
        println!();
        return Ok(exit_code(&report));
    };
    let app_url = args
        .app_url
        .or_else(|| {
            std::env::var("LABBY_PUBLIC_URL")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .ok_or_else(|| anyhow::anyhow!("--app-url is required (or set LABBY_PUBLIC_URL)"))?;
    let mcp_url = args
        .mcp_url
        .or_else(|| {
            std::env::var("LABBY_MCP_GATEWAY_URL")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .ok_or_else(|| anyhow::anyhow!("--mcp-url is required (or set LABBY_MCP_GATEWAY_URL)"))?;
    let mut params = serde_json::json!({
        "app_url": app_url,
        "mcp_url": mcp_url,
        "route": route,
    });
    if let Some(backend_url) = &args.backend_url {
        params["backend_url"] = serde_json::Value::String(backend_url.clone());
    }
    let value = crate::dispatch::doctor::dispatch("proxy.check", params)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let report: Report = serde_json::from_value(value)?;

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "Reverse proxy checks");
    for finding in &report.findings {
        print_finding_indented(theme, finding);
    }
    println!();

    Ok(exit_code(&report))
}

// ---------------------------------------------------------------------------
// system subcommand
// ---------------------------------------------------------------------------

async fn run_system(format: OutputFormat) -> Result<ExitCode> {
    let findings = run_system_checks().await;

    let report = Report { findings };

    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code(&report));
    }

    let theme = CliTheme::from_context(format.render_context());
    print_section(theme, "System checks");

    // Group by check prefix (before ':')
    let groups: &[(&str, &str)] = &[
        ("env:", "Environment variables"),
        ("config:", "Config files"),
        ("docker:", "Docker"),
        ("rust:", "Toolchain"),
        ("disk:", "Disk"),
    ];

    let mut last_group = "";
    for f in &report.findings {
        let prefix = f.check.split(':').next().unwrap_or("");
        let group_label = groups
            .iter()
            .find(|(pfx, _)| pfx.trim_end_matches(':') == prefix)
            .map(|(_, label)| *label)
            .unwrap_or("Other");
        if group_label != last_group {
            if !last_group.is_empty() {
                println!();
            }
            println!("  {}", theme.primary(&format!("{group_label}:")));
            last_group = group_label;
        }
        print_finding_indented(theme, f);
    }
    println!();

    Ok(exit_code(&report))
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn print_section(theme: CliTheme, title: &str) {
    // Aurora section style: bold-cyan title over a muted underline divider.
    println!("{}", theme.heading(title));
    println!();
}

fn print_finding(theme: CliTheme, f: &Finding) {
    println!(
        "{badge} {service} {check}: {msg}",
        badge = severity_badge(theme, f.severity),
        service = theme.muted(format!("[{}]", f.service)),
        check = theme.section(&f.check),
        msg = theme.muted(&f.message),
    );
}

fn print_finding_indented(theme: CliTheme, f: &Finding) {
    // Strip the category prefix (auth:, docker:, etc.) from the check name for cleaner display
    let check_label = f
        .check
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(&f.check);
    println!(
        "    {badge}  {check}: {msg}",
        badge = severity_badge(theme, f.severity),
        check = theme.section(check_label),
        msg = theme.muted(&f.message),
    );
}

/// Status glyph painted via the Aurora success/warn/error tokens, symbol-mode aware.
fn severity_badge(theme: CliTheme, s: Severity) -> String {
    match s {
        Severity::Ok => theme.ok_badge(),
        Severity::Warn => theme.warn_badge(),
        Severity::Fail => theme.error_badge(),
    }
}

fn exit_code(report: &Report) -> ExitCode {
    match report.worst() {
        Severity::Ok => ExitCode::SUCCESS,
        Severity::Warn => ExitCode::from(1),
        Severity::Fail => ExitCode::from(2),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::{Cli, Command};

    #[test]
    fn auth_checks_returns_findings() {
        let findings = crate::dispatch::doctor::run_auth_checks();
        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.check == "auth:mode"));
        assert!(findings.iter().any(|f| f.check == "auth:bearer-token"));
        assert!(findings.iter().any(|f| f.check == "auth:public-url"));
    }

    #[tokio::test]
    async fn live_probe_requires_an_authelia_configuration_without_network_io() {
        let finding = super::run_auth_live_probe(&labby_auth::config::AuthConfig::default()).await;
        assert_eq!(finding.check, "auth:live-provider-probe");
        assert!(matches!(
            finding.severity,
            crate::dispatch::doctor::Severity::Warn
        ));
    }

    #[test]
    fn doctor_oauth_relay_cli_parses_probe_targets() {
        let cli = Cli::try_parse_from(["lab", "doctor", "oauth-relay", "--probe-targets"])
            .expect("oauth relay doctor command should parse");

        match cli.command {
            Command::Doctor(super::DoctorArgs {
                check: Some(super::DoctorCheck::OauthRelay(args)),
            }) => assert!(args.probe_targets),
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
