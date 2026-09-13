use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use thiserror::Error;
use verify_scenario::{MAX_SCENARIO_BYTES, Scenario, ValidatedScenario};

/// Non-overwriting insertion result; first-discovery provenance is preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertResult {
    /// Complete artifact atomically published.
    Inserted,
    /// Same content identity already exists; existing provenance/status retained.
    Existing,
}

/// Corpus IO/integrity failure.
#[derive(Debug, Error)]
pub enum CorpusError {
    /// Filesystem failure.
    #[error("corpus IO: {0}")]
    Io(#[from] std::io::Error),
    /// Existing data is malformed, oversized or has a stale fingerprint.
    #[error("corpus envelope: {0}")]
    Envelope(#[from] verify_scenario::EnvelopeError),
    /// A corpus directory/file is a symlink or a different object type.
    #[error("corpus path must be an ordinary directory/file, not a symlink")]
    UnsafePath,
    /// Path already contains different content; never overwrite it.
    #[error("corpus destination contains a different scenario")]
    Conflict,
}

pub(crate) fn read_scenario(path: &Path) -> Result<ValidatedScenario, CorpusError> {
    // Do not enter a blocking FIFO/device open before any replay budget exists.
    // The caller-owned path must not be concurrently replaced (see insert docs).
    if !fs::metadata(path)?.is_file() {
        return Err(CorpusError::UnsafePath);
    }
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(CorpusError::UnsafePath);
    }
    let mut input = String::new();
    file.take(MAX_SCENARIO_BYTES as u64 + 1)
        .read_to_string(&mut input)?;
    Ok(Scenario::from_json(&input)?)
}

/// Publish a complete scenario at its content-addressed path without overwrite.
/// Root and its ancestors must be trusted, caller-owned directories: symlinks
/// below root are rejected, but this is not a hostile-filesystem sandbox against
/// concurrent directory replacement. No implicit status promotion occurs.
pub fn insert_scenario(
    root: &Path,
    scenario: &ValidatedScenario,
) -> Result<InsertResult, CorpusError> {
    fs::create_dir_all(root)?;
    if fs::symlink_metadata(root)?.file_type().is_symlink() || !root.is_dir() {
        return Err(CorpusError::UnsafePath);
    }
    let relative = scenario.corpus_path();
    let parent = relative.parent().ok_or(CorpusError::UnsafePath)?;
    let mut directory = root.to_path_buf();
    for component in parent.components() {
        directory.push(component);
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let kind = fs::symlink_metadata(&directory)?.file_type();
        if kind.is_symlink() || !kind.is_dir() {
            return Err(CorpusError::UnsafePath);
        }
    }
    let destination = root.join(relative);
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(&mut temporary, scenario.scenario())
        .map_err(verify_scenario::EnvelopeError::from)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&destination) {
        Ok(_) => Ok(InsertResult::Inserted),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let kind = fs::symlink_metadata(&destination)?.file_type();
            if kind.is_symlink() || !kind.is_file() {
                return Err(CorpusError::UnsafePath);
            }
            let existing = read_scenario(&destination)?;
            if existing.fingerprint() == scenario.fingerprint() {
                Ok(InsertResult::Existing)
            } else {
                Err(CorpusError::Conflict)
            }
        }
        Err(error) => Err(error.error.into()),
    }
}
