//! Binary-owned local setup checks and safe filesystem repair.
//!
//! This module deliberately contains no Claude plugin lifecycle or plugin-option
//! synchronization. Client plugins connect to Labby; they do not configure the
//! Labby host. Keep host validation here so setup check/repair remains usable
//! without coupling server health to any client installation.

use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use crate::access::{AccessHealthStatus, inspect_health};
use crate::dispatch::error::ToolError;

use super::client::{env_path, lab_home};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    Repair,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SetupCheck {
    pub name: &'static str,
    pub ok: bool,
    pub severity: SetupSeverity,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repaired: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupSeverity {
    Blocking,
    Advisory,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SetupReport {
    pub exit_policy: &'static str,
    pub ran_repair: bool,
    pub no_repair: bool,
    pub blocking_failures: Vec<String>,
    pub advisory_failures: Vec<String>,
    pub ok: bool,
    pub changed: bool,
    pub mode: &'static str,
    pub checks: Vec<SetupCheck>,
}

/// Check or repair local Labby filesystem prerequisites.
pub fn run(mode: Mode) -> Result<SetupReport, ToolError> {
    let access_store = crate::config::access_db_path().map_err(|_| ToolError::Sdk {
        sdk_kind: "setup_check_failed".into(),
        message: "unable to resolve the access store path".to_string(),
    })?;
    let config_candidates = crate::config::toml_candidates().map_err(|_| ToolError::Sdk {
        sdk_kind: "setup_check_failed".into(),
        message: "unable to resolve the config.toml location".to_string(),
    })?;
    run_for_paths_with(
        mode,
        lab_home(),
        env_path(),
        access_store,
        Some(&config_candidates),
    )
}

#[cfg(test)]
fn run_for_paths(
    mode: Mode,
    lab_home: PathBuf,
    env: PathBuf,
    access_store: PathBuf,
) -> Result<SetupReport, ToolError> {
    run_for_paths_with(mode, lab_home, env, access_store, None)
}

fn run_for_paths_with(
    mode: Mode,
    lab_home: PathBuf,
    env: PathBuf,
    access_store: PathBuf,
    config_candidates: Option<&[PathBuf]>,
) -> Result<SetupReport, ToolError> {
    let mut checks = Vec::with_capacity(4);
    let mut changed = false;

    checks.push(check_lab_home(mode, &lab_home, &mut changed)?);
    checks.push(check_env_file(mode, &env, &mut changed)?);
    checks.push(check_access_store(&access_store));
    if let Some(candidates) = config_candidates {
        checks.push(check_config(candidates));
    }

    let blocking_failures = checks
        .iter()
        .filter(|check| !check.ok && check.severity == SetupSeverity::Blocking)
        .map(|check| check.name.to_string())
        .collect::<Vec<_>>();
    let advisory_failures = checks
        .iter()
        .filter(|check| !check.ok && check.severity == SetupSeverity::Advisory)
        .map(|check| check.name.to_string())
        .collect::<Vec<_>>();
    let exit_policy = if !blocking_failures.is_empty() {
        "blocking_failure"
    } else if !advisory_failures.is_empty() {
        "advisory_failure"
    } else {
        "success"
    };

    Ok(SetupReport {
        exit_policy,
        ran_repair: mode == Mode::Repair,
        no_repair: mode == Mode::Check,
        ok: blocking_failures.is_empty(),
        changed,
        mode: match mode {
            Mode::Check => "check",
            Mode::Repair => "repair",
        },
        blocking_failures,
        advisory_failures,
        checks,
    })
}

/// Validate config.toml with the same loader and startup checks used by serve.
fn check_config(candidates: &[PathBuf]) -> SetupCheck {
    let path = candidates
        .iter()
        .find(|path| path.exists())
        .or_else(|| candidates.first())
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let mut problems = crate::composition::config_check::check_config_candidates(candidates);
    problems
        .extend(crate::composition::config_check::check_runtime_guard_candidates(candidates, None));
    let has_fatal = problems.iter().any(|problem| problem.fatal);
    SetupCheck {
        name: "config",
        ok: problems.is_empty(),
        severity: if has_fatal {
            SetupSeverity::Blocking
        } else {
            SetupSeverity::Advisory
        },
        path,
        repaired: None,
        message: (!problems.is_empty()).then(|| {
            problems
                .iter()
                .map(|problem| {
                    let class = if problem.fatal { "fatal" } else { "degraded" };
                    format!("{class} [{}]: {}", problem.code, problem.message)
                })
                .collect::<Vec<_>>()
                .join("; ")
        }),
    }
}

fn check_access_store(path: &Path) -> SetupCheck {
    let health = inspect_health(path);
    let (ok, severity) = match health.status {
        AccessHealthStatus::Ready => (true, SetupSeverity::Advisory),
        AccessHealthStatus::Missing
        | AccessHealthStatus::Uninitialized
        | AccessHealthStatus::Prepared => (false, SetupSeverity::Advisory),
        AccessHealthStatus::Insecure
        | AccessHealthStatus::Corrupt
        | AccessHealthStatus::NewerSchema
        | AccessHealthStatus::Locked
        | AccessHealthStatus::ReadOnly
        | AccessHealthStatus::Unavailable => (false, SetupSeverity::Blocking),
    };
    SetupCheck {
        name: "access_store",
        ok,
        severity,
        path: path.display().to_string(),
        repaired: None,
        message: (!ok).then(|| health.detail.to_string()),
    }
}

fn check_lab_home(mode: Mode, path: &Path, changed: &mut bool) -> Result<SetupCheck, ToolError> {
    if path.is_dir() {
        return Ok(ok_check("lab_home", path, None));
    }
    if path.exists() {
        return Ok(failed_check(
            "lab_home",
            path,
            SetupSeverity::Blocking,
            "path exists but is not a directory",
        ));
    }
    if mode == Mode::Repair {
        create_lab_home(path).map_err(|error| io_error("lab_home", path, error))?;
        *changed = true;
        return Ok(ok_check("lab_home", path, Some(true)));
    }
    Ok(failed_check(
        "lab_home",
        path,
        SetupSeverity::Blocking,
        "directory is missing",
    ))
}

#[cfg(unix)]
fn create_lab_home(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_lab_home(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

fn check_env_file(mode: Mode, path: &Path, changed: &mut bool) -> Result<SetupCheck, ToolError> {
    if path.is_file() {
        return Ok(ok_check("env_file", path, None));
    }
    if path.exists() {
        return Ok(failed_check(
            "env_file",
            path,
            SetupSeverity::Blocking,
            "path exists but is not a regular file",
        ));
    }
    if mode == Mode::Repair {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error("env_file", parent, error))?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| io_error("env_file", path, error))?;
        *changed = true;
        return Ok(ok_check("env_file", path, Some(true)));
    }
    Ok(failed_check(
        "env_file",
        path,
        SetupSeverity::Advisory,
        "file is missing; process env can supply setup values",
    ))
}

fn ok_check(name: &'static str, path: &Path, repaired: Option<bool>) -> SetupCheck {
    SetupCheck {
        name,
        ok: true,
        severity: SetupSeverity::Advisory,
        path: path.display().to_string(),
        repaired,
        message: None,
    }
}

fn failed_check(
    name: &'static str,
    path: &Path,
    severity: SetupSeverity,
    message: &'static str,
) -> SetupCheck {
    SetupCheck {
        name,
        ok: false,
        severity,
        path: path.display().to_string(),
        repaired: None,
        message: Some(message.to_string()),
    }
}

fn io_error(check: &'static str, path: &Path, error: std::io::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "setup_repair_failed".into(),
        message: format!("failed to repair {check} at {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn secure(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("secure permissions");
    }

    #[cfg(not(unix))]
    fn secure(_path: &Path, _mode: u32) {}

    #[test]
    fn check_reports_missing_paths_without_creating_them() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        let report = run_for_paths(
            Mode::Check,
            home.clone(),
            env.clone(),
            home.join("access.db"),
        )
        .expect("check report");
        assert!(!report.ok);
        assert!(!report.changed);
        assert_eq!(report.exit_policy, "blocking_failure");
        assert_eq!(report.blocking_failures, ["lab_home"]);
        assert_eq!(report.advisory_failures, ["env_file", "access_store"]);
        assert!(!home.exists());
        assert!(!env.exists());
    }

    #[test]
    fn setup_check_config_passes_for_a_valid_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        fs::create_dir_all(&home).unwrap();
        let config = home.join("config.toml");
        fs::write(&config, "[mcp]\nport = 8765\n").unwrap();
        let report = run_for_paths_with(
            Mode::Check,
            home.clone(),
            home.join(".env"),
            home.join("access.db"),
            Some(std::slice::from_ref(&config)),
        )
        .expect("check report");
        let check = report
            .checks
            .iter()
            .find(|check| check.name == "config")
            .unwrap();
        assert!(check.ok, "{check:?}");
    }

    #[test]
    fn setup_check_config_blocks_on_a_startup_fatal_config() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        fs::create_dir_all(&home).unwrap();
        let config = home.join("config.toml");
        fs::write(&config, "[depot.private_hosts]\n\"depot.internal\" = []\n").unwrap();
        let report = run_for_paths_with(
            Mode::Check,
            home.clone(),
            home.join(".env"),
            home.join("access.db"),
            Some(std::slice::from_ref(&config)),
        )
        .expect("check report");
        assert!(!report.ok);
        assert!(report.blocking_failures.contains(&"config".to_string()));
    }

    #[test]
    fn repair_creates_lab_home_and_env_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        let report = run_for_paths(
            Mode::Repair,
            home.clone(),
            env.clone(),
            home.join("access.db"),
        )
        .expect("repair report");
        assert!(report.ok);
        assert!(report.changed);
        assert_eq!(report.exit_policy, "advisory_failure");
        assert!(home.is_dir());
        assert!(env.is_file());
    }

    #[test]
    fn repair_is_idempotent_after_paths_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        fs::create_dir_all(&home).expect("lab home");
        secure(&home, 0o700);
        fs::write(&env, "APPRISE_URL=http://localhost\n").expect("env file");
        let report = run_for_paths(Mode::Repair, home.clone(), env, home.join("access.db"))
            .expect("repair report");
        assert!(report.ok);
        assert!(!report.changed);
    }

    #[test]
    fn repair_never_mutates_an_uninitialized_access_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        let access = home.join("access.db");
        fs::create_dir_all(&home).expect("lab home");
        secure(&home, 0o700);
        fs::write(&env, "").expect("env file");
        fs::write(&access, b"").expect("access store");
        secure(&access, 0o600);
        let before = fs::read(&access).expect("access bytes");
        let report = run_for_paths(Mode::Repair, home, env, access.clone()).expect("repair report");
        assert!(report.ok);
        assert!(!report.changed);
        assert_eq!(fs::read(&access).expect("access bytes"), before);
    }

    #[test]
    fn corrupt_access_store_is_blocking_and_remains_unrepaired() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("lab-home");
        let env = home.join(".env");
        let access = home.join("access.db");
        fs::create_dir_all(&home).expect("lab home");
        secure(&home, 0o700);
        fs::write(&env, "").expect("env file");
        fs::write(&access, b"not a sqlite database").expect("access store");
        secure(&access, 0o600);
        let before = fs::read(&access).expect("access bytes");
        let report = run_for_paths(Mode::Repair, home, env, access.clone()).expect("repair report");
        assert!(!report.ok);
        assert_eq!(report.blocking_failures, ["access_store"]);
        assert_eq!(fs::read(&access).expect("access bytes"), before);
    }
}
