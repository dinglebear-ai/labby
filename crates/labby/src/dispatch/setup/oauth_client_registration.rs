//! Direct OAuth clients retain their own authentication. Only a supplemental,
//! expiring observation proof is written into their private configuration.
use super::*;
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Managed {
    gateway: String,
    proofs: BTreeMap<String, Vec<String>>,
}

fn state_path(root: &Path, gateway: &str) -> Result<PathBuf> {
    let paths = crate::installation::InstallationPaths::from_root(root.to_path_buf())?;
    Ok(paths.root().join(format!(
        "oauth-client-observation-{}.json",
        hex::encode(Sha256::digest(gateway.as_bytes()))
    )))
}
fn read_managed(path: &Path, gateway: &str) -> Result<Managed> {
    let raw = match crate::dispatch::setup::secure_file::read_private(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Managed {
                gateway: gateway.into(),
                ..Managed::default()
            });
        }
        Err(error) => return Err(error.into()),
    };
    let managed: Managed = serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("Protected OAuth observation state is invalid"))?;
    if managed.gateway != gateway {
        bail!("OAuth observation state belongs to another gateway");
    }
    Ok(managed)
}
fn descriptor(client: ExternalClient, gateway: &str, proof: Option<&str>) -> serde_json::Value {
    let mut value = match client {
        ExternalClient::Codex => serde_json::json!({"url":gateway}),
        ExternalClient::ClaudeCode => serde_json::json!({"type":"http","url":gateway}),
    };
    if let Some(proof) = proof {
        value[header_key(client)] =
            serde_json::json!({crate::dispatch::setup::client_evidence::HEADER:proof});
    }
    value
}
fn header_key(client: ExternalClient) -> &'static str {
    match client {
        ExternalClient::Codex => "http_headers",
        ExternalClient::ClaudeCode => "headers",
    }
}
fn entry(client: ExternalClient, raw: &str) -> Result<Option<serde_json::Value>> {
    match client {
        ExternalClient::Codex => {
            let document: toml::Value = toml::from_str(raw)
                .map_err(|_| anyhow::anyhow!("Codex configuration is not valid TOML"))?;
            document
                .get("mcp_servers")
                .and_then(|servers| servers.get("lab"))
                .map(serde_json::to_value)
                .transpose()
                .map_err(Into::into)
        }
        ExternalClient::ClaudeCode => {
            if raw.trim().is_empty() {
                return Ok(None);
            }
            let document: serde_json::Value = serde_json::from_str(raw)
                .map_err(|_| anyhow::anyhow!("Claude Code configuration is not valid JSON"))?;
            Ok(document
                .get("mcpServers")
                .and_then(|servers| servers.get("lab"))
                .cloned())
        }
    }
}
/// Rotation is permitted only for the exact descriptor previously published by
/// this helper, or its unobserved URL-only form. Any custom auth/header setting
/// is a conflict and remains untouched.
fn render(
    client: ExternalClient,
    raw: &str,
    gateway: &str,
    proof: &str,
    previous: Option<&str>,
) -> Result<String> {
    let desired = descriptor(client, gateway, Some(proof));
    if let Some(existing) = entry(client, raw)? {
        if existing == desired {
            return Ok(raw.into());
        }
        if existing != descriptor(client, gateway, None)
            && !previous.is_some_and(|old| existing == descriptor(client, gateway, Some(old)))
        {
            bail!(
                "The existing lab MCP entry differs from the protected registration record. No client configuration was changed; reconcile it before retrying."
            );
        }
    }
    match client {
        ExternalClient::Codex => {
            let mut document: toml_edit::DocumentMut = raw
                .parse()
                .map_err(|_| anyhow::anyhow!("Codex configuration is not valid TOML"))?;
            if document.get("mcp_servers").is_none() {
                document["mcp_servers"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let servers = document
                .get_mut("mcp_servers")
                .and_then(toml_edit::Item::as_table_like_mut)
                .context("Codex mcp_servers must be a table")?;
            let mut headers = toml_edit::InlineTable::new();
            headers.insert(
                crate::dispatch::setup::client_evidence::HEADER,
                proof.into(),
            );
            let mut registration = toml_edit::InlineTable::new();
            registration.insert("url", gateway.into());
            registration.insert("http_headers", toml_edit::Value::InlineTable(headers));
            servers.insert(
                "lab",
                toml_edit::value(toml_edit::Value::InlineTable(registration)),
            );
            Ok(document.to_string())
        }
        ExternalClient::ClaudeCode => {
            let mut document: serde_json::Value = if raw.trim().is_empty() {
                serde_json::json!({})
            } else {
                serde_json::from_str(raw)
                    .map_err(|_| anyhow::anyhow!("Claude Code configuration is not valid JSON"))?
            };
            let servers = document
                .as_object_mut()
                .context("Claude Code configuration must be an object")?
                .entry("mcpServers")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .context("Claude Code mcpServers must be an object")?;
            servers.insert("lab".into(), desired);
            Ok(format!("{}\n", serde_json::to_string_pretty(&document)?))
        }
    }
}

fn apply(
    home: &Path,
    client: ExternalClient,
    gateway: &str,
    expected: &str,
    proof: &str,
    previous: Option<&str>,
) -> Result<RegistrationResult> {
    let config_path = target(home, client)?;
    let lock = HostConfigLock::acquire(&config_path)?;
    let raw = lock.read_raw()?;
    if version(&raw) != expected {
        bail!("Client configuration changed. Review a new registration before retrying.");
    }
    let output = render(client, &raw, gateway, proof, previous)?;
    let changed = output != raw;
    let backup_path = if changed && !raw.is_empty() {
        let path = config_path.with_file_name(format!(
            "{}.labby-backup-{}",
            config_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        labby_runtime::secure_atomic_file::write_secure_atomic(&path, raw.as_bytes())?;
        Some(path)
    } else {
        None
    };
    if lock.read_raw()? != raw {
        bail!(
            "Client configuration changed while preparing its protected backup. No registration was written."
        );
    }
    if changed {
        lock.write(&output)?;
    }
    Ok(RegistrationResult {
        plan: RegistrationPlan {
            client,
            config_path,
            config_version: version(&output),
            gateway_url: gateway.into(),
            authentication: "oauth",
            status: "oauth_login_required",
            readiness_supported: true,
            readiness_limitation: None,
            login_instruction: "Sign in to Labby as the same user in this application, then invoke a successful tool. Every selected application's tool use must succeed before readiness completes. Observation proofs expire after one hour; rerun connect to renew them.",
            bridge: None,
        },
        changed,
        backup_path,
        connected: false,
        bridge_verified: false,
    })
}

pub(super) async fn connect(
    home: PathBuf,
    root: PathBuf,
    gateway: String,
    clients: Vec<ExternalClient>,
) -> Vec<serde_json::Value> {
    let prepared = async {
        verify_oauth(&gateway).await?;
        let gateway = validated_endpoint(&gateway)?.to_string();
        let base = crate::config::cli::normalize_server(&gateway)?;
        let server = url::Url::parse(&base)?;
        if server.path() != "/" || validated_endpoint(&gateway)?.path() != "/mcp" {
            bail!("OAuth client observation requires the /mcp resource and /v1/setup control plane on the same server origin; mounted or separate control planes are unsupported");
        }
        let token = crate::oauth::cli_session::token_for_root(&root, &server).await?
            .context("Sign in to this gateway with labby auth login using the selected state root before connecting OAuth clients. No client configuration was changed.")?;
        let live = crate::live_gateway::detect_bound_bearer_target(&base, token).await?;
        register_authenticated(home, root, gateway, clients.clone(), &live).await
    }.await;
    match prepared {
        Ok(results) => results,
        Err(error) => clients.into_iter().map(|client|serde_json::json!({"client":client,"status":"needs_attention","connected":false,"message":error.to_string()})).collect(),
    }
}

/// The caller supplies a gateway authenticated through the existing credential
/// machinery. The HTTP endpoint establishes the principal; names/proofs never
/// claim identity or successful use.
async fn register_authenticated(
    home: PathBuf,
    root: PathBuf,
    gateway: String,
    clients: Vec<ExternalClient>,
    live: &crate::live_gateway::LiveGateway,
) -> Result<Vec<serde_json::Value>> {
    let clients = normalize_client_selection(clients);
    let path = state_path(&root, &gateway)?;
    crate::dispatch::setup::secure_file::create_private_dir(&root)?;
    let state_lock = HostConfigLock::acquire(&path)?;
    let mut managed = read_managed(&path, &gateway)?;
    let mut versions = Vec::new();
    let mut previous_proofs = Vec::new();
    for client in &clients {
        let raw = read_config_snapshot(&target(&home, *client)?)?;
        let previous = entry(*client, &raw)?
            .and_then(|value| {
                value
                    .get(header_key(*client))
                    .and_then(|headers| {
                        headers.get(crate::dispatch::setup::client_evidence::HEADER)
                    })
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|proof| {
                managed
                    .proofs
                    .get(client.key())
                    .is_some_and(|known| known.contains(proof))
            });
        render(
            *client,
            &raw,
            &gateway,
            "preflight-proof",
            previous.as_deref(),
        )?;
        versions.push(version(&raw));
        previous_proofs.push(previous);
    }
    let response = live
        .dispatch_client_session(
            "clients.session.start",
            serde_json::json!({"clients":clients}),
        )
        .await?;
    let issued: crate::dispatch::setup::client_evidence::Issued = serde_json::from_value(response)
        .map_err(|_| anyhow::anyhow!("Invalid client observation response"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let expected_clients = clients
        .iter()
        .map(|client| client.key())
        .collect::<std::collections::BTreeSet<_>>();
    if uuid::Uuid::parse_str(&issued.generation).is_err()
        || issued.expires <= now
        || issued.expires > now.saturating_add(3600)
        || issued
            .sessions
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>()
            != expected_clients
        || issued
            .sessions
            .values()
            .any(|proof| proof.len() != 64 || !proof.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        bail!("Invalid client observation response; no client configuration was changed");
    }
    // Persist both the exact installed proof and the pending replacement before
    // publishing descriptors. Interrupted writes remain safely resumable.
    for (client, previous) in clients.iter().zip(&previous_proofs) {
        let mut known = previous.iter().cloned().collect::<Vec<_>>();
        known.push(issued.sessions[client.key()].clone());
        managed.proofs.insert(client.key().into(), known);
    }
    crate::dispatch::setup::secure_file::replace_journal(
        state_lock.path(),
        &serde_json::to_vec(&managed)?,
    )?;
    let mut results = Vec::new();
    for ((client, expected), previous) in clients.into_iter().zip(versions).zip(previous_proofs) {
        let proof = issued
            .sessions
            .get(client.key())
            .context("Client observation response is missing a selected application")?;
        let result = apply(
            &home,
            client,
            &gateway,
            &expected,
            proof,
            previous.as_deref(),
        );
        results.push(match result {
            Ok(result) => {
                serde_json::json!({"client":client,"result":result,"observation_expires":issued.expires})
            }
            Err(error) => serde_json::json!({"client":client,"status":"needs_attention","connected":false,"message":error.to_string()}),
        });
    }
    Ok(results)
}

#[cfg(test)]
#[path = "oauth_client_registration_tests.rs"]
mod tests;
