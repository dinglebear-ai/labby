//! Guided Tailscale Funnel exposure for Browser + ChatGPT onboarding.
//!
//! This is distinct from the ephemeral MCP proxy. It exposes the main Labby
//! HTTP server through Tailscale-managed public HTTPS. Mutations are restricted
//! to local setup transports by the setup action boundary.

use std::ffi::OsString;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::dispatch::error::ToolError;
use crate::proxy::tailscale::{ServeStatus, TailscaleStatus, run_checked};

const DEFAULT_BACKEND_URL: &str = "http://127.0.0.1:8765";
const DEFAULT_HTTPS_PORT: u16 = 443;
const SUPPORTED_HTTPS_PORTS: [u16; 3] = [443, 8443, 10000];
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const READINESS_ATTEMPTS: usize = 20;
const CONFIGURE_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_CAPTURE_LIMIT: usize = 16 * 1024;
const TAILSCALE_HTTPS_CAPABILITY: &str = "https";
const TAILSCALE_FUNNEL_CAPABILITY: &str = "funnel";

#[derive(Debug, Clone, Deserialize)]
pub struct TailscaleFunnelRequest {
    #[serde(default)]
    pub backend_url: Option<String>,
    #[serde(default = "default_https_port")]
    pub https_port: u16,
}

impl Default for TailscaleFunnelRequest {
    fn default() -> Self {
        Self {
            backend_url: None,
            https_port: DEFAULT_HTTPS_PORT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailscaleFunnelInspection {
    pub cli_available: bool,
    pub version: Option<String>,
    pub backend_running: bool,
    pub online: bool,
    pub dns_name: Option<String>,
    pub public_origin: Option<String>,
    pub https_port: u16,
    pub funnel_status_readable: bool,
    pub configured_backend: Option<String>,
    pub https_enabled: bool,
    pub funnel_enabled: bool,
    pub activation_required: bool,
    pub ready_to_configure: bool,
    pub blockers: Vec<String>,
    pub verification: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailscaleFunnelMutationOutcome {
    pub changed: bool,
    pub configured: bool,
    pub public_origin: String,
    pub backend_origin: String,
    pub https_port: u16,
    pub oauth_callback_url: String,
    pub mcp_url: String,
    pub activation_required: bool,
    pub activation_url: Option<String>,
    pub activation_message: Option<String>,
    pub verification: Vec<String>,
}

pub async fn inspect(https_port: u16) -> Result<TailscaleFunnelInspection, ToolError> {
    validate_https_port(https_port)?;
    inspect_with_executable(&tailscale_executable(), https_port).await
}

pub async fn configure(
    request: TailscaleFunnelRequest,
) -> Result<TailscaleFunnelMutationOutcome, ToolError> {
    configure_with_executable(&tailscale_executable(), request).await
}

pub async fn disable(
    request: TailscaleFunnelRequest,
) -> Result<TailscaleFunnelMutationOutcome, ToolError> {
    disable_with_executable(&tailscale_executable(), request).await
}

async fn inspect_with_executable(
    executable: &PathBuf,
    https_port: u16,
) -> Result<TailscaleFunnelInspection, ToolError> {
    validate_https_port(https_port)?;

    let version = match run_checked(executable, ["version"]).await {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => {
            return Ok(unavailable_inspection(
                https_port,
                "Tailscale CLI returned an empty version".into(),
            ));
        }
        Err(error) => {
            return Ok(unavailable_inspection(
                https_port,
                format!("Tailscale CLI is unavailable: {error:#}"),
            ));
        }
    };

    let status_raw = match run_checked(executable, ["status", "--json"]).await {
        Ok(value) => value,
        Err(error) => {
            let mut outcome = unavailable_inspection(
                https_port,
                format!("Tailscale status is unavailable: {error:#}"),
            );
            outcome.cli_available = true;
            outcome.version = Some(first_line(&version));
            return Ok(outcome);
        }
    };

    let funnel_raw = run_checked(executable, ["funnel", "status", "--json"])
        .await
        .ok();
    inspection_from_outputs(&version, &status_raw, funnel_raw.as_deref(), https_port)
}

async fn configure_with_executable(
    executable: &PathBuf,
    request: TailscaleFunnelRequest,
) -> Result<TailscaleFunnelMutationOutcome, ToolError> {
    validate_https_port(request.https_port)?;
    let backend = validated_loopback_backend(
        request
            .backend_url
            .as_deref()
            .unwrap_or(DEFAULT_BACKEND_URL),
    )?;
    let backend_origin = super::public_proxy::origin_string(&backend);
    let inspection = inspect_with_executable(executable, request.https_port).await?;
    require_ready(&inspection)?;
    let public_origin = inspection
        .public_origin
        .clone()
        .ok_or_else(|| unavailable("Tailscale did not report a public DNS name"))?;

    match mapping_disposition(
        inspection.configured_backend.as_deref(),
        backend_origin.as_str(),
    ) {
        MappingDisposition::Reuse => {
            return Ok(mutation_outcome(
                false,
                true,
                public_origin,
                backend_origin,
                request.https_port,
            ));
        }
        MappingDisposition::Conflict(existing) => {
            return Err(ToolError::Sdk {
                sdk_kind: "conflict".into(),
                message: format!(
                    "Tailscale Funnel port {} already proxies to {existing}; refusing to replace an existing Funnel mapping",
                    request.https_port
                ),
            });
        }
        MappingDisposition::Create => {}
    }

    let command = run_configure_command(
        executable,
        configure_args(request.https_port, &backend_origin),
    )
    .await?;
    let output = command.combined_output();
    let activation_url = activation_url_from_output(&output, &public_origin);
    let dns_name = inspection
        .dns_name
        .as_deref()
        .ok_or_else(|| unavailable("Tailscale did not report a DNS name"))?;

    if !command.status.success() && !(inspection.activation_required && activation_url.is_some()) {
        return Err(unavailable(format!(
            "Tailscale Funnel configuration exited with {}: {}",
            command.status,
            if output.trim().is_empty() {
                "no diagnostic output".to_string()
            } else {
                output.clone()
            }
        )));
    }

    match current_funnel_backend(executable, dns_name, request.https_port).await? {
        Some(current) if current == backend_origin => {
            return Ok(mutation_outcome(
                true,
                true,
                public_origin,
                backend_origin,
                request.https_port,
            ));
        }
        Some(current) => {
            return Err(ToolError::Sdk {
                sdk_kind: "conflict".into(),
                message: format!(
                    "Tailscale Funnel port {} changed ownership to {current} while Labby was configuring it",
                    request.https_port
                ),
            });
        }
        None => {}
    }

    if inspection.activation_required && !command.status.success() {
        return Ok(activation_outcome(
            public_origin,
            backend_origin,
            request.https_port,
            activation_url,
            output,
        ));
    }
    wait_for_mapping(
        executable,
        dns_name,
        request.https_port,
        Some(backend_origin.as_str()),
    )
    .await?;

    Ok(mutation_outcome(
        true,
        true,
        public_origin,
        backend_origin,
        request.https_port,
    ))
}

async fn disable_with_executable(
    executable: &PathBuf,
    request: TailscaleFunnelRequest,
) -> Result<TailscaleFunnelMutationOutcome, ToolError> {
    validate_https_port(request.https_port)?;
    let backend = validated_loopback_backend(
        request
            .backend_url
            .as_deref()
            .unwrap_or(DEFAULT_BACKEND_URL),
    )?;
    let backend_origin = super::public_proxy::origin_string(&backend);
    let inspection = inspect_with_executable(executable, request.https_port).await?;
    require_ready(&inspection)?;
    let public_origin = inspection
        .public_origin
        .clone()
        .ok_or_else(|| unavailable("Tailscale did not report a public DNS name"))?;

    match mapping_disposition(
        inspection.configured_backend.as_deref(),
        backend_origin.as_str(),
    ) {
        MappingDisposition::Create => {
            return Ok(mutation_outcome(
                false,
                false,
                public_origin,
                backend_origin,
                request.https_port,
            ));
        }
        MappingDisposition::Conflict(existing) => {
            return Err(ToolError::Sdk {
                sdk_kind: "conflict".into(),
                message: format!(
                    "Tailscale Funnel port {} belongs to {existing}; refusing to disable a mapping Labby does not own",
                    request.https_port
                ),
            });
        }
        MappingDisposition::Reuse => {}
    }

    run_checked(executable, disable_args(request.https_port))
        .await
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "unavailable".into(),
            message: format!("Tailscale Funnel could not be disabled: {error:#}"),
        })?;
    wait_for_mapping(
        executable,
        inspection
            .dns_name
            .as_deref()
            .ok_or_else(|| unavailable("Tailscale did not report a DNS name"))?,
        request.https_port,
        None,
    )
    .await?;

    Ok(mutation_outcome(
        true,
        false,
        public_origin,
        backend_origin,
        request.https_port,
    ))
}

fn inspection_from_outputs(
    version: &str,
    status_raw: &str,
    funnel_raw: Option<&str>,
    https_port: u16,
) -> Result<TailscaleFunnelInspection, ToolError> {
    let status =
        TailscaleStatus::parse(status_raw).map_err(|error| unavailable(error.to_string()))?;
    let backend_running = status.backend_running();
    let online = status.online();
    let https_enabled = status.has_capability(TAILSCALE_HTTPS_CAPABILITY);
    let funnel_enabled = status.has_capability(TAILSCALE_FUNNEL_CAPABILITY);
    let dns_name = (!status.dns_name().trim().is_empty())
        .then(|| status.dns_name().trim_end_matches('.').to_string());
    let public_origin = dns_name
        .as_deref()
        .map(|dns| public_origin(dns, https_port));

    let mut blockers = Vec::new();
    if !backend_running {
        blockers.push("Tailscale backend is not Running".into());
    }
    if !online {
        blockers.push("local Tailscale node is offline".into());
    }
    if dns_name.is_none() {
        blockers.push("local Tailscale node has no MagicDNS name".into());
    }

    let (funnel_status_readable, configured_backend) = match funnel_raw {
        Some(raw) => match ServeStatus::parse(raw) {
            Ok(status) => {
                let serve_backend = dns_name
                    .as_deref()
                    .and_then(|dns| status.backend_for(dns, https_port));
                let funnel_backend = dns_name
                    .as_deref()
                    .and_then(|dns| status.funnel_backend_for(dns, https_port));
                if let Some(private_backend) = serve_backend.filter(|_| funnel_backend.is_none()) {
                    blockers.push(format!(
                        "Tailscale port {https_port} is already configured as tailnet-only Serve to {private_backend}; Labby will not promote it to public Funnel automatically"
                    ));
                }
                (true, funnel_backend.map(str::to_string))
            }
            Err(error) => {
                blockers.push(format!("Tailscale Funnel status is invalid: {error:#}"));
                (false, None)
            }
        },
        None => {
            blockers.push(
                "Tailscale Funnel status is unavailable. Funnel may need first-time HTTPS or policy approval in Tailscale."
                    .into(),
            );
            (false, None)
        }
    };

    let activation_required = configured_backend.is_none() && !(https_enabled && funnel_enabled);

    Ok(TailscaleFunnelInspection {
        cli_available: true,
        version: Some(first_line(version)),
        backend_running,
        online,
        dns_name,
        public_origin: public_origin.clone(),
        https_port,
        funnel_status_readable,
        configured_backend,
        https_enabled,
        funnel_enabled,
        activation_required,
        ready_to_configure: blockers.is_empty(),
        blockers,
        verification: public_origin
            .map(|origin| verification_commands(&origin))
            .unwrap_or_default(),
    })
}

fn unavailable_inspection(https_port: u16, blocker: String) -> TailscaleFunnelInspection {
    TailscaleFunnelInspection {
        cli_available: false,
        version: None,
        backend_running: false,
        online: false,
        dns_name: None,
        public_origin: None,
        https_port,
        funnel_status_readable: false,
        configured_backend: None,
        https_enabled: false,
        funnel_enabled: false,
        activation_required: false,
        ready_to_configure: false,
        blockers: vec![blocker],
        verification: Vec::new(),
    }
}

fn require_ready(inspection: &TailscaleFunnelInspection) -> Result<(), ToolError> {
    if inspection.ready_to_configure {
        return Ok(());
    }
    Err(unavailable(format!(
        "Tailscale Funnel is not ready: {}",
        inspection.blockers.join("; ")
    )))
}

async fn current_funnel_backend(
    executable: &PathBuf,
    dns_name: &str,
    https_port: u16,
) -> Result<Option<String>, ToolError> {
    let raw = run_checked(executable, ["funnel", "status", "--json"])
        .await
        .map_err(|error| unavailable(format!("read Tailscale Funnel status: {error:#}")))?;
    let status = ServeStatus::parse(&raw)
        .map_err(|error| unavailable(format!("parse Tailscale Funnel status: {error:#}")))?;
    Ok(status
        .funnel_backend_for(dns_name, https_port)
        .map(str::to_string))
}

async fn wait_for_mapping(
    executable: &PathBuf,
    dns_name: &str,
    https_port: u16,
    expected_backend: Option<&str>,
) -> Result<(), ToolError> {
    for _ in 0..READINESS_ATTEMPTS {
        let raw = run_checked(executable, ["funnel", "status", "--json"])
            .await
            .map_err(|error| unavailable(format!("read Tailscale Funnel status: {error:#}")))?;
        let status = ServeStatus::parse(&raw)
            .map_err(|error| unavailable(format!("parse Tailscale Funnel status: {error:#}")))?;
        let actual = status.funnel_backend_for(dns_name, https_port);
        if actual == expected_backend {
            return Ok(());
        }
        if actual.is_some() && expected_backend.is_some() {
            return Err(ToolError::Sdk {
                sdk_kind: "conflict".into(),
                message: format!(
                    "Tailscale Funnel port {https_port} changed ownership while Labby was configuring it"
                ),
            });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(unavailable(format!(
        "Tailscale Funnel did not converge on port {https_port} after configuration"
    )))
}

#[derive(Debug)]
struct ConfigureCommandResult {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl ConfigureCommandResult {
    fn combined_output(&self) -> String {
        match (self.stdout.trim(), self.stderr.trim()) {
            ("", "") => String::new(),
            (stdout, "") => stdout.to_string(),
            ("", stderr) => stderr.to_string(),
            (stdout, stderr) => format!("{stdout}\n{stderr}"),
        }
    }
}

async fn run_configure_command(
    executable: &PathBuf,
    args: Vec<OsString>,
) -> Result<ConfigureCommandResult, ToolError> {
    run_configure_command_with_timeout(executable, args, CONFIGURE_COMMAND_TIMEOUT).await
}

async fn run_configure_command_with_timeout(
    executable: &PathBuf,
    args: Vec<OsString>,
    timeout: Duration,
) -> Result<ConfigureCommandResult, ToolError> {
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            unavailable(format!(
                "failed to execute '{}': {error}",
                executable.display()
            ))
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| unavailable("Tailscale stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| unavailable("Tailscale stderr pipe was unavailable"))?;
    let stdout_task = tokio::spawn(capture_output(stdout));
    let stderr_task = tokio::spawn(capture_output(stderr));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            return Err(unavailable(format!(
                "failed while waiting for Tailscale Funnel configuration: {error}"
            )));
        }
        Err(_) => {
            let _kill_result = child.start_kill();
            let _wait_result = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(unavailable(format!(
                "Tailscale Funnel did not finish within {} ms; the process was stopped. Retry after checking the Funnel status.",
                timeout.as_millis()
            )));
        }
    };

    let stdout = join_capture(stdout_task).await?;
    let stderr = join_capture(stderr_task).await?;
    Ok(ConfigureCommandResult {
        status,
        stdout,
        stderr,
    })
}

async fn capture_output(mut pipe: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = pipe.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read]);
        if output.len() > OUTPUT_CAPTURE_LIMIT {
            output.drain(..output.len() - OUTPUT_CAPTURE_LIMIT);
        }
    }
    Ok(output)
}

async fn join_capture(
    task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<String, ToolError> {
    let bytes = task
        .await
        .map_err(|error| unavailable(format!("Tailscale output task failed: {error}")))?
        .map_err(|error| unavailable(format!("could not read Tailscale output: {error}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn activation_url_from_output(output: &str, public_origin: &str) -> Option<String> {
    let public_host = url::Url::parse(public_origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string));
    output
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|ch: char| {
                matches!(ch, '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | '"')
            })
        })
        .filter_map(|token| url::Url::parse(token).ok())
        .find(|url| {
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| Some(host) != public_host.as_deref())
        })
        .map(|url| url.to_string())
}

fn activation_outcome(
    public_origin: String,
    backend_origin: String,
    https_port: u16,
    activation_url: Option<String>,
    activation_message: String,
) -> TailscaleFunnelMutationOutcome {
    TailscaleFunnelMutationOutcome {
        oauth_callback_url: format!("{public_origin}/auth/google/callback"),
        mcp_url: format!("{public_origin}/mcp"),
        verification: verification_commands(&public_origin),
        changed: false,
        configured: false,
        public_origin,
        backend_origin,
        https_port,
        activation_required: true,
        activation_url,
        activation_message: (!activation_message.trim().is_empty())
            .then(|| activation_message.trim().to_string()),
    }
}

fn validated_loopback_backend(raw: &str) -> Result<url::Url, ToolError> {
    let url = super::public_proxy::validate_backend_origin(raw)?;
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if !loopback {
        return Err(ToolError::InvalidParam {
            param: "backend_url".into(),
            message: "Tailscale Funnel onboarding may expose only a loopback Labby backend".into(),
        });
    }
    Ok(url)
}

fn validate_https_port(port: u16) -> Result<(), ToolError> {
    if SUPPORTED_HTTPS_PORTS.contains(&port) {
        return Ok(());
    }
    Err(ToolError::InvalidParam {
        param: "https_port".into(),
        message: "Tailscale Funnel HTTPS port must be 443, 8443, or 10000".into(),
    })
}

fn tailscale_executable() -> PathBuf {
    std::env::var_os("LABBY_TAILSCALE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tailscale"))
}

fn configure_args(port: u16, backend: &str) -> Vec<OsString> {
    vec![
        "funnel".into(),
        "--bg".into(),
        "--yes".into(),
        format!("--https={port}").into(),
        backend.into(),
    ]
}

fn disable_args(port: u16) -> Vec<OsString> {
    vec![
        "funnel".into(),
        "--yes".into(),
        format!("--https={port}").into(),
        "off".into(),
    ]
}

fn public_origin(dns_name: &str, port: u16) -> String {
    if port == DEFAULT_HTTPS_PORT {
        format!("https://{}", dns_name.trim_end_matches('.'))
    } else {
        format!("https://{}:{port}", dns_name.trim_end_matches('.'))
    }
}

fn mutation_outcome(
    changed: bool,
    configured: bool,
    public_origin: String,
    backend_origin: String,
    https_port: u16,
) -> TailscaleFunnelMutationOutcome {
    TailscaleFunnelMutationOutcome {
        oauth_callback_url: format!("{public_origin}/auth/google/callback"),
        mcp_url: format!("{public_origin}/mcp"),
        activation_required: false,
        activation_url: None,
        activation_message: None,
        verification: verification_commands(&public_origin),
        changed,
        configured,
        public_origin,
        backend_origin,
        https_port,
    }
}

fn verification_commands(public_origin: &str) -> Vec<String> {
    vec![
        format!("curl --fail-with-body {public_origin}/health"),
        format!("curl -fsS {public_origin}/.well-known/oauth-authorization-server"),
        format!("curl -i {public_origin}/mcp"),
        "labby doctor oauth".into(),
    ]
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or_default().trim().to_string()
}

fn unavailable(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unavailable".into(),
        message: message.into(),
    }
}

const fn default_https_port() -> u16 {
    DEFAULT_HTTPS_PORT
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MappingDisposition {
    Create,
    Reuse,
    Conflict(String),
}

fn mapping_disposition(existing: Option<&str>, desired: &str) -> MappingDisposition {
    match existing {
        None => MappingDisposition::Create,
        Some(existing) if existing == desired => MappingDisposition::Reuse,
        Some(existing) => MappingDisposition::Conflict(existing.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{
        "BackendState": "Running",
        "Self": {"Online": true, "DNSName": "labby.example-tail.ts.net."}
    }"#;

    const FUNNEL_STATUS: &str = r#"{
        "TCP": {},
        "Web": {
            "labby.example-tail.ts.net:443": {
                "Handlers": {"/": {"Proxy": "http://127.0.0.1:8765"}}
            }
        },
        "AllowFunnel": {"labby.example-tail.ts.net:443": true}
    }"#;

    #[test]
    fn inspection_projects_existing_funnel_and_exact_public_endpoints() {
        let result = inspection_from_outputs("1.90.0\n", STATUS, Some(FUNNEL_STATUS), 443).unwrap();
        assert!(result.cli_available);
        assert!(result.ready_to_configure);
        assert!(!result.activation_required);
        assert_eq!(result.version.as_deref(), Some("1.90.0"));
        assert_eq!(
            result.public_origin.as_deref(),
            Some("https://labby.example-tail.ts.net")
        );
        assert_eq!(
            result.configured_backend.as_deref(),
            Some("http://127.0.0.1:8765")
        );
        assert!(
            result
                .verification
                .iter()
                .any(|command| command.contains("/.well-known/oauth-authorization-server"))
        );
    }

    #[test]
    fn tailnet_only_serve_is_not_reported_or_promoted_as_funnel() {
        let private_serve = r#"{
            "TCP": {},
            "Web": {
                "labby.example-tail.ts.net:443": {
                    "Handlers": {"/": {"Proxy": "http://127.0.0.1:8765"}}
                }
            },
            "AllowFunnel": {}
        }"#;
        let result = inspection_from_outputs("1.90.0\n", STATUS, Some(private_serve), 443).unwrap();
        assert!(result.funnel_status_readable);
        assert_eq!(result.configured_backend, None);
        assert!(!result.ready_to_configure);
        assert!(
            result
                .blockers
                .iter()
                .any(|blocker| blocker.contains("tailnet-only Serve"))
        );
    }

    #[test]
    fn first_run_null_serve_config_is_ready_for_funnel_configuration() {
        let result = inspection_from_outputs("1.90.0\n", STATUS, Some("null"), 443).unwrap();
        assert!(result.funnel_status_readable);
        assert_eq!(result.configured_backend, None);
        assert!(result.ready_to_configure);
        assert!(result.activation_required);
        assert!(!result.https_enabled);
        assert!(!result.funnel_enabled);
        assert!(result.blockers.is_empty());
    }

    #[test]
    fn activation_url_parser_ignores_the_public_funnel_url() {
        let output = "Approve Funnel at https://login.tailscale.com/admin/feature/funnel\nAvailable on the internet: https://labby.example-tail.ts.net";
        assert_eq!(
            activation_url_from_output(output, "https://labby.example-tail.ts.net").as_deref(),
            Some("https://login.tailscale.com/admin/feature/funnel")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configure_surfaces_first_run_activation_url_without_hanging() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("tailscale");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
case "$1" in
  version)
    echo "1.90.0"
    exit 0
    ;;
  status)
    echo '{"BackendState":"Running","Self":{"Online":true,"DNSName":"labby.example-tail.ts.net.","CapMap":{}}}'
    exit 0
    ;;
  funnel)
    if [ "$2" = "status" ]; then
      echo null
      exit 0
    fi
    echo 'Funnel is not enabled. Visit https://login.tailscale.com/admin/feature/funnel'
    exit 1
    ;;
esac
exit 2
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();

        let result = configure_with_executable(&executable, TailscaleFunnelRequest::default())
            .await
            .unwrap();
        assert!(result.activation_required);
        assert!(!result.configured);
        assert!(!result.changed);
        assert_eq!(
            result.activation_url.as_deref(),
            Some("https://login.tailscale.com/admin/feature/funnel")
        );
        assert_eq!(result.public_origin, "https://labby.example-tail.ts.net");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn configure_runner_stops_a_hung_cli_within_the_bound() {
        let started = std::time::Instant::now();
        let error = run_configure_command_with_timeout(
            &PathBuf::from("/bin/sh"),
            vec![OsString::from("-c"), OsString::from("exec sleep 30")],
            Duration::from_millis(75),
        )
        .await
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("did not finish within 75 ms"));
    }

    #[test]
    fn funnel_only_allows_tailscale_public_https_ports() {
        for port in SUPPORTED_HTTPS_PORTS {
            assert!(validate_https_port(port).is_ok());
        }
        for port in [0, 80, 444, 8765] {
            assert!(validate_https_port(port).is_err());
        }
    }

    #[test]
    fn funnel_backend_is_loopback_only_and_credential_free() {
        for accepted in [
            "http://127.0.0.1:8765",
            "http://localhost:8765",
            "https://[::1]:8765",
        ] {
            assert!(
                validated_loopback_backend(accepted).is_ok(),
                "rejected {accepted}"
            );
        }
        for rejected in [
            "http://192.168.1.20:8765",
            "https://labby.example.com",
            "http://token@127.0.0.1:8765",
            "http://127.0.0.1:8765/private",
        ] {
            assert!(
                validated_loopback_backend(rejected).is_err(),
                "accepted {rejected}"
            );
        }
    }

    #[test]
    fn mutation_never_replaces_or_disables_foreign_mapping() {
        assert_eq!(
            mapping_disposition(None, DEFAULT_BACKEND_URL),
            MappingDisposition::Create
        );
        assert_eq!(
            mapping_disposition(Some(DEFAULT_BACKEND_URL), DEFAULT_BACKEND_URL),
            MappingDisposition::Reuse
        );
        assert_eq!(
            mapping_disposition(Some("http://127.0.0.1:9999"), DEFAULT_BACKEND_URL),
            MappingDisposition::Conflict("http://127.0.0.1:9999".into())
        );
    }

    #[test]
    fn command_arguments_are_bounded_and_non_shell() {
        assert_eq!(
            configure_args(443, DEFAULT_BACKEND_URL),
            vec![
                OsString::from("funnel"),
                OsString::from("--bg"),
                OsString::from("--yes"),
                OsString::from("--https=443"),
                OsString::from(DEFAULT_BACKEND_URL),
            ]
        );
        assert_eq!(
            disable_args(8443),
            vec![
                OsString::from("funnel"),
                OsString::from("--yes"),
                OsString::from("--https=8443"),
                OsString::from("off"),
            ]
        );
    }

    #[test]
    fn alternate_funnel_port_is_reflected_in_public_origin() {
        assert_eq!(
            public_origin("labby.example-tail.ts.net.", 8443),
            "https://labby.example-tail.ts.net:8443"
        );
    }

    #[test]
    fn public_proxy_origin_serialization_preserves_ipv6_brackets() {
        let backend = validated_loopback_backend("http://[::1]:8765").unwrap();
        assert_eq!(
            super::super::public_proxy::origin_string(&backend),
            "http://[::1]:8765"
        );
    }
}
