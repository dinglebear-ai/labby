//! Stdio MCP proxy command.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::config::LabConfig;
use crate::output::OutputFormat;

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

/// Prepare the proxy invocation. Runtime wiring lands in the next checkpoint.
pub async fn run(args: ProxyArgs, config: &LabConfig, _format: OutputFormat) -> Result<ExitCode> {
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

    let _bearer_token = if matches!(prefs.auth, crate::proxy::config::ProxyAuthMode::Bearer) {
        Some(args.read_bearer_token().await?.ok_or_else(|| {
            anyhow::anyhow!(
                "bearer auth requires LABBY_PROXY_BEARER_TOKEN, --bearer-token, or --bearer-token-stdin"
            )
        })?)
    } else {
        None
    };

    tracing::info!(
        surface = "cli",
        service = "proxy",
        action = "proxy.prepare",
        command = %command.display,
        exposure = ?prefs.exposure,
        auth = ?prefs.auth,
        path = %prefs.path,
        "prepared stdio MCP proxy invocation"
    );

    anyhow::bail!("stdio MCP proxy runtime is not implemented in this checkpoint")
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
}
