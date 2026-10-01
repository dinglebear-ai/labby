//! An isolated Labby MCP aggregator for an explicitly selected `.mcp.json`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::command::ProxyCommand;

#[derive(Deserialize)]
struct McpFile {
    #[serde(rename = "mcpServers")]
    servers: BTreeMap<String, McpEntry>,
}

#[derive(Deserialize)]
struct McpEntry {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct IsolatedConfig {
    upstream: Vec<IsolatedUpstream>,
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

#[derive(Serialize)]
struct IsolatedUpstream {
    name: String,
    transport: &'static str,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

pub struct PreparedMcpJson {
    _home: tempfile::TempDir,
    pub command: ProxyCommand,
    pub child_env: Vec<(OsString, OsString)>,
}

pub fn prepare(path: &Path) -> Result<PreparedMcpJson> {
    let path = path
        .canonicalize()
        .with_context(|| format!("open MCP configuration {}", path.display()))?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read MCP configuration {}", path.display()))?;
    let file: McpFile = serde_json::from_str(&raw)
        .context(".mcp.json must contain a valid mcpServers object with stdio commands")?;
    if file.servers.is_empty() {
        bail!(".mcp.json contains no MCP servers");
    }
    if file.servers.len() > 16 {
        bail!(".mcp.json may contain at most 16 MCP servers");
    }
    let count = file.servers.len();
    let mut upstream = Vec::with_capacity(count);
    let mut extra_stdio_commands = Vec::with_capacity(count);
    for (name, entry) in file.servers {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            bail!("MCP server names must contain only letters, digits, `_`, or `-`");
        }
        if entry.command.trim().is_empty() {
            bail!("MCP server `{name}` has no stdio command");
        }
        if Path::new(&entry.command)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case("labby"))
        {
            bail!("MCP server `{name}` cannot recursively launch Labby");
        }
        if entry.env.keys().any(|key| {
            key.is_empty()
                || !key.bytes().enumerate().all(|(index, byte)| {
                    byte.is_ascii_alphabetic()
                        || byte == b'_'
                        || (index > 0 && byte.is_ascii_digit())
                })
        }) {
            bail!("MCP server `{name}` has an invalid environment variable name");
        }
        extra_stdio_commands.push(entry.command.clone());
        upstream.push(IsolatedUpstream {
            name,
            transport: "stdio",
            command: entry.command,
            args: entry.args,
            env: entry.env,
        });
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
    let executable = std::env::current_exe().context("locate current Labby executable")?;
    let cwd: PathBuf = path
        .parent()
        .context("MCP configuration has no parent directory")?
        .to_path_buf();
    let command = ProxyCommand {
        program: executable.into_os_string(),
        args: vec![OsString::from("mcp")],
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
