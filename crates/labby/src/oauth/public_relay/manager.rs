use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};

use super::policy::{PUBLIC_GLOBAL_CONCURRENCY, PUBLIC_PER_MACHINE_CONCURRENCY};
use super::store::PublicRelayRegistryStore;
use super::types::{
    ImportReport, MachineId, PublicRelayEntry, PublicRelayError, PublicRelayMachineView,
    PublicRelaySnapshot, RegistryWriteOutcome, RelayTarget,
};

#[derive(Clone)]
pub struct PublicRelayRegistryManager {
    store: PublicRelayRegistryStore,
    snapshot: Arc<RwLock<PublicRelaySnapshot>>,
    mutation_lock: Arc<Mutex<()>>,
    global_limit: Arc<Semaphore>,
    per_machine_limits: Arc<RwLock<BTreeMap<MachineId, Arc<Semaphore>>>>,
}

pub struct PublicRelayForwardPermit {
    _global: OwnedSemaphorePermit,
    _machine: OwnedSemaphorePermit,
}

impl PublicRelayRegistryManager {
    pub async fn load(store: PublicRelayRegistryStore) -> Result<Self, PublicRelayError> {
        let snapshot = store.load_snapshot().await?;
        let manager = Self {
            store,
            snapshot: Arc::new(RwLock::new(snapshot)),
            mutation_lock: Arc::new(Mutex::new(())),
            global_limit: Arc::new(Semaphore::new(PUBLIC_GLOBAL_CONCURRENCY)),
            per_machine_limits: Arc::new(RwLock::new(BTreeMap::new())),
        };
        manager.rebuild_machine_limits().await;
        Ok(manager)
    }

    pub fn store(&self) -> &PublicRelayRegistryStore {
        &self.store
    }

    pub async fn resolve(&self, machine_id: &MachineId) -> Result<RelayTarget, PublicRelayError> {
        let snapshot = self.snapshot.read().await;
        let entry = snapshot
            .entries
            .get(machine_id)
            .ok_or(PublicRelayError::UnknownMachine)?;
        if entry.disabled {
            return Err(PublicRelayError::DisabledMachine);
        }
        entry.target()
    }

    pub async fn list(&self) -> Vec<PublicRelayMachineView> {
        let snapshot = self.snapshot.read().await;
        snapshot
            .entries
            .values()
            .map(PublicRelayMachineView::from_entry)
            .collect()
    }

    pub async fn entry(&self, machine_id: &MachineId) -> Option<PublicRelayEntry> {
        self.snapshot.read().await.entries.get(machine_id).cloned()
    }

    pub async fn count(&self) -> usize {
        self.snapshot.read().await.entries.len()
    }

    pub async fn replace_entries(
        &self,
        entries: Vec<PublicRelayEntry>,
    ) -> Result<ImportReport, PublicRelayError> {
        let _mutation = self.mutation_lock.lock().await;
        let report = super::store::parse_registry_value(
            serde_json::to_value(&entries)
                .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?,
        )?;
        self.store.save_entries(report.entries.clone()).await?;
        self.reload().await?;
        Ok(report)
    }

    pub async fn import_report(
        &self,
        report: ImportReport,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let _mutation = self.mutation_lock.lock().await;
        let outcome = self.store.save_entries(report.entries).await?;
        self.reload().await?;
        Ok(outcome)
    }

    pub async fn upsert(
        &self,
        entry: PublicRelayEntry,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let _mutation = self.mutation_lock.lock().await;
        entry.target()?;
        let mut entries = self.entries().await;
        entries.insert(entry.machine_id.clone(), entry);
        let outcome = self
            .store
            .save_entries(entries.into_values().collect())
            .await?;
        self.reload().await?;
        Ok(outcome)
    }

    pub async fn remove(
        &self,
        machine_id: &MachineId,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let _mutation = self.mutation_lock.lock().await;
        let mut entries = self.entries().await;
        if entries.remove(machine_id).is_none() {
            return Err(PublicRelayError::UnknownMachine);
        }
        let outcome = self
            .store
            .save_entries(entries.into_values().collect())
            .await?;
        self.reload().await?;
        Ok(outcome)
    }

    pub async fn set_disabled(
        &self,
        machine_id: &MachineId,
        disabled: bool,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let _mutation = self.mutation_lock.lock().await;
        let mut entries = self.entries().await;
        let entry = entries
            .get_mut(machine_id)
            .ok_or(PublicRelayError::UnknownMachine)?;
        entry.disabled = disabled;
        let outcome = self
            .store
            .save_entries(entries.into_values().collect())
            .await?;
        self.reload().await?;
        Ok(outcome)
    }

    pub async fn acquire_forward_permit(
        &self,
        machine_id: &MachineId,
    ) -> Result<PublicRelayForwardPermit, PublicRelayError> {
        let global = self
            .global_limit
            .clone()
            .try_acquire_owned()
            .map_err(|_| PublicRelayError::Overloaded)?;
        let machine = self.machine_limit(machine_id).await;
        let machine = machine
            .try_acquire_owned()
            .map_err(|_| PublicRelayError::Overloaded)?;
        Ok(PublicRelayForwardPermit {
            _global: global,
            _machine: machine,
        })
    }

    async fn reload(&self) -> Result<(), PublicRelayError> {
        let snapshot = self.store.load_snapshot().await?;
        *self.snapshot.write().await = snapshot;
        self.rebuild_machine_limits().await;
        Ok(())
    }

    async fn entries(&self) -> BTreeMap<MachineId, PublicRelayEntry> {
        self.snapshot.read().await.entries.clone()
    }

    async fn machine_limit(&self, machine_id: &MachineId) -> Arc<Semaphore> {
        if let Some(limit) = self
            .per_machine_limits
            .read()
            .await
            .get(machine_id)
            .cloned()
        {
            return limit;
        }
        let mut limits = self.per_machine_limits.write().await;
        limits
            .entry(machine_id.clone())
            .or_insert_with(|| Arc::new(Semaphore::new(PUBLIC_PER_MACHINE_CONCURRENCY)))
            .clone()
    }

    async fn rebuild_machine_limits(&self) {
        let snapshot = self.snapshot.read().await;
        let mut limits = self.per_machine_limits.write().await;
        limits.retain(|machine, _| snapshot.entries.contains_key(machine));
        for machine in snapshot.entries.keys() {
            limits
                .entry(machine.clone())
                .or_insert_with(|| Arc::new(Semaphore::new(PUBLIC_PER_MACHINE_CONCURRENCY)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_entry(machine_id: &str, target_url: &str) -> PublicRelayEntry {
        PublicRelayEntry {
            machine_id: MachineId::parse(machine_id).unwrap(),
            target_url: target_url.to_string(),
            description: None,
            disabled: false,
        }
    }

    #[tokio::test]
    async fn public_relay_manager_resolves_valid_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        store
            .save_entries(vec![live_entry(
                "dookie",
                "http://100.88.16.79:38935/callback/dookie",
            )])
            .await
            .unwrap();
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        let target = manager
            .resolve(&MachineId::parse("dookie").unwrap())
            .await
            .unwrap();

        assert_eq!(target.redacted_label(), "dookie@100.88.16.79");
    }

    #[tokio::test]
    async fn public_relay_manager_replace_entries_updates_live_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        manager
            .replace_entries(vec![live_entry(
                "tootie",
                "http://100.120.242.29:38935/callback/tootie",
            )])
            .await
            .unwrap();

        assert_eq!(manager.count().await, 1);
        assert!(
            manager
                .resolve(&MachineId::parse("tootie").unwrap())
                .await
                .is_ok()
        );
    }
}
