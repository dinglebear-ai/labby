//! CLI adapter for explicit remote operator sign-in.
use crate::output::{OutputFormat, print};
use anyhow::{Context, Result};
use clap::Args;
use labby_runtime::gateway_config::UpstreamOauthRegistration;
use std::process::ExitCode;

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Filled from the invocation target selector before adapting to OAuth.
    #[arg(skip)]
    pub server: Option<String>,
    /// Public HTTPS client metadata document for servers that require CIMD.
    #[arg(long, conflicts_with_all = ["client_id", "client_secret_env", "dynamic_registration"])]
    pub client_metadata_url: Option<String>,
    /// Client identifier registered with the server in advance.
    #[arg(long, conflicts_with = "dynamic_registration")]
    pub client_id: Option<String>,
    /// Environment variable containing the preregistered client secret.
    #[arg(long, requires = "client_id")]
    pub client_secret_env: Option<String>,
    /// Use server-advertised dynamic registration instead of a saved selection.
    #[arg(long)]
    pub dynamic_registration: bool,
}

pub async fn run(args: LoginArgs, format: OutputFormat) -> Result<ExitCode> {
    let raw = args
        .server
        .or_else(|| std::env::var("LABBY_SERVER_URL").ok())
        .context("set LABBY_SERVER_URL or pass --server")?;
    let server = crate::oauth::cli_session::server_url(&raw)?;
    let registration = if let Some(url) = args.client_metadata_url {
        Some(UpstreamOauthRegistration::ClientMetadataDocument { url })
    } else if let Some(client_id) = args.client_id {
        Some(UpstreamOauthRegistration::Preregistered {
            client_id,
            client_secret_env: args.client_secret_env,
        })
    } else if args.dynamic_registration {
        Some(UpstreamOauthRegistration::Dynamic)
    } else {
        None
    };
    crate::oauth::cli_session::login(&server, registration).await?;
    print(
        &serde_json::json!({"authenticated": true, "server": server.as_str()}),
        format,
    )?;
    Ok(ExitCode::SUCCESS)
}
