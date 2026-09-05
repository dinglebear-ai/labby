//! Read/write `.env.draft` via the shared `env_merge` primitive.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::dispatch::error::ToolError;
use crate::dispatch::setup::DraftEntry;

use crate::config::env_merge::{
    self, EnvEntry, MergeError, MergeOutcome, MergeRequest, strip_quotes,
};

/// Parse an `.env`-style file into `(key, value)` entries. Comments and
/// blank lines are dropped; quoted values are unwrapped.
#[derive(Debug, Error)]
pub enum DraftReadError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("invalid environment assignment in {path} at line {line}")]
    Malformed {
        path: std::path::PathBuf,
        line: usize,
    },
    #[error("draft path is a symbolic link: {0}")]
    InsecurePath(std::path::PathBuf),
    #[error("draft changed after it was read: {0}")]
    Changed(std::path::PathBuf),
    #[error("atomic no-replace draft quarantine is unsupported on this platform/build: {0}")]
    Unsupported(std::path::PathBuf),
}

impl From<DraftReadError> for ToolError {
    fn from(error: DraftReadError) -> Self {
        Self::Sdk {
            sdk_kind: "config_read_error".into(),
            message: error.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct DraftSnapshot {
    pub entries: Vec<DraftEntry>,
    raw: Vec<u8>,
}

static CLAIM_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct ClaimedDraft {
    original: std::path::PathBuf,
    claimed: std::path::PathBuf,
}

impl ClaimedDraft {
    pub fn discard(self) -> std::io::Result<bool> {
        discard(&self.claimed)
    }

    pub fn restore(self) -> std::io::Result<()> {
        restore_quarantine(&self.claimed, &self.original).map_err(draft_error_to_io)
    }
}

impl DraftSnapshot {
    #[cfg(test)]
    pub fn ensure_unchanged(&self, path: &Path) -> Result<(), DraftReadError> {
        reject_symlink(path)?;
        let current = fs::read(path).map_err(|source| DraftReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if current == self.raw {
            Ok(())
        } else {
            Err(DraftReadError::Changed(path.to_path_buf()))
        }
    }

    /// Atomically move the verified draft aside so later path replacement can
    /// never cause commit cleanup to unlink an attacker-controlled substitute.
    /// Unix builds use the OS atomic no-replace primitive in every product slice.
    pub fn claim(&self, path: &Path) -> Result<ClaimedDraft, DraftReadError> {
        let claimed = quarantine_path(path);
        self.claim_to(path, &claimed)
    }

    fn claim_to(&self, path: &Path, claimed: &Path) -> Result<ClaimedDraft, DraftReadError> {
        move_noreplace(path, claimed)?;
        if fs::symlink_metadata(claimed).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            drop(restore_quarantine(claimed, path));
            return Err(DraftReadError::InsecurePath(claimed.to_path_buf()));
        }
        let current = read_nofollow(&claimed)?;
        if current != self.raw {
            drop(restore_quarantine(claimed, path));
            return Err(DraftReadError::Changed(path.to_path_buf()));
        }
        Ok(ClaimedDraft {
            original: path.to_path_buf(),
            claimed: claimed.to_path_buf(),
        })
    }
}

#[cfg(any(test, not(any(unix, windows))))]
fn reject_symlink(path: &Path) -> Result<(), DraftReadError> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(DraftReadError::InsecurePath(path.to_path_buf()));
    }
    Ok(())
}

pub fn read_snapshot(path: &Path) -> Result<DraftSnapshot, DraftReadError> {
    let raw = read_nofollow(path)?;
    let text = std::str::from_utf8(&raw).map_err(|source| DraftReadError::Read {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    let entries = text
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            Some((index + 1, trimmed))
        })
        .map(|(line, trimmed)| {
            let (key, value) = trimmed
                .split_once('=')
                .filter(|(key, _)| !key.trim().is_empty())
                .ok_or_else(|| DraftReadError::Malformed {
                    path: path.to_path_buf(),
                    line,
                })?;
            let key = key.trim();
            if !key.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(DraftReadError::Malformed {
                    path: path.to_path_buf(),
                    line,
                });
            }
            Ok(DraftEntry {
                key: key.to_string(),
                value: strip_quotes(value.trim()),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DraftSnapshot { entries, raw })
}

#[cfg(unix)]
fn read_nofollow(path: &Path) -> Result<Vec<u8>, DraftReadError> {
    use nix::fcntl::{OFlag, open};
    use nix::sys::stat::Mode;

    let fd = open(path, OFlag::O_RDONLY | OFlag::O_NOFOLLOW, Mode::empty()).map_err(|error| {
        DraftReadError::Read {
            path: path.to_path_buf(),
            source: std::io::Error::from_raw_os_error(error as i32),
        }
    })?;
    let mut file = fs::File::from(fd);
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .map_err(|source| DraftReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(raw)
}

#[cfg(windows)]
fn read_nofollow(path: &Path) -> Result<Vec<u8>, DraftReadError> {
    use std::os::windows::fs::OpenOptionsExt;

    let claimed = quarantine_path(path);
    fs::rename(path, &claimed).map_err(|source| DraftReadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&claimed)
            .map_err(|source| DraftReadError::Read {
                path: claimed.clone(),
                source,
            })?;
        if file
            .metadata()
            .map_err(|source| DraftReadError::Read {
                path: claimed.clone(),
                source,
            })?
            .file_type()
            .is_symlink()
        {
            return Err(DraftReadError::InsecurePath(claimed.clone()));
        }
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)
            .map_err(|source| DraftReadError::Read {
                path: claimed.clone(),
                source,
            })?;
        Ok(raw)
    })();
    if restore_quarantine(&claimed, path).is_err() {
        return Err(DraftReadError::Changed(path.to_path_buf()));
    }
    result
}

#[cfg(not(any(unix, windows)))]
fn read_nofollow(path: &Path) -> Result<Vec<u8>, DraftReadError> {
    reject_symlink(path)?;
    fs::read(path).map_err(|source| DraftReadError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn quarantine_path(path: &Path) -> std::path::PathBuf {
    let suffix = CLAIM_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("draft.read.{}.{}", std::process::id(), suffix))
}

fn restore_quarantine(claimed: &Path, original: &Path) -> Result<(), DraftReadError> {
    move_noreplace(claimed, original)
}

#[cfg(all(test, unix))]
fn restore_quarantine_with(
    claimed: &Path,
    original: &Path,
    before_move: impl FnOnce(),
) -> Result<(), DraftReadError> {
    before_move();
    move_noreplace(claimed, original)
}

/// Move a regular draft without ever replacing the destination.
///
/// `renameat2(RENAME_NOREPLACE)`/`renamex_np(RENAME_EXCL)` is used through
/// rustix on Unix. Builds without an OS atomic no-replace primitive fail closed
/// and leave both paths untouched.
#[cfg(unix)]
fn move_noreplace(source: &Path, destination: &Path) -> Result<(), DraftReadError> {
    use rustix::fs::{RenameFlags, renameat_with};

    match renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::NOSYS | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::INVAL) => {
            // A hard-link-then-unlink emulation is not safe: the source path
            // can be replaced between those operations, causing the unlink to
            // delete an inode we never verified. Fail closed when the OS does
            // not provide an atomic no-replace rename primitive.
            Err(DraftReadError::Unsupported(source.to_path_buf()))
        }
        Err(error) => Err(DraftReadError::Read {
            path: source.to_path_buf(),
            source: std::io::Error::from_raw_os_error(error.raw_os_error()),
        }),
    }
}

#[cfg(test)]
fn unsupported_move_noreplace(source: &Path, _destination: &Path) -> Result<(), DraftReadError> {
    Err(DraftReadError::Unsupported(source.to_path_buf()))
}

#[cfg(not(unix))]
fn move_noreplace(source: &Path, destination: &Path) -> Result<(), DraftReadError> {
    if destination.exists() {
        return Err(DraftReadError::Changed(destination.to_path_buf()));
    }
    fs::rename(source, destination).map_err(|source_error| DraftReadError::Read {
        path: source.to_path_buf(),
        source: source_error,
    })
}

fn draft_error_to_io(error: DraftReadError) -> std::io::Error {
    let kind = match error {
        DraftReadError::Unsupported(_) => std::io::ErrorKind::Unsupported,
        DraftReadError::Changed(_) => std::io::ErrorKind::AlreadyExists,
        _ => std::io::ErrorKind::Other,
    };
    std::io::Error::new(kind, error)
}

pub fn read_entries(path: &Path) -> Result<Vec<DraftEntry>, DraftReadError> {
    Ok(read_snapshot(path)?.entries)
}

/// Merge `entries` into `path` (typically `.env.draft`).
pub fn merge_entries(
    path: &Path,
    entries: Vec<DraftEntry>,
    force: bool,
) -> Result<MergeOutcome, MergeError> {
    env_merge::merge(
        path,
        MergeRequest {
            entries: entries
                .into_iter()
                .map(|e| EnvEntry::new(e.key, e.value))
                .collect(),
            force,
            expected_mtime: None,
        },
    )
}

pub fn discard(path: &Path) -> std::io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_quoted_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        merge_entries(
            &path,
            vec![
                DraftEntry {
                    key: "FOO".into(),
                    value: "bar baz".into(),
                },
                DraftEntry {
                    key: "BAZ".into(),
                    value: "qux".into(),
                },
            ],
            false,
        )
        .unwrap();
        let entries = read_entries(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, "FOO");
        assert_eq!(entries[0].value, "bar baz");
        assert_eq!(entries[1].key, "BAZ");
        assert_eq!(entries[1].value, "qux");
    }

    #[test]
    fn malformed_non_comment_line_fails_instead_of_appearing_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        fs::write(&path, "THIS IS NOT AN ENV ASSIGNMENT\n").unwrap();

        let error = read_entries(&path).unwrap_err();

        assert!(error.to_string().contains("line 1"));
        assert!(path.exists(), "reading malformed input must preserve it");

        fs::write(&path, "NOT A KEY=value\n").unwrap();
        assert!(read_entries(&path).is_err());
    }

    #[test]
    fn invalid_utf8_and_directory_paths_fail_instead_of_appearing_empty() {
        let dir = tempfile::tempdir().unwrap();
        let invalid_utf8 = dir.path().join("invalid.env.draft");
        fs::write(&invalid_utf8, [0xff, b'=', b'1']).unwrap();

        assert!(read_entries(&invalid_utf8).is_err());
        assert!(read_entries(dir.path()).is_err());
        assert!(
            invalid_utf8.exists(),
            "failed reads must preserve exact bytes"
        );
    }

    #[test]
    fn audited_draft_snapshot_rejects_same_length_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        fs::write(&path, "LABBY_LOG=info\n").unwrap();
        let snapshot = read_snapshot(&path).unwrap();
        fs::write(&path, "LABBY_LOG=evil\n").unwrap();

        assert!(snapshot.ensure_unchanged(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "LABBY_LOG=evil\n");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn claimed_draft_cleanup_never_deletes_a_replacement_at_the_original_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        fs::write(&path, "LABBY_LOG=info\n").unwrap();
        let claimed = read_snapshot(&path).unwrap().claim(&path).unwrap();
        fs::write(&path, "LABBY_LOG=replacement\n").unwrap();
        assert!(claimed.discard().unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "LABBY_LOG=replacement\n"
        );
    }

    #[test]
    fn quarantine_restore_never_overwrites_a_concurrent_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        let quarantined = quarantine_path(&path);
        fs::write(&path, "ORIGINAL=1\n").unwrap();
        fs::rename(&path, &quarantined).unwrap();
        fs::write(&path, "REPLACEMENT=1\n").unwrap();

        assert!(restore_quarantine(&quarantined, &path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "REPLACEMENT=1\n");
    }

    #[test]
    fn unsupported_noreplace_fallback_preserves_source_and_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        fs::write(&source, "SOURCE\n").unwrap();
        fs::write(&destination, "DESTINATION\n").unwrap();

        assert!(matches!(
            unsupported_move_noreplace(&source, &destination),
            Err(DraftReadError::Unsupported(_))
        ));
        assert_eq!(fs::read_to_string(&source).unwrap(), "SOURCE\n");
        assert_eq!(fs::read_to_string(&destination).unwrap(), "DESTINATION\n");
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_claim_never_overwrites_a_precreated_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        let quarantined = dir.path().join("claimed");
        fs::write(&path, "ORIGINAL=1\n").unwrap();
        fs::write(&quarantined, "ATTACKER=1\n").unwrap();
        let snapshot = read_snapshot(&path).unwrap();

        assert!(snapshot.claim_to(&path, &quarantined).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "ORIGINAL=1\n");
        assert_eq!(fs::read_to_string(&quarantined).unwrap(), "ATTACKER=1\n");
    }

    #[cfg(unix)]
    #[test]
    fn restore_hook_replacement_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        let quarantined = dir.path().join("claimed");
        fs::write(&quarantined, "ORIGINAL=1\n").unwrap();

        let result = restore_quarantine_with(&quarantined, &path, || {
            fs::write(&path, "REPLACEMENT=1\n").unwrap();
        });

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "REPLACEMENT=1\n");
        assert_eq!(fs::read_to_string(&quarantined).unwrap(), "ORIGINAL=1\n");
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_draft_is_not_followed() {
        use std::os::windows::fs::symlink_file;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.env");
        let path = dir.path().join(".env.draft");
        fs::write(&target, "SECRET=untouched\n").unwrap();
        if symlink_file(&target, &path).is_err() {
            return; // Symlink creation requires Developer Mode on some Windows hosts.
        }
        assert!(read_snapshot(&path).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "SECRET=untouched\n");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_swap_between_snapshot_and_claim_never_traverses_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        let target = dir.path().join("secret");
        fs::write(&path, "LABBY_LOG=info\n").unwrap();
        fs::write(&target, "do-not-read-or-delete\n").unwrap();
        let snapshot = read_snapshot(&path).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();

        assert!(matches!(
            snapshot.claim(&path),
            Err(DraftReadError::InsecurePath(_))
        ));
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "do-not-read-or-delete\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_drafts_are_rejected_without_touching_the_target() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.env");
        let draft = dir.path().join(".env.draft");
        fs::write(&target, "LABBY_LOG=info\n").unwrap();
        symlink(&target, &draft).unwrap();

        assert!(read_entries(&draft).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "LABBY_LOG=info\n");
    }

    #[test]
    fn discard_removes_existing_draft_and_reports_missing_as_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env.draft");
        fs::write(&path, "LABBY_TEST=1\n").unwrap();

        assert!(discard(&path).unwrap());
        assert!(!path.exists());
        assert!(!discard(&path).unwrap());
    }
}
