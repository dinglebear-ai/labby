//! Configuration discovery and an isolated Labby MCP aggregator.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamTransport};
use serde::{Deserialize, Serialize};

use super::command::ProxyCommand;

#[derive(Deserialize)]
struct McpFile {
    #[serde(rename = "mcpServers")]
    servers: BTreeMap<String, McpEntry>,
}

type McpEntry = serde_json::Map<String, serde_json::Value>;

fn valid_env_name(key: &str) -> bool {
    !key.is_empty()
        && key.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphabetic() || byte == b'_' || (index > 0 && byte.is_ascii_digit())
        })
}

/// Adapt common MCP JSON entries to the gateway's canonical configuration.
fn upstream_entry(name: &str, mut entry: McpEntry) -> Result<UpstreamConfig> {
    if let Some(kind) = entry.remove("type") {
        let transport = match kind.as_str() {
            Some("http" | "streamable-http" | "streamable_http") => "http",
            Some("stdio") => "stdio",
            Some("websocket") => "websocket",
            Some("unix_socket") => "unix_socket",
            _ => bail!(
                "MCP server `{name}` has an unsupported transport type; use http, stdio, websocket, or unix_socket"
            ),
        };
        if let Some(existing) = entry.get("transport") {
            anyhow::ensure!(
                existing.as_str() == Some(transport),
                "MCP server `{name}` has conflicting type and transport"
            );
        }
        entry.insert("transport".into(), transport.into());
    }
    entry.insert("name".into(), name.into());
    serde_json::from_value(serde_json::Value::Object(entry))
        .with_context(|| format!("MCP server `{name}` has invalid upstream fields"))
}

#[derive(Serialize)]
struct IsolatedConfig {
    upstream: Vec<UpstreamConfig>,
    code_mode: IsolatedCodeMode,
    gateway: IsolatedGateway,
}

#[derive(Serialize)]
struct IsolatedGateway {
    extra_stdio_commands: Vec<String>,
}

#[derive(Serialize)]
struct IsolatedCodeMode {
    enabled: bool,
}

pub struct PreparedMcpJson {
    _home: tempfile::TempDir,
    pub command: ProxyCommand,
    pub child_env: Vec<(OsString, OsString)>,
}

/// Find the first default configuration without masking inspection failures.
/// The effective Labby home takes precedence over the native executable directory.
pub fn discover(home: &Path, executable: &Path) -> Result<Option<PathBuf>> {
    let executable_dir = executable
        .parent()
        .context("Labby executable has no parent directory")?;
    for directory in [home, executable_dir] {
        let candidate = directory.join(".mcp.json");
        match std::fs::metadata(&candidate) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    bail!("MCP configuration {} is not a file", candidate.display());
                }
                return Ok(Some(candidate));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect MCP configuration {}", candidate.display()));
            }
        }
    }
    Ok(None)
}

pub fn prepare(path: &Path) -> Result<PreparedMcpJson> {
    prepare_with_overrides(path, None, &[], &[])
}

pub fn prepare_with_overrides(
    path: &Path,
    cwd: Option<&Path>,
    explicit_env: &[(OsString, OsString)],
    inherit_env: &[OsString],
) -> Result<PreparedMcpJson> {
    let to_text = |value: &OsString| {
        value
            .to_str()
            .map(str::to_owned)
            .context("aggregate MCP environment names and values must be UTF-8")
    };
    let mut inherited = BTreeMap::new();
    for name in inherit_env {
        if let Some(value) = std::env::var_os(name) {
            inherited.insert(to_text(name)?, to_text(&value)?);
        }
    }
    let explicit = explicit_env
        .iter()
        .map(|(name, value)| Ok((to_text(name)?, to_text(value)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let path = path
        .canonicalize()
        .with_context(|| format!("open MCP configuration {}", path.display()))?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read MCP configuration {}", path.display()))?;
    let file: McpFile = serde_json::from_str(&raw).context(
        ".mcp.json must contain a valid mcpServers object with commands or upstream URLs",
    )?;
    if file.servers.is_empty() {
        bail!(".mcp.json contains no MCP servers");
    }
    if file.servers.len() > 16 {
        bail!(".mcp.json may contain at most 16 MCP servers");
    }
    let count = file.servers.len();
    let mut upstream = Vec::with_capacity(count);
    let mut extra_stdio_commands = Vec::with_capacity(count);
    let mut credentials = BTreeMap::new();
    let credential_prefix = format!("LABBY_AGGREGATE_TOKEN_{}", uuid::Uuid::new_v4().simple());
    for (index, (name, entry)) in file.servers.into_iter().enumerate() {
        let mut entry = upstream_entry(&name, entry)?;
        // Common MCP configs carry bearer Authorization in headers. Translate
        // it into the gateway's credential reference rather than weakening its
        // custom-header policy or leaking the token into stdio environments.
        let authorization = entry
            .headers
            .keys()
            .filter(|key| key.eq_ignore_ascii_case("authorization"))
            .cloned()
            .collect::<Vec<_>>();
        anyhow::ensure!(
            authorization.len() <= 1,
            "MCP server `{name}` has duplicate Authorization headers"
        );
        if let Some(key) = authorization.first() {
            anyhow::ensure!(
                entry.effective_transport() != Some(UpstreamTransport::Stdio),
                "MCP server `{name}` cannot use HTTP Authorization with stdio"
            );
            anyhow::ensure!(
                entry.bearer_token_env.is_none() && entry.oauth.is_none(),
                "MCP server `{name}` has conflicting authentication"
            );
            let value = entry.headers.remove(key).expect("selected header exists");
            let (scheme, token) = value
                .split_once(' ')
                .context("Authorization must use Bearer followed by a token")?;
            anyhow::ensure!(
                scheme.eq_ignore_ascii_case("Bearer")
                    && !token.trim().is_empty()
                    && !token.chars().any(char::is_control),
                "MCP server `{name}` requires a valid Bearer Authorization header"
            );
            let key = format!("{credential_prefix}_{index}");
            credentials.insert(key.clone(), token.trim().to_owned());
            entry.bearer_token_env = Some(key);
        } else if let Some(key) = entry.bearer_token_env.clone() {
            anyhow::ensure!(
                valid_env_name(&key),
                "MCP server `{name}` has an invalid bearer_token_env name"
            );
            let token = explicit.get(&key).or_else(|| entry.env.get(&key)).or_else(|| inherited.get(&key)).cloned()
                .or_else(|| labby_gateway::upstream::auth::configured_bearer_token(&key))
                .filter(|token| !token.trim().is_empty())
                .with_context(|| format!("MCP server `{name}` requires credential `{key}`; anonymous fallback is disabled"))?;
            // Separate names avoid cross-server collisions and do not forward
            // HTTP credentials to unrelated stdio children.
            if entry.effective_transport() == Some(UpstreamTransport::Stdio) {
                entry.env.insert(key, token);
                entry.bearer_token_env = None;
            } else {
                let private_key = format!("{credential_prefix}_{index}");
                credentials.insert(private_key.clone(), token);
                entry.bearer_token_env = Some(private_key);
            }
        }
        entry
            .validate()
            .with_context(|| format!("invalid MCP server `{name}`"))?;
        if entry.effective_transport() == Some(UpstreamTransport::Stdio) {
            let command = entry
                .command
                .as_ref()
                .context("stdio command is required")?;
            if Path::new(command)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case("labby"))
            {
                bail!("MCP server `{name}` cannot recursively launch Labby");
            }
            let mut environment = inherited.clone();
            environment.extend(entry.env);
            environment.extend(explicit.clone());
            anyhow::ensure!(
                environment.keys().all(|key| valid_env_name(key)),
                "MCP server `{name}` has an invalid environment variable name"
            );
            entry.env = environment;
            extra_stdio_commands.push(command.clone());
        } else {
            anyhow::ensure!(
                entry.args.is_empty(),
                "MCP server `{name}` cannot use child args with a URL transport"
            );
            // URL entries may select credentials from env but do not spawn.
            entry.env.clear();
        }
        upstream.push(entry);
    }
    let home = tempfile::tempdir().context("create isolated MCP aggregator home")?;
    let config = toml::to_string(&IsolatedConfig {
        upstream,
        code_mode: IsolatedCodeMode { enabled: false },
        gateway: IsolatedGateway {
            extra_stdio_commands,
        },
    })
    .context("serialize isolated MCP aggregator configuration")?;
    let config_path = home.path().join("config.toml");
    std::fs::write(&config_path, config).context("write isolated MCP aggregator configuration")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600))
            .context("protect isolated MCP aggregator configuration")?;
    }
    if !credentials.is_empty() {
        let dotenv = credentials
            .iter()
            .map(|(key, value)| {
                // JSON string quoting is accepted by dotenv and protects quotes,
                // backslashes and line breaks without logging credential values.
                format!(
                    "{key}={}\n",
                    serde_json::to_string(value)
                        .expect("string serializes")
                        .replace('$', "\\$")
                )
            })
            .collect::<String>();
        let env_path = home.path().join(".env");
        std::fs::write(&env_path, dotenv).context("write isolated upstream credentials")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&env_path, std::fs::Permissions::from_mode(0o600))
                .context("protect isolated upstream credentials")?;
        }
    }
    let executable = std::env::current_exe().context("locate current Labby executable")?;
    let cwd = cwd.map_or_else(
        || {
            path.parent()
                .context("MCP configuration has no parent directory")
                .map(Path::to_path_buf)
        },
        |cwd| {
            cwd.canonicalize()
                .context("resolve aggregate MCP working directory")
        },
    )?;
    anyhow::ensure!(
        cwd.is_dir(),
        "aggregate MCP working directory is not a directory"
    );
    let command = ProxyCommand {
        program: executable.into_os_string(),
        args: vec![OsString::from("--quiet"), OsString::from("mcp")],
        cwd,
        display: format!("labby mcp ({count} servers from {})", path.display()),
    };
    let child_env = vec![
        (
            OsString::from("LABBY_HOME"),
            home.path().as_os_str().to_os_string(),
        ),
        (
            OsString::from("LABBY_MCP_FORCE_STANDALONE"),
            OsString::from("1"),
        ),
        (
            OsString::from("LABBY_MCP_PROXY_AGGREGATE"),
            OsString::from("1"),
        ),
        (OsString::from("LABBY_SPAWN_DEPTH"), OsString::from("0")),
    ];
    Ok(PreparedMcpJson {
        _home: home,
        command,
        child_env,
    })
}

#[cfg(test)]
#[path = "mcp_json/tests.rs"]
mod tests;
