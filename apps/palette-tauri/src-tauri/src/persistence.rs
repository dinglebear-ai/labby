//! Disk persistence for the palette: reads/writes the palette preferences file
//! (`settings.json`) beside the OAuth credential file in the app config dir.
//!
//! The palette does not manage a `labby serve` instance's `~/.labby/.env` or
//! `config.toml` — that is owned by `labby setup`. This module only persists the
//! desktop app's own preferences (server URL, optional static bearer token,
//! shortcut, theme, and UX toggles).
//!
//! # Atomic writes
//!
//! `settings.json` writes use an atomic rename pattern: write to a per-write
//! unique temp file, fsync, then atomically replace the target on every supported
//! platform. Secret-bearing directories get an explicit user-only Windows ACL;
//! Unix files are created with mode `0o600`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use tauri::{AppHandle, Manager};

use crate::{LabbySettings, PartialPaletteSettings, SETTINGS_FILE};

pub(crate) async fn run_blocking_io<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|err| format!("persistence worker failed: {err}"))?
}

pub(crate) fn read_settings_result(app: &AppHandle) -> Result<PartialPaletteSettings, String> {
    let path = match settings_path(app) {
        Ok(p) => p,
        Err(err) => {
            crate::warn(err);
            return Ok(PartialPaletteSettings::default());
        }
    };
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(PartialPaletteSettings::default());
        }
        Err(err) => {
            return Err(format!(
                "failed to read palette settings at {}: {err}",
                path.display()
            ));
        }
    };
    parse_settings_json(&contents, &path)
}

pub(crate) fn parse_settings_json(
    contents: &str,
    path: &Path,
) -> Result<PartialPaletteSettings, String> {
    serde_json::from_str(contents).map_err(|err| {
        format!(
            "failed to parse palette settings at {}: {err}",
            path.display()
        )
    })
}

pub(crate) fn write_settings(
    app: &AppHandle,
    settings: &LabbySettings,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, serde_json::to_string_pretty(settings)?.as_bytes())?;
    Ok(())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .map_err(|err| format!("failed to resolve app config directory: {err}"))
}

/// Read an environment variable, returning `None` for a missing or blank value.
pub(crate) fn value_for(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Write `data` to `path` atomically, replacing an existing destination.
///
/// The temp name carries a UUID so two concurrent writers of the same `path`
/// (e.g. a login racing a refresh writing `oauth.json`) do not collide on a
/// fixed `<path>.tmp`. If any step fails the temp file is best-effort removed
/// so unique temps don't accumulate on error.
///
/// On Unix, the temp file is created with mode `0o600`. On Windows the parent
/// directory is first hardened to an explicit current-user-only inheritable ACL,
/// so the library's temporary file and committed destination are protected for
/// their entire lifetime.
pub(crate) fn atomic_write(path: &Path, data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .ok_or("credential path has no parent directory")?;
    harden_secret_directory(parent)?;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    atomicwrites::AtomicFile::new(path, atomicwrites::AllowOverwrite)
        .write_with_options(
            |file| {
                use std::io::Write;
                file.write_all(data)
            },
            options,
        )
        .map_err(io::Error::from)?;
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn harden_secret_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn powershell_module_path_from(system_root: Option<std::ffi::OsString>) -> io::Result<PathBuf> {
    system_root
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("SystemRoot is unavailable"))
        .map(|root| {
            root.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("Modules")
        })
}

#[cfg(windows)]
pub(crate) fn harden_secret_directory(path: &Path) -> io::Result<()> {
    use std::process::Command;

    // GUI processes can start without PSModulePath. PowerShell then finds
    // Set-Acl by name but cannot autoload Microsoft.PowerShell.Security.
    let powershell_module_path = powershell_module_path_from(std::env::var_os("SystemRoot"))?;

    // Build a new protected DACL rather than editing the inherited/existing
    // one. `icacls /grant:r` only replaces ACEs for that principal and would
    // leave an explicit Everyone/Users (or arbitrary third-party) grant alive.
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$path = $env:LABBY_SECRET_DIR
$acl = [System.Security.AccessControl.DirectorySecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$inherit = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'
$propagate = [System.Security.AccessControl.PropagationFlags]::None
$allow = [System.Security.AccessControl.AccessControlType]::Allow
$identities = @(
  [System.Security.Principal.WindowsIdentity]::GetCurrent().User,
  [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18'),
  [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
)
foreach ($identity in $identities) {
  $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
    $identity, [System.Security.AccessControl.FileSystemRights]::FullControl,
    $inherit, $propagate, $allow)
  $acl.AddAccessRule($rule) | Out-Null
}
Set-Acl -LiteralPath $path -AclObject $acl
"#;
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("LABBY_SECRET_DIR", path)
        .env("PSModulePath", powershell_module_path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(1024)
            .collect::<String>();
        let detail = if stderr.is_empty() {
            "no diagnostic output".to_owned()
        } else {
            stderr
        };
        return Err(io::Error::other(format!(
            "PowerShell exited with {} while installing the authoritative secret-directory ACL: {detail}",
            output.status
        )));
    }
    Ok(())
}

#[cfg(test)]
mod async_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn slow_disk_work_does_not_stall_async_commands() {
        let write = run_blocking_io(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(())
        });
        let timer = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            tokio::time::sleep(std::time::Duration::from_millis(5)),
        );
        let (write_result, timer_result) = tokio::join!(write, timer);
        assert!(write_result.is_ok());
        assert!(timer_result.is_ok(), "disk work stalled the async executor");
    }
}

#[cfg(all(test, windows))]
mod windows_acl_tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn hardening_removes_preexisting_everyone_grant() {
        let dir =
            std::env::temp_dir().join(format!("labby acl ' special {}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let status = Command::new("icacls")
            .arg(&dir)
            .args(["/grant", "*S-1-1-0:(OI)(CI)F"])
            .status()
            .unwrap();
        assert!(status.success());

        harden_secret_directory(&dir).unwrap();
        let script = r#"$acl=Get-Acl -LiteralPath $args[0]; $sids=@($acl.Access | ForEach-Object { $_.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value }); $current=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; if ($sids -contains 'S-1-1-0') { exit 2 }; if (-not $acl.AreAccessRulesProtected) { exit 3 }; if ($sids.Count -ne 3) { exit 4 }; foreach ($expected in @($current, 'S-1-5-18', 'S-1-5-32-544')) { if ($sids -notcontains $expected) { exit 5 } }"#;
        let checked = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .arg(&dir)
            .status()
            .unwrap();
        assert!(checked.success(), "broad ACE survived authoritative ACL");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_system_root_has_an_actionable_error() {
        let error = powershell_module_path_from(None).unwrap_err();
        assert_eq!(error.to_string(), "SystemRoot is unavailable");
    }

    #[test]
    fn powershell_failure_preserves_exit_and_diagnostics() {
        let missing = std::env::temp_dir().join(format!("labby-missing-{}", uuid::Uuid::new_v4()));
        let error = harden_secret_directory(&missing).unwrap_err().to_string();
        assert!(error.contains("PowerShell exited with"), "{error}");
        assert!(
            error.contains("authoritative secret-directory ACL"),
            "{error}"
        );
        assert!(!error.ends_with("no diagnostic output"), "{error}");
    }
}
