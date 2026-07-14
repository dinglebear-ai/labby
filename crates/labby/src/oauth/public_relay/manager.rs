use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock as StdRwLock};

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

static PUBLIC_RELAY_MANAGER: OnceLock<StdRwLock<Option<Arc<PublicRelayRegistryManager>>>> =
    OnceLock::new();

pub fn install_public_relay_manager(manager: Arc<PublicRelayRegistryManager>) {
    set_public_relay_manager(Some(manager));
}

pub fn set_public_relay_manager(manager: Option<Arc<PublicRelayRegistryManager>>) {
    let lock = PUBLIC_RELAY_MANAGER.get_or_init(|| StdRwLock::new(None));
    // Recover from poisoning instead of propagating it: a single panic
    // anywhere else while this lock is held must not permanently brick
    // every future `doctor` dispatch and relay lookup process-wide.
    let mut guard = lock
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = manager;
}

pub fn current_public_relay_manager() -> Option<Arc<PublicRelayRegistryManager>> {
    PUBLIC_RELAY_MANAGER
        .get_or_init(|| StdRwLock::new(None))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
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

    pub async fn probe_targets(&self) -> Vec<(MachineId, Result<RelayTarget, PublicRelayError>)> {
        let snapshot = self.snapshot.read().await;
        snapshot
            .entries
            .values()
            .map(|entry| {
                let machine_id = entry.machine_id.clone();
                let target = if entry.disabled {
                    Err(PublicRelayError::DisabledMachine)
                } else {
                    entry.target()
                };
                (machine_id, target)
            })
            .collect()
    }

    pub async fn count(&self) -> usize {
        self.snapshot.read().await.entries.len()
    }

    /// Clone of the current in-memory (live) snapshot.
    ///
    /// Used by content-based staleness checks (e.g. `doctor oauth-relay`) that
    /// need to compare machine ids + target URLs against the persisted
    /// registry, not just an entry count that can mask a same-count swap.
    pub async fn snapshot(&self) -> PublicRelaySnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn import_report(
        &self,
        report: ImportReport,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        report.ensure_complete_import()?;
        let _mutation = self.mutation_lock.lock().await;
        self.validate_and_install(report.entries).await
    }

    pub async fn upsert(
        &self,
        entry: PublicRelayEntry,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let _mutation = self.mutation_lock.lock().await;
        let mut entries = self.entries().await;
        entries.insert(entry.machine_id.clone(), entry);
        self.validate_and_install(entries.into_values().collect())
            .await
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
        self.validate_and_install(entries.into_values().collect())
            .await
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
        self.validate_and_install(entries.into_values().collect())
            .await
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

    /// Single source of truth for validating and persisting a candidate
    /// entry set. Every mutation method (`upsert`, `remove`, `set_disabled`,
    /// `import_report`) builds its candidate `entries` and routes through
    /// this one helper instead of validating (`entry.target()`) redundantly
    /// itself -- so there is exactly one place that decides whether a
    /// mutation is allowed to reach the live snapshot and the persisted
    /// store.
    async fn validate_and_install(
        &self,
        entries: Vec<PublicRelayEntry>,
    ) -> Result<RegistryWriteOutcome, PublicRelayError> {
        let snapshot = snapshot_from_entries(entries.clone())?;
        let outcome = self.store.save_entries(entries).await?;
        *self.snapshot.write().await = snapshot;
        self.rebuild_machine_limits().await;
        Ok(outcome)
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

fn snapshot_from_entries(
    entries: Vec<PublicRelayEntry>,
) -> Result<PublicRelaySnapshot, PublicRelayError> {
    let mut snapshot = PublicRelaySnapshot::default();
    for entry in entries {
        entry.target()?;
        snapshot.entries.insert(entry.machine_id.clone(), entry);
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_entry(machine_id: &str, target_url: &str) -> PublicRelayEntry {
        PublicRelayEntry::new(
            MachineId::parse(machine_id).unwrap(),
            target_url.to_string(),
            None,
            false,
        )
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
    async fn public_relay_manager_import_report_updates_live_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();
        let report = super::super::store::parse_registry_value(
            serde_json::to_value(vec![live_entry(
                "tootie",
                "http://100.120.242.29:38935/callback/tootie",
            )])
            .unwrap(),
        )
        .unwrap();

        manager.import_report(report).await.unwrap();

        assert_eq!(manager.count().await, 1);
        assert!(
            manager
                .resolve(&MachineId::parse("tootie").unwrap())
                .await
                .is_ok()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn public_relay_manager_mutation_lock_prevents_lost_updates_under_real_concurrency() {
        // Each `upsert` does an internal read-modify-write of the full
        // entry set (read current entries -> insert -> persist), guarded by
        // `mutation_lock`. Without that lock actually serializing the whole
        // read-modify-write span (not just the final write), N concurrent
        // upserts of N distinct machines would race and lose updates: two
        // upserts could both read the same starting snapshot, each add their
        // own machine, and the second writer's save would clobber the
        // first's addition. This test races real concurrent tasks (a
        // multi-thread runtime, not pre-acquired permits or sequential
        // `.await`s) to prove that doesn't happen.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        const N: usize = 6;
        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            let manager = manager.clone();
            handles.push(tokio::spawn(async move {
                manager
                    .upsert(live_entry(
                        &format!("m{i}"),
                        &format!("http://100.88.16.79:38935/callback/m{i}"),
                    ))
                    .await
                    .expect("concurrent upsert should succeed")
            }));
        }
        for handle in handles {
            handle.await.expect("upsert task should not panic");
        }

        assert_eq!(
            manager.count().await,
            N,
            "expected all {N} concurrent upserts to land in the live snapshot with no lost updates"
        );

        // The persisted registry must match too -- not just the in-memory
        // snapshot -- so a lost update can't hide behind a stale disk write.
        let persisted = manager.store().load_snapshot().await.unwrap();
        assert_eq!(
            persisted.entries.len(),
            N,
            "expected all {N} concurrent upserts to be persisted to disk"
        );
        for i in 0..N {
            let machine_id = MachineId::parse(&format!("m{i}")).unwrap();
            assert!(
                persisted.entries.contains_key(&machine_id),
                "persisted registry missing m{i}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn public_relay_manager_concurrent_remove_and_upsert_do_not_lose_updates() {
        // Mix mutation kinds (not just upsert) racing concurrently: upserts
        // of new machines interleaved with a remove of a pre-seeded one, all
        // going through the same `mutation_lock`-guarded read-modify-write.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        store
            .save_entries(vec![live_entry(
                "seed",
                "http://100.88.16.79:38935/callback/seed",
            )])
            .await
            .unwrap();
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        const N: usize = 6;
        let mut handles = Vec::with_capacity(N + 1);
        handles.push(tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .remove(&MachineId::parse("seed").unwrap())
                    .await
                    .expect("remove should succeed");
            }
        }));
        for i in 0..N {
            let manager = manager.clone();
            handles.push(tokio::spawn(async move {
                manager
                    .upsert(live_entry(
                        &format!("c{i}"),
                        &format!("http://100.88.16.79:38935/callback/c{i}"),
                    ))
                    .await
                    .expect("concurrent upsert should succeed");
            }));
        }
        for handle in handles {
            handle.await.expect("mutation task should not panic");
        }

        assert_eq!(
            manager.count().await,
            N,
            "expected the seed machine removed and all {N} concurrent upserts present, no lost updates"
        );
        assert!(
            manager
                .resolve(&MachineId::parse("seed").unwrap())
                .await
                .is_err(),
            "seed machine should have been removed"
        );
    }
}
