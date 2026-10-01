//! Credential-bound local stdio adapter. Credentials are read from one protected
//! installation root at process start, never included in client configuration.
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeDescriptor {
    pub command: String,
    pub args: Vec<String>,
}

pub fn descriptor(root: &Path, gateway: &str, binary: &Path) -> Result<BridgeDescriptor> {
    let paths = crate::installation::InstallationPaths::from_root(root.to_path_buf())?;
    let normalized = crate::config::cli::normalize_server(gateway)?;
    if !binary.is_absolute() || !binary.is_file() {
        bail!("The Labby bridge needs an installed absolute executable path");
    }
    Ok(BridgeDescriptor {
        command: binary.to_string_lossy().into_owned(),
        args: vec![
            "setup".into(),
            "clients".into(),
            "bridge".into(),
            "--gateway-url".into(),
            normalized,
            "--state-root".into(),
            paths.root().to_string_lossy().into_owned(),
        ],
    })
}

pub fn descriptor_for_client(
    root: &Path,
    gateway: &str,
    binary: &Path,
    client: super::client_registration::ExternalClient,
) -> Result<BridgeDescriptor> {
    let mut descriptor = descriptor(root, gateway, binary)?;
    descriptor
        .args
        .extend(["--client".into(), client.key().into()]);
    Ok(descriptor)
}
#[derive(Serialize, Deserialize)]
struct SavedSessions {
    gateway: String,
    issued: super::client_evidence::Issued,
}
pub async fn prepare_sessions(
    root: &Path,
    gateway: &str,
    clients: &[super::client_registration::ExternalClient],
) -> Result<()> {
    #[cfg(feature = "gateway")]
    {
        let live = connect(root, gateway).await?;
        let response = live
            .dispatch_client_session(
                "clients.session.start",
                serde_json::json!({"clients":clients}),
            )
            .await?;
        let issued: super::client_evidence::Issued = serde_json::from_value(response)
            .context("Invalid client observation session response")?;
        if clients
            .iter()
            .any(|client| !issued.sessions.contains_key(client.key()))
        {
            bail!("Client observation response is missing a selected application");
        }
        let path = root.join("client-observation.json");
        labby_runtime::path_safety::reject_existing_symlink_ancestors(root, &path)?;
        labby_runtime::secure_atomic_file::write_secure_atomic(
            &path,
            &serde_json::to_vec(&SavedSessions {
                gateway: crate::config::cli::normalize_server(gateway)?,
                issued,
            })?,
        )?;
        Ok(())
    }
    #[cfg(not(feature = "gateway"))]
    {
        let _ = (root, gateway, clients);
        bail!("This build does not include client observation sessions")
    }
}
fn saved_evidence(
    root: &Path,
    gateway: &str,
    client: super::client_registration::ExternalClient,
) -> Result<Option<String>> {
    let path = root.join("client-observation.json");
    labby_runtime::path_safety::reject_existing_symlink_ancestors(root, &path)?;
    let raw = match super::secure_file::read_private(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let row: SavedSessions =
        serde_json::from_slice(&raw).context("Saved client observation state is invalid")?;
    if row.gateway != crate::config::cli::normalize_server(gateway)? {
        bail!("Saved client observation state belongs to another gateway");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    if row.issued.expires <= now {
        return Ok(None);
    }
    Ok(row.issued.sessions.get(client.key()).cloned())
}

/// Read only the explicit installation's protected saved CLI connection. The
/// saved target and requested target must agree before a token can leave disk.
fn saved_connection(root: &Path) -> Result<(String, Option<String>)> {
    let paths = crate::installation::InstallationPaths::from_root(root.to_path_buf())?;
    let root = paths.root();
    let path = root.join(".env");
    labby_runtime::path_safety::reject_existing_symlink_ancestors(root, &path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let metadata =
            std::fs::symlink_metadata(&path).context("Saved CLI credentials are missing")?;
        let parent = std::fs::metadata(root)?;
        let uid = nix::unistd::Uid::effective().as_raw();
        if !metadata.is_file()
            || metadata.uid() != uid
            || metadata.permissions().mode() & 0o077 != 0
            || parent.uid() != uid
            || parent.permissions().mode() & 0o022 != 0
        {
            bail!(
                "Saved CLI credentials must be owned by this user in a protected directory and readable only by that user"
            );
        }
    }
    #[cfg(not(unix))]
    bail!(
        "The protected saved-credential bridge currently supports macOS and Linux; use the OAuth adapter on this platform"
    );
    #[cfg(unix)]
    {
        let raw = crate::config::host_write::read_config_snapshot(&path)?;
        let mut server = None;
        let mut token = None;
        for line in raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let selected = match key.trim() {
                "LABBY_SERVER_URL" => &mut server,
                "LABBY_MCP_HTTP_TOKEN" => &mut token,
                _ => continue,
            };
            if selected.is_some() {
                bail!(
                    "Saved CLI connection contains duplicate assignments; repair it before registering clients"
                );
            }
            *selected = Some(crate::config::env_merge::strip_quotes(value.trim()));
        }
        let server = crate::config::cli::normalize_server(
            &server.context("Saved CLI server URL is missing")?,
        )?;
        if token
            .as_ref()
            .is_some_and(|token| token.contains(['\r', '\n', '\0']))
        {
            bail!("Saved CLI bearer credential is invalid");
        }
        let token = token.filter(|token| !token.trim().is_empty());
        Ok((server, token))
    }
}

/// Read the protected persisted connection identity without exposing its credential.
pub fn saved_connection_identity(root: &Path) -> Result<(String, bool)> {
    let (server, token) = saved_connection(root)?;
    Ok((server, token.is_some()))
}

fn credentials(root: &Path, gateway: &str) -> Result<(String, String)> {
    let (server, token) = saved_connection(root)?;
    if server != crate::config::cli::normalize_server(gateway)? {
        bail!(
            "Saved CLI credentials belong to a different gateway. No fallback or credential reuse was attempted"
        );
    }
    Ok((
        server,
        token.context("Saved CLI bearer credential is missing or invalid")?,
    ))
}

#[cfg(feature = "gateway")]
async fn connect(root: &Path, gateway: &str) -> Result<crate::live_gateway::LiveGateway> {
    let (server, token) = credentials(root, gateway)?;
    Ok(crate::live_gateway::detect_bound_bearer_target(&server, token).await?)
}

#[cfg(feature = "gateway")]
async fn run_io<R, W>(root: PathBuf, gateway: String, read: R, write: W) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    run_io_with_evidence(root, gateway, read, write, None).await
}

#[cfg(feature = "gateway")]
async fn run_io_with_evidence<R, W>(
    root: PathBuf,
    gateway: String,
    read: R,
    write: W,
    evidence: Option<String>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use rmcp::ServiceExt as _;
    let live = connect(&root, &gateway).await?;
    let service = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        live.connect_service_with_evidence(
            crate::mcp::bridge::BridgeClientHandler::new(),
            evidence.as_deref(),
        ),
    )
    .await
    .context("Client bridge initialization timed out")??;
    let handler = crate::mcp::bridge::BridgeServerHandler::new(service);
    let running = handler.serve((read, write)).await?;
    running.waiting().await?;
    Ok(())
}

pub async fn run_stdio(
    root: PathBuf,
    gateway: String,
    client: Option<super::client_registration::ExternalClient>,
) -> Result<()> {
    #[cfg(feature = "gateway")]
    {
        let evidence = match client {
            Some(client) => saved_evidence(&root, &gateway, client)?,
            None => None,
        };
        return run_io_with_evidence(
            root,
            gateway,
            tokio::io::stdin(),
            tokio::io::stdout(),
            evidence,
        )
        .await;
    }
    #[cfg(not(feature = "gateway"))]
    {
        let _ = (root, gateway, client);
        bail!("This Labby build does not include the MCP gateway bridge");
    }
}

/// Verify the same byte transport clients launch: stdio framing, HTTP
/// authentication, initialize and one bounded tools/list round trip.
/// This proves the local bridge, not that an external application has used it.
pub async fn verify(root: PathBuf, gateway: String) -> Result<usize> {
    #[cfg(feature = "gateway")]
    {
        use rmcp::ServiceExt as _;
        let (client_io, bridge_io) = tokio::io::duplex(256 * 1024);
        let (read, write) = tokio::io::split(bridge_io);
        credentials(&root, &gateway)?;
        let mut task = tokio::spawn(run_io(root, gateway, read, write));
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let client = ().serve(client_io).await?;
            let result = client.peer().list_tools(None).await;
            drop(client.cancel().await);
            anyhow::Ok(result?.tools.len())
        })
        .await;
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(2), &mut task).await;
        match stopped {
            Ok(Ok(Err(error))) => return Err(error),
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {
                task.abort();
                drop(task.await);
            }
            Ok(Ok(Ok(()))) => {}
        }
        result.context("Saved client bridge verification timed out")?
    }
    #[cfg(not(feature = "gateway"))]
    {
        let _ = (root, gateway);
        bail!("This Labby build does not include the MCP gateway bridge");
    }
}

#[cfg(test)]
#[path = "client_bridge_tests.rs"]
mod tests;
