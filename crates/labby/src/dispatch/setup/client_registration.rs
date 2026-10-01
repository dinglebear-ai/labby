//! Local, binary-owned MCP client registration with OAuth or a protected saved
//! connection bridge. No gateway credential is copied into client configuration.

use crate::config::host_write::{HostConfigLock, read_config_snapshot};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalClient {
    Codex,
    ClaudeCode,
}

impl ExternalClient {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }
    fn relative_path(self) -> &'static str {
        match self {
            Self::Codex => ".codex/config.toml",
            Self::ClaudeCode => ".claude.json",
        }
    }
    fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RegistrationPlan {
    pub client: ExternalClient,
    pub config_path: PathBuf,
    pub config_version: String,
    pub gateway_url: String,
    pub authentication: &'static str,
    pub status: &'static str,
    pub readiness_supported: bool,
    pub readiness_limitation: Option<&'static str>,
    pub login_instruction: &'static str,
    pub bridge: Option<super::client_bridge::BridgeDescriptor>,
}

#[derive(Debug, Serialize)]
pub struct RegistrationResult {
    pub plan: RegistrationPlan,
    pub changed: bool,
    pub backup_path: Option<PathBuf>,
    pub connected: bool,
    pub bridge_verified: bool,
}

#[derive(Debug, Serialize)]
pub struct ClientDetection {
    pub client: ExternalClient,
    pub executable_found: bool,
    pub configuration_exists: bool,
    pub config_path: PathBuf,
}

pub fn detect(home: &Path) -> Vec<ClientDetection> {
    [ExternalClient::Codex, ExternalClient::ClaudeCode]
        .into_iter()
        .map(|client| {
            let config_path = home.join(client.relative_path());
            let executable_found =
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                    .any(|directory| executable_present(&directory, client.executable()));
            ClientDetection {
                client,
                executable_found,
                configuration_exists: config_path.is_file(),
                config_path,
            }
        })
        .collect()
}

fn executable_present(directory: &Path, name: &str) -> bool {
    #[cfg(windows)]
    let suffixes = [".exe", ".cmd", ".bat", ""];
    #[cfg(not(windows))]
    let suffixes = [""];
    suffixes.into_iter().any(|suffix| {
        let Ok(metadata) = std::fs::metadata(directory.join(format!("{name}{suffix}"))) else {
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
}

pub async fn verified_plan(
    home: PathBuf,
    client: ExternalClient,
    gateway: String,
) -> Result<RegistrationPlan> {
    verify_oauth(&gateway).await?;
    tokio::task::spawn_blocking(move || plan(&home, client, &gateway)).await?
}

pub async fn verified_register(
    home: PathBuf,
    client: ExternalClient,
    gateway: String,
    expected_version: String,
) -> Result<RegistrationResult> {
    verify_oauth(&gateway).await?;
    tokio::task::spawn_blocking(move || register(&home, client, &gateway, &expected_version))
        .await?
}

pub async fn saved_plan(
    home: PathBuf,
    root: PathBuf,
    client: ExternalClient,
    gateway: String,
    binary: PathBuf,
) -> Result<RegistrationPlan> {
    super::client_bridge::verify(root.clone(), gateway.clone()).await?;
    let bridge = super::client_bridge::descriptor_for_client(&root, &gateway, &binary, client)?;
    tokio::task::spawn_blocking(move || plan_connection(&home, client, &gateway, Some(bridge)))
        .await?
}

pub async fn saved_register(
    home: PathBuf,
    root: PathBuf,
    client: ExternalClient,
    gateway: String,
    binary: PathBuf,
    expected_version: String,
) -> Result<RegistrationResult> {
    super::client_bridge::verify(root.clone(), gateway.clone()).await?;
    let bridge = super::client_bridge::descriptor_for_client(&root, &gateway, &binary, client)?;
    tokio::task::spawn_blocking(move || {
        register_connection(&home, client, &gateway, &expected_version, Some(bridge))
    })
    .await?
}

pub async fn register_saved_clients(
    home: PathBuf,
    root: PathBuf,
    gateway: String,
    clients: Vec<ExternalClient>,
    binary: PathBuf,
) -> Vec<serde_json::Value> {
    if let Err(error) = super::client_bridge::prepare_sessions(&root, &gateway, &clients).await {
        return clients.into_iter().map(|client|serde_json::json!({"client":client,"status":"needs_attention","connected":false,"message":error.to_string()})).collect();
    }
    let mut results = Vec::new();
    for client in clients {
        let result = async {
            let proposed = saved_plan(
                home.clone(),
                root.clone(),
                client,
                gateway.clone(),
                binary.clone(),
            )
            .await?;
            saved_register(
                home.clone(),
                root.clone(),
                client,
                gateway.clone(),
                binary.clone(),
                proposed.config_version,
            )
            .await
        }
        .await;
        results.push(match result { Ok(result) => serde_json::json!({"client":client,"result":result}), Err(error) => serde_json::json!({"client":client,"status":"needs_attention","connected":false,"message":error.to_string()}) });
    }
    results
}

pub async fn register_oauth_clients(
    home: PathBuf,
    root: PathBuf,
    gateway: String,
    clients: Vec<ExternalClient>,
) -> Vec<serde_json::Value> {
    let clients = normalize_client_selection(clients);
    #[cfg(feature = "gateway")]
    {
        oauth_observation::connect(home, root, gateway, clients).await
    }
    #[cfg(not(feature = "gateway"))]
    {
        let _ = (home, root, gateway);
        clients.into_iter().map(|client|serde_json::json!({"client":client,"status":"needs_attention","connected":false,"message":"This build does not include authenticated client observation"})).collect()
    }
}

fn normalize_client_selection(clients: Vec<ExternalClient>) -> Vec<ExternalClient> {
    [ExternalClient::Codex, ExternalClient::ClaudeCode]
        .into_iter()
        .filter(|client| {
            clients
                .iter()
                .any(|selected| selected.key() == client.key())
        })
        .collect()
}

#[cfg(feature = "gateway")]
#[path = "oauth_client_registration.rs"]
mod oauth_observation;

fn version(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}

fn target(home: &Path, client: ExternalClient) -> Result<PathBuf> {
    if !home.is_absolute() {
        bail!("Client home must be an absolute directory");
    }
    let home = home.canonicalize().context("Client home does not exist")?;
    let path = home.join(client.relative_path());
    labby_runtime::path_safety::reject_existing_symlink_ancestors(&home, &path)?;
    Ok(path)
}

fn plan(home: &Path, client: ExternalClient, gateway: &str) -> Result<RegistrationPlan> {
    plan_connection(home, client, gateway, None)
}

fn plan_connection(
    home: &Path,
    client: ExternalClient,
    gateway: &str,
    bridge: Option<super::client_bridge::BridgeDescriptor>,
) -> Result<RegistrationPlan> {
    let gateway_url = validated_endpoint(gateway)?.to_string();
    let config_path = target(home, client)?;
    let raw = read_config_snapshot(&config_path)?;
    render_connection(client, &raw, &gateway_url, bridge.as_ref())?;
    Ok(RegistrationPlan {
        client,
        config_path,
        config_version: version(&raw),
        gateway_url,
        authentication: if bridge.is_some() {
            "protected_saved_cli_bridge"
        } else {
            "oauth"
        },
        status: if bridge.is_some() {
            "external_client_use_not_verified"
        } else {
            "oauth_login_required"
        },
        readiness_supported: bridge.is_some(),
        readiness_limitation: if bridge.is_some() {
            None
        } else {
            Some(
                "Direct OAuth registration does not install principal-bound client observation proofs. OAuth login and tool use cannot complete Labby's selected-client readiness check in this mode.",
            )
        },
        login_instruction: if bridge.is_some() {
            "Use labby setup clients connect to issue observation sessions for all selected applications, then restart each client and invoke a Labby tool. Plan/register alone do not issue observation sessions; client use remains unverified."
        } else {
            match client {
                ExternalClient::Codex => {
                    "Run codex mcp login lab, then verify a Labby tool in Codex."
                }
                ExternalClient::ClaudeCode => {
                    "Open Claude Code, run /mcp, authenticate lab, then verify a Labby tool."
                }
            }
        },
        bridge,
    })
}

/// Apply a reviewed snapshot. Existing unrelated entries survive and an existing
/// conflicting `lab` entry requires user reconciliation rather than replacement.
/// The lock serializes Labby writers; the second snapshot check catches client
/// edits observed before publication. Clients do not honor Labby's advisory lock.
fn register(
    home: &Path,
    client: ExternalClient,
    gateway: &str,
    expected_version: &str,
) -> Result<RegistrationResult> {
    register_connection(home, client, gateway, expected_version, None)
}

fn register_connection(
    home: &Path,
    client: ExternalClient,
    gateway: &str,
    expected_version: &str,
    bridge: Option<super::client_bridge::BridgeDescriptor>,
) -> Result<RegistrationResult> {
    let bridge_verified = bridge.is_some();
    let mut plan = plan_connection(home, client, gateway, bridge)?;
    let lock = HostConfigLock::acquire(&plan.config_path)?;
    let raw = lock.read_raw()?;
    if version(&raw) != expected_version {
        bail!("Client configuration changed. Review a new registration plan before retrying.");
    }
    let output = render_connection(client, &raw, &plan.gateway_url, plan.bridge.as_ref())?;
    if output == raw {
        return Ok(RegistrationResult {
            plan,
            changed: false,
            backup_path: None,
            connected: false,
            bridge_verified,
        });
    }
    let backup_path = if raw.is_empty() {
        None
    } else {
        let path = lock.path().with_file_name(format!(
            "{}.labby-backup-{}",
            lock.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        labby_runtime::secure_atomic_file::write_secure_atomic(&path, raw.as_bytes())?;
        Some(path)
    };
    if lock.read_raw()? != raw {
        bail!(
            "Client configuration changed while preparing the backup. No registration was written."
        );
    }
    lock.write(&output)?;
    plan.config_version = version(&output);
    Ok(RegistrationResult {
        plan,
        changed: true,
        backup_path,
        connected: false,
        bridge_verified,
    })
}

#[cfg(test)]
fn render(client: ExternalClient, raw: &str, gateway: &str) -> Result<String> {
    render_connection(client, raw, gateway, None)
}

fn render_connection(
    client: ExternalClient,
    raw: &str,
    gateway: &str,
    bridge: Option<&super::client_bridge::BridgeDescriptor>,
) -> Result<String> {
    let desired = if let Some(bridge) = bridge {
        serde_json::json!({"command":bridge.command,"args":bridge.args})
    } else {
        serde_json::json!({"url":gateway})
    };
    match client {
        ExternalClient::Codex => {
            let mut document: toml_edit::DocumentMut = raw
                .parse()
                .map_err(|_| anyhow::anyhow!("Codex configuration is not valid TOML; existing contents were not displayed or modified"))?;
            if document
                .get("mcp_servers")
                .and_then(|servers| servers.get("lab"))
                .is_some()
            {
                let parsed: toml::Value = toml::from_str(raw)
                    .map_err(|_| anyhow::anyhow!("Codex configuration is not valid TOML"))?;
                let value = &parsed["mcp_servers"]["lab"];
                if serde_json::to_value(value)? == desired {
                    return Ok(raw.into());
                }
                bail!(
                    "Codex already has a different lab MCP entry. Resolve that conflict in Codex before registering."
                );
            }
            if document
                .get("mcp_servers")
                .is_some_and(|item| !item.is_table_like())
            {
                bail!("Codex mcp_servers must be a table");
            }
            if document.get("mcp_servers").is_none() {
                document["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let mut registration = toml_edit::InlineTable::new();
            if let Some(bridge) = bridge {
                registration.insert("command", bridge.command.as_str().into());
                let mut args = toml_edit::Array::new();
                for arg in &bridge.args {
                    args.push(arg.as_str());
                }
                registration.insert("args", toml_edit::Value::Array(args));
            } else {
                registration.insert("url", gateway.into());
            }
            document
                .get_mut("mcp_servers")
                .and_then(toml_edit::Item::as_table_like_mut)
                .context("Codex mcp_servers must be a table")?
                .insert(
                    "lab",
                    toml_edit::Item::Value(toml_edit::Value::InlineTable(registration)),
                );
            Ok(document.to_string())
        }
        ExternalClient::ClaudeCode => {
            let mut document: serde_json::Value = if raw.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(raw).map_err(|_| anyhow::anyhow!("Claude Code configuration is not valid JSON; existing contents were not displayed or modified"))?
            };
            let object = document
                .as_object_mut()
                .context("Claude Code configuration must be an object")?;
            let servers = object
                .entry("mcpServers")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .context("Claude Code mcpServers must be an object")?;
            let desired = if let Some(bridge) = bridge {
                serde_json::json!({"type":"stdio","command":bridge.command,"args":bridge.args})
            } else {
                serde_json::json!({ "type": "http", "url": gateway })
            };
            if let Some(existing) = servers.get("lab") {
                if existing == &desired {
                    return Ok(raw.into());
                }
                bail!(
                    "Claude Code already has a different lab MCP entry. Resolve that conflict in Claude Code before registering."
                );
            }
            servers.insert("lab".into(), desired);
            Ok(format!("{}\n", serde_json::to_string_pretty(&document)?))
        }
    }
}

fn validated_endpoint(raw: &str) -> Result<url::Url> {
    let url = url::Url::parse(raw).context("Enter a full MCP gateway URL")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if raw
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control() || byte == b'\\')
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        bail!(
            "MCP gateway URLs must use HTTPS, or HTTP on loopback, without credentials, query, or fragment"
        );
    }
    Ok(url)
}

async fn metadata(client: &reqwest::Client, url: url::Url) -> Result<serde_json::Value> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 65_536 {
            bail!("OAuth metadata exceeds the supported size");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Require the gateway's advertised OAuth resource and a dynamic-registration
/// authorization server before writing. Bearer-only native setups are explicitly
/// unsupported by these adapters until a protected local bridge is available.
pub async fn verify_oauth(gateway: &str) -> Result<()> {
    let gateway = validated_endpoint(gateway)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resource = metadata(&client, gateway.join("/.well-known/oauth-protected-resource")?).await
        .context("This gateway has not advertised usable OAuth. Bearer-only gateways need a protected local bridge; no client configuration was changed.")?;
    validate_resource_metadata(&gateway, &resource)?;
    let servers = resource["authorization_servers"]
        .as_array()
        .context("OAuth authorization servers are missing")?;
    let issuer = validated_endpoint(servers[0].as_str().context("OAuth issuer is invalid")?)?;
    if gateway.scheme() == "https" && issuer.scheme() != "https" {
        bail!("Remote OAuth metadata cannot direct this helper to an insecure local issuer");
    }
    let authorization = metadata(
        &client,
        issuer.join("/.well-known/oauth-authorization-server")?,
    )
    .await?;
    for key in [
        "authorization_endpoint",
        "token_endpoint",
        "registration_endpoint",
    ] {
        let endpoint = validated_endpoint(authorization[key].as_str().with_context(|| {
            format!("OAuth {key} is unavailable; this adapter requires dynamic registration")
        })?)?;
        if endpoint.origin() != issuer.origin() {
            bail!("OAuth metadata endpoint origin differs from its issuer");
        }
    }
    if authorization["issuer"].as_str() != Some(issuer.as_str().trim_end_matches('/'))
        && authorization["issuer"].as_str() != Some(issuer.as_str())
    {
        bail!("OAuth metadata issuer does not match");
    }
    Ok(())
}

fn validate_resource_metadata(gateway: &url::Url, resource: &serde_json::Value) -> Result<()> {
    let identity = validated_endpoint(
        resource["resource"]
            .as_str()
            .context("OAuth resource identity is missing")?,
    )?;
    if identity.as_str().trim_end_matches('/') != gateway.as_str().trim_end_matches('/') {
        bail!("OAuth resource identity does not match the selected MCP gateway URL");
    }
    let servers = resource["authorization_servers"]
        .as_array()
        .context("OAuth authorization servers are missing")?;
    if servers.len() != 1 {
        bail!("This adapter requires one advertised OAuth authorization server");
    }
    validated_endpoint(servers[0].as_str().context("OAuth issuer is invalid")?)?;
    Ok(())
}

#[cfg(test)]
#[path = "client_registration_tests.rs"]
mod tests;
