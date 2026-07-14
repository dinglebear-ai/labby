use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::types::{
    ImportReport, MachineId, PublicRelayEntry, PublicRelayError, PublicRelaySnapshot,
    QuarantinedEntry, RegistryWriteOutcome,
};

#[derive(Debug, Clone)]
pub struct PublicRelayRegistryStore {
    path: PathBuf,
}

const REGISTRY_FILE_VERSION: u16 = 1;

/// Maximum number of `registry.json.bak.*` sidecar files kept per save.
/// Each mutating save writes a fresh timestamped backup before replacing
/// the live file; without pruning, `~/.labby/oauth-public-relay/` grows one
/// backup per mutation forever.
pub(crate) const MAX_REGISTRY_BACKUPS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryFile {
    version: u16,
    entries: Vec<PublicRelayEntry>,
}

impl PublicRelayRegistryStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        super::policy::default_registry_path()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load_snapshot(&self) -> Result<PublicRelaySnapshot, PublicRelayError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || load_snapshot_sync(&path))
            .await
            .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?
    }

    pub async fn save_entries(
        &self,
        entries: Vec<PublicRelayEntry>,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || save_entries_sync(path, entries))
            .await
            .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?
    }

    pub fn parse_standalone_registry(raw: &str) -> Result<ImportReport, PublicRelayError> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
        parse_registry_value(value)
    }

    /// Count `<path>.bak.*` sidecar files currently on disk for this store's
    /// registry path.
    ///
    /// Backups are capped at [`MAX_REGISTRY_BACKUPS`] by design
    /// (`prune_old_backups` runs after every save), so a count exceeding
    /// that cap can only mean pruning has been persistently failing —
    /// `labby doctor oauth-relay` uses this to surface that otherwise
    /// invisible failure mode. Best-effort: returns 0 if the directory
    /// can't be listed, matching `prune_old_backups`'s own fail-open
    /// behavior for the same listing.
    pub fn count_backups(&self) -> usize {
        list_backup_paths(&self.path).len()
    }
}

/// Parse a standalone registry JSON value into an [`ImportReport`].
///
/// Dispatches on the JSON [`serde_json::Value`] *shape* first (array,
/// versioned-file object, object-of-entries, or flat machine->url map)
/// instead of try-cascading through whole-collection `serde_json::from_value`
/// attempts. The old cascade deserialized each candidate shape as a single
/// atomic operation: one invalid `machine_id` anywhere in an array or
/// object-of-entries payload made the *entire* attempt fail, so parsing fell
/// through to the next (wrong) shape and surfaced a misleading error such as
/// `invalid type: sequence, expected a map` instead of quarantining just the
/// offending entry. Dispatching on shape up front and parsing each entry
/// independently means a single bad entry is quarantined with a clear,
/// per-entry reason while the rest of a correctly-shaped registry still
/// imports normally.
pub fn parse_registry_value(value: serde_json::Value) -> Result<ImportReport, PublicRelayError> {
    match value {
        serde_json::Value::Array(items) => parse_array_shape(items),
        serde_json::Value::Object(map) => parse_object_shape(map),
        other => Err(PublicRelayError::InvalidRegistryInput(format!(
            "registry must be a JSON array or object, got {}",
            json_type_name(&other)
        ))),
    }
}

/// Parse a top-level JSON object. Detects the versioned `{"version", "entries"}`
/// file shape by *value type* (not mere key presence) so a flat map or
/// object-of-entries registry that happens to contain machines literally
/// named `version`/`entries` is never misread as the versioned-file shape.
/// Otherwise dispatches each key independently: a string value is a flat
/// `machine_id -> target_url` entry, an object value is a full entry object.
fn parse_object_shape(
    mut map: serde_json::Map<String, serde_json::Value>,
) -> Result<ImportReport, PublicRelayError> {
    let is_versioned_file = matches!(map.get("version"), Some(serde_json::Value::Number(_)))
        && matches!(map.get("entries"), Some(serde_json::Value::Array(_)));
    if is_versioned_file {
        let version = map
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                PublicRelayError::InvalidRegistryInput(
                    "registry version must be a non-negative integer".into(),
                )
            })?;
        if version != u64::from(REGISTRY_FILE_VERSION) {
            return Err(PublicRelayError::InvalidRegistryInput(format!(
                "unsupported registry version {version}; expected {REGISTRY_FILE_VERSION}"
            )));
        }
        let Some(serde_json::Value::Array(entries)) = map.remove("entries") else {
            unreachable!("is_versioned_file guarantees `entries` is a JSON array");
        };
        return parse_array_shape(entries);
    }

    let mut parsed_entries = Vec::new();
    let mut quarantined = Vec::new();
    for (machine_id, value) in map {
        match value {
            serde_json::Value::String(target_url) => match MachineId::parse(&machine_id) {
                Ok(machine) => {
                    parsed_entries.push(PublicRelayEntry::new(machine, target_url, None, false))
                }
                Err(error) => quarantined.push(QuarantinedEntry {
                    machine_id,
                    reason: error.to_string(),
                }),
            },
            serde_json::Value::Object(_) => {
                match serde_json::from_value::<PublicRelayEntry>(value) {
                    Ok(entry) if machine_id == entry.machine_id.as_str() => {
                        parsed_entries.push(entry);
                    }
                    Ok(entry) => quarantined.push(QuarantinedEntry {
                        machine_id,
                        reason: format!("entry machine_id is `{}`", entry.machine_id),
                    }),
                    Err(error) => quarantined.push(QuarantinedEntry {
                        machine_id,
                        reason: error.to_string(),
                    }),
                }
            }
            other => quarantined.push(QuarantinedEntry {
                machine_id,
                reason: format!(
                    "expected a target URL string or entry object, got {}",
                    json_type_name(&other)
                ),
            }),
        }
    }

    let mut report = import_entries(parsed_entries)?;
    quarantined.extend(report.quarantined);
    report.quarantined = quarantined;
    Ok(report)
}

/// Parse a top-level JSON array of entry objects, quarantining each item
/// that fails to deserialize into a [`PublicRelayEntry`] (including invalid
/// `machine_id` values) individually rather than failing the whole array.
fn parse_array_shape(items: Vec<serde_json::Value>) -> Result<ImportReport, PublicRelayError> {
    let mut parsed_entries = Vec::new();
    let mut quarantined = Vec::new();
    for item in items {
        let label = item
            .get("machine_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        match serde_json::from_value::<PublicRelayEntry>(item) {
            Ok(entry) => parsed_entries.push(entry),
            Err(error) => quarantined.push(QuarantinedEntry {
                machine_id: label.unwrap_or_else(|| "<unknown>".to_string()),
                reason: error.to_string(),
            }),
        }
    }

    let mut report = import_entries(parsed_entries)?;
    quarantined.extend(report.quarantined);
    report.quarantined = quarantined;
    Ok(report)
}

fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn import_entries(entries: Vec<PublicRelayEntry>) -> Result<ImportReport, PublicRelayError> {
    let mut report = ImportReport::empty();
    let mut accepted = BTreeMap::new();
    for entry in entries {
        let machine_id = entry.machine_id.to_string();
        match entry.target() {
            Ok(_) => {
                if accepted.contains_key(&entry.machine_id) {
                    report.quarantined.push(QuarantinedEntry {
                        machine_id,
                        reason: "duplicate machine_id".to_string(),
                    });
                    continue;
                }
                report.accepted.push(machine_id.clone());
                accepted.insert(entry.machine_id.clone(), entry);
            }
            Err(error) => report.quarantined.push(QuarantinedEntry {
                machine_id,
                reason: error.to_string(),
            }),
        }
    }
    report.entries = accepted.into_values().collect();
    Ok(report)
}

fn load_snapshot_sync(path: &Path) -> Result<PublicRelaySnapshot, PublicRelayError> {
    if !path.exists() {
        return Ok(PublicRelaySnapshot::default());
    }
    let raw = fs::read_to_string(path)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    let report = PublicRelayRegistryStore::parse_standalone_registry(&raw)?;
    if let Some(summary) = report.quarantine_summary() {
        return Err(PublicRelayError::InvalidTarget(format!(
            "registry contains invalid entries: {summary}"
        )));
    }
    let entries = report
        .entries
        .into_iter()
        .map(|entry| (entry.machine_id.clone(), entry))
        .collect();
    Ok(PublicRelaySnapshot { entries })
}

fn save_entries_sync(
    path: PathBuf,
    entries: Vec<PublicRelayEntry>,
) -> Result<RegistryWriteOutcome, PublicRelayError> {
    let report = import_entries(entries)?;
    if let Some(reason) = report.quarantine_summary() {
        return Err(PublicRelayError::InvalidTarget(reason));
    }

    let parent = path.parent().ok_or_else(|| {
        PublicRelayError::RegistryUnavailable("registry path has no parent".into())
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    let backup_path = if path.exists() {
        let backup = create_backup(&path)?;
        prune_old_backups(&path, MAX_REGISTRY_BACKUPS);
        Some(backup)
    } else {
        None
    };
    let file = RegistryFile {
        version: REGISTRY_FILE_VERSION,
        entries: report.entries,
    };
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    write_bytes_atomic(&path, parent, &bytes)?;
    Ok(RegistryWriteOutcome {
        path,
        backup_path,
        entry_count: file.entries.len(),
    })
}

fn write_bytes_atomic(path: &Path, parent: &Path, bytes: &[u8]) -> Result<(), PublicRelayError> {
    let mut tmp = NamedTempFile::new_in(parent)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    tmp.write_all(bytes)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    tmp.write_all(b"\n")
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    tmp.as_file()
        .sync_all()
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    tmp.persist(path).map_err(|error| {
        PublicRelayError::RegistryUnavailable(format!(
            "persist {} failed: {}",
            path.display(),
            error.error
        ))
    })?;
    sync_parent(parent)
}

fn create_backup(path: &Path) -> Result<PathBuf, PublicRelayError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?
        .as_millis();
    let backup = PathBuf::from(format!(
        "{}.bak.{now}.{}",
        path.display(),
        std::process::id()
    ));
    let mut src = File::open(path)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    let mut dst = open_backup_file(&backup)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    io::copy(&mut src, &mut dst)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    dst.sync_all()
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    Ok(backup)
}

/// Create the backup file owner-only (`0o600`) on Unix, matching the
/// primary registry file (written via `NamedTempFile`, which already
/// defaults to `0o600`). Plain `OpenOptions` without `.mode()` would create
/// it at the OS default (`0o666`, minus umask) — more permissive than the
/// file it's a copy of.
#[cfg(unix)]
fn open_backup_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_backup_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

/// Filter an already-opened directory listing down to `<path>.bak.*`
/// sidecars, paired with their mtime. Shared by `prune_old_backups` (which
/// needs the full list to decide what's stale) and
/// `PublicRelayRegistryStore::count_backups` (which only needs the count).
fn backup_entries_from_read_dir(path: &Path, read_dir: fs::ReadDir) -> Vec<(SystemTime, PathBuf)> {
    let mut backups: Vec<(SystemTime, PathBuf)> = Vec::new();
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return backups;
    };
    let prefix = format!("{file_name}.bak.");
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        backups.push((modified, entry.path()));
    }
    backups
}

/// List `<path>.bak.*` sidecar files alongside `path`. Fails open (returns
/// an empty list) if the directory can't be listed — used for read-only
/// inspection (`count_backups`), where the caller doesn't need the log
/// noise `prune_old_backups` emits for its own listing failure.
fn list_backup_paths(path: &Path) -> Vec<(SystemTime, PathBuf)> {
    let Some(parent) = path.parent() else {
        return Vec::new();
    };
    let Ok(read_dir) = fs::read_dir(parent) else {
        return Vec::new();
    };
    backup_entries_from_read_dir(path, read_dir)
}

/// Best-effort prune of stale `<path>.bak.*` sidecars, keeping the `keep`
/// most recently modified ones. Never fails the caller's save — pruning
/// is hygiene, not correctness, so a listing/removal error here is logged
/// and swallowed rather than propagated.
fn prune_old_backups(path: &Path, keep: usize) {
    let Some(parent) = path.parent() else {
        return;
    };
    let read_dir = match fs::read_dir(parent) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            tracing::warn!(
                surface = "store",
                service = "oauth_relay",
                action = "backup.prune",
                dir = %parent.display(),
                error = %error,
                "failed to list relay registry backup directory; skipping prune"
            );
            return;
        }
    };
    let mut backups = backup_entries_from_read_dir(path, read_dir);

    if backups.len() <= keep {
        return;
    }
    // Newest first, so `skip(keep)` yields exactly the stale tail.
    backups.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, stale) in backups.into_iter().skip(keep) {
        if let Err(error) = fs::remove_file(&stale) {
            tracing::warn!(
                surface = "store",
                service = "oauth_relay",
                action = "backup.prune",
                path = %stale.display(),
                error = %error,
                "failed to remove stale relay registry backup"
            );
        }
    }
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<(), PublicRelayError> {
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<(), PublicRelayError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE_REGISTRY_JSON: &str = r#"{
  "dookie": "http://100.88.16.79:38935/callback/dookie",
  "shart": "http://100.118.209.1:38935/callback/shart",
  "squirts": "http://100.75.111.118:38935/callback/squirts",
  "steamy": "http://100.119.83.39:38935/callback/steamy",
  "steamy-wsl": "http://100.74.16.82:38935/callback/steamy-wsl",
  "tootie": "http://100.120.242.29:38935/callback/tootie",
  "vivobook-wsl": "http://100.104.50.17:38935/callback/vivobook-wsl"
}"#;

    #[test]
    fn public_relay_store_imports_live_registry_json() {
        let report = PublicRelayRegistryStore::parse_standalone_registry(LIVE_REGISTRY_JSON)
            .expect("registry should parse");
        assert_eq!(report.accepted.len(), 7);
        assert!(report.quarantined.is_empty());
    }

    #[test]
    fn public_relay_store_imports_live_object_registry_json() {
        let raw = r#"{
            "dookie": {
                "machine_id": "dookie",
                "target_url": "http://100.88.16.79:38935/callback/dookie",
                "description": "dookie codex callback tailscale"
            },
            "steamy-wsl": {
                "machine_id": "steamy-wsl",
                "target_url": "http://100.74.16.82:38935/callback/steamy-wsl",
                "description": "steamy-wsl codex callback tailscale"
            }
        }"#;
        let report = PublicRelayRegistryStore::parse_standalone_registry(raw)
            .expect("object registry should parse");
        assert_eq!(report.accepted, vec!["dookie", "steamy-wsl"]);
        assert!(report.quarantined.is_empty());
    }

    #[test]
    fn public_relay_store_quarantines_object_registry_key_mismatch() {
        let raw = r#"{
            "dookie": {
                "machine_id": "tootie",
                "target_url": "http://100.120.242.29:38935/callback/tootie"
            }
        }"#;
        let report = PublicRelayRegistryStore::parse_standalone_registry(raw)
            .expect("object registry should parse");
        assert!(report.accepted.is_empty());
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(report.quarantined[0].machine_id, "dookie");
    }

    #[test]
    fn public_relay_store_quarantines_invalid_entries() {
        let raw = r#"{
            "dookie": "http://100.88.16.79:38935/callback/dookie",
            "bad": "http://127.0.0.1:38935/callback/bad"
        }"#;
        let report =
            PublicRelayRegistryStore::parse_standalone_registry(raw).expect("registry parses");
        assert_eq!(report.accepted, vec!["dookie"]);
        assert_eq!(report.quarantined.len(), 1);
    }

    #[test]
    fn public_relay_store_quarantines_duplicate_machine_ids() {
        let raw = r#"[
            {
                "machine_id": "dookie",
                "target_url": "http://100.88.16.79:38935/callback/dookie"
            },
            {
                "machine_id": "dookie",
                "target_url": "http://100.88.16.79:38935/callback/dookie",
                "description": "duplicate"
            }
        ]"#;
        let report =
            PublicRelayRegistryStore::parse_standalone_registry(raw).expect("registry parses");

        assert_eq!(report.accepted, vec!["dookie"]);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(report.quarantined[0].machine_id, "dookie");
        assert!(report.quarantined[0].reason.contains("duplicate"));
    }

    #[test]
    fn public_relay_store_rejects_unsupported_registry_version() {
        let raw = r#"{
            "version": 2,
            "entries": [
                {
                    "machine_id": "dookie",
                    "target_url": "http://100.88.16.79:38935/callback/dookie"
                }
            ]
        }"#;
        let error = PublicRelayRegistryStore::parse_standalone_registry(raw)
            .expect_err("future registry versions must be rejected");

        assert_eq!(error.kind(), "invalid_param");
        assert!(error.to_string().contains("unsupported registry version"));
    }

    #[test]
    fn public_relay_store_quarantines_invalid_machine_ids() {
        let raw = r#"{
            "bad/machine": "http://100.88.16.79:38935/callback/bad-machine"
        }"#;
        let report =
            PublicRelayRegistryStore::parse_standalone_registry(raw).expect("registry parses");

        assert!(report.accepted.is_empty());
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(report.quarantined[0].machine_id, "bad/machine");
    }

    // lab-021l8: an invalid `machine_id` inside an array-shaped registry must
    // quarantine just that entry with a clear, entry-specific reason — not
    // fail the whole array and fall through to a wrong-shape parser that
    // produces a misleading `invalid type: sequence, expected a map` error.
    #[test]
    fn public_relay_store_quarantines_invalid_machine_id_inside_array_shape() {
        let raw = r#"[
            {
                "machine_id": "dookie",
                "target_url": "http://100.88.16.79:38935/callback/dookie"
            },
            {
                "machine_id": "bad/machine",
                "target_url": "http://100.88.16.79:38935/callback/bad-machine"
            }
        ]"#;
        let report =
            PublicRelayRegistryStore::parse_standalone_registry(raw).expect("registry parses");

        assert_eq!(report.accepted, vec!["dookie"]);
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(report.quarantined[0].machine_id, "bad/machine");
        assert!(
            !report.quarantined[0].reason.contains("expected a map"),
            "quarantine reason should describe the invalid machine_id, not a wrong-shape parser error: {}",
            report.quarantined[0].reason
        );
    }

    // lab-021l8: same fix, applied to the object-of-entries shape — one
    // invalid machine_id in one entry must not abort parsing of the whole map.
    #[test]
    fn public_relay_store_quarantines_invalid_machine_id_inside_object_entries_shape() {
        let raw = r#"{
            "dookie": {
                "machine_id": "dookie",
                "target_url": "http://100.88.16.79:38935/callback/dookie"
            },
            "bad/machine": {
                "machine_id": "bad/machine",
                "target_url": "http://100.88.16.79:38935/callback/bad-machine"
            }
        }"#;
        let report =
            PublicRelayRegistryStore::parse_standalone_registry(raw).expect("registry parses");

        assert_eq!(report.accepted, vec!["dookie"]);
        assert_eq!(report.quarantined.len(), 1);
        assert_eq!(report.quarantined[0].machine_id, "bad/machine");
        assert!(
            !report.quarantined[0].reason.contains("expected a map"),
            "quarantine reason should describe the invalid machine_id, not a wrong-shape parser error: {}",
            report.quarantined[0].reason
        );
    }

    // lab-021l8: a versioned-file registry with a machine literally named
    // `entries`/`version` must not be misdetected as the versioned-file
    // shape purely by key presence — only `version: <number>` + `entries:
    // <array>` value *types* select that shape.
    #[test]
    fn public_relay_store_does_not_misdetect_flat_map_keys_named_like_versioned_file() {
        let raw = r#"{
            "version": "http://100.88.16.79:38935/callback/version",
            "entries": "http://100.120.242.29:38935/callback/entries"
        }"#;
        let report =
            PublicRelayRegistryStore::parse_standalone_registry(raw).expect("registry parses");

        let mut accepted = report.accepted.clone();
        accepted.sort();
        assert_eq!(accepted, vec!["entries", "version"]);
        assert!(report.quarantined.is_empty());
    }

    #[tokio::test]
    async fn public_relay_store_writes_backup_before_replacement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("registry.json");
        fs::write(&path, "not json").expect("seed corrupt registry");
        let store = PublicRelayRegistryStore::new(path.clone());
        let report = PublicRelayRegistryStore::parse_standalone_registry(LIVE_REGISTRY_JSON)
            .expect("registry should parse");

        let outcome = store
            .save_entries(report.entries)
            .await
            .expect("save should succeed");

        assert!(
            outcome
                .backup_path
                .as_ref()
                .is_some_and(|path| path.exists())
        );
        assert!(fs::read_to_string(&path).unwrap().contains("\"version\""));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn public_relay_store_backup_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("registry.json");
        fs::write(&path, "not json").expect("seed corrupt registry");
        let store = PublicRelayRegistryStore::new(path.clone());
        let report = PublicRelayRegistryStore::parse_standalone_registry(LIVE_REGISTRY_JSON)
            .expect("registry should parse");

        let outcome = store
            .save_entries(report.entries)
            .await
            .expect("save should succeed");

        let backup_path = outcome.backup_path.expect("backup should be written");
        let mode = fs::metadata(&backup_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "backup file must be owner-only (0o600), got {mode:o}"
        );
    }

    #[tokio::test]
    async fn public_relay_store_prunes_old_backups_keeping_most_recent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("registry.json");
        let store = PublicRelayRegistryStore::new(path.clone());

        // Seed the registry so subsequent saves have a pre-existing file to
        // back up. The first save (file did not exist yet) writes no backup.
        store
            .save_entries(vec![PublicRelayEntry::new(
                MachineId::parse("seed").unwrap(),
                "http://100.88.16.79:38935/callback/seed",
                None,
                false,
            )])
            .await
            .expect("seed save should succeed");

        // Seven more saves each produce a backup of the prior file.
        for i in 0..=MAX_REGISTRY_BACKUPS {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            store
                .save_entries(vec![PublicRelayEntry::new(
                    MachineId::parse(&format!("m{i}")).unwrap(),
                    format!("http://100.88.16.79:38935/callback/m{i}"),
                    None,
                    false,
                )])
                .await
                .expect("save should succeed");
        }

        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
            .collect();

        assert_eq!(
            backups.len(),
            MAX_REGISTRY_BACKUPS,
            "expected pruning to keep exactly {MAX_REGISTRY_BACKUPS} backups, found {}",
            backups.len()
        );
    }

    #[test]
    fn prune_old_backups_keeps_exactly_keep_most_recent_at_multiple_counts() {
        // Precise, deterministic unit test of `prune_old_backups` directly
        // (not through `save_entries`): synthetic backup files get explicit,
        // distinct mtimes via `File::set_modified` instead of relying on
        // `tokio::time::sleep` between real saves to establish ordering.
        for total in [
            MAX_REGISTRY_BACKUPS - 1,
            MAX_REGISTRY_BACKUPS,
            MAX_REGISTRY_BACKUPS + 1,
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("registry.json");

            let mut backups = Vec::new();
            for i in 0..total {
                let backup_path = dir.path().join(format!("registry.json.bak.{i}"));
                fs::write(&backup_path, b"{}").expect("write synthetic backup");
                let mtime = SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(1_700_000_000 + i as u64);
                // `File::open` only requests read access. On Windows,
                // `SetFileTime` (what `set_modified` calls under the hood)
                // needs a handle opened with write access -- a read-only
                // handle fails with `PermissionDenied`. Open for write
                // explicitly so this works cross-platform.
                OpenOptions::new()
                    .write(true)
                    .open(&backup_path)
                    .expect("open synthetic backup for write")
                    .set_modified(mtime)
                    .expect("set synthetic mtime");
                backups.push(backup_path);
            }

            prune_old_backups(&path, MAX_REGISTRY_BACKUPS);

            let remaining: Vec<_> = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
                .collect();

            let expected_remaining = total.min(MAX_REGISTRY_BACKUPS);
            assert_eq!(
                remaining.len(),
                expected_remaining,
                "total={total}: expected {expected_remaining} backups to remain, found {}",
                remaining.len()
            );

            // The newest `expected_remaining` backups (highest index / mtime)
            // must be the ones that survive; older ones must be removed.
            let survivor_cutoff = total.saturating_sub(expected_remaining);
            for (i, backup_path) in backups.iter().enumerate() {
                let should_survive = i >= survivor_cutoff;
                assert_eq!(
                    backup_path.exists(),
                    should_survive,
                    "total={total}, index={i}: expected exists={should_survive}"
                );
            }
        }
    }

    #[tokio::test]
    async fn public_relay_store_rejects_quarantined_persisted_registry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("registry.json");
        fs::write(
            &path,
            r#"{
                "dookie": "http://100.88.16.79:38935/callback/dookie",
                "bad": "http://127.0.0.1:38935/callback/bad"
            }"#,
        )
        .expect("write registry");
        let store = PublicRelayRegistryStore::new(path);

        let error = store
            .load_snapshot()
            .await
            .expect_err("invalid persisted entries must fail closed");

        assert_eq!(error.kind(), "relay_invalid_target");
        assert!(error.to_string().contains("bad"));
    }
}
