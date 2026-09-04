#![allow(clippy::redundant_pub_crate)]

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
#[cfg(feature = "http-axum")]
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

use crate::error::AuthError;

fn validate_restricted_file_path(path: &Path) -> Result<(), AuthError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AuthError::Storage(
            "restricted files require an absolute, traversal-free path".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_restricted_unix(path: &Path, create_new: bool) -> Result<(std::fs::File, bool), AuthError> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::os::fd::AsFd;

    let parent = path
        .parent()
        .ok_or_else(|| AuthError::Storage("restricted file path has no parent".into()))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| AuthError::Storage(format!("resolve restricted file parent: {error}")))?;
    let parent = std::fs::File::open(parent)
        .map_err(|error| AuthError::Storage(format!("open restricted file parent: {error}")))?;
    let name = path
        .file_name()
        .ok_or_else(|| AuthError::Storage("restricted file path has no file name".into()))?;
    let base = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;

    if !create_new {
        match openat(parent.as_fd(), name, base, Mode::empty()) {
            Ok(fd) => return Ok((std::fs::File::from(fd), true)),
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => {
                return Err(AuthError::Storage(format!(
                    "open existing restricted file: {error}"
                )));
            }
        }
    }

    let fd = openat(
        parent.as_fd(),
        name,
        base | OFlags::CREATE | OFlags::EXCL,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| AuthError::Storage(format!("create restricted file: {error}")))?;
    Ok((std::fs::File::from(fd), false))
}

fn reject_final_component_symlink(path: &Path) -> Result<bool, AuthError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AuthError::Storage(
            "restricted file path must not be a symbolic link".into(),
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AuthError::Storage(format!(
            "inspect restricted file path: {error}"
        ))),
    }
}

#[cfg(unix)]
fn harden_open_file(file: &std::fs::File, _path: &Path) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| AuthError::Storage(format!("harden open restricted file: {error}")))
}

#[cfg(windows)]
fn harden_open_file(file: &std::fs::File, path: &Path) -> Result<(), AuthError> {
    harden_secret_file(path)?;
    if same_open_file(file, path)? {
        Ok(())
    } else {
        Err(AuthError::Storage(
            "restricted file changed while its ACL was hardened".into(),
        ))
    }
}

#[cfg(unix)]
fn same_open_file(file: &std::fs::File, path: &Path) -> Result<bool, AuthError> {
    use std::os::unix::fs::MetadataExt;

    let opened = file
        .metadata()
        .map_err(|error| AuthError::Storage(format!("inspect open restricted file: {error}")))?;
    let current = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(AuthError::Storage(format!(
                "inspect restricted file path: {error}"
            )));
        }
    };
    Ok(!current.file_type().is_symlink()
        && opened.dev() == current.dev()
        && opened.ino() == current.ino())
}

#[cfg(windows)]
fn same_open_file(file: &std::fs::File, path: &Path) -> Result<bool, AuthError> {
    use std::os::windows::fs::MetadataExt;

    let opened = file
        .metadata()
        .map_err(|error| AuthError::Storage(format!("inspect open restricted file: {error}")))?;
    let current = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(AuthError::Storage(format!(
                "inspect restricted file path: {error}"
            )));
        }
    };
    Ok(!current.file_type().is_symlink()
        && opened.volume_serial_number() == current.volume_serial_number()
        && opened.file_index() == current.file_index())
}

pub fn now_unix() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    i64::try_from(secs).unwrap_or(i64::MAX)
}

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

/// Apply the platform's current-user-only file policy to a secret-bearing file.
///
/// On Windows this replaces inherited and explicit ACEs with one FullControl
/// rule for the current user. Passing the path through an environment variable
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
# empty DACL before the current user SID receives the sole FullControl rule.
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
$ruleSid = $rules[0].IdentityReference.Translate(
  [System.Security.Principal.SecurityIdentifier]
).Value
if ($ruleSid -ne $sid -or
    $rules[0].AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or
    ($rules[0].FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne
      [System.Security.AccessControl.FileSystemRights]::FullControl) {
  throw 'secret file ACL is not a current-user-only FullControl rule'
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

/// Create a new, empty secret file and apply the platform-private access policy
/// before returning the handle to code that can write sensitive bytes.
pub fn create_restricted_secret_file(path: &Path) -> Result<std::fs::File, AuthError> {
    validate_restricted_file_path(path)?;
    reject_final_component_symlink(path)?;
    #[cfg(unix)]
    let file = open_restricted_unix(path, true)?.0;
    #[cfg(not(unix))]
    let file = {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_SHARE_WRITE: u32 = 0x0000_0002;
            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(path).map_err(|error| {
            AuthError::Storage(format!(
                "create restricted secret `{}`: {error}",
                path.display()
            ))
        })?
    };
    if let Err(error) = harden_open_file(&file, path) {
        let still_same = same_open_file(&file, path).map_err(|identity_error| {
            AuthError::Storage(format!(
                "{error}; verify failed restricted file before cleanup: {identity_error}"
            ))
        })?;
        drop(file);
        if still_same && let Err(cleanup) = std::fs::remove_file(path) {
            return Err(AuthError::Storage(format!(
                "{error}; cleanup newly-created restricted file failed: {cleanup}"
            )));
        }
        return Err(error);
    }
    Ok(file)
}

/// Open a persistent secret-adjacent lock and harden it immediately. A lock
/// created by this call is removed on hardening failure; both failures are
/// retained when cleanup also fails.
pub fn open_restricted_lock_file(path: &Path) -> Result<std::fs::File, AuthError> {
    open_restricted_lock_file_with(path, harden_open_file, |path| std::fs::remove_file(path))
}

#[doc(hidden)]
pub fn open_restricted_lock_file_with<H, R>(
    path: &Path,
    harden: H,
    remove: R,
) -> Result<std::fs::File, AuthError>
where
    H: FnOnce(&std::fs::File, &Path) -> Result<(), AuthError>,
    R: FnOnce(&Path) -> std::io::Result<()>,
{
    validate_restricted_file_path(path)?;
    #[cfg(not(unix))]
    let existed = reject_final_component_symlink(path)?;
    #[cfg(unix)]
    reject_final_component_symlink(path)?;
    #[cfg(unix)]
    let (file, existed) = open_restricted_unix(path, false)?;
    #[cfg(not(unix))]
    let file = {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_SHARE_WRITE: u32 = 0x0000_0002;
            options
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        options.open(path).map_err(|error| {
            AuthError::Storage(format!("open lock `{}`: {error}", path.display()))
        })?
    };
    if let Err(error) = harden(&file, path) {
        let still_same = same_open_file(&file, path)?;
        drop(file);
        if !existed
            && still_same
            && let Err(cleanup) = remove(path)
        {
            return Err(AuthError::Storage(format!(
                "{error}; cleanup lock `{}` failed: {cleanup}",
                path.display()
            )));
        }
        return Err(error);
    }
    Ok(file)
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
        let mut file = match create_restricted_secret_file(&temporary) {
            Ok(file) => file,
            Err(AuthError::Storage(message)) if temporary.exists() => {
                last_collision = Some(message);
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
        last_collision.unwrap_or_else(|| "name collision".to_string())
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

#[cfg(test)]
mod restricted_lock_tests {
    use super::*;

    fn denied(_file: &std::fs::File, _path: &Path) -> Result<(), AuthError> {
        Err(AuthError::Storage("hardening denied".into()))
    }

    #[test]
    fn new_lock_is_removed_when_hardening_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.lock");
        let error =
            open_restricted_lock_file_with(&path, denied, |path| std::fs::remove_file(path))
                .unwrap_err();
        assert!(error.to_string().contains("hardening denied"));
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_failure_is_compounded_with_hardening_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.lock");
        let error = open_restricted_lock_file_with(&path, denied, |_| {
            Err(std::io::Error::other("cleanup denied"))
        })
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("hardening denied"), "{message}");
        assert!(message.contains("cleanup denied"), "{message}");
    }

    #[test]
    fn preexisting_lock_is_preserved_when_hardening_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.lock");
        std::fs::write(&path, b"sentinel").unwrap();
        let mut removal_attempted = false;
        let error = open_restricted_lock_file_with(&path, denied, |_| {
            removal_attempted = true;
            Ok(())
        })
        .unwrap_err();
        assert!(!removal_attempted, "preexisting lock must not be removed");
        assert!(error.to_string().contains("hardening denied"));
        assert_eq!(std::fs::read(&path).unwrap(), b"sentinel");
    }

    #[cfg(unix)]
    #[test]
    fn replacement_is_not_removed_after_hardening_race() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.lock");
        let displaced = dir.path().join("displaced.lock");
        let error = open_restricted_lock_file_with(
            &path,
            |_, path| {
                std::fs::rename(path, &displaced).unwrap();
                std::fs::write(path, b"replacement").unwrap();
                Err(AuthError::Storage("hardening denied".into()))
            },
            |path| std::fs::remove_file(path),
        )
        .unwrap_err();

        assert!(error.to_string().contains("hardening denied"));
        assert_eq!(std::fs::read(path).unwrap(), b"replacement");
        assert!(displaced.exists());
    }

    #[test]
    fn restricted_files_reject_relative_paths() {
        let error = create_restricted_secret_file(Path::new("secret.env")).unwrap_err();
        assert!(error.to_string().contains("absolute"));
    }

    #[cfg(unix)]
    #[test]
    fn restricted_files_resolve_symbolic_link_ancestors_before_opening() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, &link).unwrap();

        let file = create_restricted_secret_file(&link.join("secret.env")).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        assert!(target.join("secret.env").exists());
    }

    #[cfg(unix)]
    #[test]
    fn restricted_files_reject_final_component_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("config.lock");
        std::fs::write(&target, b"sentinel").unwrap();
        symlink(&target, &link).unwrap();

        let error = open_restricted_lock_file(&link).unwrap_err();
        assert!(error.to_string().contains("symbolic link"));
        assert_eq!(std::fs::read(target).unwrap(), b"sentinel");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn secret_acl_is_protected_and_contains_only_current_user_rule() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.env");
        std::fs::write(&path, "TOKEN=secret\n").unwrap();
        harden_secret_file(&path).unwrap();

        let script = r#"
$acl = Get-Acl -LiteralPath $env:LABBY_SECRET_FILE_PATH
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
if (-not $acl.AreAccessRulesProtected) { exit 2 }
if (@($acl.Access).Count -ne 1) { exit 3 }
$ruleSid = $acl.Access[0].IdentityReference.Translate(
  [System.Security.Principal.SecurityIdentifier]
).Value
if ($ruleSid -ne $sid) { exit 4 }
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

    #[test]
    fn newly_created_secret_is_private_before_callers_can_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new-secret.env");
        let file = create_restricted_secret_file(&path).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        drop(file);

        assert_private_windows_acl(&path);
    }

    fn assert_private_windows_acl(path: &Path) {
        let script = r#"
$acl = Get-Acl -LiteralPath $env:LABBY_SECRET_FILE_PATH
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
if (-not $acl.AreAccessRulesProtected) { exit 2 }
if (@($acl.Access).Count -ne 1) { exit 3 }
$ruleSid = $acl.Access[0].IdentityReference.Translate(
  [System.Security.Principal.SecurityIdentifier]
).Value
if ($ruleSid -ne $sid) { exit 4 }
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
            .env("LABBY_SECRET_FILE_PATH", path)
            .status()
            .unwrap();
        assert!(status.success(), "unexpected ACL shape: {status}");
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn newly_created_secret_is_private_before_callers_can_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new-secret.env");
        let file = create_restricted_secret_file(&path).unwrap();

        assert_eq!(file.metadata().unwrap().len(), 0);
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}
