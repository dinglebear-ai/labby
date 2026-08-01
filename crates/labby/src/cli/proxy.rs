//! Stdio MCP proxy command.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use crate::config::LabConfig;
use crate::output::{OutputFormat, print};

/// Exit code for the proxy command.
pub type ExitCode = std::process::ExitCode;

/// Proxy a stdio MCP server to Streamable HTTP.
#[derive(Debug, Args)]
pub struct ProxyArgs {
    /// Override the external port for this invocation.
    #[arg(long)]
    pub port: Option<u16>,

    /// Override the configured auth policy.
    #[arg(long, value_enum)]
    pub auth: Option<crate::proxy::config::ProxyAuthMode>,

    /// One-run static bearer token; implies bearer auth.
    #[arg(long, env = "LABBY_PROXY_BEARER_TOKEN", hide_env_values = true)]
    pub bearer_token: Option<String>,

    /// Read a one-run static bearer token from stdin; implies bearer auth.
    #[arg(long, conflicts_with = "bearer_token")]
    pub bearer_token_stdin: bool,

    /// Override exposure to a local loopback URL.
    #[arg(long)]
    pub local: bool,

    /// Child working directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Explicit child environment entry; repeatable.
    #[arg(long = "env", value_name = "NAME=VALUE")]
    pub env: Vec<String>,

    /// Inherit one ambient environment variable; repeatable.
    #[arg(long = "inherit-env", value_name = "NAME")]
    pub inherit_env: Vec<String>,

    /// Child program or script followed by its arguments.
    #[arg(required = true, trailing_var_arg = true)]
    pub command: Vec<OsString>,
}

impl ProxyArgs {
    /// Resolve proxy preferences from CLI overrides and persisted configuration.
    pub fn resolve_preferences(
        &self,
        config: &LabConfig,
    ) -> crate::proxy::config::ProxyPreferences {
        let mut prefs = config.proxy.clone();

        if self.local {
            prefs.exposure = crate::proxy::config::ProxyExposure::Local;
        }
        if let Some(auth) = self.auth {
            prefs.auth = auth;
        }
        if self.bearer_token_stdin || self.bearer_token.is_some() {
            prefs.auth = crate::proxy::config::ProxyAuthMode::Bearer;
        }
        if let Some(port) = self.port {
            prefs.port = crate::proxy::config::ProxyPortPreference::Fixed(port);
        }

        prefs
    }

    /// Read a bearer token from the CLI/env value or stdin.
    pub async fn read_bearer_token(&self) -> Result<Option<String>> {
        if self.bearer_token_stdin {
            let mut token = String::new();
            let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
            tokio::io::AsyncBufReadExt::read_line(&mut stdin, &mut token).await?;
            Ok(Some(token.trim().to_string()))
        } else {
            Ok(self.bearer_token.clone())
        }
    }
}

#[cfg(feature = "gateway")]
#[derive(serde::Serialize)]
struct ProxyReadyOutput {
    url: String,
    exposure: &'static str,
    auth: &'static str,
    external_port: u16,
    local_addr: String,
    command: Vec<String>,
    child_pid: Option<u32>,
    protocol_version: String,
}

#[cfg(feature = "gateway")]
fn parse_explicit_env(values: &[String]) -> Result<Vec<(OsString, OsString)>> {
    values
        .iter()
        .map(|entry| {
            let (name, value) = entry
                .split_once('=')
                .filter(|(name, _)| !name.is_empty())
                .ok_or_else(|| anyhow::anyhow!("--env requires NAME=VALUE, got `{entry}`"))?;
            Ok((OsString::from(name), OsString::from(value)))
        })
        .collect()
}

#[cfg(feature = "gateway")]
fn tailscale_options(
    local_addr: std::net::SocketAddr,
    prefs: &crate::proxy::config::ProxyPreferences,
) -> crate::proxy::tailscale::TailscaleServeOptions {
    let mut options = crate::proxy::tailscale::TailscaleServeOptions::for_proxy(
        local_addr,
        prefs.path.clone(),
        prefs.port,
        prefs.port_range_start,
        prefs.port_range_end,
    );
    if let Some(executable) = std::env::var_os("LABBY_TAILSCALE_BIN") {
        options.executable = executable.into();
    }
    options
}

#[cfg(feature = "gateway")]
fn local_runtime_preferences(
    preferences: &crate::proxy::config::ProxyPreferences,
) -> Result<crate::proxy::config::ProxyPreferences> {
    use crate::proxy::config::{
        ProxyAuthMode, ProxyExposure, ProxyPortPreference, ProxyPreferences,
    };

    let auth = match (preferences.exposure, preferences.auth) {
        (ProxyExposure::Tailscale, ProxyAuthMode::Tailnet) => ProxyAuthMode::None,
        (_, auth @ (ProxyAuthMode::None | ProxyAuthMode::Bearer | ProxyAuthMode::Oauth)) => auth,
        (ProxyExposure::Local, ProxyAuthMode::Tailnet) => {
            anyhow::bail!("tailnet auth requires Tailscale exposure")
        }
    };
    Ok(ProxyPreferences {
        exposure: ProxyExposure::Local,
        auth,
        port: ProxyPortPreference::default(),
        ..preferences.clone()
    })
}

/// Run the stdio MCP proxy command in the foreground.
#[cfg(feature = "gateway")]
pub async fn run(args: ProxyArgs, config: &LabConfig, format: OutputFormat) -> Result<ExitCode> {
    let cwd = args.cwd.clone().unwrap_or(std::env::current_dir()?);
    let command = crate::proxy::command::resolve_proxy_command(
        &args.command,
        &cwd,
        std::env::var_os("PATH").as_deref(),
    )
    .map_err(|error| anyhow::anyhow!("proxy command resolution failed: {error}"))?;

    let prefs = args.resolve_preferences(config);
    prefs
        .validate()
        .map_err(|error| anyhow::anyhow!("proxy preferences validation failed: {error}"))?;

    let local_preferences = local_runtime_preferences(&prefs)?;
    let bearer_token = if matches!(prefs.auth, crate::proxy::config::ProxyAuthMode::Bearer) {
        Some(
            args.read_bearer_token()
                .await?
                .or_else(|| std::env::var(&prefs.bearer_token_env).ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "bearer auth requires {}, --bearer-token, or --bearer-token-stdin",
                        prefs.bearer_token_env
                    )
                })?,
        )
    } else {
        None
    };

    let mut inherit_env = prefs
        .inherit_env
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    inherit_env.extend(args.inherit_env.iter().map(OsString::from));
    let command_json = std::iter::once(command.program.to_string_lossy().into_owned())
        .chain(
            command
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>();

    tracing::info!(
        surface = "cli",
        service = "proxy",
        action = "proxy.start",
        command = %command.display,
        exposure = ?prefs.exposure,
        auth = ?prefs.auth,
        path = %prefs.path,
        "starting stdio MCP proxy"
    );

    let local_options = crate::proxy::runtime::LocalProxyOptions {
        command,
        preferences: local_preferences,
        bearer_token: bearer_token.clone(),
        explicit_env: parse_explicit_env(&args.env)?,
        inherit_env,
    };
    let (mut proxy, mut oauth_lease, oauth_tailscale) = if prefs.auth
        == crate::proxy::config::ProxyAuthMode::Oauth
    {
        let mut prepared = crate::proxy::runtime::LocalProxy::prepare(local_options)
            .await
            .map_err(|error| anyhow::anyhow!("proxy preparation failed: {error}"))?;
        let oauth = crate::proxy::oauth::ProxyOauthContext::prepare(config).await?;
        if prefs.exposure == crate::proxy::config::ProxyExposure::Local {
            anyhow::bail!(
                "local OAuth exposure is not enabled because the daemon lease API accepts HTTPS resources only; use Tailscale exposure"
            );
        }
        let owner = crate::proxy::oauth::owner_fingerprint();
        let mut abandoned_ports = std::collections::BTreeSet::new();
        let max_attempts = 32_usize;
        let mut attempt = 0_usize;
        loop {
            attempt += 1;
            let plan = if prefs.exposure == crate::proxy::config::ProxyExposure::Tailscale {
                let mut options = tailscale_options(prepared.local_addr(), &prefs);
                options.max_attempts = max_attempts;
                let plan = crate::proxy::tailscale::TailscaleServePlan::prepare(options).await?;
                if abandoned_ports.contains(&plan.external_port()) && attempt < max_attempts {
                    continue;
                }
                Some(plan)
            } else {
                None
            };
            let resource = plan.as_ref().map_or_else(
                || prepared.local_url().clone(),
                |plan| plan.public_url().clone(),
            );
            let mut lease = crate::proxy::oauth::OAuthLeaseGuard::create(
                oauth.gateway.clone(),
                resource.as_str(),
                prefs.oauth_scopes.clone(),
                &owner,
                crate::proxy::oauth::OAuthLeaseTiming::proxy_default(),
            )
            .await?;
            let started = prepared.start(crate::proxy::runtime::LocalProxyAuthPolicy::Oauth {
                auth_state: std::sync::Arc::clone(&oauth.auth_state),
                resource: resource.clone(),
                issuer: oauth.issuer.clone(),
                required_scopes: prefs.oauth_scopes.clone(),
            });
            let mut proxy = match started {
                Ok(proxy) => proxy,
                Err(error) => {
                    let release = lease.release().await;
                    return Err(combine_cleanup_errors(
                        error.context("proxy OAuth router startup failed"),
                        [("OAuth lease", release.err())],
                    ));
                }
            };
            let Some(plan) = plan else {
                unreachable!("local OAuth exposure is rejected before lease creation")
            };
            match plan.claim_typed().await {
                Ok(serve) => {
                    if let Err(error) = crate::proxy::oauth::verify_protected_resource_metadata(
                        proxy.url(),
                        &resource,
                    )
                    .await
                    {
                        let http_cleanup = proxy.stop_http().await;
                        let serve_cleanup = serve.shutdown().await;
                        proxy.stop_child().await;
                        let lease_cleanup = lease.release().await;
                        return Err(combine_cleanup_errors(
                            error,
                            [
                                ("LocalProxy HTTP", http_cleanup.err()),
                                ("Tailscale Serve", serve_cleanup.err()),
                                ("OAuth lease", lease_cleanup.err()),
                            ],
                        ));
                    }
                    break (proxy, Some(lease), Some(serve));
                }
                Err(crate::proxy::tailscale::TailscaleClaimError::Collision(error))
                    if prefs.port.fixed().is_none() && attempt < max_attempts =>
                {
                    let port = resource.port().unwrap_or_default();
                    abandoned_ports.insert(port);
                    let rollback = proxy.rollback_to_prepared().await;
                    let release = lease.release().await;
                    match rollback {
                        Ok(next) if release.is_ok() => prepared = next,
                        Ok(_) => {
                            return Err(combine_cleanup_errors(
                                error.context("Tailscale collision rollback failed"),
                                [("OAuth lease", release.err())],
                            ));
                        }
                        Err(rollback) => {
                            return Err(combine_cleanup_errors(
                                error.context("Tailscale collision rollback failed"),
                                [
                                    ("LocalProxy rollback", Some(rollback)),
                                    ("OAuth lease", release.err()),
                                ],
                            ));
                        }
                    }
                }
                Err(error) => {
                    let proxy_cleanup = proxy.shutdown().await;
                    let lease_cleanup = lease.release().await;
                    return Err(combine_cleanup_errors(
                        anyhow::Error::new(error),
                        [
                            ("LocalProxy", proxy_cleanup.err()),
                            ("OAuth lease", lease_cleanup.err()),
                        ],
                    ));
                }
            }
        }
    } else {
        (
            crate::proxy::runtime::LocalProxy::start(local_options)
                .await
                .map_err(|error| anyhow::anyhow!("proxy startup failed: {error}"))?,
            None,
            None,
        )
    };
    let info = proxy.info().clone();
    let mut tailscale = if oauth_tailscale.is_some() {
        oauth_tailscale
    } else if prefs.exposure == crate::proxy::config::ProxyExposure::Tailscale {
        let options = tailscale_options(info.local_addr, &prefs);
        match crate::proxy::tailscale::TailscaleServe::start(options).await {
            Ok(serve) => Some(serve),
            Err(error) => {
                let shutdown = proxy.shutdown().await;
                if let Err(shutdown) = shutdown {
                    return Err(error).context(format!(
                        "LocalProxy cleanup also failed after Tailscale startup failure: {shutdown:#}"
                    ));
                }
                return Err(error).context("Tailscale Serve publication failed");
            }
        }
    } else {
        None
    };
    let public_url = tailscale
        .as_ref()
        .map_or_else(|| info.url.clone(), |serve| serve.public_url().clone());
    let external_port = tailscale.as_ref().map_or(
        info.local_addr.port(),
        crate::proxy::tailscale::TailscaleServe::external_port,
    );
    let exposure = if tailscale.is_some() {
        "tailscale"
    } else {
        "local"
    };
    let auth = match prefs.auth {
        crate::proxy::config::ProxyAuthMode::Tailnet => "tailnet",
        crate::proxy::config::ProxyAuthMode::Bearer => "bearer",
        crate::proxy::config::ProxyAuthMode::Oauth => "oauth",
        crate::proxy::config::ProxyAuthMode::None => "none",
    };

    let ready_output = if format.is_json() {
        print(
            &ProxyReadyOutput {
                url: public_url.to_string(),
                exposure,
                auth,
                external_port,
                local_addr: info.local_addr.to_string(),
                command: command_json,
                child_pid: info.child_pid,
                protocol_version: info.protocol_version.to_string(),
            },
            format,
        )
    } else {
        #[allow(clippy::print_stdout)]
        {
            println!("MCP proxy ready");
            println!();
            println!("  Server   {}", info.command);
            println!("  URL      {public_url}");
            println!(
                "  Exposure {}",
                if tailscale.is_some() {
                    "Tailscale Serve"
                } else {
                    "Local"
                }
            );
            println!(
                "  Auth     {}",
                match prefs.auth {
                    crate::proxy::config::ProxyAuthMode::Tailnet => "Tailnet",
                    crate::proxy::config::ProxyAuthMode::Bearer => "Bearer token",
                    crate::proxy::config::ProxyAuthMode::Oauth => "OAuth",
                    crate::proxy::config::ProxyAuthMode::None => "None",
                }
            );
            println!();
            println!("Press Ctrl+C to stop.");
        }
        Ok(())
    };
    if let Err(output_error) = ready_output {
        let http_shutdown = proxy.stop_http().await;
        let tailscale_shutdown = match tailscale.take() {
            Some(serve) => serve.shutdown().await,
            None => Ok(()),
        };
        proxy.stop_child().await;
        let lease_shutdown = match oauth_lease.as_mut() {
            Some(lease) => lease.release().await,
            None => Ok(()),
        };
        return Err(combine_cleanup_errors(
            output_error,
            [
                ("LocalProxy HTTP", http_shutdown.err()),
                ("Tailscale Serve", tailscale_shutdown.err()),
                ("OAuth lease", lease_shutdown.err()),
            ],
        ));
    }

    let failure = if let (Some(serve), Some(lease)) = (tailscale.as_mut(), oauth_lease.as_mut()) {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => Some(signal.map_err(anyhow::Error::from)),
            result = proxy.wait_for_failure() => Some(result),
            result = serve.wait_for_failure() => Some(result),
            result = lease.wait_for_failure() => Some(result),
        }
    } else if let Some(lease) = oauth_lease.as_mut() {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => Some(signal.map_err(anyhow::Error::from)),
            result = proxy.wait_for_failure() => Some(result),
            result = lease.wait_for_failure() => Some(result),
        }
    } else if let Some(serve) = tailscale.as_mut() {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => Some(signal.map_err(anyhow::Error::from)),
            result = proxy.wait_for_failure() => Some(result),
            result = serve.wait_for_failure() => Some(result),
        }
    } else {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => Some(signal.map_err(anyhow::Error::from)),
            result = proxy.wait_for_failure() => Some(result),
        }
    };
    let http_shutdown = proxy.stop_http().await;
    let tailscale_shutdown = match tailscale {
        Some(serve) => serve.shutdown().await,
        None => Ok(()),
    };
    proxy.stop_child().await;
    let lease_shutdown = match oauth_lease.as_mut() {
        Some(lease) => lease.release().await,
        None => Ok(()),
    };
    let primary = failure.and_then(Result::err);
    if let Some(primary) = primary {
        return Err(combine_cleanup_errors(
            primary,
            [
                ("Tailscale Serve", tailscale_shutdown.err()),
                ("LocalProxy HTTP", http_shutdown.err()),
                ("OAuth lease", lease_shutdown.err()),
            ],
        ));
    }
    if tailscale_shutdown.is_err() || http_shutdown.is_err() || lease_shutdown.is_err() {
        return Err(combine_cleanup_errors(
            anyhow::anyhow!("proxy shutdown failed"),
            [
                ("Tailscale Serve", tailscale_shutdown.err()),
                ("LocalProxy HTTP", http_shutdown.err()),
                ("OAuth lease", lease_shutdown.err()),
            ],
        ));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "gateway")]
fn combine_cleanup_errors<const N: usize>(
    primary: anyhow::Error,
    cleanups: [(&str, Option<anyhow::Error>); N],
) -> anyhow::Error {
    let failures = cleanups
        .into_iter()
        .filter_map(|(name, error)| error.map(|error| format!("{name}: {error:#}")))
        .collect::<Vec<_>>();
    if failures.is_empty() {
        primary
    } else {
        primary.context(format!("cleanup failures: {}", failures.join("; ")))
    }
}

#[cfg(not(feature = "gateway"))]
pub async fn run(_args: ProxyArgs, _config: &LabConfig, _format: OutputFormat) -> Result<ExitCode> {
    anyhow::bail!("stdio MCP proxy runtime requires the `gateway` feature")
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        proxy: ProxyArgs,
    }

    fn parse<const N: usize>(args: [&str; N]) -> ProxyArgs {
        TestCli::parse_from(args).proxy
    }

    #[test]
    fn proxy_parses_js_file_with_child_args() {
        let args = parse([
            "proxy",
            "/path/to/dist.js",
            "--workspace",
            "/srv/data",
            "--read-only",
        ]);
        assert_eq!(args.command.len(), 4);
        assert_eq!(args.command[0], "/path/to/dist.js");
        assert_eq!(args.command[1], "--workspace");
        assert_eq!(args.command[2], "/srv/data");
        assert_eq!(args.command[3], "--read-only");
    }

    #[test]
    fn proxy_requires_command() {
        let error = TestCli::try_parse_from(["proxy"]).expect_err("proxy should require a command");
        assert!(error.to_string().contains("required"));
    }

    #[test]
    fn proxy_accepts_explicit_separator() {
        let args = parse([
            "proxy",
            "--",
            "npx",
            "-y",
            "@modelcontextprotocol/server-filesystem",
            "/srv/data",
        ]);
        assert_eq!(args.command[0], "npx");
    }

    #[test]
    fn configured_auth_is_preserved_without_override() {
        let args = parse(["proxy", "/path/to/dist.js"]);
        let mut config = LabConfig::default();
        config.proxy.auth = crate::proxy::config::ProxyAuthMode::Oauth;
        assert_eq!(
            args.resolve_preferences(&config).auth,
            crate::proxy::config::ProxyAuthMode::Oauth
        );
    }

    #[test]
    fn proxy_bearer_token_implies_bearer_auth() {
        let args = parse(["proxy", "--bearer-token", "secret", "/path/to/dist.js"]);
        assert_eq!(
            args.resolve_preferences(&LabConfig::default()).auth,
            crate::proxy::config::ProxyAuthMode::Bearer
        );
        assert_eq!(args.bearer_token, Some("secret".to_string()));
    }

    #[test]
    fn proxy_auth_override_wins() {
        let args = parse(["proxy", "--auth", "oauth", "/path/to/dist.js"]);
        assert_eq!(
            args.resolve_preferences(&LabConfig::default()).auth,
            crate::proxy::config::ProxyAuthMode::Oauth
        );
    }

    #[test]
    fn proxy_local_implies_local_exposure() {
        let args = parse(["proxy", "--local", "/path/to/dist.js"]);
        assert_eq!(
            args.resolve_preferences(&LabConfig::default()).exposure,
            crate::proxy::config::ProxyExposure::Local
        );
    }

    #[test]
    fn proxy_env_flags_are_repeatable() {
        let args = parse([
            "proxy",
            "--env",
            "FOO=bar",
            "--env",
            "BAZ=qux",
            "/path/to/dist.js",
        ]);
        assert_eq!(args.env, vec!["FOO=bar", "BAZ=qux"]);
    }

    #[test]
    fn proxy_inherit_env_is_repeatable() {
        let args = parse([
            "proxy",
            "--inherit-env",
            "PATH",
            "--inherit-env",
            "HOME",
            "/path/to/dist.js",
        ]);
        assert_eq!(args.inherit_env, vec!["PATH", "HOME"]);
    }

    #[test]
    fn tailscale_tailnet_uses_ephemeral_loopback_without_application_auth() {
        let prefs = crate::proxy::config::ProxyPreferences {
            port: crate::proxy::config::ProxyPortPreference::Fixed(52_177),
            ..Default::default()
        };
        let local = local_runtime_preferences(&prefs).unwrap();
        assert_eq!(local.exposure, crate::proxy::config::ProxyExposure::Local);
        assert_eq!(local.auth, crate::proxy::config::ProxyAuthMode::None);
        assert_eq!(local.port.fixed(), None);
    }

    #[test]
    fn tailscale_bearer_stays_bearer_on_loopback() {
        let prefs = crate::proxy::config::ProxyPreferences {
            auth: crate::proxy::config::ProxyAuthMode::Bearer,
            ..Default::default()
        };
        assert_eq!(
            local_runtime_preferences(&prefs).unwrap().auth,
            crate::proxy::config::ProxyAuthMode::Bearer
        );
    }

    #[test]
    fn oauth_is_preserved_for_the_finalized_local_router_policy() {
        let prefs = crate::proxy::config::ProxyPreferences {
            auth: crate::proxy::config::ProxyAuthMode::Oauth,
            ..Default::default()
        };
        assert_eq!(
            local_runtime_preferences(&prefs).unwrap().auth,
            crate::proxy::config::ProxyAuthMode::Oauth
        );
    }
}
