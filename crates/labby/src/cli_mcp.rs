//! Standard stdio MCP server for administrator-selected, read-only CLI presets.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Semaphore;

const CONFIG_LIMIT: usize = 64 * 1024;
const OUTPUT_LIMIT: usize = 64 * 1024;
const TOOL_LIMIT: usize = 32;
const ARG_LIMIT: usize = 32;
const CHOICE_LIMIT: usize = 32;
const RUN_LIMIT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    tools: Vec<Preset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    name: String,
    description: String,
    executable: PathBuf,
    #[serde(default = "root_directory")]
    working_directory: PathBuf,
    #[serde(default)]
    args: Vec<String>,
    // The operator selects one more fixed argument sequence, not raw caller argv.
    #[serde(default)]
    choices: BTreeMap<String, Vec<String>>,
    read_only: bool,
}

fn root_directory() -> PathBuf {
    PathBuf::from("/")
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

fn bounded_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn validate_args(args: &[String]) -> bool {
    args.len() <= ARG_LIMIT && args.iter().all(|arg| bounded_text(arg, 1024))
}

fn reject_shell_or_launcher(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    matches!(
        name,
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "ksh"
            | "csh"
            | "tcsh"
            | "nu"
            | "busybox"
            | "toybox"
            | "env"
            | "sudo"
            | "su"
            | "doas"
            | "xargs"
            | "ssh"
            | "docker"
            | "podman"
            | "kubectl"
            | "nsenter"
            | "unshare"
            | "setsid"
            | "nohup"
            | "timeout"
            | "node"
            | "bun"
            | "deno"
            | "ruby"
            | "perl"
            | "php"
            | "lua"
            | "awk"
            | "gawk"
            | "mawk"
            | "npx"
            | "uvx"
    ) || name.starts_with("python")
}

fn trusted_metadata(metadata: &std::fs::Metadata) -> bool {
    metadata.mode() & 0o6022 == 0
        && (metadata.uid() == 0 || metadata.uid() == nix::unistd::geteuid().as_raw())
}

fn checked_ancestor_directories(path: &Path) -> bool {
    path.ancestors().skip(1).all(|directory| {
        directory
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir() && trusted_metadata(&metadata))
    })
}

fn checked_executable(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.canonicalize().ok().as_deref() == Some(path),
        "executable must be an absolute canonical path"
    );
    ensure!(
        checked_ancestor_directories(path),
        "executable directory is not trusted"
    );
    ensure!(
        !reject_shell_or_launcher(path),
        "shells and launchers are unsupported"
    );
    let metadata = path
        .metadata()
        .context("executable metadata is unavailable")?;
    ensure!(
        metadata.is_file()
            && trusted_metadata(&metadata)
            && metadata.permissions().mode() & 0o111 != 0,
        "executable must be a regular non-privileged file not writable by other users"
    );
    // Script interpreters are another executable boundary not represented by
    // the manifest. Use a pinned native application CLI in this first slice.
    let mut header = [0; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .context("executable header is unavailable")?;
    ensure!(
        header == *b"\x7fELF",
        "only native Linux CLI executables are supported"
    );
    Ok(())
}

impl Preset {
    fn check(&self) -> Result<()> {
        ensure!(identifier(&self.name), "invalid tool name");
        ensure!(
            bounded_text(&self.description, 512),
            "invalid tool description"
        );
        ensure!(self.read_only, "only read-only CLI tools are supported");
        ensure!(validate_args(&self.args), "invalid fixed argument list");
        ensure!(self.choices.len() <= CHOICE_LIMIT, "too many tool choices");
        ensure!(
            self.choices.iter().all(|(name, args)| {
                identifier(name) && validate_args(args) && args.len() + self.args.len() <= ARG_LIMIT
            }),
            "invalid fixed arguments for a tool choice"
        );
        ensure!(
            self.working_directory.is_absolute()
                && self.working_directory.is_dir()
                && self.working_directory.canonicalize().ok().as_deref()
                    == Some(&self.working_directory),
            "working directory must be an absolute canonical directory"
        );
        checked_executable(&self.executable)
    }

    // This is an independent user-configured upstream, not the Labby product catalog.
    #[allow(clippy::disallowed_methods)]
    fn tool(&self) -> Tool {
        let schema = if self.choices.is_empty() {
            json!({"type": "object", "properties": {}, "additionalProperties": false})
        } else {
            json!({
                "type": "object",
                "properties": {"choice": {"type": "string", "enum": self.choices.keys().collect::<Vec<_>>()}},
                "required": ["choice"],
                "additionalProperties": false
            })
        };
        let Value::Object(input_schema) = schema else {
            unreachable!("literal CLI input schema is an object");
        };
        let mut tool = Tool::new(
            self.name.clone(),
            self.description.clone(),
            Arc::new(input_schema),
        );
        tool.annotations = Some(ToolAnnotations::new().read_only(true).destructive(false));
        tool
    }

    fn selected_args<'a>(
        &'a self,
        params: Option<&Map<String, Value>>,
    ) -> Result<&'a [String], ErrorData> {
        if self.choices.is_empty() && params.is_none_or(Map::is_empty) {
            return Ok(&[]);
        }
        if let Some(params) = params
            && params.len() == 1
            && let Some(choice) = params.get("choice").and_then(Value::as_str)
            && let Some(args) = self.choices.get(choice)
        {
            return Ok(args);
        }
        Err(ErrorData::invalid_params(
            "supply only a configured choice (or no arguments for a fixed tool)",
            None,
        ))
    }
}

fn load_manifest(path: &Path) -> Result<Manifest> {
    ensure!(
        path.is_absolute()
            && path.canonicalize().ok().as_deref() == Some(path)
            && checked_ancestor_directories(path),
        "manifest requires an absolute canonical path in a trusted directory"
    );
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .context("CLI tool manifest could not be opened")?;
    let metadata = file
        .metadata()
        .context("CLI tool manifest metadata is unavailable")?;
    ensure!(
        metadata.is_file() && trusted_metadata(&metadata) && metadata.len() <= CONFIG_LIMIT as u64,
        "manifest must be a bounded regular file not writable by other users"
    );
    let mut text = String::new();
    file.take(CONFIG_LIMIT as u64 + 1)
        .read_to_string(&mut text)
        .context("CLI tool manifest could not be read as UTF-8")?;
    ensure!(text.len() <= CONFIG_LIMIT, "CLI tool manifest is too large");
    let manifest: Manifest = toml::from_str(&text)
        .map_err(|_| anyhow::anyhow!("invalid CLI tool manifest; see Add CLI documentation"))?;
    ensure!(
        (1..=TOOL_LIMIT).contains(&manifest.tools.len()),
        "manifest must contain 1–32 CLI tools"
    );
    let mut names = BTreeSet::new();
    for preset in &manifest.tools {
        preset.check()?;
        ensure!(names.insert(&preset.name), "duplicate CLI tool name");
    }
    Ok(manifest)
}

/// Prepare the restricted stdio selector for a host-provisioned Add CLI manifest.
#[cfg(feature = "gateway")]
pub(crate) fn registration_command(path: &Path) -> Result<(String, Vec<String>)> {
    use labby_gateway::security::guided_cli;
    let args = vec![
        "cli".into(),
        "serve".into(),
        "--manifest".into(),
        path.to_str()
            .context("the manifest path must be UTF-8")?
            .into(),
    ];
    guided_cli::prepare(&args)?;
    load_manifest(path)?;
    Ok((guided_cli::SELECTOR.into(), args))
}

struct ProcessGroup(Pid);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // Bound CLI descendants on deadline, MCP cancellation and output caps.
        let _ = killpg(self.0, Signal::SIGKILL);
    }
}

async fn read_capped(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    reader
        .take(OUTPUT_LIMIT as u64 + 1)
        .read_to_end(&mut output)
        .await?;
    ensure!(
        output.len() <= OUTPUT_LIMIT,
        "CLI tool output exceeds the limit"
    );
    Ok(output)
}

async fn run_preset(preset: &Preset, selected_args: &[String]) -> Result<String> {
    checked_executable(&preset.executable)?;
    let mut child = Command::new(&preset.executable)
        .current_dir(&preset.working_directory)
        .args(&preset.args)
        .args(selected_args)
        .env_clear()
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()
        .map_err(|_| anyhow::anyhow!("CLI tool could not start"))?;
    let pid = child.id().context("CLI tool process ID is unavailable")?;
    let _group = ProcessGroup(Pid::from_raw(i32::try_from(pid)?));
    let stdout = child
        .stdout
        .take()
        .context("CLI tool stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("CLI tool stderr is unavailable")?;
    let (stdout, _stderr, status) = tokio::time::timeout(RUN_LIMIT, async {
        tokio::try_join!(read_capped(stdout), read_capped(stderr), async {
            child.wait().await.context("CLI tool wait failed")
        })
    })
    .await
    .map_err(|_| anyhow::anyhow!("CLI tool exceeded its deadline"))??;
    ensure!(status.success(), "CLI tool exited unsuccessfully");
    String::from_utf8(stdout).context("CLI tool output is not UTF-8")
}

struct CliMcp {
    tools: Vec<Preset>,
    // An upstream may be called concurrently or directly, so bound locally too.
    slots: Semaphore,
    shutdown: tokio_util::sync::CancellationToken,
}

impl ServerHandler for CliMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("labby-cli", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            self.tools.iter().map(Preset::tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let preset = self
            .tools
            .iter()
            .find(|preset| preset.name == request.name.as_ref())
            .ok_or_else(|| ErrorData::invalid_params("unknown CLI tool", None))?;
        let args = preset.selected_args(request.arguments.as_ref())?;
        let _permit = self
            .slots
            .try_acquire()
            .map_err(|_| ErrorData::internal_error("CLI MCP is busy", None))?;
        let result = tokio::select! {
            biased;
            () = self.shutdown.cancelled() => {
                return Err(ErrorData::internal_error("CLI tool server is shutting down", None));
            }
            () = context.ct.cancelled() => {
                return Err(ErrorData::internal_error("CLI tool call cancelled", None));
            }
            result = run_preset(preset, args) => result,
        };
        let reply = match result {
            Ok(stdout) => CallToolResult::success(vec![ContentBlock::text(stdout)]),
            Err(_) => CallToolResult::error(vec![ContentBlock::text(
                "CLI tool failed, timed out, or exceeded its output limit",
            )]),
        };
        Ok(reply.into())
    }
}

pub(crate) async fn serve(path: &Path) -> Result<()> {
    let manifest = load_manifest(path)?;
    use tokio::signal::unix::{SignalKind, signal};

    let shutdown = tokio_util::sync::CancellationToken::new();
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let server = CliMcp {
        tools: manifest.tools,
        slots: Semaphore::new(2),
        shutdown: shutdown.clone(),
    };
    let running = tokio::select! {
        _ = terminate.recv() => return Ok(()),
        _ = interrupt.recv() => return Ok(()),
        result = server.serve(rmcp::transport::stdio()) => result?,
    };
    let cancel = running.cancellation_token();
    let waiting = running.waiting();
    tokio::pin!(waiting);
    tokio::select! {
        _ = terminate.recv() => (),
        _ = interrupt.recv() => (),
        result = &mut waiting => {
            shutdown.cancel();
            result?;
            return Ok(());
        }
    }
    shutdown.cancel();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), &mut waiting).await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset() -> Preset {
        Preset {
            name: "hello".to_owned(),
            description: "Show a fixed greeting".to_owned(),
            executable: PathBuf::from("/usr/bin/printf"),
            working_directory: root_directory(),
            args: vec!["%s".to_owned()],
            choices: BTreeMap::from([("world".to_owned(), vec!["Hello world".to_owned()])]),
            read_only: true,
        }
    }

    #[tokio::test]
    async fn launches_only_the_approved_argv() -> Result<()> {
        let preset = preset();
        let params = Map::from_iter([("choice".to_owned(), json!("world"))]);
        let args = preset
            .selected_args(Some(&params))
            .map_err(anyhow::Error::from)?;
        assert_eq!(run_preset(&preset, args).await?, "Hello world");
        assert!(
            preset
                .tool()
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                == Some(true)
        );
        Ok(())
    }

    #[test]
    fn cannot_supply_raw_arguments() {
        let preset = preset();
        let params = Map::from_iter([("choice".to_owned(), json!("--new-argument"))]);
        assert!(preset.selected_args(Some(&params)).is_err());
        assert!(preset.selected_args(None).is_err());
        assert!(checked_executable(Path::new("/usr/bin/bash")).is_err());
    }

    #[tokio::test]
    async fn output_cap_is_enforced() {
        let bytes = vec![b'x'; OUTPUT_LIMIT + 1];
        assert!(read_capped(bytes.as_slice()).await.is_err());
    }
}
