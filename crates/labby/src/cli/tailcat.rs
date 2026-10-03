//! Local control adapters. Raw credentials and delivery packets never print.
use crate::dispatch::tailcat::exchange::{BrowserRequest, Exchange};
use crate::{
    config::LabConfig,
    dispatch::tailcat::client::{Action, LocalClient, credential_wire},
    output::{OutputFormat, print},
};
use anyhow::{Context as _, Result};
use clap::{Args, Subcommand};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use std::{io::Read as _, path::PathBuf, process::ExitCode};

#[derive(Debug, Args)]
pub struct TailcatArgs {
    /// Explicit same-host control socket; no remote HTTP fallback.
    #[arg(long, global = true, conflicts_with_all = ["server", "context"])]
    pub socket: Option<PathBuf>,
    #[command(subcommand)]
    pub command: TailcatCommand,
}
#[derive(Debug, Subcommand)]
pub enum TailcatCommand {
    /// Approve a browser key for the selected sandbox upstream.
    Pair(PairArgs),
    /// Show local native session states without credentials or addresses.
    Status,
    /// Stop one local session and retire its credential.
    Stop { id: String },
}
#[derive(Debug, Args)]
pub struct PairArgs {
    /// Browser-exported request JSON containing its public key and HTTPS origin.
    #[arg(
        long,
        value_name = "PATH",
        required_unless_present = "rendezvous",
        conflicts_with = "rendezvous"
    )]
    pub request: Option<PathBuf>,
    /// HTTPS dashboard origin; the exchange code is requested privately.
    #[arg(long, requires = "pairing_id")]
    pub rendezvous: Option<String>,
    /// Public dashboard pairing identifier.
    #[arg(long, requires = "rendezvous")]
    pub pairing_id: Option<String>,
    /// Read the private exchange code from stdin for noninteractive approval.
    #[arg(long, requires = "rendezvous")]
    pub pair_code_stdin: bool,
    /// Private project credential file, owned by the current user.
    #[arg(long, value_name = "PATH")]
    pub credential_file: PathBuf,
    /// New private delivery file to import into the requesting browser.
    #[arg(
        long,
        value_name = "PATH",
        required_unless_present = "rendezvous",
        conflicts_with = "rendezvous"
    )]
    pub output: Option<PathBuf>,
    /// Explicitly approve the reviewed origin, peer key and sandbox upstream.
    #[arg(short = 'y', long)]
    pub yes: bool,
}
#[derive(Debug)]
pub struct Operation {
    pub socket: Option<PathBuf>,
    pub command: TailcatCommand,
}
impl TailcatArgs {
    pub fn operation(self) -> super::Command {
        super::Command::TailcatOperation(Operation {
            socket: self.socket,
            command: self.command,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    id: String,
    nonce: String,
    origin: String,
    peer: String,
    upstream: String,
}

pub async fn run(
    operation: Operation,
    config: &LabConfig,
    format: OutputFormat,
) -> Result<ExitCode> {
    if let Some(target) = &config.cli_target {
        if target.source == crate::config::cli::TargetSource::Argument || operation.socket.is_none()
        {
            anyhow::bail!(
                "Tailcat uses same-host authority; select an explicit --socket instead of a remote context"
            );
        }
    }
    let client = LocalClient::new(
        operation
            .socket
            .unwrap_or_else(|| config.tailcat.socket_path()),
    )?;
    match operation.command {
        TailcatCommand::Status => print(&client.call(Action::Status, None).await?, format)?,
        TailcatCommand::Stop { id } => {
            let result = client
                .call(Action::Stop, Some(serde_json::json!({"id":id})))
                .await?;
            print(&result, format)?;
        }
        TailcatCommand::Pair(args) => pair(args, &client, format).await?,
    }
    Ok(ExitCode::SUCCESS)
}
async fn pair(args: PairArgs, client: &LocalClient, format: OutputFormat) -> Result<()> {
    let mut exchange = None;
    let mut public_fingerprint = None;
    let request = if let Some(origin) = &args.rendezvous {
        let code = if args.pair_code_stdin {
            let mut bytes = Vec::new();
            std::io::stdin().take(128).read_to_end(&mut bytes)?;
            String::from_utf8(bytes)
                .map_err(|_| anyhow::anyhow!("invalid exchange code"))?
                .trim()
                .to_owned()
        } else {
            if !super::helpers::interactive_allowed() {
                anyhow::bail!("enter the exchange code interactively or use --pair-code-stdin");
            }
            dialoguer::Password::new()
                .with_prompt("Dashboard pairing code")
                .interact()?
        };
        let id = args
            .pairing_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("pairing identifier required"))?;
        let rendezvous = Exchange::new(origin, id, &code).await?;
        let request = rendezvous.fetch().await?;
        public_fingerprint = Some(crate::dispatch::tailcat::exchange::fingerprint(
            id, &request,
        )?);
        exchange = Some(rendezvous);
        request
    } else {
        let mut bytes = Vec::new();
        std::fs::File::open(
            args.request
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("request file required"))?,
        )?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 {
            anyhow::bail!("Tailcat request file exceeds 16 KiB")
        }
        let request: BrowserRequest = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid Tailcat request file"))?;
        request
    };
    if request.version != 1 {
        anyhow::bail!("unsupported Tailcat request version")
    }
    let source = credential_wire(&args.credential_file)?;
    let prepared: Prepared = serde_json::from_value(client.call(Action::Prepare, Some(serde_json::json!({
        "origin": request.origin, "peer":request.peer, "upstream":request.upstream, "source_credential":source,
    }))).await?).map_err(|_| anyhow::anyhow!("invalid native pairing response"))?;
    if prepared.origin != request.origin
        || prepared.peer != request.peer
        || prepared.upstream != request.upstream
    {
        anyhow::bail!("native pairing response does not match the requested browser");
    }
    let fingerprint =
        public_fingerprint.unwrap_or(hex::encode(Sha256::digest(serde_json::to_vec(&(
            "labby.tailcat.local-approval/v1",
            &prepared.peer,
            &prepared.nonce,
            &prepared.origin,
            &prepared.upstream,
        ))?)));
    if !args.yes {
        if !super::helpers::interactive_allowed() {
            anyhow::bail!("local approval required; review the request and pass --yes")
        }
        print(
            &serde_json::json!({"origin":prepared.origin,"upstream":prepared.upstream,"fingerprint":fingerprint}),
            format,
        )?;
        if !dialoguer::Confirm::new()
            .with_prompt(
                "Allow this browser to use the selected sandbox tools for up to 15 minutes?",
            )
            .default(false)
            .interact()?
        {
            anyhow::bail!("pairing canceled")
        }
    }
    let delivery = client.call(Action::Approve, Some(serde_json::json!({
        "id":prepared.id,"nonce":prepared.nonce,"origin":prepared.origin,"peer":prepared.peer,"upstream":prepared.upstream,
        "exchange_id":args.pairing_id,
    }))).await?;
    let id = delivery
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("pairing may have started; inspect tailcat status"))?
        .to_owned();
    let mut publication =
        crate::dispatch::tailcat::client::PublicationGuard::new(client.clone(), id.clone());
    if let Some(exchange) = exchange {
        let deposited = match delivery.get("packet") {
            Some(packet) => exchange.deposit(packet).await,
            None => Err(anyhow::anyhow!(
                "native controller returned no sealed delivery"
            )),
        };
        if let Err(error) = deposited {
            // Keep cancellation ownership until this cleanup attempt returns.
            let cleanup = client
                .call(Action::Stop, Some(serde_json::json!({"id":id})))
                .await;
            // A completed uncertain response is reported, not automatically retried.
            publication.disarm();
            if cleanup.is_err() {
                anyhow::bail!(
                    "delivery failed and session cleanup is unconfirmed; inspect tailcat status"
                );
            }
            return Err(error).context("delivery failed; native session stopped");
        }
        publication.disarm();
        return print(
            &serde_json::json!({"id":id,"state":"paired","origin":request.origin,"upstream":request.upstream,"fingerprint":fingerprint}),
            format,
        );
    }
    let output = args
        .output
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("private output file required"))?;
    let bytes = serde_json::to_vec(&delivery)?;
    if let Err(error) = crate::installation::secure_file::publish_private_artifact(output, &bytes) {
        let cleanup = client
            .call(Action::Stop, Some(serde_json::json!({"id":id})))
            .await;
        publication.disarm();
        if cleanup.is_err() {
            anyhow::bail!(
                "delivery publication failed and session cleanup is unconfirmed; inspect tailcat status"
            )
        }
        return Err(error).context("delivery publication failed; native session stopped");
    }
    publication.disarm();
    print(
        &serde_json::json!({"id":id,"state":"ready","delivery_file":args.output,
        "origin":request.origin,"upstream":request.upstream,"fingerprint":fingerprint}),
        format,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[tokio::test]
    async fn cancellation_during_publication_cleanup_keeps_stop_ownership() {
        use tokio::io::AsyncReadExt as _;
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let client = LocalClient::new(socket).unwrap();
        let operation = tokio::spawn(async move {
            let mut guard = crate::dispatch::tailcat::client::PublicationGuard::new(
                client.clone(),
                "cancel-test".into(),
            );
            let _result = client
                .call(Action::Stop, Some(serde_json::json!({"id":"cancel-test"})))
                .await;
            guard.disarm();
        });
        // Keep the first Stop unresolved, modeling cancellation during failed-delivery cleanup.
        let (mut first, _) =
            tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut bytes = [0; 4096];
        assert!(first.read(&mut bytes).await.unwrap() > 0);
        operation.abort();
        let _aborted = operation.await;
        let (mut fallback, _) =
            tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut request = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !String::from_utf8_lossy(&request).contains("cancel-test") {
                let count = fallback.read(&mut bytes).await.unwrap();
                assert!(count > 0 && request.len() + count <= 4096);
                request.extend_from_slice(&bytes[..count]);
            }
        })
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&request).contains("POST /"));
    }

    #[test]
    fn tailcat_commands_lower_and_refuse_remote_authority() {
        let parsed = crate::cli::Cli::try_parse_from([
            "labby",
            "tailcat",
            "--socket",
            "/private/tmp/control.sock",
            "status",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command.into_operation(),
            crate::cli::Command::TailcatOperation(Operation {
                command: TailcatCommand::Status,
                ..
            })
        ));
        assert!(
            crate::cli::Cli::try_parse_from([
                "labby",
                "tailcat",
                "pair",
                "--request",
                "request.json",
                "--credential-file",
                "credential",
                "--output",
                "delivery.json",
                "--yes",
            ])
            .is_ok()
        );
        assert!(
            crate::cli::Cli::try_parse_from(
                ["labby", "tailcat", "pair", "--credential", "secret",]
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn explicit_remote_target_is_denied_before_socket_exchange() {
        let mut config = LabConfig::default();
        config.cli_target = Some(crate::config::cli::SelectedTarget {
            server: "https://lab.example".into(),
            team_id: None,
            source: crate::config::cli::TargetSource::Argument,
        });
        let result = run(
            Operation {
                socket: Some("/private/tmp/missing.sock".into()),
                command: TailcatCommand::Status,
            },
            &config,
            crate::cli::Cli::try_parse_from(["labby", "tailcat", "status"])
                .unwrap()
                .format(),
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("same-host authority")
        );
    }

    #[test]
    fn request_file_accepts_public_material_only() {
        let request = serde_json::json!({"version":1,"origin":"https://depot.example",
            "peer":"nodekey:public", "upstream":"microsandbox"});
        assert!(serde_json::from_value::<BrowserRequest>(request.clone()).is_ok());
        let mut private = request;
        private["privateKey"] = serde_json::json!("must-not-cross-native-boundary");
        assert!(serde_json::from_value::<BrowserRequest>(private).is_err());
    }
}
