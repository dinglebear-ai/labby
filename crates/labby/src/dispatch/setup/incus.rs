//! Local-only Incus helpers for host-side Labby gateway bootstrap.
//!
//! These helpers are intentionally CLI-only. They are not in the setup action
//! catalog and must not be exposed through MCP, HTTP, or Code Mode.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_yaml_ng::Value;
use sha2::{Digest, Sha256};

use crate::dispatch::error::ToolError;

const INCUS_BOOTSTRAP_SCRIPT: &str = include_str!("../../../../../scripts/incus-bootstrap.sh");
const INSTALL_SCRIPT: &str = include_str!("../../../../../scripts/install.sh");
const GATEWAY_PROFILE_YAML: &str =
    include_str!("../../../../../config/incus/labby-gateway-profile.yaml");
const BACKUP_CONFIG_YAML: &str = include_str!("../../../../../config/incus/labby-backup.yaml");

const SUPPORTED_BACKUP_KEYS: &[&str] = &[
    "snapshots.schedule",
    "snapshots.expiry",
    "snapshots.pattern",
    "snapshots.schedule.stopped",
];

const DEFAULT_CONTAINER_NAME: &str = "labby";
const SERVICE_NAME: &str = "labby.service";
const REMOTE_BINARY_PATH: &str = "/usr/local/bin/labby";
const REMOTE_WEB_ASSETS_DIR: &str = "/home/labby/.labby/web-assets";
const READY_URL: &str = "http://127.0.0.1:8765/ready";
const PREVIOUS_RELEASE_DIR: &str = "/var/lib/labby/deployments/previous-release";
const COMMAND_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
const INCUS_TARGET_CONCURRENCY: usize = 8;

#[derive(Debug)]
struct BoundedCommandOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[derive(Debug)]
struct OutputTail {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct IncusConfigDocument {
    config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct BackupConfigApplyOutcome {
    pub container: String,
    pub dry_run: bool,
    pub applied: Vec<BackupConfigEntry>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapOptions {
    pub name: Option<String>,
    pub image: Option<String>,
    pub profile_name: Option<String>,
    pub backup_config: Option<PathBuf>,
    pub no_backup_config: bool,
    pub runtime_profile_name: Option<String>,
    pub storage_driver: Option<String>,
    pub storage_pool: Option<String>,
    pub storage_source: Option<String>,
    pub version: Option<String>,
    pub local_binary: Option<PathBuf>,
    pub skip_install: bool,
    pub dry_run: bool,
    pub tailscale_ssh: bool,
    pub tailscale_hostname: Option<String>,
    pub allow_source_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapArtifacts {
    pub root: PathBuf,
    pub bootstrap_script: PathBuf,
    pub install_script: PathBuf,
    pub profile_file: PathBuf,
    pub backup_config_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusBootstrapCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct IncusSyncOptions {
    pub container: Option<String>,
    pub binary: Option<PathBuf>,
    pub web_assets_dir: Option<PathBuf>,
    pub sync_web_assets: bool,
    pub check_url: Option<String>,
    pub force_fallback: bool,
    pub dry_run: bool,
    pub rollback: bool,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct IncusSyncOutcome {
    pub container: String,
    pub binary: PathBuf,
    pub web_assets_dir: Option<PathBuf>,
    pub remote_web_assets_dir: Option<String>,
    pub dry_run: bool,
    pub fallback_restart_used: bool,
    pub old_pid: Option<u32>,
    pub new_pid: Option<u32>,
    pub local_sha256: Option<String>,
    pub remote_sha256: Option<String>,
    pub local_version: Option<String>,
    pub remote_version: Option<String>,
    pub local_web_index_sha256: Option<String>,
    pub served_web_index_sha256: Option<String>,
    pub ready: bool,
    pub check_url: Option<String>,
    pub check_url_ok: Option<bool>,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IncusSshBootstrapOptions {
    pub container: String,
    pub user: String,
    pub ssh_config: PathBuf,
    pub key_path: String,
    pub dry_run: bool,
    pub fail_fast: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub install_config: bool,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct IncusSshTarget {
    pub alias: String,
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct IncusSshBootstrapOutcome {
    pub container: String,
    pub user: String,
    pub key_path: String,
    pub dry_run: bool,
    pub targets: Vec<IncusSshTarget>,
    pub authorized: Vec<String>,
    pub failed: Vec<IncusSshFailure>,
    pub skipped_github: Vec<String>,
    pub skipped_wildcard: Vec<String>,
    pub skipped_unsafe: Vec<String>,
    pub unsupported_include: Vec<String>,
    pub skipped_excluded: Vec<String>,
    pub skipped_not_included: Vec<String>,
    pub config_installed: bool,
    pub steps: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct IncusSshFailure {
    pub target: String,
    pub error: String,
}

#[derive(Debug, Default, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct IncusSshVerifyOutcome {
    pub container: String,
    pub user: String,
    pub key_path: String,
    pub targets: Vec<IncusSshTarget>,
    pub verified: Vec<String>,
    pub failed: Vec<IncusSshFailure>,
    pub skipped_github: Vec<String>,
    pub skipped_wildcard: Vec<String>,
    pub skipped_unsafe: Vec<String>,
    pub unsupported_include: Vec<String>,
    pub skipped_excluded: Vec<String>,
    pub skipped_not_included: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ParsedSshConfig {
    targets: Vec<IncusSshTarget>,
    skipped_github: Vec<String>,
    skipped_wildcard: Vec<String>,
    skipped_unsafe: Vec<String>,
    unsupported_include: Vec<String>,
}

fn parse_ssh_config(raw: &str) -> ParsedSshConfig {
    let mut parsed = ParsedSshConfig::default();
    let mut current: Vec<String> = Vec::new();
    let mut host_name: Option<String> = None;
    let mut user: Option<String> = None;
    let mut port: Option<u16> = None;

    fn flush(
        parsed: &mut ParsedSshConfig,
        current: &mut Vec<String>,
        host_name: &mut Option<String>,
        user: &mut Option<String>,
        port: &mut Option<u16>,
    ) {
        for alias in current.drain(..) {
            let host = host_name.clone().unwrap_or_else(|| alias.clone());
            if is_wildcard_ssh_config_host(&alias) {
                parsed.skipped_wildcard.push(alias);
                continue;
            }
            if !is_safe_ssh_alias(&alias) || !is_safe_ssh_config_value(&host) {
                parsed.skipped_unsafe.push(alias);
                continue;
            }
            if is_github_ssh_config_host(&alias, &host) {
                parsed.skipped_github.push(alias);
                continue;
            }
            parsed.targets.push(IncusSshTarget {
                host,
                alias,
                user: user.clone(),
                port: *port,
            });
        }
        *host_name = None;
        *user = None;
        *port = None;
    }

    for line in raw.lines() {
        let line = line.split_once('#').map_or(line, |(head, _)| head).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = split_ssh_config_line(line) else {
            continue;
        };
        if key.eq_ignore_ascii_case("host") {
            flush(
                &mut parsed,
                &mut current,
                &mut host_name,
                &mut user,
                &mut port,
            );
            current = value.split_whitespace().map(str::to_string).collect();
        } else if !current.is_empty() && key.eq_ignore_ascii_case("hostname") {
            host_name = Some(value.to_string());
        } else if !current.is_empty() && key.eq_ignore_ascii_case("user") {
            user = Some(value.to_string());
        } else if !current.is_empty() && key.eq_ignore_ascii_case("port") {
            port = value.parse().ok();
        } else if key.eq_ignore_ascii_case("include") {
            parsed.unsupported_include.push(value.to_string());
        }
    }
    flush(
        &mut parsed,
        &mut current,
        &mut host_name,
        &mut user,
        &mut port,
    );
    parsed
}

fn is_safe_ssh_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('-')
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_safe_ssh_config_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

pub(crate) fn incus_ssh_bootstrap_plan(
    options: &IncusSshBootstrapOptions,
) -> Result<IncusSshBootstrapOutcome, ToolError> {
    let raw = std::fs::read_to_string(&options.ssh_config).map_err(|e| ToolError::Sdk {
        message: format!(
            "failed to read SSH config {}: {e}",
            options.ssh_config.display()
        ),
        sdk_kind: "incus_ssh_config_read_failed".into(),
    })?;
    let parsed = parse_ssh_config(&raw);
    let filtered = filter_ssh_targets(parsed.targets, &options.include, &options.exclude);
    let targets = filtered.targets;
    if targets.is_empty() {
        return Err(ToolError::Sdk {
            message: format!(
                "no concrete Host entries found in {}",
                options.ssh_config.display()
            ),
            sdk_kind: "incus_ssh_config_empty".into(),
        });
    }
    let mut steps = vec![format!(
        "incus exec {} --user {} -- ssh-keygen -t ed25519 -f {} -N '' -C labby-incus-{}",
        options.container, options.user, options.key_path, options.container
    )];
    steps.extend(targets.iter().map(|target| {
        format!(
            "authorize container public key on {}",
            target_ssh_destination(target)
        )
    }));
    if options.install_config {
        steps.push(format!(
            "install sanitized SSH config in container {} for {} hosts",
            options.container,
            targets.len()
        ));
    }
    Ok(IncusSshBootstrapOutcome {
        container: options.container.clone(),
        user: options.user.clone(),
        key_path: options.key_path.clone(),
        dry_run: options.dry_run,
        targets,
        authorized: Vec::new(),
        failed: Vec::new(),
        skipped_github: parsed.skipped_github,
        skipped_wildcard: parsed.skipped_wildcard,
        skipped_unsafe: parsed.skipped_unsafe,
        unsupported_include: parsed.unsupported_include,
        skipped_excluded: filtered.skipped_excluded,
        skipped_not_included: filtered.skipped_not_included,
        config_installed: false,
        steps,
    })
}

pub(crate) fn incus_ssh_bootstrap(
    options: &IncusSshBootstrapOptions,
) -> Result<IncusSshBootstrapOutcome, ToolError> {
    let mut outcome = incus_ssh_bootstrap_plan(options)?;
    if options.dry_run {
        return Ok(outcome);
    }

    run_status(
        Command::new("incus")
            .arg("exec")
            .arg(&options.container)
            .arg("--")
            .arg("sh")
            .arg("-lc")
            .arg(format!(
                "su - {} -c {}",
                shell_quote(&options.user),
                shell_quote(&format!(
                    "mkdir -p \"$(dirname {0})\" && (test -f {0} || ssh-keygen -t ed25519 -f {0} -N '' -C labby-incus-{1})",
                    shell_quote(&options.key_path),
                    shell_quote(&options.container)
                ))
            )),
        "incus_ssh_keygen_failed",
        Duration::from_secs(options.timeout_seconds),
    )?;

    let public_key_output = command_output_with_timeout(
        Command::new("incus")
            .arg("exec")
            .arg(&options.container)
            .arg("--")
            .arg("su")
            .arg("-")
            .arg(&options.user)
            .arg("-c")
            .arg(format!(
                "cat {}",
                shell_quote(&format!("{}.pub", options.key_path))
            )),
        "incus_ssh_public_key_read_failed",
        Duration::from_secs(options.timeout_seconds),
    )?;
    if !public_key_output.status.success() {
        return Err(ToolError::Sdk {
            message: format!(
                "failed to read container SSH public key: {}{}",
                String::from_utf8_lossy(&public_key_output.stderr),
                if public_key_output.stderr_truncated {
                    " [stderr tail truncated]"
                } else {
                    ""
                }
            ),
            sdk_kind: "incus_ssh_public_key_read_failed".into(),
        });
    }
    if public_key_output.stdout_truncated {
        return Err(ToolError::Sdk {
            message: "container SSH public key output exceeded the capture budget".into(),
            sdk_kind: "incus_ssh_public_key_read_failed".into(),
        });
    }
    let public_key = String::from_utf8_lossy(&public_key_output.stdout)
        .trim()
        .to_string();
    if public_key.is_empty() {
        return Err(ToolError::Sdk {
            message: "container SSH public key was empty".into(),
            sdk_kind: "incus_ssh_public_key_read_failed".into(),
        });
    }
    let public_key_for_jobs = public_key.clone();
    let ssh_config = options.ssh_config.clone();
    let results = run_bounded_target_jobs(
        &outcome.targets,
        INCUS_TARGET_CONCURRENCY,
        Duration::from_secs(options.timeout_seconds),
        options.fail_fast,
        IncusTargetJob::Authorize {
            public_key: public_key_for_jobs,
            ssh_config,
        },
    );
    for (target, result) in outcome.targets.iter().zip(results) {
        match result {
            Ok(destination) => outcome.authorized.push(destination),
            Err(err) if options.fail_fast => return Err(err),
            Err(err) => {
                outcome.failed.push(IncusSshFailure {
                    target: target_ssh_destination(target),
                    error: err.to_string(),
                });
            }
        }
    }
    if options.install_config {
        install_container_ssh_config(options, &outcome.targets)?;
        outcome.config_installed = true;
    }
    Ok(outcome)
}

pub(crate) fn incus_ssh_verify(
    options: &IncusSshBootstrapOptions,
) -> Result<IncusSshVerifyOutcome, ToolError> {
    let plan = incus_ssh_bootstrap_plan(options)?;
    if options.install_config {
        install_container_ssh_config(options, &plan.targets)?;
    }
    let mut outcome = IncusSshVerifyOutcome {
        container: plan.container,
        user: plan.user,
        key_path: plan.key_path,
        targets: plan.targets,
        verified: Vec::new(),
        failed: Vec::new(),
        skipped_github: plan.skipped_github,
        skipped_wildcard: plan.skipped_wildcard,
        skipped_unsafe: plan.skipped_unsafe,
        unsupported_include: plan.unsupported_include,
        skipped_excluded: plan.skipped_excluded,
        skipped_not_included: plan.skipped_not_included,
    };
    let results = run_bounded_target_jobs(
        &outcome.targets,
        INCUS_TARGET_CONCURRENCY,
        Duration::from_secs(options.timeout_seconds),
        options.fail_fast,
        IncusTargetJob::Verify {
            options: options.clone(),
        },
    );
    for (target, result) in outcome.targets.iter().zip(results) {
        match result {
            Ok(destination) => outcome.verified.push(destination),
            Err(err) if options.fail_fast => return Err(err),
            Err(err) => outcome.failed.push(IncusSshFailure {
                target: target_ssh_destination(target),
                error: err.to_string(),
            }),
        }
    }
    Ok(outcome)
}

pub(crate) fn parse_backup_config(path: &Path) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ToolError::Sdk {
        message: format!("failed to read Incus backup config {}: {e}", path.display()),
        sdk_kind: "incus_backup_config_read_failed".into(),
    })?;
    parse_backup_config_str(&raw)
}

pub(crate) fn parse_backup_config_str(raw: &str) -> Result<Vec<BackupConfigEntry>, ToolError> {
    let doc: IncusConfigDocument =
        super::constrained_yaml::parse(raw).map_err(|e| ToolError::Sdk {
            message: format!("invalid Incus backup YAML: {e}"),
            sdk_kind: "incus_backup_config_invalid_yaml".into(),
        })?;

    let mut entries = Vec::new();
    for (key, value) in doc.config {
        validate_backup_key(&key)?;
        entries.push(BackupConfigEntry {
            key,
            value: scalar_to_string(value)?,
        });
    }
    if entries.is_empty() {
        return Err(ToolError::Sdk {
            message: "Incus backup config must contain at least one supported config key".into(),
            sdk_kind: "incus_backup_config_empty".into(),
        });
    }
    Ok(entries)
}

pub(crate) fn apply_backup_config(
    container: &str,
    path: &Path,
    dry_run: bool,
) -> Result<BackupConfigApplyOutcome, ToolError> {
    if container.trim().is_empty() {
        return Err(ToolError::MissingParam {
            message: "missing required parameter `container`".into(),
            param: "container".into(),
        });
    }
    let entries = parse_backup_config(path)?;
    if !dry_run {
        for entry in &entries {
            let status = Command::new("incus")
                .arg("config")
                .arg("set")
                .arg(container)
                .arg(&entry.key)
                .arg(&entry.value)
                .bounded_status()
                .map_err(|e| ToolError::Sdk {
                    message: format!("failed to run incus config set: {e}"),
                    sdk_kind: "incus_config_set_failed".into(),
                })?;
            if !status.success() {
                return Err(ToolError::Sdk {
                    message: format!(
                        "incus config set failed for {} on container {}",
                        entry.key, container
                    ),
                    sdk_kind: "incus_config_set_failed".into(),
                });
            }
        }
    }
    Ok(BackupConfigApplyOutcome {
        container: container.to_string(),
        dry_run,
        applied: entries,
    })
}

pub(crate) fn materialize_bootstrap_artifacts(
    root: &Path,
) -> Result<IncusBootstrapArtifacts, ToolError> {
    let scripts_dir = root.join("scripts");
    let config_dir = root.join("config").join("incus");
    std::fs::create_dir_all(&scripts_dir).map_err(|e| ToolError::Sdk {
        message: format!("failed to create {}: {e}", scripts_dir.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    std::fs::create_dir_all(&config_dir).map_err(|e| ToolError::Sdk {
        message: format!("failed to create {}: {e}", config_dir.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;

    let bootstrap_script = scripts_dir.join("incus-bootstrap.sh");
    let install_script = scripts_dir.join("install.sh");
    let profile_file = config_dir.join("labby-gateway-profile.yaml");
    let backup_config_file = config_dir.join("labby-backup.yaml");

    write_materialized_file(&bootstrap_script, INCUS_BOOTSTRAP_SCRIPT, 0o755)?;
    write_materialized_file(&install_script, INSTALL_SCRIPT, 0o755)?;
    write_materialized_file(&profile_file, GATEWAY_PROFILE_YAML, 0o644)?;
    write_materialized_file(&backup_config_file, BACKUP_CONFIG_YAML, 0o644)?;

    Ok(IncusBootstrapArtifacts {
        root: root.to_path_buf(),
        bootstrap_script,
        install_script,
        profile_file,
        backup_config_file,
    })
}

pub(crate) fn bootstrap_command(
    artifacts: &IncusBootstrapArtifacts,
    options: &IncusBootstrapOptions,
) -> Result<IncusBootstrapCommand, ToolError> {
    let mut args = vec![artifacts.bootstrap_script.as_os_str().to_os_string()];
    if options.no_backup_config && options.backup_config.is_some() {
        return Err(ToolError::Sdk {
            message: "--backup-config cannot be combined with --no-backup-config".into(),
            sdk_kind: "incus_bootstrap_invalid_options".into(),
        });
    }
    push_option(&mut args, "--name", options.name.as_deref());
    push_option(&mut args, "--image", options.image.as_deref());
    push_option(&mut args, "--profile-name", options.profile_name.as_deref());
    push_path_option(&mut args, "--profile-file", &artifacts.profile_file);
    if options.no_backup_config {
        push_flag(&mut args, "--no-backup-config", true);
    } else {
        let backup_config = options
            .backup_config
            .clone()
            .or_else(backup_config_from_env)
            .as_ref()
            .map(|path| absolutize_user_path(path))
            .transpose()?
            .unwrap_or_else(|| artifacts.backup_config_file.clone());
        push_path_option(&mut args, "--backup-config", &backup_config);
    }
    push_option(
        &mut args,
        "--runtime-profile-name",
        options.runtime_profile_name.as_deref(),
    );
    push_option(
        &mut args,
        "--storage-driver",
        options.storage_driver.as_deref(),
    );
    push_option(&mut args, "--storage-pool", options.storage_pool.as_deref());
    push_option(
        &mut args,
        "--storage-source",
        options.storage_source.as_deref(),
    );
    push_option(&mut args, "--version", options.version.as_deref());
    if let Some(local_binary) = &options.local_binary {
        push_path_option(
            &mut args,
            "--local-binary",
            &absolutize_user_path(local_binary)?,
        );
    }
    push_flag(&mut args, "--skip-install", options.skip_install);
    push_flag(&mut args, "--dry-run", options.dry_run);
    push_flag(&mut args, "--tailscale-ssh", options.tailscale_ssh);
    if let Some(hostname) = options
        .tailscale_hostname
        .as_deref()
        .or(options.name.as_deref())
    {
        push_option(&mut args, "--tailscale-hostname", Some(hostname));
    }
    push_flag(
        &mut args,
        "--allow-source-fallback",
        options.allow_source_fallback,
    );

    Ok(IncusBootstrapCommand {
        program: OsString::from("sh"),
        args,
        current_dir: artifacts.root.clone(),
    })
}

pub(crate) fn run_incus_bootstrap(options: IncusBootstrapOptions) -> Result<(), ToolError> {
    let tempdir = tempfile::tempdir().map_err(|e| ToolError::Sdk {
        message: format!("failed to create Incus bootstrap tempdir: {e}"),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    let artifacts = materialize_bootstrap_artifacts(tempdir.path())?;
    let command = bootstrap_command(&artifacts, &options)?;
    let status = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.current_dir)
        .bounded_status()
        .map_err(|e| ToolError::Sdk {
            message: format!("failed to run Incus bootstrap: {e}"),
            sdk_kind: "incus_bootstrap_failed".into(),
        })?;
    if !status.success() {
        return Err(ToolError::Sdk {
            message: format!("Incus bootstrap failed with status {status}"),
            sdk_kind: "incus_bootstrap_failed".into(),
        });
    }
    Ok(())
}

pub(crate) fn sync_incus_binary(options: IncusSyncOptions) -> Result<IncusSyncOutcome, ToolError> {
    let container = resolve_sync_container(options.container.as_deref())?;
    if options.rollback {
        if options.dry_run {
            return Ok(IncusSyncOutcome {
                container,
                binary: PathBuf::from("<previous-release>"),
                web_assets_dir: None,
                remote_web_assets_dir: Some(REMOTE_WEB_ASSETS_DIR.into()),
                dry_run: true,
                fallback_restart_used: false,
                old_pid: None,
                new_pid: None,
                local_sha256: None,
                remote_sha256: None,
                local_version: None,
                remote_version: None,
                local_web_index_sha256: None,
                served_web_index_sha256: None,
                ready: false,
                check_url: options.check_url,
                check_url_ok: None,
                steps: vec!["restore the retained previous Incus release transactionally".into()],
            });
        }
        incus_exec(
            &container,
            &[
                "sh",
                "-lc",
                &remote_release_rollback_script(PREVIOUS_RELEASE_DIR),
            ],
        )?;
        let (remote_version, new_pid, remote_sha256) = verify_explicit_rollback(&container)?;
        return Ok(IncusSyncOutcome {
            container,
            binary: PathBuf::from("<previous-release>"),
            web_assets_dir: None,
            remote_web_assets_dir: Some(REMOTE_WEB_ASSETS_DIR.into()),
            dry_run: false,
            fallback_restart_used: false,
            old_pid: None,
            new_pid: Some(new_pid),
            local_sha256: None,
            remote_sha256: Some(remote_sha256),
            local_version: None,
            remote_version: Some(remote_version),
            local_web_index_sha256: None,
            served_web_index_sha256: None,
            ready: true,
            check_url: options.check_url,
            check_url_ok: None,
            steps: vec!["restored and verified the retained previous Incus release".into()],
        });
    }
    let binary = resolve_sync_binary(options.binary.as_deref())?;
    let web_assets_dir = if options.sync_web_assets {
        resolve_sync_web_assets_dir(options.web_assets_dir.as_deref())?
    } else {
        None
    };
    let local_sha256 = if options.dry_run {
        None
    } else {
        Some(file_sha256(&binary)?)
    };
    let local_web_index_sha256 = if options.dry_run {
        None
    } else {
        web_assets_dir
            .as_ref()
            .map(|dir| file_sha256(&dir.join("index.html")))
            .transpose()?
    };
    let local_version = Some(require_version_output(
        command_stdout(
            Command::new(&binary).arg("--version").bounded_output(),
            "incus_sync_local_version_failed",
            "failed to read local labby version",
        ),
        "incus_sync_local_version_failed",
        "local labby --version",
    )?);
    let mut steps = Vec::new();
    let mut fallback_restart_used = false;

    if options.dry_run {
        steps.push(format!("resolve container `{container}`"));
        steps.push(format!("resolve binary `{}`", binary.display()));
        steps.push(format!("stop {SERVICE_NAME}"));
        steps.push(format!(
            "push binary and atomically install to {REMOTE_BINARY_PATH}"
        ));
        if let Some(path) = &web_assets_dir {
            steps.push(format!(
                "sync web assets `{}` to {REMOTE_WEB_ASSETS_DIR}",
                path.display()
            ));
        } else if options.sync_web_assets {
            steps.push(format!(
                "clear {REMOTE_WEB_ASSETS_DIR} so embedded web assets are used"
            ));
        }
        steps.push(format!("start {SERVICE_NAME} and verify {READY_URL}"));
        if let Some(url) = &options.check_url {
            steps.push(format!("check {url}"));
        }
        return Ok(IncusSyncOutcome {
            container,
            binary,
            web_assets_dir,
            remote_web_assets_dir: options
                .sync_web_assets
                .then(|| REMOTE_WEB_ASSETS_DIR.to_string()),
            dry_run: true,
            fallback_restart_used,
            old_pid: None,
            new_pid: None,
            local_sha256,
            remote_sha256: None,
            local_version,
            remote_version: None,
            local_web_index_sha256,
            served_web_index_sha256: None,
            ready: false,
            check_url: options.check_url,
            check_url_ok: None,
            steps,
        });
    }

    ensure_container_running(&container)?;
    let deployment_dir = PREVIOUS_RELEASE_DIR.to_string();
    incus_exec(
        &container,
        &["sh", "-lc", &remote_release_backup_script(&deployment_dir)],
    )?;
    let mut activation = IncusActivationGuard::new(container.clone(), deployment_dir.clone());
    macro_rules! activate {
        ($operation:expr) => {
            match $operation {
                Ok(value) => value,
                Err(error) => return Err(activation.failure(error)),
            }
        };
    }
    steps.push(format!(
        "prepared prior-release manifest at {deployment_dir}/manifest.env"
    ));
    let old_pid = activate!(service_main_pid(&container));
    steps.push(format!(
        "old {SERVICE_NAME} MainPID: {}",
        old_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "none".to_string())
    ));

    if let Err(err) = incus_exec(&container, &["systemctl", "stop", SERVICE_NAME]) {
        steps.push(format!("systemctl stop failed: {}", err));
        if options.force_fallback {
            activate!(force_restart_container(&container));
            fallback_restart_used = true;
        } else {
            return Err(activation.failure(err));
        }
    } else {
        steps.push(format!("stopped {SERVICE_NAME}"));
    }
    if let Some(pid) = old_pid {
        activate!(wait_pid_gone(&container, pid, Duration::from_secs(20)));
        steps.push(format!("old MainPID {pid} exited"));
    }
    if let Err(err) = reap_lingering_service_processes(&container) {
        steps.push(format!("lingering service reaper failed: {err}"));
        if options.force_fallback {
            activate!(force_restart_container(&container));
            fallback_restart_used = true;
            activate!(ensure_container_running(&container));
            drop(incus_exec(&container, &["systemctl", "stop", SERVICE_NAME]));
            activate!(reap_lingering_service_processes(&container));
            steps.push("used Incus force restart fallback".to_string());
        } else {
            return Err(activation.failure(err));
        }
    }
    steps.push(format!("reaped lingering {SERVICE_NAME} processes"));

    let remote_tmp = format!("/tmp/.labby-sync-{}", std::process::id());
    let target = format!("{container}{remote_tmp}");
    activate!(command_ok(
        Command::new("incus")
            .arg("file")
            .arg("push")
            .arg(&binary)
            .arg(&target)
            .bounded_output(),
        "incus_sync_push_failed",
        "failed to push labby binary into Incus container",
    ));
    steps.push(format!("pushed `{}` to `{remote_tmp}`", binary.display()));
    activate!(incus_exec(
        &container,
        &[
            "sh",
            "-lc",
            &format!(
                "set -eu; install -m 0755 {remote_tmp} {REMOTE_BINARY_PATH}.new; mv -f {REMOTE_BINARY_PATH}.new {REMOTE_BINARY_PATH}; rm -f {remote_tmp}"
            ),
        ],
    ));
    steps.push(format!("installed {REMOTE_BINARY_PATH} atomically"));
    activate!(incus_sync_checkpoint("binary"));

    if let Some(assets_dir) = &web_assets_dir {
        activate!(sync_web_assets_to_container(&container, assets_dir));
        steps.push(format!(
            "synced web assets `{}` to `{REMOTE_WEB_ASSETS_DIR}`",
            assets_dir.display()
        ));
    } else if options.sync_web_assets {
        activate!(clear_remote_web_assets(&container));
        steps.push(format!(
            "cleared `{REMOTE_WEB_ASSETS_DIR}` so embedded web assets are used"
        ));
    }
    activate!(incus_sync_checkpoint("assets"));

    drop(incus_exec(
        &container,
        &["systemctl", "reset-failed", SERVICE_NAME],
    ));
    if let Err(err) = incus_exec(&container, &["systemctl", "start", SERVICE_NAME]) {
        steps.push(format!("systemctl start failed: {}", err));
        if options.force_fallback {
            activate!(force_restart_container(&container));
            fallback_restart_used = true;
        } else {
            return Err(activation.failure(err));
        }
    } else {
        steps.push(format!("started {SERVICE_NAME}"));
    }
    activate!(incus_sync_checkpoint("service-start"));

    let new_pid = activate!(wait_service_pid(
        &container,
        old_pid,
        Duration::from_secs(30)
    ));
    steps.push(format!("new {SERVICE_NAME} MainPID: {new_pid}"));
    activate!(wait_ready(&container, Duration::from_secs(30)));
    activate!(incus_sync_checkpoint("readiness"));
    steps.push(format!("verified {READY_URL}"));

    let served_web_index_sha256 = if let Some(expected) = local_web_index_sha256.as_deref() {
        let actual = activate!(remote_web_index_sha256(&container));
        activate!(verify_web_index_hash(expected, &actual));
        steps.push("verified served web index hash".to_string());
        Some(actual)
    } else {
        None
    };

    let remote_sha256 = Some(activate!(remote_sha256(&container)));
    if local_sha256 != remote_sha256 {
        return Err(activation.failure(ToolError::Sdk {
            message: "remote labby binary hash does not match local binary after sync".into(),
            sdk_kind: "incus_sync_hash_mismatch".into(),
        }));
    }
    steps.push("verified remote binary hash".to_string());

    let remote_version = Some(activate!(require_version_output(
        incus_exec_stdout(&container, &[REMOTE_BINARY_PATH, "--version"]),
        "incus_sync_remote_version_failed",
        "deployed labby --version",
    )));
    if local_version != remote_version {
        return Err(activation.failure(ToolError::Sdk {
            message: "remote labby version does not match local binary after sync".into(),
            sdk_kind: "incus_sync_version_mismatch".into(),
        }));
    }
    steps.push("verified remote binary version".to_string());

    let check_url_ok = if let Some(url) = &options.check_url {
        activate!(curl_check_url(url));
        steps.push(format!("verified {url}"));
        Some(true)
    } else {
        None
    };

    if let Err(error) = activation.commit() {
        return Err(activation.failure(error));
    }
    Ok(IncusSyncOutcome {
        container,
        binary,
        web_assets_dir,
        remote_web_assets_dir: options
            .sync_web_assets
            .then(|| REMOTE_WEB_ASSETS_DIR.to_string()),
        dry_run: false,
        fallback_restart_used,
        old_pid,
        new_pid: Some(new_pid),
        local_sha256,
        remote_sha256,
        local_version,
        remote_version,
        local_web_index_sha256,
        served_web_index_sha256,
        ready: true,
        check_url: options.check_url,
        check_url_ok,
        steps,
    })
}

fn incus_sync_checkpoint(label: &str) -> Result<(), ToolError> {
    if std::env::var("LABBY_INCUS_SYNC_FAIL_AFTER").as_deref() == Ok(label) {
        Err(ToolError::Sdk {
            sdk_kind: "incus_sync_injected_failure".into(),
            message: format!("injected Incus sync failure after {label}"),
        })
    } else {
        Ok(())
    }
}

struct IncusActivationGuard {
    container: String,
    deployment_dir: String,
    armed: bool,
}

impl IncusActivationGuard {
    fn new(container: String, deployment_dir: String) -> Self {
        Self {
            container,
            deployment_dir,
            armed: true,
        }
    }

    fn rollback(&mut self) -> Result<(), ToolError> {
        if !self.armed {
            return Ok(());
        }
        let script = remote_release_rollback_script(&self.deployment_dir);
        let result = incus_exec(&self.container, &["sh", "-lc", &script]);
        if result.is_ok() {
            self.armed = false;
        }
        result
    }

    fn failure(&mut self, primary: ToolError) -> ToolError {
        let message = primary.to_string();
        match self.rollback() {
            Ok(()) => deployment_failure(&message, Ok(())),
            Err(rollback) => deployment_failure(&message, Err(rollback.user_message())),
        }
    }

    fn commit(&mut self) -> Result<(), ToolError> {
        // Keep one verified prior release so an explicit production rollback
        // uses the same transactional restore path as activation failures.
        self.armed = false;
        Ok(())
    }
}

impl Drop for IncusActivationGuard {
    fn drop(&mut self) {
        if self.armed {
            drop(self.rollback());
        }
    }
}

fn remote_release_backup_script(deployment_dir: &str) -> String {
    format!(
        "set -eu; d='{deployment_dir}'; rm -rf \"$d\"; install -d -m 0700 \"$d\"; \
         binary_present=0; assets_present=0; \
         if test -e {REMOTE_BINARY_PATH}; then cp -a {REMOTE_BINARY_PATH} \"$d/labby\"; binary_present=1; fi; \
         if test -e {REMOTE_WEB_ASSETS_DIR}; then cp -a {REMOTE_WEB_ASSETS_DIR} \"$d/web\"; assets_present=1; fi; \
         state=$(systemctl show {SERVICE_NAME} --property=ActiveState,UnitFileState --no-pager); \
         active_state=$(printf '%s\\n' \"$state\" | sed -n 's/^ActiveState=//p'); unit_file_state=$(printf '%s\\n' \"$state\" | sed -n 's/^UnitFileState=//p'); \
         case \"$active_state\" in active|inactive|failed) ;; *) printf 'unsupported or missing ActiveState: %s\\n' \"$active_state\" >&2; exit 70 ;; esac; \
         case \"$unit_file_state\" in enabled|enabled-runtime|disabled) ;; *) printf 'unsupported or missing UnitFileState: %s\\n' \"$unit_file_state\" >&2; exit 70 ;; esac; \
         printf 'binary_present=%s\\nassets_present=%s\\nactive_state=%s\\nunit_file_state=%s\\n' \"$binary_present\" \"$assets_present\" \"$active_state\" \"$unit_file_state\" >\"$d/manifest.env\"; \
         sync \"$d/manifest.env\""
    )
}

fn remote_release_rollback_script(deployment_dir: &str) -> String {
    remote_release_rollback_script_for(
        deployment_dir,
        REMOTE_BINARY_PATH,
        REMOTE_WEB_ASSETS_DIR,
        SERVICE_NAME,
        READY_URL,
    )
}

fn remote_release_rollback_script_for(
    deployment_dir: &str,
    binary_path: &str,
    assets_dir: &str,
    service_name: &str,
    ready_url: &str,
) -> String {
    format!(
        "set -u; d='{deployment_dir}'; test -f \"$d/manifest.env\" || exit 70; . \"$d/manifest.env\"; failed=''; \
         systemctl stop {service_name} || failed=\"$failed service-stop\"; \
         if test \"$binary_present\" = 1; then install -m 0755 \"$d/labby\" {binary_path} || failed=\"$failed binary\"; else rm -f {binary_path} || failed=\"$failed binary-remove\"; fi; \
         if test \"$assets_present\" = 1; then rm -rf {assets_dir} || failed=\"$failed assets-remove\"; cp -a \"$d/web\" {assets_dir} || failed=\"$failed assets\"; else rm -rf {assets_dir} || failed=\"$failed assets-remove\"; fi; \
         case \"$unit_file_state\" in enabled) systemctl enable {service_name} || failed=\"$failed service-enable\" ;; enabled-runtime) systemctl disable {service_name} || failed=\"$failed service-disable\"; systemctl enable --runtime {service_name} || failed=\"$failed service-enable-runtime\" ;; disabled) systemctl disable {service_name} || failed=\"$failed service-disable\" ;; *) failed=\"$failed invalid-unit-file-state\" ;; esac; \
         case \"$active_state\" in active) systemctl start {service_name} || failed=\"$failed service-start\"; curl -fsS {ready_url} >/dev/null || failed=\"$failed readiness\" ;; inactive) systemctl stop {service_name} || failed=\"$failed service-stop-final\"; systemctl reset-failed {service_name} || failed=\"$failed service-reset-failed\" ;; failed) systemctl start {service_name} >/dev/null 2>&1 || :; actual=$(systemctl show {service_name} --property=ActiveState --value --no-pager); test \"$actual\" = failed || failed=\"$failed service-failed-state\" ;; *) failed=\"$failed invalid-active-state\" ;; esac; \
         if test -n \"$failed\"; then printf 'rollback residuals:%s\\n' \"$failed\" >&2; exit 70; fi; rm -rf \"$d\""
    )
}

fn deployment_failure(primary: &str, rollback: Result<(), &str>) -> ToolError {
    match rollback {
        Ok(()) => ToolError::Sdk {
            message: format!(
                "Incus activation failed and the prior release was restored: {primary}"
            ),
            sdk_kind: "incus_sync_rolled_back".into(),
        },
        Err(rollback) => ToolError::Sdk {
            message: format!(
                "Incus activation failed: {primary}; restoring the prior release also failed: {rollback}; recovery requires inspecting the retained deployment manifest"
            ),
            sdk_kind: "incus_sync_rollback_failed".into(),
        },
    }
}

fn write_materialized_file(path: &Path, content: &str, mode: u32) -> Result<(), ToolError> {
    std::fs::write(path, content).map_err(|e| ToolError::Sdk {
        message: format!("failed to write {}: {e}", path.display()),
        sdk_kind: "incus_bootstrap_materialize_failed".into(),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| {
            ToolError::Sdk {
                message: format!("failed to chmod {}: {e}", path.display()),
                sdk_kind: "incus_bootstrap_materialize_failed".into(),
            }
        })?;
    }
    let _ = mode;
    Ok(())
}

fn resolve_sync_container(explicit: Option<&str>) -> Result<String, ToolError> {
    if let Some(container) = explicit.filter(|value| !value.trim().is_empty()) {
        return Ok(container.to_string());
    }
    if let Some(container) = std::env::var("LABBY_INCUS_CONTAINER")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(container);
    }

    let raw = command_stdout(
        Command::new("incus")
            .arg("list")
            .arg("--format")
            .arg("csv")
            .arg("-c")
            .arg("ns")
            .bounded_output(),
        "incus_sync_list_failed",
        "failed to list Incus containers",
    )?;
    let containers: Vec<(String, String)> = raw
        .lines()
        .filter_map(|line| {
            let (name, state) = line.split_once(',')?;
            Some((name.trim().to_string(), state.trim().to_string()))
        })
        .collect();

    if containers
        .iter()
        .any(|(name, _)| name == DEFAULT_CONTAINER_NAME)
    {
        return Ok(DEFAULT_CONTAINER_NAME.to_string());
    }

    let labby_running: Vec<_> = containers
        .iter()
        .filter(|(name, state)| name.starts_with("labby-") && state.eq_ignore_ascii_case("RUNNING"))
        .map(|(name, _)| name.clone())
        .collect();
    if labby_running.len() == 1 {
        return Ok(labby_running[0].clone());
    }

    let labby_any: Vec<_> = containers
        .iter()
        .filter(|(name, _)| name.starts_with("labby-"))
        .map(|(name, _)| name.clone())
        .collect();
    if labby_any.len() == 1 {
        return Ok(labby_any[0].clone());
    }

    Err(ToolError::Sdk {
        message: if labby_any.is_empty() {
            "could not discover a Labby Incus container; pass --container or set LABBY_INCUS_CONTAINER".into()
        } else {
            format!(
                "multiple Labby-like Incus containers found ({}); pass --container or set LABBY_INCUS_CONTAINER",
                labby_any.join(", ")
            )
        },
        sdk_kind: "incus_sync_container_discovery_failed".into(),
    })
}

fn resolve_sync_binary(explicit: Option<&Path>) -> Result<PathBuf, ToolError> {
    if let Some(path) = explicit {
        return require_binary(path);
    }
    if let Some(path) = std::env::var_os("LABBY_INCUS_BINARY").filter(|value| !value.is_empty()) {
        return require_binary(Path::new(&path));
    }

    let repo_debug = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join("target").join("debug").join("labby"))
        .filter(|path| path.is_file());
    if let Some(path) = repo_debug {
        return require_binary(&path);
    }

    let exe = std::env::current_exe().map_err(|e| ToolError::Sdk {
        message: format!("failed to resolve current labby executable: {e}"),
        sdk_kind: "incus_sync_binary_resolve_failed".into(),
    })?;
    require_binary(&exe)
}

fn require_binary(path: &Path) -> Result<PathBuf, ToolError> {
    let path = absolutize_user_path(path)?;
    if path.is_file() {
        Ok(path)
    } else {
        Err(ToolError::Sdk {
            message: format!("labby binary does not exist: {}", path.display()),
            sdk_kind: "incus_sync_binary_missing".into(),
        })
    }
}

fn resolve_sync_web_assets_dir(explicit: Option<&Path>) -> Result<Option<PathBuf>, ToolError> {
    let candidate = if let Some(path) = explicit {
        Some(path.to_path_buf())
    } else {
        std::env::var_os("LABBY_INCUS_WEB_ASSETS_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let Some(path) = candidate else {
        return Ok(None);
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| ToolError::Sdk {
                message: format!("failed to resolve current directory: {e}"),
                sdk_kind: "incus_sync_web_assets_resolve_failed".into(),
            })?
            .join(path)
    };
    let index = path.join("index.html");
    if index.is_file() {
        Ok(Some(path))
    } else {
        Err(ToolError::Sdk {
            message: format!(
                "web assets directory {} does not contain index.html",
                path.display()
            ),
            sdk_kind: "incus_sync_web_assets_missing".into(),
        })
    }
}

fn ensure_container_running(container: &str) -> Result<(), ToolError> {
    let state = command_stdout(
        Command::new("incus")
            .arg("list")
            .arg(container)
            .arg("--format")
            .arg("csv")
            .arg("-c")
            .arg("s")
            .bounded_output(),
        "incus_sync_container_state_failed",
        "failed to read Incus container state",
    )?;
    if state.lines().any(|line| line.trim() == "RUNNING") {
        return Ok(());
    }
    command_ok(
        Command::new("incus")
            .arg("start")
            .arg(container)
            .bounded_output(),
        "incus_sync_container_start_failed",
        "failed to start Incus container",
    )
}

fn sync_web_assets_to_container(container: &str, assets_dir: &Path) -> Result<(), ToolError> {
    let archive = std::env::temp_dir().join(format!(
        "labby-web-assets-{}-{}.tar",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let remote_archive = format!("/tmp/.labby-web-assets-{}.tar", std::process::id());
    let result = (|| {
        command_ok(
            Command::new("tar")
                .arg("-C")
                .arg(assets_dir)
                .arg("-cf")
                .arg(&archive)
                .arg(".")
                .bounded_output(),
            "incus_sync_web_assets_archive_failed",
            "failed to archive local web assets",
        )?;
        command_ok(
            Command::new("incus")
                .arg("file")
                .arg("push")
                .arg(&archive)
                .arg(format!("{container}{remote_archive}"))
                .bounded_output(),
            "incus_sync_web_assets_push_failed",
            "failed to push web assets archive into Incus container",
        )?;
        let script = format!(
            "set -eu; \
             rm -rf {remote}.new {remote}.prev; \
             mkdir -p {remote}.new; \
             tar -C {remote}.new -xf {archive}; \
             if [ -d {remote} ]; then mv {remote} {remote}.prev; fi; \
             mv {remote}.new {remote}; \
             rm -f {archive}",
            remote = shell_quote(REMOTE_WEB_ASSETS_DIR),
            archive = shell_quote(&remote_archive),
        );
        incus_exec(container, &["sh", "-lc", &script])
    })();
    drop(std::fs::remove_file(&archive));
    result
}

fn clear_remote_web_assets(container: &str) -> Result<(), ToolError> {
    let script = format!(
        "set -eu; \
         rm -rf {remote}.prev; \
         if [ -d {remote} ]; then mv {remote} {remote}.prev; fi",
        remote = shell_quote(REMOTE_WEB_ASSETS_DIR),
    );
    incus_exec(container, &["sh", "-lc", &script])
}

fn remote_web_index_sha256(container: &str) -> Result<String, ToolError> {
    incus_exec_stdout(
        container,
        &[
            "sh",
            "-lc",
            "curl -fsS http://127.0.0.1:8765/ | sha256sum | awk '{print $1}'",
        ],
    )
    .map(|value| value.trim().to_string())
    .map_err(|error| ToolError::Sdk {
        message: format!("failed to hash the web index served by the Incus runtime: {error}"),
        sdk_kind: "incus_sync_web_assets_hash_failed".into(),
    })
}

fn verify_web_index_hash(expected: &str, actual: &str) -> Result<(), ToolError> {
    if expected == actual {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            message: format!(
                "served web index hash {actual} does not match built export hash {expected}"
            ),
            sdk_kind: "incus_sync_web_assets_hash_mismatch".into(),
        })
    }
}

fn service_main_pid(container: &str) -> Result<Option<u32>, ToolError> {
    service_main_pid_with_timeout(container, DEPLOYMENT_COMMAND_TIMEOUT)
}

fn service_main_pid_with_timeout(
    container: &str,
    timeout: Duration,
) -> Result<Option<u32>, ToolError> {
    let raw = incus_exec_stdout_with_timeout(
        container,
        &[
            "systemctl",
            "show",
            SERVICE_NAME,
            "--property",
            "MainPID",
            "--value",
        ],
        timeout,
    )?;
    parse_service_main_pid(&raw)
}

fn parse_service_main_pid(raw: &str) -> Result<Option<u32>, ToolError> {
    let pid = raw.trim().parse::<u32>().map_err(|error| ToolError::Sdk {
        message: format!("systemctl returned an invalid {SERVICE_NAME} MainPID: {error}"),
        sdk_kind: "incus_sync_main_pid_invalid".into(),
    })?;
    Ok((pid > 0).then_some(pid))
}

fn wait_pid_gone(container: &str, pid: u32, timeout: Duration) -> Result<(), ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        let probe_timeout = remaining.min(Duration::from_secs(2));
        let state = incus_exec_stdout_with_timeout(
            container,
            &[
                "sh",
                "-lc",
                &format!("if [ -d /proc/{pid} ]; then printf alive; else printf gone; fi"),
            ],
            probe_timeout,
        )?;
        match state.trim() {
            "gone" => return Ok(()),
            "alive" => {}
            value => {
                return Err(ToolError::Sdk {
                    message: format!(
                        "old {SERVICE_NAME} MainPID probe returned invalid state: {value}"
                    ),
                    sdk_kind: "incus_sync_old_pid_probe_invalid".into(),
                });
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for old {SERVICE_NAME} MainPID {pid} to exit"),
        sdk_kind: "incus_sync_old_pid_timeout".into(),
    })
}

fn wait_service_pid(
    container: &str,
    old_pid: Option<u32>,
    timeout: Duration,
) -> Result<u32, ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        if let Some(pid) =
            service_main_pid_with_timeout(container, remaining.min(Duration::from_secs(2)))?
        {
            if Some(pid) != old_pid {
                return Ok(pid);
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for {SERVICE_NAME} to start with a new MainPID"),
        sdk_kind: "incus_sync_new_pid_timeout".into(),
    })
}

fn wait_ready(container: &str, timeout: Duration) -> Result<(), ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        let state = incus_exec_stdout_with_timeout(
            container,
            &[
                "sh",
                "-lc",
                &format!(
                    "if curl -fsS {} >/dev/null; then printf ready; else printf not-ready; fi",
                    shell_quote(READY_URL)
                ),
            ],
            remaining.min(Duration::from_secs(2)),
        )?;
        match state.trim() {
            "ready" => return Ok(()),
            "not-ready" => {}
            value => {
                return Err(ToolError::Sdk {
                    message: format!("readiness probe returned invalid state: {value}"),
                    sdk_kind: "incus_sync_ready_probe_invalid".into(),
                });
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for {READY_URL} inside Incus container"),
        sdk_kind: "incus_sync_ready_timeout".into(),
    })
}

fn reap_lingering_service_processes(container: &str) -> Result<(), ToolError> {
    incus_exec(
        container,
        &[
            "sh",
            "-lc",
            &format!(
                "systemctl kill {SERVICE_NAME} --kill-who=all --signal=SIGTERM >/dev/null 2>&1 || true"
            ),
        ],
    )?;
    if wait_no_labby_serve(container, Duration::from_secs(5)).is_ok() {
        return Ok(());
    }
    incus_exec(
        container,
        &[
            "sh",
            "-lc",
            &format!(
                "systemctl kill {SERVICE_NAME} --kill-who=all --signal=SIGKILL >/dev/null 2>&1 || true; pkill -KILL -f '^/usr/local/bin/labby serve' >/dev/null 2>&1 || true"
            ),
        ],
    )?;
    wait_no_labby_serve(container, Duration::from_secs(5))
}

fn wait_no_labby_serve(container: &str, timeout: Duration) -> Result<(), ToolError> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let remaining = timeout.saturating_sub(start.elapsed());
        let state = incus_exec_stdout_with_timeout(
            container,
            &[
                "sh",
                "-lc",
                "if pgrep -f '^/usr/local/bin/labby serve' >/dev/null; then printf alive; elif [ $? -eq 1 ]; then printf gone; else exit 2; fi",
            ],
            remaining.min(Duration::from_secs(2)),
        )?;
        match state.trim() {
            "gone" => return Ok(()),
            "alive" => {}
            value => {
                return Err(ToolError::Sdk {
                    message: format!("lingering-process probe returned invalid state: {value}"),
                    sdk_kind: "incus_sync_lingering_process_probe_invalid".into(),
                });
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(ToolError::Sdk {
        message: format!("timed out waiting for lingering {SERVICE_NAME} processes to exit"),
        sdk_kind: "incus_sync_lingering_process_timeout".into(),
    })
}

fn force_restart_container(container: &str) -> Result<(), ToolError> {
    command_ok(
        Command::new("incus")
            .arg("stop")
            .arg(container)
            .arg("--force")
            .bounded_output(),
        "incus_sync_force_stop_failed",
        "failed to force stop Incus container",
    )?;
    command_ok(
        Command::new("incus")
            .arg("start")
            .arg(container)
            .bounded_output(),
        "incus_sync_force_start_failed",
        "failed to start Incus container after force stop",
    )
}

fn remote_sha256(container: &str) -> Result<String, ToolError> {
    let raw = incus_exec_stdout(
        container,
        &[
            "sh",
            "-lc",
            &format!("sha256sum {REMOTE_BINARY_PATH} | awk '{{print $1}}'"),
        ],
    )?;
    Ok(raw.trim().to_string())
}

fn require_version_output(
    output: Result<String, ToolError>,
    kind: &'static str,
    probe: &'static str,
) -> Result<String, ToolError> {
    let value = output.map_err(|error| ToolError::Sdk {
        message: format!("{probe} failed: {}", error.user_message()),
        sdk_kind: kind.into(),
    })?;
    let value = value.trim();
    if value.is_empty() {
        return Err(ToolError::Sdk {
            message: format!("{probe} returned empty output"),
            sdk_kind: kind.into(),
        });
    }
    Ok(value.to_string())
}

fn verify_explicit_rollback(container: &str) -> Result<(String, u32, String), ToolError> {
    verify_explicit_rollback_with(
        || incus_exec_stdout(container, &[REMOTE_BINARY_PATH, "--version"]),
        || service_main_pid(container),
        || remote_sha256(container),
        || wait_ready(container, Duration::from_secs(30)),
    )
}

fn verify_explicit_rollback_with<V, P, H, R>(
    version: V,
    pid: P,
    sha256: H,
    readiness: R,
) -> Result<(String, u32, String), ToolError>
where
    V: FnOnce() -> Result<String, ToolError>,
    P: FnOnce() -> Result<Option<u32>, ToolError>,
    H: FnOnce() -> Result<String, ToolError>,
    R: FnOnce() -> Result<(), ToolError>,
{
    let version = require_version_output(
        version(),
        "incus_sync_rollback_version_failed",
        "restored labby --version",
    )?;
    let pid = pid()
        .map_err(|error| ToolError::Sdk {
            message: format!(
                "failed to verify restored labby MainPID: {}",
                error.user_message()
            ),
            sdk_kind: "incus_sync_rollback_pid_failed".into(),
        })?
        .ok_or_else(|| ToolError::Sdk {
            message: "restored labby service has no running MainPID".into(),
            sdk_kind: "incus_sync_rollback_pid_failed".into(),
        })?;
    let sha256 = sha256().map_err(|error| ToolError::Sdk {
        message: format!(
            "failed to verify restored labby binary hash: {}",
            error.user_message()
        ),
        sdk_kind: "incus_sync_rollback_hash_failed".into(),
    })?;
    if sha256.trim().is_empty() {
        return Err(ToolError::Sdk {
            message: "restored labby binary hash was empty".into(),
            sdk_kind: "incus_sync_rollback_hash_failed".into(),
        });
    }
    readiness().map_err(|error| ToolError::Sdk {
        message: format!(
            "failed to verify restored labby readiness: {}",
            error.user_message()
        ),
        sdk_kind: "incus_sync_rollback_readiness_failed".into(),
    })?;
    Ok((version, pid, sha256.trim().to_string()))
}

fn file_sha256(path: &Path) -> Result<String, ToolError> {
    let mut file = File::open(path).map_err(|e| ToolError::Sdk {
        message: format!("failed to open {}: {e}", path.display()),
        sdk_kind: "incus_sync_hash_failed".into(),
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 8192];
    loop {
        let read = file.read(&mut buf).map_err(|e| ToolError::Sdk {
            message: format!("failed to read {}: {e}", path.display()),
            sdk_kind: "incus_sync_hash_failed".into(),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex_bytes(&hasher.finalize()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn curl_check_url(url: &str) -> Result<(), ToolError> {
    command_ok(
        Command::new("curl").arg("-fsS").arg(url).bounded_output(),
        "incus_sync_check_url_failed",
        "failed optional sync check URL",
    )
}

fn incus_exec(container: &str, args: &[&str]) -> Result<(), ToolError> {
    let mut command = Command::new("incus");
    command.arg("exec").arg(container).arg("--").args(args);
    command_ok(
        command.bounded_output(),
        "incus_sync_exec_failed",
        "failed to run command inside Incus container",
    )
}

fn incus_exec_stdout(container: &str, args: &[&str]) -> Result<String, ToolError> {
    incus_exec_stdout_with_timeout(container, args, DEPLOYMENT_COMMAND_TIMEOUT)
}

fn incus_exec_stdout_with_timeout(
    container: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, ToolError> {
    let mut command = Command::new("incus");
    command.arg("exec").arg(container).arg("--").args(args);
    command_stdout(
        command_output_with_timeout(&mut command, "incus_command_failed", timeout),
        "incus_sync_exec_failed",
        "failed to run command inside Incus container",
    )
}

const DEPLOYMENT_COMMAND_TIMEOUT: Duration = Duration::from_mins(2);

trait BoundedCommandExt {
    fn bounded_output(&mut self) -> Result<BoundedCommandOutput, ToolError>;
    fn bounded_status(&mut self) -> std::io::Result<std::process::ExitStatus>;
}

impl BoundedCommandExt for Command {
    fn bounded_output(&mut self) -> Result<BoundedCommandOutput, ToolError> {
        command_output_with_timeout(self, "incus_command_failed", DEPLOYMENT_COMMAND_TIMEOUT)
    }

    fn bounded_status(&mut self) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            self.process_group(0);
        }
        let mut child = self.spawn()?;
        let mut tree_guard = command_tree_guard(&mut child, "incus_command_failed")
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let result = wait_child_with_timeout(
            &mut child,
            DEPLOYMENT_COMMAND_TIMEOUT,
            "deployment command",
            "incus_command_failed",
        )
        .map_err(|error| std::io::Error::other(error.to_string()));
        if result.is_ok() {
            tree_guard.disarm();
        }
        result
    }
}

fn command_ok(
    output: Result<BoundedCommandOutput, ToolError>,
    sdk_kind: &'static str,
    context: &'static str,
) -> Result<(), ToolError> {
    let output = output.map_err(|e| ToolError::Sdk {
        message: format!("{context}: {e}"),
        sdk_kind: sdk_kind.into(),
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(output, sdk_kind, context))
    }
}

fn command_stdout(
    output: Result<BoundedCommandOutput, ToolError>,
    sdk_kind: &'static str,
    context: &'static str,
) -> Result<String, ToolError> {
    let output = output.map_err(|e| ToolError::Sdk {
        message: format!("{context}: {e}"),
        sdk_kind: sdk_kind.into(),
    })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(command_error(output, sdk_kind, context))
    }
}

fn command_error(
    output: BoundedCommandOutput,
    sdk_kind: &'static str,
    context: &'static str,
) -> ToolError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if !stderr.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    let truncation = match (output.stdout_truncated, output.stderr_truncated) {
        (true, true) => " (stdout and stderr tails truncated)",
        (true, false) => " (stdout tail truncated)",
        (false, true) => " (stderr tail truncated)",
        (false, false) => "",
    };
    ToolError::Sdk {
        message: if detail.is_empty() {
            format!("{context}: command exited with {}", output.status)
        } else {
            format!("{context}: {detail}{truncation}")
        },
        sdk_kind: sdk_kind.into(),
    }
}

fn push_option(args: &mut Vec<OsString>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        args.push(OsString::from(flag));
        args.push(OsString::from(value));
    }
}

fn push_path_option(args: &mut Vec<OsString>, flag: &str, value: &Path) {
    args.push(OsString::from(flag));
    args.push(value.as_os_str().to_os_string());
}

fn push_flag(args: &mut Vec<OsString>, flag: &str, enabled: bool) {
    if enabled {
        args.push(OsString::from(flag));
    }
}

fn backup_config_from_env() -> Option<PathBuf> {
    std::env::var_os("LABBY_INCUS_BACKUP_CONFIG")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn absolutize_user_path(path: &Path) -> Result<PathBuf, ToolError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| ToolError::Sdk {
        message: format!("failed to resolve current directory: {e}"),
        sdk_kind: "incus_bootstrap_path_resolve_failed".into(),
    })?;
    Ok(cwd.join(path))
}

fn validate_backup_key(key: &str) -> Result<(), ToolError> {
    if SUPPORTED_BACKUP_KEYS.contains(&key) {
        return Ok(());
    }
    Err(ToolError::Sdk {
        message: format!("unsupported Incus backup config key: {key}"),
        sdk_kind: "incus_backup_config_unsupported_key".into(),
    })
}

fn scalar_to_string(value: Value) -> Result<String, ToolError> {
    match value {
        Value::String(value) => Ok(value),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null | Value::Sequence(_) | Value::Mapping(_) | Value::Tagged(_) => {
            Err(ToolError::Sdk {
                message: "Incus backup config values must be scalar strings, booleans, or numbers"
                    .into(),
                sdk_kind: "incus_backup_config_non_scalar".into(),
            })
        }
    }
}

fn split_ssh_config_line(line: &str) -> Option<(&str, &str)> {
    if let Some((key, value)) = line.split_once(char::is_whitespace) {
        return Some((key.trim(), value.trim()));
    }
    line.split_once('=')
        .map(|(key, value)| (key.trim(), value.trim()))
}

fn is_wildcard_ssh_config_host(alias: &str) -> bool {
    let alias_lower = alias.to_ascii_lowercase();
    alias.contains('*') || alias.contains('?') || alias_lower == "all"
}

fn is_github_ssh_config_host(alias: &str, host: &str) -> bool {
    let alias_lower = alias.to_ascii_lowercase();
    let host_lower = host.to_ascii_lowercase();
    alias_lower.contains("github") || host_lower.contains("github")
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct FilteredSshTargets {
    targets: Vec<IncusSshTarget>,
    skipped_excluded: Vec<String>,
    skipped_not_included: Vec<String>,
}

fn filter_ssh_targets(
    targets: Vec<IncusSshTarget>,
    include: &[String],
    exclude: &[String],
) -> FilteredSshTargets {
    let mut filtered = FilteredSshTargets::default();
    for target in targets {
        if !include.is_empty()
            && !include
                .iter()
                .any(|filter| ssh_target_matches(&target, filter))
        {
            filtered.skipped_not_included.push(target.alias);
            continue;
        }
        if exclude
            .iter()
            .any(|filter| ssh_target_matches(&target, filter))
        {
            filtered.skipped_excluded.push(target.alias);
            continue;
        }
        filtered.targets.push(target);
    }
    filtered
}

fn ssh_target_matches(target: &IncusSshTarget, filter: &str) -> bool {
    let filter = filter.to_ascii_lowercase();
    let alias = target.alias.to_ascii_lowercase();
    let host = target.host.to_ascii_lowercase();
    alias == filter || host == filter || alias.contains(&filter) || host.contains(&filter)
}

fn target_ssh_destination(target: &IncusSshTarget) -> String {
    let mut dest = String::new();
    if let Some(user) = &target.user {
        dest.push_str(user);
        dest.push('@');
    }
    dest.push_str(&target.host);
    if let Some(port) = target.port {
        dest.push(':');
        dest.push_str(&port.to_string());
    }
    dest
}

fn render_sanitized_ssh_config(targets: &[IncusSshTarget]) -> String {
    let mut out = String::from("# Generated by labby setup incus-ssh. Safe to overwrite.\n");
    for target in targets {
        out.push_str("\nHost ");
        out.push_str(&target.alias);
        out.push('\n');
        out.push_str("  HostName ");
        out.push_str(&target.host);
        out.push('\n');
        if let Some(user) = &target.user {
            out.push_str("  User ");
            out.push_str(user);
            out.push('\n');
        }
        if let Some(port) = target.port {
            out.push_str("  Port ");
            out.push_str(&port.to_string());
            out.push('\n');
        }
        out.push_str("  IdentityFile ~/.ssh/id_ed25519\n");
        out.push_str("  IdentitiesOnly yes\n");
        out.push_str("  BatchMode yes\n");
        out.push_str("  StrictHostKeyChecking accept-new\n");
    }
    out
}

fn install_container_ssh_config(
    options: &IncusSshBootstrapOptions,
    targets: &[IncusSshTarget],
) -> Result<(), ToolError> {
    let config = render_sanitized_ssh_config(targets);
    let mut command = Command::new("incus");
    command
        .arg("exec")
        .arg(&options.container)
        .arg("--")
        .arg("su")
        .arg("-")
        .arg(&options.user)
        .arg("-c")
        .arg("umask 077; mkdir -p ~/.ssh; cat > ~/.ssh/config")
        .stdin(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| ToolError::Sdk {
        message: format!("failed to start container SSH config install: {e}"),
        sdk_kind: "incus_ssh_config_install_failed".into(),
    })?;
    let mut tree_guard = command_tree_guard(&mut child, "incus_ssh_config_install_failed")?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(config.as_bytes())
            .map_err(|e| ToolError::Sdk {
                message: format!("failed to write container SSH config: {e}"),
                sdk_kind: "incus_ssh_config_install_failed".into(),
            })?;
    }
    let status = wait_child_with_timeout(
        &mut child,
        Duration::from_secs(options.timeout_seconds),
        "container SSH config install",
        "incus_ssh_config_install_failed",
    )?;
    tree_guard.disarm();
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            message: format!("container SSH config install exited with status {status}"),
            sdk_kind: "incus_ssh_config_install_failed".into(),
        })
    }
}

fn authorize_target(
    target: &IncusSshTarget,
    public_key: &str,
    ssh_config: &Path,
    timeout_seconds: u64,
    cancellation: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ToolError> {
    let mut command = authorize_target_command(target, ssh_config, timeout_seconds);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ToolError::Sdk {
            message: format!(
                "failed to start ssh for {}: {e}",
                target_ssh_destination(target)
            ),
            sdk_kind: "incus_ssh_authorize_failed".into(),
        })?;
    let mut tree_guard = command_tree_guard(&mut child, "incus_ssh_authorize_failed")?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(public_key.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| ToolError::Sdk {
                message: format!(
                    "failed to send public key to {}: {e}",
                    target_ssh_destination(target)
                ),
                sdk_kind: "incus_ssh_authorize_failed".into(),
            })?;
    }
    let status = wait_child_with_timeout_or_cancel(
        &mut child,
        Duration::from_secs(timeout_seconds),
        &format!("ssh authorization on {}", target_ssh_destination(target)),
        "incus_ssh_authorize_failed",
        cancellation,
    )?;
    tree_guard.disarm();
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            message: format!(
                "ssh authorization failed on {}",
                target_ssh_destination(target)
            ),
            sdk_kind: "incus_ssh_authorize_failed".into(),
        })
    }
}

fn authorize_target_command(
    target: &IncusSshTarget,
    ssh_config: &Path,
    timeout_seconds: u64,
) -> Command {
    let mut command = Command::new("ssh");
    command
        .arg("-F")
        .arg(ssh_config)
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={timeout_seconds}"))
        .arg("--")
        .arg(&target.alias)
        .arg("sh -c 'umask 077; mkdir -p ~/.ssh; touch ~/.ssh/authorized_keys; read key; grep -qxF \"$key\" ~/.ssh/authorized_keys || printf \"%s\\n\" \"$key\" >> ~/.ssh/authorized_keys'");
    command
}

fn verify_container_target(
    options: &IncusSshBootstrapOptions,
    target: &IncusSshTarget,
    cancellation: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ToolError> {
    run_status_with_cancellation(
        Command::new("incus")
            .arg("exec")
            .arg(&options.container)
            .arg("--")
            .arg("su")
            .arg("-")
            .arg(&options.user)
            .arg("-c")
            .arg(format!(
                "ssh -i {} -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o ConnectTimeout={} -- {} true",
                shell_quote(&options.key_path),
                options.timeout_seconds,
                shell_quote(&target.alias)
            )),
        "incus_ssh_verify_failed",
        Duration::from_secs(options.timeout_seconds),
        cancellation,
    )
}

fn run_status(command: &mut Command, kind: &str, timeout: Duration) -> Result<(), ToolError> {
    run_status_with_cancellation(command, kind, timeout, None)
}

fn run_status_with_cancellation(
    command: &mut Command,
    kind: &str,
    timeout: Duration,
    cancellation: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| ToolError::Sdk {
        message: format!("failed to run command: {e}"),
        sdk_kind: kind.into(),
    })?;
    let mut tree_guard = command_tree_guard(&mut child, kind)?;
    let status =
        wait_child_with_timeout_or_cancel(&mut child, timeout, "command", kind, cancellation)?;
    tree_guard.disarm();
    if status.success() {
        Ok(())
    } else {
        Err(ToolError::Sdk {
            message: format!("command exited with status {status}"),
            sdk_kind: kind.into(),
        })
    }
}

fn command_output_with_timeout(
    command: &mut Command,
    kind: &str,
    timeout: Duration,
) -> Result<BoundedCommandOutput, ToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ToolError::Sdk {
            message: format!("failed to run command: {e}"),
            sdk_kind: kind.into(),
        })?;
    let mut tree_guard = command_tree_guard(&mut child, kind)?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_reader = thread::spawn(move || read_output_tail(stdout));
    let stderr_reader = thread::spawn(move || read_output_tail(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| ToolError::Sdk {
            message: format!("failed to poll command: {e}"),
            sdk_kind: kind.into(),
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_command_group(&mut child);
            drop(child.wait());
            drop(stdout_reader.join());
            drop(stderr_reader.join());
            return Err(ToolError::Sdk {
                message: format!("command timed out after {}s", timeout.as_secs_f64()),
                sdk_kind: kind.into(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    };
    tree_guard.disarm();
    let stdout = join_output_reader(stdout_reader, deadline, &mut child, "stdout", kind)?;
    let stderr = join_output_reader(stderr_reader, deadline, &mut child, "stderr", kind)?;
    Ok(BoundedCommandOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    })
}

fn read_output_tail(mut pipe: impl Read) -> std::io::Result<OutputTail> {
    let mut tail = Vec::with_capacity(COMMAND_OUTPUT_TAIL_BYTES);
    let mut chunk = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = pipe.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if tail.len() + read > COMMAND_OUTPUT_TAIL_BYTES {
            let overflow = tail.len() + read - COMMAND_OUTPUT_TAIL_BYTES;
            if overflow >= tail.len() {
                tail.clear();
            } else {
                tail.drain(..overflow);
            }
            truncated = true;
        }
        let start = read.saturating_sub(COMMAND_OUTPUT_TAIL_BYTES);
        tail.extend_from_slice(&chunk[start..read]);
    }
    Ok(OutputTail {
        bytes: tail,
        truncated,
    })
}

fn join_output_reader(
    reader: thread::JoinHandle<std::io::Result<OutputTail>>,
    deadline: Instant,
    child: &mut std::process::Child,
    stream: &str,
    kind: &str,
) -> Result<OutputTail, ToolError> {
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            terminate_command_group(child);
            drop(child.wait());
            drop(reader.join());
            return Err(ToolError::Sdk {
                message: format!("command timed out while draining {stream}"),
                sdk_kind: kind.into(),
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
    reader
        .join()
        .map_err(|_| ToolError::Sdk {
            message: format!("command {stream} reader panicked"),
            sdk_kind: kind.into(),
        })?
        .map_err(|error| ToolError::Sdk {
            message: format!("failed to read command {stream}: {error}"),
            sdk_kind: kind.into(),
        })
}

#[cfg(unix)]
fn terminate_command_group(child: &mut std::process::Child) {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
    drop(child.kill());
}

#[cfg(not(unix))]
fn terminate_command_group(child: &mut std::process::Child) {
    drop(child.kill());
}

#[cfg(windows)]
struct CommandTreeGuard {
    job: Option<labby_winjob::JobObject>,
}

#[cfg(windows)]
impl CommandTreeGuard {
    fn disarm(&mut self) {
        self.job.take();
    }
}

#[cfg(windows)]
fn command_tree_guard(
    child: &mut std::process::Child,
    kind: &str,
) -> Result<CommandTreeGuard, ToolError> {
    labby_winjob::JobObject::assign(child.id())
        .map(|job| CommandTreeGuard { job: Some(job) })
        .map_err(|error| {
            terminate_command_group(child);
            drop(child.wait());
            ToolError::Sdk {
                message: format!("failed to contain command process tree: {error}"),
                sdk_kind: kind.into(),
            }
        })
}

#[cfg(unix)]
struct CommandTreeGuard {
    process_group: i32,
    armed: bool,
}

#[cfg(unix)]
impl CommandTreeGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl Drop for CommandTreeGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        use nix::sys::signal::{Signal, killpg};
        use nix::sys::wait::waitpid;
        use nix::unistd::Pid;
        let pid = Pid::from_raw(self.process_group);
        let _ = killpg(pid, Signal::SIGKILL);
        let _ = waitpid(pid, None);
    }
}

#[cfg(unix)]
fn command_tree_guard(
    child: &mut std::process::Child,
    _kind: &str,
) -> Result<CommandTreeGuard, ToolError> {
    Ok(CommandTreeGuard {
        process_group: i32::try_from(child.id()).unwrap_or(i32::MAX),
        armed: true,
    })
}

#[cfg(not(any(unix, windows)))]
struct CommandTreeGuard;

#[cfg(not(any(unix, windows)))]
impl CommandTreeGuard {
    fn disarm(&mut self) {}
}

#[cfg(not(any(unix, windows)))]
fn command_tree_guard(
    child: &mut std::process::Child,
    kind: &str,
) -> Result<CommandTreeGuard, ToolError> {
    terminate_command_group(child);
    drop(child.wait());
    Err(ToolError::Sdk {
        message: "process-tree containment is unavailable on this platform".into(),
        sdk_kind: kind.into(),
    })
}

fn wait_child_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
    label: &str,
    kind: &str,
) -> Result<std::process::ExitStatus, ToolError> {
    wait_child_with_timeout_or_cancel(child, timeout, label, kind, None)
}

fn wait_child_with_timeout_or_cancel(
    child: &mut std::process::Child,
    timeout: Duration,
    label: &str,
    kind: &str,
    cancellation: Option<&std::sync::atomic::AtomicBool>,
) -> Result<std::process::ExitStatus, ToolError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| ToolError::Sdk {
            message: format!("failed to poll {label}: {e}"),
            sdk_kind: kind.into(),
        })? {
            return Ok(status);
        }
        if cancellation.is_some_and(|token| token.load(std::sync::atomic::Ordering::Acquire)) {
            terminate_command_group(child);
            drop(child.wait());
            return Err(ToolError::Sdk {
                message: format!("{label} cancelled at aggregate deadline"),
                sdk_kind: kind.into(),
            });
        }
        if Instant::now() >= deadline {
            terminate_command_group(child);
            drop(child.wait());
            return Err(ToolError::Sdk {
                message: format!("{label} timed out after {}s", timeout.as_secs()),
                sdk_kind: kind.into(),
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[derive(Clone)]
enum IncusTargetJob {
    Authorize {
        public_key: String,
        ssh_config: PathBuf,
    },
    Verify {
        options: IncusSshBootstrapOptions,
    },
    #[cfg(test)]
    Fixture(TargetJobFixture),
}

#[cfg(test)]
#[derive(Clone)]
enum TargetJobFixture {
    Concurrent {
        active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    },
    FailEveryTen,
    Cooperative {
        started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    },
    HangingProcess,
    FailFirst {
        started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    },
}

impl IncusTargetJob {
    fn execute(
        &self,
        target: &IncusSshTarget,
        remaining: Duration,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<String, ToolError> {
        match self {
            Self::Authorize {
                public_key,
                ssh_config,
            } => {
                authorize_target(
                    target,
                    public_key,
                    ssh_config,
                    remaining.as_secs().max(1),
                    Some(cancelled),
                )?;
                Ok(target_ssh_destination(target))
            }
            Self::Verify { options } => {
                let mut options = options.clone();
                options.timeout_seconds = remaining.as_secs().max(1);
                verify_container_target(&options, target, Some(cancelled))?;
                Ok(target_ssh_destination(target))
            }
            #[cfg(test)]
            Self::Fixture(fixture) => fixture.execute(target, remaining, cancelled),
        }
    }
}

#[cfg(test)]
impl TargetJobFixture {
    fn execute(
        &self,
        target: &IncusSshTarget,
        remaining: Duration,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<String, ToolError> {
        use std::sync::atomic::Ordering;
        let index: usize = target.alias.parse().expect("numeric fixture alias");
        match self {
            Self::Concurrent { active, peak } => {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                thread::sleep(Duration::from_millis((10 - index.min(10)) as u64));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(target.alias.clone())
            }
            Self::FailEveryTen if index.is_multiple_of(10) => Err(ToolError::Sdk {
                message: format!("failed-{index}"),
                sdk_kind: "test_target_failed".into(),
            }),
            Self::FailEveryTen => Ok(target.alias.clone()),
            Self::Cooperative { started } => {
                started.fetch_add(1, Ordering::SeqCst);
                while !cancelled.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(target.alias.clone())
            }
            Self::HangingProcess => {
                let mut command = Command::new("sh");
                command.args(["-c", "sleep 30 & wait"]);
                run_status_with_cancellation(
                    &mut command,
                    "test_hanging_target",
                    remaining,
                    Some(cancelled),
                )?;
                Ok(target.alias.clone())
            }
            Self::FailFirst { started } => {
                started.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    Err(ToolError::Sdk {
                        message: "injected".into(),
                        sdk_kind: "injected".into(),
                    })
                } else {
                    Ok(target.alias.clone())
                }
            }
        }
    }
}

fn run_bounded_target_jobs(
    targets: &[IncusSshTarget],
    concurrency: usize,
    aggregate_timeout: Duration,
    fail_fast: bool,
    job: IncusTargetJob,
) -> Vec<Result<String, ToolError>> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};

    let deadline = Instant::now() + aggregate_timeout;
    let queue = Arc::new(Mutex::new(
        targets.iter().cloned().enumerate().collect::<VecDeque<_>>(),
    ));
    let cancelled = Arc::new(AtomicBool::new(false));
    let job = Arc::new(job);
    let (tx, rx) = mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..concurrency.max(1).min(targets.len().max(1)) {
        let queue = Arc::clone(&queue);
        let cancelled = Arc::clone(&cancelled);
        let job = Arc::clone(&job);
        let tx = tx.clone();
        workers.push(thread::spawn(move || {
            loop {
                if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
                    break;
                }
                let next = queue.lock().expect("target job queue lock").pop_front();
                let Some((index, target)) = next else {
                    break;
                };
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                let result = job.execute(&target, remaining, &cancelled);
                if fail_fast && result.is_err() {
                    cancelled.store(true, Ordering::Release);
                }
                if tx.send((index, result)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);
    let mut results: Vec<Option<Result<String, ToolError>>> = std::iter::repeat_with(|| None)
        .take(targets.len())
        .collect();
    let mut received = 0;
    while received < targets.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok((index, result)) => {
                results[index] = Some(result);
                received += 1;
            }
            Err(_) => break,
        }
    }
    cancelled.store(true, Ordering::Release);
    for worker in workers {
        // Production jobs are restricted to the cancellation-aware command
        // primitives above. They own their process-tree guard, poll this token,
        // kill/reap the tree, and return before their passed `remaining`
        // deadline. Joining here proves no worker survives the fleet result.
        drop(worker.join());
    }
    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.unwrap_or_else(|| {
                Err(ToolError::Sdk {
                    message: format!(
                        "target {index} was cancelled at the aggregate deadline after {:.3}s",
                        aggregate_timeout.as_secs_f64()
                    ),
                    sdk_kind: "incus_ssh_aggregate_timeout".into(),
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_main_pid_accepts_only_numeric_systemd_output() {
        assert_eq!(parse_service_main_pid("0\n").unwrap(), None);
        assert_eq!(parse_service_main_pid("42\n").unwrap(), Some(42));
        assert_eq!(
            parse_service_main_pid("not-a-pid").unwrap_err().kind(),
            "incus_sync_main_pid_invalid"
        );
        assert_eq!(
            parse_service_main_pid("").unwrap_err().kind(),
            "incus_sync_main_pid_invalid"
        );
    }

    #[cfg(unix)]
    fn process_is_running(pid: i32) -> bool {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        match kill(Pid::from_raw(pid), None) {
            Err(Errno::ESRCH) => false,
            Err(_) => true,
            Ok(()) => {
                // An orphaned descendant can briefly remain as a zombie until
                // the platform's reaper collects it. `kill(pid, 0)` reports
                // that PID as present even though it can no longer execute.
                let output = Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output();
                !matches!(output, Ok(output) if output.status.success()
                    && String::from_utf8_lossy(&output.stdout).trim_start().starts_with('Z'))
            }
        }
    }

    #[cfg(unix)]
    fn assert_process_terminates(pid: i32) {
        for _ in 0..80 {
            if !process_is_running(pid) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("descendant process {pid} remained runnable after process-tree termination");
    }

    #[test]
    fn deployment_code_has_no_direct_unbounded_command_execution() {
        let source = include_str!("incus.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source prefix");
        assert!(!production.contains(".output()"));
        assert!(!production.contains(".status()"));
    }

    #[cfg(unix)]
    #[test]
    fn command_capture_drains_both_streams_concurrently_with_bounded_tails() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 8192 ]; do printf '0123456789abcdef' >&1; printf 'fedcba9876543210' >&2; i=$((i+1)); done",
        ]);

        let output = command_output_with_timeout(
            &mut command,
            "test_command_failed",
            Duration::from_secs(5),
        )
        .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.len(), COMMAND_OUTPUT_TAIL_BYTES);
        assert_eq!(output.stderr.len(), COMMAND_OUTPUT_TAIL_BYTES);
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
        assert!(output.stdout.ends_with(b"0123456789abcdef"));
        assert!(output.stderr.ends_with(b"fedcba9876543210"));
    }

    #[cfg(unix)]
    #[test]
    fn command_timeout_kills_the_spawned_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("grandchild.pid");
        let mut command = Command::new("sh");
        command.env("LABBY_TEST_GRANDCHILD_PID", &pid_path).args([
            "-c",
            "sleep 30 & echo $! > \"$LABBY_TEST_GRANDCHILD_PID\"; wait",
        ]);

        let error = command_output_with_timeout(
            &mut command,
            "test_command_failed",
            Duration::from_millis(200),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "test_command_failed");

        let pid: i32 = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_process_terminates(pid);
    }

    #[cfg(unix)]
    #[test]
    fn command_tree_guard_terminates_descendant_when_stdin_write_returns_early() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("early-return-grandchild.pid");
        let result = (|| -> std::io::Result<()> {
            let mut command = Command::new("sh");
            command
                .env("LABBY_TEST_GRANDCHILD_PID", &pid_path)
                .args([
                    "-c",
                    "sleep 30 & echo $! > \"$LABBY_TEST_GRANDCHILD_PID\"; exec 0<&-; sleep 30",
                ])
                .stdin(std::process::Stdio::piped());
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
            let mut child = command.spawn()?;
            let _guard = command_tree_guard(&mut child, "test")
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            for _ in 0..100 {
                if pid_path.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(Duration::from_millis(20));
            child
                .stdin
                .as_mut()
                .expect("piped stdin")
                .write_all(&vec![b'x'; 1 << 20])?;
            Ok(())
        })();
        assert!(result.is_err(), "fixture must force the early-return path");
        let pid: i32 = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_process_terminates(pid);
    }

    #[cfg(unix)]
    #[test]
    fn status_timeout_kills_the_spawned_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("status-grandchild.pid");
        let mut command = Command::new("sh");
        command.env("LABBY_TEST_GRANDCHILD_PID", &pid_path).args([
            "-c",
            "sleep 30 & echo $! > \"$LABBY_TEST_GRANDCHILD_PID\"; wait",
        ]);

        run_status(
            &mut command,
            "test_status_failed",
            Duration::from_millis(200),
        )
        .unwrap_err();
        let pid: i32 = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_process_terminates(pid);
    }

    #[test]
    fn target_jobs_bound_concurrency_and_preserve_order_for_ten() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let targets = fixture_targets(10);
        let results = run_bounded_target_jobs(
            &targets,
            3,
            Duration::from_secs(2),
            false,
            IncusTargetJob::Fixture(TargetJobFixture::Concurrent {
                active: Arc::clone(&active),
                peak: Arc::clone(&peak),
            }),
        );

        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(
            results.into_iter().collect::<Result<Vec<_>, _>>().unwrap(),
            targets
                .into_iter()
                .map(|target| target.alias)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn target_jobs_return_deterministic_failures_for_one_hundred() {
        let targets = fixture_targets(100);
        let results = run_bounded_target_jobs(
            &targets,
            8,
            Duration::from_secs(2),
            false,
            IncusTargetJob::Fixture(TargetJobFixture::FailEveryTen),
        );

        let failed: Vec<usize> = results
            .iter()
            .enumerate()
            .filter_map(|(index, result)| result.is_err().then_some(index))
            .collect();
        assert_eq!(failed, vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]);
    }

    #[test]
    fn target_jobs_fail_fast_cancels_pending_work_immediately() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let started = Arc::new(AtomicUsize::new(0));
        let targets = fixture_targets(100);
        let results = run_bounded_target_jobs(
            &targets,
            1,
            Duration::from_secs(2),
            true,
            IncusTargetJob::Fixture(TargetJobFixture::FailFirst {
                started: Arc::clone(&started),
            }),
        );

        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert!(results.iter().all(Result::is_err));
    }

    #[test]
    fn target_jobs_cancel_unstarted_work_at_aggregate_deadline_for_one_thousand() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let started = Arc::new(AtomicUsize::new(0));
        let targets = fixture_targets(1000);
        let results = run_bounded_target_jobs(
            &targets,
            4,
            Duration::from_millis(20),
            false,
            IncusTargetJob::Fixture(TargetJobFixture::Cooperative {
                started: Arc::clone(&started),
            }),
        );

        assert_eq!(results.len(), 1000);
        assert!(started.load(Ordering::SeqCst) <= 4);
        assert!(results.iter().all(Result::is_err));
        assert!(
            results[999]
                .as_ref()
                .unwrap_err()
                .to_string()
                .contains("aggregate deadline")
        );
    }

    #[test]
    #[cfg(unix)]
    fn target_jobs_cancel_and_join_a_hanging_external_process() {
        let started = Instant::now();
        let results = run_bounded_target_jobs(
            &fixture_targets(1),
            1,
            Duration::from_millis(50),
            false,
            IncusTargetJob::Fixture(TargetJobFixture::HangingProcess),
        );

        assert!(started.elapsed() < Duration::from_millis(400));
        assert!(results[0].is_err());
    }

    fn fixture_targets(count: usize) -> Vec<IncusSshTarget> {
        (0..count)
            .map(|index| IncusSshTarget {
                alias: index.to_string(),
                host: "fixture.invalid".into(),
                user: None,
                port: None,
            })
            .collect()
    }

    #[test]
    fn release_backup_manifest_versions_binary_and_assets() {
        let script = remote_release_backup_script("/var/lib/labby/deployments/tx-7");
        assert!(script.contains("manifest.env"));
        assert!(script.contains("binary_present="));
        assert!(script.contains("assets_present="));
        assert!(script.contains("active_state=%s"));
        assert!(script.contains("unit_file_state=%s"));
        assert!(script.contains("cp -a /usr/local/bin/labby"));
        assert!(script.contains("cp -a /home/labby/.labby/web-assets"));
    }

    #[test]
    fn release_manifest_and_rollback_preserve_exact_supported_systemd_states() {
        let backup = remote_release_backup_script(PREVIOUS_RELEASE_DIR);
        assert!(backup.contains("active|inactive|failed"));
        assert!(backup.contains("enabled|enabled-runtime|disabled"));
        assert!(!backup.contains("service_active="));
        assert!(!backup.contains("service_enabled="));

        let rollback = remote_release_rollback_script(PREVIOUS_RELEASE_DIR);
        assert!(rollback.contains("systemctl enable --runtime"));
        assert!(rollback.contains("systemctl reset-failed"));
        assert!(rollback.contains("test \"$actual\" = failed"));
    }

    #[test]
    fn successful_sync_retains_the_previous_release_for_explicit_rollback() {
        assert_eq!(
            PREVIOUS_RELEASE_DIR,
            "/var/lib/labby/deployments/previous-release"
        );
        let guard_source = include_str!("incus.rs");
        assert!(guard_source.contains("Keep one verified prior release"));
    }

    #[cfg(unix)]
    #[test]
    fn release_backup_rejects_incomplete_service_state_before_manifest() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let deployment = root.path().join("deployment");
        std::fs::create_dir(&bin).unwrap();
        let systemctl = bin.join("systemctl");
        std::fs::write(&systemctl, "#!/bin/sh\nprintf 'ActiveState=active\\n'\n").unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new("sh")
            .args([
                "-c",
                &remote_release_backup_script(deployment.to_str().unwrap()),
            ])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(70));
        assert!(!deployment.join("manifest.env").exists());
    }

    #[test]
    fn rollback_failure_preserves_primary_and_recovery_failure() {
        let error = deployment_failure("candidate verification failed", Err("restore denied"));
        assert_eq!(error.kind(), "incus_sync_rollback_failed");
        let message = error.to_string();
        assert!(message.contains("candidate verification failed"));
        assert!(message.contains("restore denied"));
    }

    fn probe_failure(message: &str) -> ToolError {
        ToolError::Sdk {
            message: message.into(),
            sdk_kind: "fixture_failure".into(),
        }
    }

    #[test]
    fn version_verification_rejects_command_failure_and_empty_output() {
        for (output, expected) in [
            (Err(probe_failure("command failed")), "command failed"),
            (Ok(" \n\t".to_string()), "empty output"),
        ] {
            let error = require_version_output(
                output,
                "incus_sync_local_version_failed",
                "local labby --version",
            )
            .unwrap_err();
            assert_eq!(error.kind(), "incus_sync_local_version_failed");
            assert!(error.to_string().contains(expected));
        }

        for (output, expected) in [
            (
                Err(probe_failure("remote command failed")),
                "remote command failed",
            ),
            (Ok(String::new()), "empty output"),
        ] {
            let error = require_version_output(
                output,
                "incus_sync_remote_version_failed",
                "deployed labby --version",
            )
            .unwrap_err();
            assert_eq!(error.kind(), "incus_sync_remote_version_failed");
            assert!(error.to_string().contains(expected));
        }
    }

    #[test]
    fn explicit_rollback_rejects_version_probe_failure() {
        let error = verify_explicit_rollback_with(
            || Err(probe_failure("version unavailable")),
            || Ok(Some(42)),
            || Ok("abc".into()),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "incus_sync_rollback_version_failed");
    }

    #[test]
    fn explicit_rollback_rejects_absent_pid() {
        let error = verify_explicit_rollback_with(
            || Ok("labby 1.2.3".into()),
            || Ok(None),
            || Ok("abc".into()),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "incus_sync_rollback_pid_failed");
    }

    #[test]
    fn explicit_rollback_rejects_pid_probe_failure() {
        let error = verify_explicit_rollback_with(
            || Ok("labby 1.2.3".into()),
            || Err(probe_failure("pid unavailable")),
            || Ok("abc".into()),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "incus_sync_rollback_pid_failed");
    }

    #[test]
    fn explicit_rollback_rejects_hash_probe_failure() {
        let error = verify_explicit_rollback_with(
            || Ok("labby 1.2.3".into()),
            || Ok(Some(42)),
            || Err(probe_failure("hash unavailable")),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "incus_sync_rollback_hash_failed");
    }

    #[test]
    fn explicit_rollback_rejects_empty_hash() {
        let error = verify_explicit_rollback_with(
            || Ok("labby 1.2.3".into()),
            || Ok(Some(42)),
            || Ok(" \n".into()),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "incus_sync_rollback_hash_failed");
    }

    #[test]
    fn explicit_rollback_rejects_readiness_probe_failure() {
        let error = verify_explicit_rollback_with(
            || Ok("labby 1.2.3".into()),
            || Ok(Some(42)),
            || Ok("abc".into()),
            || Err(probe_failure("not ready")),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "incus_sync_rollback_readiness_failed");
    }

    #[test]
    fn explicit_rollback_returns_only_fully_verified_state() {
        let verified = verify_explicit_rollback_with(
            || Ok(" labby 1.2.3\n".into()),
            || Ok(Some(42)),
            || Ok("abc\n".into()),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(verified, ("labby 1.2.3".into(), 42, "abc".into()));
    }

    #[test]
    fn bootstrap_exposes_mutation_faults_and_durable_residual_reporting() {
        for checkpoint in [
            "storage",
            "profile",
            "container-launch",
            "container-start",
            "backup-config",
            "hostname",
            "binary",
            "provision",
            "readiness",
            "tailscale-key",
            "tailscale-up",
            "tailscale-cleanup",
        ] {
            assert!(INCUS_BOOTSTRAP_SCRIPT.contains(&format!("checkpoint {checkpoint}")));
        }
        assert!(INCUS_BOOTSTRAP_SCRIPT.contains("LABBY_INCUS_FAIL_AFTER"));
        assert!(INCUS_BOOTSTRAP_SCRIPT.contains("/var/tmp/labby-incus-rollback-residual-"));
        assert!(INCUS_BOOTSTRAP_SCRIPT.contains("rollback residual"));
    }

    #[cfg(unix)]
    #[test]
    fn rollback_executes_all_independent_restores_when_systemctl_fails() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let deployment = root.path().join("deployment");
        let bin_dir = root.path().join("fake-bin");
        let binary = root.path().join("labby");
        let assets = root.path().join("web-assets");
        let log = root.path().join("commands.log");
        std::fs::create_dir_all(deployment.join("web")).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(deployment.join("labby"), b"prior-binary").unwrap();
        std::fs::write(deployment.join("web/index.html"), b"prior-assets").unwrap();
        std::fs::write(
            deployment.join("manifest.env"),
            "binary_present=1\nassets_present=1\nactive_state=active\nunit_file_state=enabled\n",
        )
        .unwrap();
        std::fs::write(&binary, b"candidate").unwrap();
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("index.html"), b"candidate-assets").unwrap();
        let systemctl = bin_dir.join("systemctl");
        std::fs::write(
            &systemctl,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >>\"$COMMAND_LOG\"\n[ \"$1\" != stop ]\n",
        )
        .unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let curl = bin_dir.join("curl");
        std::fs::write(&curl, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let script = remote_release_rollback_script_for(
            deployment.to_str().unwrap(),
            binary.to_str().unwrap(),
            assets.to_str().unwrap(),
            "labby.service",
            "http://127.0.0.1/ready",
        );
        let status = Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .env("COMMAND_LOG", &log)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(70));
        assert_eq!(std::fs::read(&binary).unwrap(), b"prior-binary");
        assert_eq!(
            std::fs::read(assets.join("index.html")).unwrap(),
            b"prior-assets"
        );
        let calls = std::fs::read_to_string(log).unwrap();
        assert!(calls.contains("stop labby.service"));
        assert!(calls.contains("enable labby.service"));
        assert!(calls.contains("start labby.service"));
        assert!(deployment.exists(), "failed rollback must retain manifest");
    }
    use std::ffi::OsStr;

    #[test]
    fn rejects_a_served_web_index_that_differs_from_the_built_export() {
        let err = verify_web_index_hash("built-index-sha", "served-index-sha").unwrap_err();

        assert_eq!(err.kind(), "incus_sync_web_assets_hash_mismatch");
    }

    #[test]
    fn accepts_a_served_web_index_that_matches_the_built_export() {
        verify_web_index_hash("same-index-sha", "same-index-sha").unwrap();
    }

    #[test]
    fn parses_supported_snapshot_keys() {
        let entries = parse_backup_config_str(
            r#"
config:
  snapshots.schedule: "@daily"
  snapshots.expiry: "14d"
  snapshots.pattern: "labby-{{ creation_date|date:'2006-01-02_15-04-05' }}"
  snapshots.schedule.stopped: false
"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 4);
        assert!(
            entries.iter().any(|entry| {
                entry.key == "snapshots.schedule.stopped" && entry.value == "false"
            })
        );
    }

    #[test]
    fn rejects_unknown_keys() {
        let err = parse_backup_config_str(
            r#"
config:
  security.privileged: true
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_unsupported_key");
    }

    #[test]
    fn rejects_non_scalar_values() {
        let err = parse_backup_config_str(
            r#"
config:
  snapshots.schedule:
    nested: nope
"#,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "incus_backup_config_non_scalar");
    }

    #[test]
    fn materializes_embedded_bootstrap_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();

        assert!(artifacts.bootstrap_script.exists());
        assert!(artifacts.install_script.exists());
        assert!(artifacts.profile_file.exists());
        assert!(artifacts.backup_config_file.exists());

        let bootstrap = std::fs::read_to_string(&artifacts.bootstrap_script).unwrap();
        assert!(bootstrap.contains("incus-bootstrap.sh"));
        assert!(bootstrap.contains("labby setup --provision --yes"));

        let profile = std::fs::read_to_string(&artifacts.profile_file).unwrap();
        assert!(profile.contains("security.privileged: \"false\""));
    }

    #[test]
    fn builds_bootstrap_command_from_embedded_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            version: Some("v1.2.3".to_string()),
            dry_run: true,
            storage_driver: Some("dir".to_string()),
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();
        let args = command.args;

        assert_eq!(command.program, OsStr::new("sh"));
        assert_eq!(args[0], artifacts.bootstrap_script.as_os_str());
        assert!(has_arg_pair(&args, "--version", OsStr::new("v1.2.3")));
        assert!(has_arg_pair(
            &args,
            "--profile-file",
            artifacts.profile_file.as_os_str()
        ));
        assert!(args.windows(2).any(|pair| pair
            == [
                OsStr::new("--backup-config"),
                artifacts.backup_config_file.as_os_str()
            ]));
        assert!(has_arg_pair(&args, "--storage-driver", OsStr::new("dir")));
        assert!(args.iter().any(|arg| arg == OsStr::new("--dry-run")));
    }

    #[test]
    fn resolves_user_paths_before_switching_to_temp_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            backup_config: Some(PathBuf::from("my-backup.yaml")),
            local_binary: Some(PathBuf::from("target/debug/labby")),
            dry_run: true,
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();
        let args = command.args;
        let cwd = std::env::current_dir().unwrap();

        assert!(has_arg_pair(
            &args,
            "--backup-config",
            cwd.join("my-backup.yaml").as_os_str()
        ));
        assert!(has_arg_pair(
            &args,
            "--local-binary",
            cwd.join("target/debug/labby").as_os_str()
        ));
    }

    #[test]
    fn rejects_conflicting_backup_config_options() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            backup_config: Some(PathBuf::from("my-backup.yaml")),
            no_backup_config: true,
            ..IncusBootstrapOptions::default()
        };

        let err = bootstrap_command(&artifacts, &options).unwrap_err();
        assert_eq!(err.kind(), "incus_bootstrap_invalid_options");
    }

    #[test]
    fn passes_container_name_as_tailscale_hostname() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = materialize_bootstrap_artifacts(dir.path()).unwrap();
        let options = IncusBootstrapOptions {
            name: Some("labby".to_string()),
            dry_run: true,
            ..IncusBootstrapOptions::default()
        };

        let command = bootstrap_command(&artifacts, &options).unwrap();

        assert!(has_arg_pair(
            &command.args,
            "--tailscale-hostname",
            OsStr::new("labby")
        ));
    }

    #[test]
    fn parses_concrete_ssh_config_hosts() {
        let parsed = parse_ssh_config(
            r#"
Host *
  User ignored

Host nas-host nas
  HostName 100.64.0.29
  User operator
  Port 2222

Host dev-host
  HostName dev-host.example-tailnet.ts.net

Host github.com
  User git

Host GitHubEnterprise
  HostName github.internal

Host -Ftmp
  HostName bad.example

Include ~/.ssh/extra
"#,
        );
        let targets = parsed.targets;

        assert_eq!(
            targets,
            vec![
                IncusSshTarget {
                    alias: "nas-host".into(),
                    host: "100.64.0.29".into(),
                    user: Some("operator".into()),
                    port: Some(2222),
                },
                IncusSshTarget {
                    alias: "nas".into(),
                    host: "100.64.0.29".into(),
                    user: Some("operator".into()),
                    port: Some(2222),
                },
                IncusSshTarget {
                    alias: "dev-host".into(),
                    host: "dev-host.example-tailnet.ts.net".into(),
                    user: None,
                    port: None,
                },
            ]
        );
        assert_eq!(parsed.skipped_unsafe, vec!["-Ftmp"]);
        assert_eq!(parsed.unsupported_include, vec!["~/.ssh/extra"]);
    }

    #[test]
    fn filters_ssh_targets_and_reports_skips() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            r#"
Host *
  User ignored

Host dev-host
  HostName dev-host.example-tailnet.ts.net

Host edge-host
  HostName edge-host

Host github.com
  User git
"#,
        )
        .unwrap();

        let outcome = incus_ssh_bootstrap_plan(&IncusSshBootstrapOptions {
            container: "labby".into(),
            user: "labby".into(),
            ssh_config: config,
            key_path: "/home/labby/.ssh/id_ed25519".into(),
            dry_run: true,
            fail_fast: false,
            include: vec!["dev-host".into()],
            exclude: vec!["edge-host".into()],
            install_config: true,
            timeout_seconds: 10,
        })
        .unwrap();

        assert_eq!(
            outcome
                .targets
                .iter()
                .map(|target| target.alias.as_str())
                .collect::<Vec<_>>(),
            vec!["dev-host"]
        );
        assert_eq!(outcome.skipped_wildcard, vec!["*"]);
        assert_eq!(outcome.skipped_github, vec!["github.com"]);
        assert_eq!(outcome.skipped_not_included, vec!["edge-host"]);
        assert!(outcome.steps.iter().any(|step| step.contains("sanitized")));
    }

    #[test]
    fn builds_authorize_ssh_command_with_config_and_option_terminator() {
        let target = IncusSshTarget {
            alias: "nas-host".into(),
            host: "100.64.0.29".into(),
            user: Some("operator".into()),
            port: Some(2222),
        };
        let config = PathBuf::from("/tmp/labby-test-ssh-config");
        let command = authorize_target_command(&target, &config, 7);
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();

        assert!(has_arg_pair(&args, "-F", config.as_os_str()));
        assert!(has_arg_pair(&args, "-o", OsStr::new("BatchMode=yes")));
        assert!(has_arg_pair(&args, "-o", OsStr::new("ConnectTimeout=7")));
        assert!(
            args.windows(2)
                .any(|pair| pair == [OsStr::new("--"), OsStr::new("nas-host")])
        );
    }

    #[test]
    fn renders_sanitized_container_ssh_config() {
        let config = render_sanitized_ssh_config(&[IncusSshTarget {
            alias: "nas-host".into(),
            host: "100.64.0.29".into(),
            user: Some("root".into()),
            port: Some(29229),
        }]);

        assert!(config.contains("Host nas-host"));
        assert!(config.contains("  HostName 100.64.0.29"));
        assert!(config.contains("  User root"));
        assert!(config.contains("  Port 29229"));
        assert!(config.contains("  IdentityFile ~/.ssh/id_ed25519"));
        assert!(!config.contains("github"));
        assert!(!config.contains("ProxyJump"));
    }

    #[test]
    fn plans_incus_ssh_bootstrap_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(
            &config,
            r#"
Host nas-host
  HostName 100.64.0.29
  User operator
"#,
        )
        .unwrap();

        let outcome = incus_ssh_bootstrap_plan(&IncusSshBootstrapOptions {
            container: "labby".into(),
            user: "labby".into(),
            ssh_config: config,
            key_path: "/home/labby/.ssh/id_ed25519".into(),
            dry_run: true,
            fail_fast: false,
            include: Vec::new(),
            exclude: Vec::new(),
            install_config: true,
            timeout_seconds: 10,
        })
        .unwrap();

        assert!(outcome.dry_run);
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(
            outcome.steps[0],
            "incus exec labby --user labby -- ssh-keygen -t ed25519 -f /home/labby/.ssh/id_ed25519 -N '' -C labby-incus-labby"
        );
        assert_eq!(
            outcome.steps[1],
            "authorize container public key on operator@100.64.0.29"
        );
    }

    fn has_arg_pair(args: &[OsString], flag: &str, value: &OsStr) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == OsStr::new(flag) && pair[1] == value)
    }
}
