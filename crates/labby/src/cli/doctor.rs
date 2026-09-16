//! `labby doctor` — focused health checks and full audit.
//!
//! Subcommands:
//!   labby doctor              — full audit (system + auth + gateway + relay)
//!   labby doctor system       — local system checks only
//!   labby doctor auth         — auth/OAuth configuration checks
//!   labby doctor oauth-relay  — public OAuth callback relay registry checks
//!
//! Plain `labby doctor` answers operational readiness: 0 = operational (warnings are recommendations), 2 = blocked.
//! Focused subcommands preserve diagnostic exit codes: 0 = ok, 1 = warnings, 2 = failures.

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
    /// Check the complete personal OAuth path: config, provider, public metadata, login redirect, and MCP challenge.
    Oauth(DoctorOauthArgs),
    /// Check public OAuth callback relay registry and optionally target sockets
    OauthRelay(DoctorOauthRelayArgs),
    /// Check public Lab and protected MCP proxy endpoints from caller-visible URLs
    Proxy(DoctorProxyArgs),
    /// Write a redacted support bundle containing versions, setup state, safe config shape, and doctor findings.
    Bundle(DoctorBundleArgs),
    /// Run local system checks (env vars, Docker, disk, toolchain)
    System,
}

#[derive(Debug, Args)]
pub struct DoctorBundleArgs {
    /// Output path for the redacted JSON bundle.
    #[arg(long, default_value = "labby-support.json")]
    pub output: std::path::PathBuf,
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
pub struct DoctorOauthArgs {
    /// Override the public Labby origin. Defaults to the resolved OAuth public URL.
    #[arg(long)]
    pub public_url: Option<String>,
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
        Some(DoctorCheck::Oauth(args)) => run_oauth(args, format, config).await,
        Some(DoctorCheck::OauthRelay(args)) => run_oauth_relay(args, format).await,
        Some(DoctorCheck::Proxy(args)) => run_proxy(args, format).await,
        Some(DoctorCheck::Bundle(args)) => run_bundle(args, format, config).await,
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
    let (resolved_auth, auth_config_error) = match crate::config::resolve_auth_for_config(config) {
        Ok(auth) => (Some(auth), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let browser_oauth_expected = resolved_auth
        .as_ref()
        .is_some_and(|auth| auth.mode == labby_auth::config::AuthMode::OAuth);

    tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_audit_full_with_relay_and_auth(
            clients,
            public_relay,
            resolved_auth,
            tx.clone(),
        )
        .await;
        if let Some(message) = auth_config_error {
            if tx
                .send(crate::dispatch::doctor::auth_config_error_finding(&message))
                .await
                .is_err()
            {
                return;
            }
        }
    });

    let mut findings: Vec<Finding> = Vec::new();

    if format.is_json() {
        while let Some(f) = rx.recv().await {
            findings.push(f);
        }
        let exit = operational_exit_code(&findings);
        let readiness =
            crate::dispatch::doctor::personal_readiness_finding(&findings, browser_oauth_expected);
        findings.push(readiness);
        let report = Report { findings };
        println!("{}", serde_json::to_string_pretty(&report)?);
        Ok(exit)
    } else {
        let theme = CliTheme::from_context(format.render_context());
        while let Some(f) = rx.recv().await {
            print_finding(theme, &f);
            findings.push(f);
        }
        let exit = operational_exit_code(&findings);
        let readiness =
            crate::dispatch::doctor::personal_readiness_finding(&findings, browser_oauth_expected);
        println!();
        print_finding(theme, &readiness);
        Ok(exit)
    }
}

fn operational_exit_code(findings: &[Finding]) -> ExitCode {
    if findings
        .iter()
        .any(|finding| matches!(finding.severity, Severity::Fail))
    {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

async fn collect_full_audit_findings(config: &crate::config::LabConfig) -> Vec<Finding> {
    use tokio::sync::mpsc;

    let clients = Arc::new(ServiceClients::from_env());
    let (tx, mut rx) = mpsc::channel(64);
    let public_relay = load_optional_public_relay_manager().await;
    let (resolved_auth, auth_config_error) = match crate::config::resolve_auth_for_config(config) {
        Ok(auth) => (Some(auth), None),
        Err(error) => (None, Some(error.to_string())),
    };

    tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_audit_full_with_relay_and_auth(
            clients,
            public_relay,
            resolved_auth,
            tx.clone(),
        )
        .await;
        if let Some(message) = auth_config_error {
            let _unused = tx
                .send(crate::dispatch::doctor::auth_config_error_finding(&message))
                .await;
        }
    });

    let mut findings = Vec::new();
    while let Some(finding) = rx.recv().await {
        findings.push(finding);
    }
    findings
}

fn redact_support_text(raw: &str) -> String {
    let redacted = labby_runtime::agent_error::redact_secret_like_segments(raw);
    crate::dispatch::helpers::redact_home(&redacted)
}

fn redact_support_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => serde_json::Value::String(redact_support_text(&value)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(redact_support_value).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, redact_support_value(value)))
                .collect(),
        ),
        value => value,
    }
}

fn write_support_bundle(path: &std::path::Path, value: &serde_json::Value) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    if let Ok(metadata) = std::fs::symlink_metadata(parent) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!("refusing to write support bundle through a symlinked directory");
        }
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!("refusing to overwrite a symlink with a support bundle");
        }
    }

    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer_pretty(temporary.as_file_mut(), value)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| anyhow::anyhow!("persist support bundle: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

async fn run_bundle(
    args: DoctorBundleArgs,
    format: OutputFormat,
    config: &crate::config::LabConfig,
) -> Result<ExitCode> {
    let findings = collect_full_audit_findings(config).await;
    let report = Report {
        findings: findings.clone(),
    };
    let setup_state = match crate::dispatch::setup::dispatch("state", serde_json::json!({})).await {
        Ok(value) => redact_support_value(value),
        Err(error) => serde_json::json!({
            "status": "unavailable",
            "error": redact_support_text(&error.to_string()),
        }),
    };
    let resolved_auth = crate::config::resolve_auth_for_config(config).ok();
    let safe_upstreams: Vec<serde_json::Value> = config
        .upstream
        .iter()
        .map(|upstream| {
            let transport = if upstream.url.is_some() {
                "http"
            } else if upstream.socket_path.is_some() {
                "unix_socket"
            } else {
                "stdio"
            };
            serde_json::json!({
                "name": upstream.name,
                "enabled": upstream.enabled,
                "transport": transport,
                "proxy_resources": upstream.proxy_resources,
                "proxy_prompts": upstream.proxy_prompts,
                "proxy_skills": upstream.proxy_skills,
                "has_bearer_binding": upstream.bearer_token_env.is_some(),
                "has_oauth": upstream.oauth.is_some(),
                "injected_env_key_count": upstream.env.len(),
            })
        })
        .collect();
    let redacted_findings: Vec<serde_json::Value> = findings
        .iter()
        .map(|finding| {
            serde_json::json!({
                "service": finding.service,
                "check": finding.check,
                "severity": finding.severity,
                "message": redact_support_text(&finding.message),
            })
        })
        .collect();
    let generated_unix_seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let bundle = serde_json::json!({
        "schema_version": 1,
        "generated_unix_seconds": generated_unix_seconds,
        "labby": {
            "version": env!("CARGO_PKG_VERSION"),
            "setup_contract": crate::cli::setup::SETUP_CONTRACT_VERSION,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "privacy": {
            "raw_config_included": false,
            "raw_environment_included": false,
            "raw_logs_included": false,
            "credential_values_included": false,
            "private_key_material_included": false,
        },
        "auth": {
            "resolved": resolved_auth.is_some(),
            "mode": resolved_auth.as_ref().map(|auth| format!("{:?}", auth.mode).to_lowercase()),
            "provider": resolved_auth.as_ref().and_then(|auth| auth.inbound_provider.as_ref()).map(|provider| format!("{provider:?}").to_lowercase()),
            "public_url_configured": resolved_auth.as_ref().is_some_and(|auth| auth.public_url.is_some()),
            "admin_identity_configured": resolved_auth.as_ref().is_some_and(|auth| !auth.admin_email.trim().is_empty()),
            "allowed_email_domain_count": resolved_auth.as_ref().map_or(0, |auth| auth.allowed_email_domains.len()),
        },
        "setup": setup_state,
        "gateway": {
            "upstream_count": safe_upstreams.len(),
            "upstreams": safe_upstreams,
        },
        "doctor": {
            "worst": format!("{:?}", report.worst()).to_lowercase(),
            "findings": redacted_findings,
        },
    });

    write_support_bundle(&args.output, &bundle)?;
    if format.is_json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "path": args.output,
                "doctor_worst": format!("{:?}", report.worst()).to_lowercase(),
            }))?
        );
    } else {
        println!(
            "redacted support bundle written to {} (raw config/env/logs and credential values excluded)",
            args.output.display()
        );
    }
    Ok(exit_code(&report))
}

// ---------------------------------------------------------------------------
// auth subcommand
// ---------------------------------------------------------------------------

async fn run_auth(
    args: DoctorAuthArgs,
    format: OutputFormat,
    config: &crate::config::LabConfig,
) -> Result<ExitCode> {
    let resolved = match crate::config::resolve_auth_for_config(config) {
        Ok(resolved) => resolved,
        Err(error) => {
            let report = Report {
                findings: vec![crate::dispatch::doctor::auth_config_error_finding(
                    &error.to_string(),
                )],
            };
            if format.is_json() {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                let theme = CliTheme::from_context(format.render_context());
                print_section(theme, "Auth / OAuth configuration");
                for finding in &report.findings {
                    print_finding_indented(theme, finding);
                }
            }
            return Ok(exit_code(&report));
        }
    };
    let resolved_for_checks = resolved.clone();
    let mut findings = tokio::task::spawn_blocking(move || {
        run_auth_checks_with_config(Some(&resolved_for_checks))
    })
    .await
    .map_err(|e| anyhow::anyhow!("auth.check panicked: {e}"))?;
    if args.live {
        findings.push(crate::dispatch::doctor::provider::live_probe(Some(&resolved)).await);
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

async fn run_oauth(
    args: DoctorOauthArgs,
    format: OutputFormat,
    config: &crate::config::LabConfig,
) -> Result<ExitCode> {
    let resolved = match crate::config::resolve_auth_for_config(config) {
        Ok(resolved) => resolved,
        Err(error) => {
            let report = Report {
                findings: vec![crate::dispatch::doctor::auth_config_error_finding(
                    &error.to_string(),
                )],
            };
            if format.is_json() {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                let theme = CliTheme::from_context(format.render_context());
                print_section(theme, "Personal OAuth readiness");
                for finding in &report.findings {
                    print_finding_indented(theme, finding);
                }
            }
            return Ok(exit_code(&report));
        }
    };

    let resolved_for_checks = resolved.clone();
    let mut findings = tokio::task::spawn_blocking(move || {
        run_auth_checks_with_config(Some(&resolved_for_checks))
    })
    .await
    .map_err(|error| anyhow::anyhow!("OAuth doctor configuration checks panicked: {error}"))?;
    findings.push(crate::dispatch::doctor::provider::live_probe(Some(&resolved)).await);

    let public_url = args
        .public_url
        .or_else(|| resolved.public_url.as_ref().map(ToString::to_string));
    if let Some(public_url) = public_url {
        match crate::cli::setup::google_oauth_public_check(&public_url).await {
            Ok(value) => {
                if let Some(checks) = value.get("checks").and_then(serde_json::Value::as_array) {
                    for (index, check) in checks.iter().enumerate() {
                        let ok = check
                            .get("ok")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        let name = check
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("public OAuth check");
                        let detail = check
                            .get("detail")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        findings.push(Finding {
                            service: "oauth".into(),
                            check: format!("oauth:public:{index}"),
                            severity: if ok { Severity::Ok } else { Severity::Fail },
                            message: format!("{name}: {detail}"),
                        });
                    }
                }
            }
            Err(error) => findings.push(Finding {
                service: "oauth".into(),
                check: "oauth:public".into(),
                severity: Severity::Fail,
                message: format!("public OAuth probe failed: {error}"),
            }),
        }
    } else {
        findings.push(Finding {
            service: "oauth".into(),
            check: "oauth:public-url".into(),
            severity: Severity::Fail,
            message: "OAuth has no public URL; configure LABBY_PUBLIC_URL or pass --public-url"
                .into(),
        });
    }

    let report = Report { findings };
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let theme = CliTheme::from_context(format.render_context());
        print_section(theme, "Personal OAuth readiness");
        for finding in &report.findings {
            print_finding_indented(theme, finding);
        }
        println!();
    }
    Ok(exit_code(&report))
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

    fn finding(severity: crate::dispatch::doctor::Severity) -> crate::dispatch::doctor::Finding {
        crate::dispatch::doctor::Finding {
            service: "test".into(),
            check: "test:check".into(),
            severity,
            message: "test".into(),
        }
    }

    #[test]
    fn plain_doctor_treats_recommendations_as_operational() {
        let findings = vec![
            finding(crate::dispatch::doctor::Severity::Ok),
            finding(crate::dispatch::doctor::Severity::Warn),
        ];
        assert_eq!(
            super::operational_exit_code(&findings),
            std::process::ExitCode::SUCCESS
        );
        let readiness = crate::dispatch::doctor::personal_readiness_finding(&findings, false);
        assert!(matches!(
            readiness.severity,
            crate::dispatch::doctor::Severity::Ok
        ));
        assert!(
            readiness
                .message
                .contains("operational for local/bearer workflows")
        );
        assert!(
            readiness
                .message
                .contains("Browser + ChatGPT public OAuth is optional")
        );
    }

    #[test]
    fn plain_doctor_blocks_only_on_failures_and_reports_oauth_readiness() {
        let blocked = vec![finding(crate::dispatch::doctor::Severity::Fail)];
        assert_eq!(
            super::operational_exit_code(&blocked),
            std::process::ExitCode::from(2)
        );
        let blocked_readiness = crate::dispatch::doctor::personal_readiness_finding(&blocked, true);
        assert!(matches!(
            blocked_readiness.severity,
            crate::dispatch::doctor::Severity::Fail
        ));
        assert!(blocked_readiness.message.contains("not operational yet"));

        let ready = vec![finding(crate::dispatch::doctor::Severity::Ok)];
        let oauth = crate::dispatch::doctor::personal_readiness_finding(&ready, true);
        assert!(
            oauth
                .message
                .contains("configured Browser + ChatGPT OAuth workflow")
        );
    }

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
        let config = labby_auth::config::AuthConfig::default();
        let finding = crate::dispatch::doctor::provider::live_probe(Some(&config)).await;
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
