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
}

pub fn parse_registry_value(value: serde_json::Value) -> Result<ImportReport, PublicRelayError> {
    if let Ok(file) = serde_json::from_value::<RegistryFile>(value.clone()) {
        return import_entries(file.entries);
    }
    if let Ok(entries) = serde_json::from_value::<Vec<PublicRelayEntry>>(value.clone()) {
        return import_entries(entries);
    }
    let map = serde_json::from_value::<BTreeMap<String, String>>(value)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    import_map_entries(map)
}

fn import_map_entries(entries: BTreeMap<String, String>) -> Result<ImportReport, PublicRelayError> {
    let mut parsed_entries = Vec::new();
    let mut quarantined = Vec::new();
    for (machine_id, target_url) in entries {
        match MachineId::parse(&machine_id) {
            Ok(machine) => parsed_entries.push(PublicRelayEntry {
                machine_id: machine,
                target_url,
                description: None,
                disabled: false,
            }),
            Err(error) => quarantined.push(QuarantinedEntry {
                machine_id,
                reason: error.to_string(),
            }),
        }
    }

    let mut report = import_entries(parsed_entries)?;
    quarantined.extend(report.quarantined);
    report.quarantined = quarantined;
    Ok(report)
}

fn import_entries(entries: Vec<PublicRelayEntry>) -> Result<ImportReport, PublicRelayError> {
    let mut report = ImportReport::empty();
    let mut accepted = BTreeMap::new();
    for entry in entries {
        let machine_id = entry.machine_id.to_string();
        match entry.target() {
            Ok(_) => {
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
        Some(create_backup(&path)?)
    } else {
        None
    };
    let file = RegistryFile {
        version: 1,
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
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    io::copy(&mut src, &mut dst)
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    dst.sync_all()
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
    Ok(backup)
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
