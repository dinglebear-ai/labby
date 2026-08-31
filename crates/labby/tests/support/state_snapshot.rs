use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Persistence {
    Durable,
    Ephemeral,
    StagedUntilRestart,
}

/// Locked persistence contract. Incidental files are intentionally excluded.
pub(crate) const PERSISTENCE_CONTRACT: &[(&str, Persistence)] = &[
    ("gateway config", Persistence::Durable),
    ("access projects and assignments", Persistence::Durable),
    ("snippets", Persistence::Durable),
    ("artifacts", Persistence::Durable),
    ("code mode workspaces", Persistence::Durable),
    ("sessions", Persistence::Durable),
    ("in-flight calls", Persistence::Ephemeral),
    ("process-local history", Persistence::Ephemeral),
    ("catalog generation", Persistence::Ephemeral),
    ("health warnings", Persistence::Ephemeral),
    (
        "protected route desired revision",
        Persistence::StagedUntilRestart,
    ),
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct OwnedProcessObservation {
    pub(crate) generation: u64,
    pub(crate) pid: Option<u32>,
    pub(crate) process_start_identity: Option<String>,
    pub(crate) listener_identity: Option<String>,
}

impl OwnedProcessObservation {
    pub(crate) fn read(root: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(root.join("ownership.json")).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    pub(crate) fn assert_restarted_from(&self, old: &Self) {
        assert!(
            self.generation > old.generation,
            "generation did not advance"
        );
        assert_ne!(self.pid, old.pid, "restart retained the old PID");
        assert_ne!(
            self.process_start_identity, old.process_start_identity,
            "restart retained the old process start identity"
        );
        assert_eq!(self.listener_identity, old.listener_identity);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PublicCatalogObservation {
    pub(crate) status: u16,
    pub(crate) services: Vec<String>,
}

impl PublicCatalogObservation {
    pub(crate) fn from_json(status: u16, body: &serde_json::Value) -> Self {
        let mut services = body
            .get("services")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|service| {
                service
                    .as_str()
                    .or_else(|| service.get("name").and_then(serde_json::Value::as_str))
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        services.sort();
        services.dedup();
        Self { status, services }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NarrowStorageObservation {
    hashes: BTreeMap<PathBuf, String>,
}

impl NarrowStorageObservation {
    /// Hash only explicitly named public/configuration files. Never recursively
    /// hash an installation root or expose access/OAuth record contents.
    pub(crate) fn read(root: &Path, relative: &[&str]) -> Result<Self, String> {
        let mut hashes = BTreeMap::new();
        for relative in relative {
            let path = PathBuf::from(relative);
            let absolute = root.join(&path);
            let digest = if absolute.exists() {
                hex::encode(Sha256::digest(
                    std::fs::read(&absolute).map_err(|e| e.to_string())?,
                ))
            } else {
                "absent".to_owned()
            };
            hashes.insert(path, digest);
        }
        Ok(Self { hashes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_table_is_explicit_and_unique() {
        let names = PERSISTENCE_CONTRACT
            .iter()
            .map(|(name, _)| *name)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), PERSISTENCE_CONTRACT.len());
        assert_eq!(PERSISTENCE_CONTRACT.len(), 11);
        assert!(PERSISTENCE_CONTRACT.contains(&("in-flight calls", Persistence::Ephemeral)));
        assert!(PERSISTENCE_CONTRACT.contains(&(
            "protected route desired revision",
            Persistence::StagedUntilRestart
        )));
    }

    #[test]
    fn narrow_storage_observation_ignores_unlisted_sensitive_state() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.toml"), "public = true").unwrap();
        std::fs::write(root.path().join("access.sqlite"), "secret auth material").unwrap();
        let before = NarrowStorageObservation::read(root.path(), &["config.toml"]).unwrap();
        std::fs::write(root.path().join("access.sqlite"), "changed secret").unwrap();
        let after = NarrowStorageObservation::read(root.path(), &["config.toml"]).unwrap();
        assert_eq!(before, after);
    }
}
