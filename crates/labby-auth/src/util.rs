#![allow(clippy::redundant_pub_crate)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
#[cfg(feature = "http-axum")]
use std::time::Duration;

#[cfg(feature = "http-axum")]
use base64::Engine;
#[cfg(feature = "http-axum")]
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::error::AuthError;

pub fn now_unix() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(secs).unwrap_or(i64::MAX)
}

#[cfg(feature = "http-axum")]
pub(crate) fn random_token(bytes: usize) -> Result<String, AuthError> {
    let mut buf = vec![0_u8; bytes];
    getrandom::fill(&mut buf)
        .map_err(|error| AuthError::Storage(format!("generate random token: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

pub fn fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut output = String::with_capacity(12);
    for byte in &digest[..6] {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
pub(crate) fn ensure_restrictive_permissions(path: &Path) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path)
        .map_err(|error| AuthError::Storage(format!("stat `{}`: {error}", path.display())))?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(AuthError::InsecurePermissions {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn ensure_restrictive_permissions(path: &Path) -> Result<(), AuthError> {
    harden_secret_file(path)
}

#[cfg(unix)]
pub(crate) fn set_restrictive_permissions(path: &Path) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| AuthError::Storage(format!("chmod 0600 `{}`: {error}", path.display())))
}

#[cfg(windows)]
pub(crate) fn set_restrictive_permissions(path: &Path) -> Result<(), AuthError> {
    harden_secret_file(path)
}

#[cfg(windows)]
fn windows_powershell() -> std::path::PathBuf {
    let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::path::PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

/// Apply the platform's owner-only file policy to a secret-bearing file.
///
/// On Windows this replaces inherited and explicit ACEs with one FullControl
/// rule for the file owner. Passing the path through an environment variable
/// avoids interpolating attacker-controlled characters into PowerShell code.
#[cfg(windows)]
pub fn harden_secret_file(path: &Path) -> Result<(), AuthError> {
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$path = $env:LABBY_SECRET_FILE_PATH
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$icacls = Join-Path $env:SystemRoot 'System32\icacls.exe'
function Invoke-Icacls {
  & $icacls @args | Out-Null
  if ($LASTEXITCODE -ne 0) {
    throw "icacls failed with exit code $LASTEXITCODE for arguments: $args"
  }
}
# Reset removes every prior explicit ACE, then removing inheritance leaves an
# empty DACL before the current owner SID receives the sole FullControl rule.
Invoke-Icacls $path '/reset'
Invoke-Icacls $path '/inheritance:r'
Invoke-Icacls $path '/grant:r' ("*" + $sid + ':(F)')
$verified = Get-Acl -LiteralPath $path
if (-not $verified.AreAccessRulesProtected) {
  throw 'secret file ACL still inherits access rules'
}
$rules = @($verified.Access)
if ($rules.Count -ne 1) {
  throw "secret file ACL contains $($rules.Count) access rules instead of one"
}
if ($rules[0].IdentityReference.Value -ne $verified.Owner -or
    $rules[0].AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or
    ($rules[0].FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne
      [System.Security.AccessControl.FileSystemRights]::FullControl) {
  throw 'secret file ACL is not an owner-only FullControl rule'
}
"#;
    let output = std::process::Command::new(windows_powershell())
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ])
        // A PowerShell 7 host can export an incompatible PSModulePath to this
        // Windows PowerShell child. Let Windows PowerShell rebuild its native
        // module path so Microsoft.PowerShell.Security can load reliably.
        .env_remove("PSModulePath")
        .env("LABBY_SECRET_FILE_PATH", path)
        .output()
        .map_err(|error| {
            AuthError::Storage(format!(
                "start Windows ACL hardening for `{}`: {error}",
                path.display()
            ))
        })?;
    if !output.status.success() {
        return Err(AuthError::Storage(format!(
            "Windows ACL hardening failed for `{}`: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub fn harden_secret_file(path: &Path) -> Result<(), AuthError> {
    set_restrictive_permissions(path)
}

/// Durably publish a secret through a restricted same-directory temporary
/// file so the final path is never observable with default permissions or
/// partially written contents.
pub(crate) fn write_secret_file_atomically(path: &Path, contents: &[u8]) -> Result<(), AuthError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("secret");
    let mut last_collision = None;

    for attempt in 0..16_u8 {
        let temporary = parent.join(format!(
            ".{file_name}.tmp-{}-{}-{attempt}",
            std::process::id(),
            now_unix()
        ));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_collision = Some(error);
                continue;
            }
            Err(error) => {
                return Err(AuthError::Storage(format!(
                    "create temporary secret `{}`: {error}",
                    temporary.display()
                )));
            }
        };

        let publish = (|| {
            file.write_all(contents).map_err(|error| {
                AuthError::Storage(format!(
                    "write temporary secret `{}`: {error}",
                    temporary.display()
                ))
            })?;
            file.sync_all().map_err(|error| {
                AuthError::Storage(format!(
                    "sync temporary secret `{}`: {error}",
                    temporary.display()
                ))
            })?;
            drop(file);
            harden_secret_file(&temporary)?;
            std::fs::rename(&temporary, path).map_err(|error| {
                AuthError::Storage(format!("publish secret `{}`: {error}", path.display()))
            })?;
            ensure_restrictive_permissions(path)?;
            if let Ok(directory) = std::fs::File::open(parent) {
                directory.sync_all().map_err(|error| {
                    AuthError::Storage(format!(
                        "sync secret directory `{}`: {error}",
                        parent.display()
                    ))
                })?;
            }
            Ok(())
        })();

        if publish.is_err() {
            drop(std::fs::remove_file(&temporary));
        }
        return publish;
    }

    Err(AuthError::Storage(format!(
        "could not allocate a temporary secret beside `{}`: {}",
        path.display(),
        last_collision
            .map(|error| error.to_string())
            .unwrap_or_else(|| "name collision".to_string())
    )))
}

#[cfg(feature = "http-axum")]
pub(crate) fn duration_secs_i64(duration: Duration, field: &str) -> Result<i64, AuthError> {
    i64::try_from(duration.as_secs())
        .map_err(|_| AuthError::Config(format!("{field} exceeds supported range")))
}

#[cfg(feature = "http-axum")]
pub(crate) fn duration_secs_usize(duration: Duration, field: &str) -> Result<usize, AuthError> {
    usize::try_from(duration.as_secs())
        .map_err(|_| AuthError::Config(format!("{field} exceeds supported range")))
}

#[cfg(feature = "http-axum")]
pub(crate) fn timestamp_usize(timestamp: i64, field: &str) -> Result<usize, AuthError> {
    usize::try_from(timestamp)
        .map_err(|_| AuthError::Storage(format!("{field} is negative or exceeds usize range")))
}

#[cfg(feature = "http-axum")]
pub(crate) fn expires_at(
    created_at: i64,
    duration: Duration,
    field: &str,
) -> Result<i64, AuthError> {
    let ttl = duration_secs_i64(duration, field)?;
    created_at
        .checked_add(ttl)
        .ok_or_else(|| AuthError::Config(format!("{field} exceeds supported range")))
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn secret_acl_is_protected_and_contains_only_owner_rule() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.env");
        std::fs::write(&path, "TOKEN=secret\n").unwrap();
        harden_secret_file(&path).unwrap();

        let script = r#"
$acl = Get-Acl -LiteralPath $env:LABBY_SECRET_FILE_PATH
if (-not $acl.AreAccessRulesProtected) { exit 2 }
if (@($acl.Access).Count -ne 1) { exit 3 }
if ($acl.Access[0].IdentityReference.Value -ne $acl.Owner) { exit 4 }
"#;
        let status = std::process::Command::new(windows_powershell())
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env_remove("PSModulePath")
            .env("LABBY_SECRET_FILE_PATH", &path)
            .status()
            .unwrap();
        assert!(status.success(), "unexpected ACL shape: {status}");
    }
}
