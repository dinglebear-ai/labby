//! Atomic, process-owned generations of first-party Skills.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use labby_runtime::artifacts::{ArtifactError, ArtifactStore, LibraryMutation, LibrarySnapshot};
use labby_runtime::skills::{ResourceDigest, limits};

use super::local::{
    LocalLoadCounters, LocalLoadLimits, LocalSkillRejection, load_local_skills_bounded,
};
use super::providers::{ArtifactSkillAccess, CollisionRejection, FirstPartySkillProviders};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GenerationLimits {
    pub(crate) active_skills: usize,
    pub(crate) aggregate_bytes: usize,
    pub(crate) per_skill_bytes: usize,
    pub(crate) total_resources: usize,
    pub(crate) live_candidate_bytes: usize,
}

/// Restart-stable identity supplied by the persisted Artifact library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GenerationSeed {
    pub(crate) version: u64,
    pub(crate) active_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "Artifact library wiring consumes this initialization result"
)]
pub(crate) enum ProcessGenerationInitialization {
    Initialized,
    AlreadyInitialized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "Artifact library wiring consumes this typed conflict"
)]
pub(crate) struct ProcessGenerationConflict {
    pub(crate) requested: GenerationSeed,
    pub(crate) initialized: GenerationSeed,
}

impl Default for GenerationLimits {
    fn default() -> Self {
        Self {
            active_skills: limits::MAX_SKILLS_PER_UPSTREAM,
            aggregate_bytes: 64 * 1024 * 1024,
            per_skill_bytes: 16 * 1024 * 1024,
            total_resources: limits::MAX_SKILLS_PER_UPSTREAM * limits::MAX_RESOURCES_PER_SKILL,
            live_candidate_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "generation diagnostics are consumed by the next dispatch bead"
)]
pub(crate) struct FirstPartyGeneration {
    pub(crate) id: u64,
    pub(crate) digest: String,
    pub(crate) active_digest: String,
    pub(crate) providers: FirstPartySkillProviders,
    pub(crate) rejected: Vec<LocalSkillRejection>,
    pub(crate) bytes: usize,
    pub(crate) resources: usize,
    pub(crate) degraded: Option<RefreshRejection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "refresh rejection is consumed by the next dispatch bead"
)]
pub(crate) enum RefreshRejection {
    Stale {
        expected: u64,
        actual: u64,
    },
    Limit {
        kind: &'static str,
        limit: usize,
        actual: usize,
    },
}

#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "refresh diagnostics are consumed by the next dispatch bead"
)]
pub(crate) struct RefreshDiagnostics {
    pub(crate) generation: u64,
    pub(crate) changed: bool,
    pub(crate) digest: String,
    pub(crate) rejected_skills: Vec<LocalSkillRejection>,
    pub(crate) collision_rejections: Vec<CollisionRejection>,
    pub(crate) skill_count: usize,
    pub(crate) resource_count: usize,
    pub(crate) bytes: usize,
    pub(crate) build_elapsed: Duration,
    pub(crate) coalesced: bool,
    pub(crate) counters: GenerationCounters,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GenerationCounters {
    pub(crate) builds: u64,
    pub(crate) scans: u64,
    pub(crate) files_scanned: u64,
    pub(crate) files_read: u64,
    pub(crate) bytes_read: u64,
    pub(crate) scan_nanos: u64,
    pub(crate) read_nanos: u64,
    pub(crate) hash_nanos: u64,
    pub(crate) validate_nanos: u64,
    pub(crate) index_nanos: u64,
    pub(crate) swap_nanos: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefreshTicket {
    generation: u64,
    epoch: u64,
    request: u64,
}

impl RefreshTicket {
    fn key(self, expected: Option<u64>) -> RefreshKey {
        RefreshKey {
            generation: self.generation,
            epoch: self.epoch,
            expected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RefreshKey {
    generation: u64,
    epoch: u64,
    expected: Option<u64>,
}

#[derive(Default)]
struct RefreshState {
    cached: BTreeMap<RefreshKey, Result<RefreshDiagnostics, RefreshRejection>>,
}

const MAX_CACHED_REFRESH_OUTCOMES: usize = 64;

impl RefreshState {
    fn cache(&mut self, key: RefreshKey, result: Result<RefreshDiagnostics, RefreshRejection>) {
        self.cached.insert(key, result);
        while self.cached.len() > MAX_CACHED_REFRESH_OUTCOMES {
            let Some(oldest) = self.cached.keys().next().copied() else {
                break;
            };
            self.cached.remove(&oldest);
        }
    }
}

#[derive(Default)]
struct AtomicGenerationCounters {
    builds: AtomicU64,
    scans: AtomicU64,
    files_scanned: AtomicU64,
    files_read: AtomicU64,
    bytes_read: AtomicU64,
    scan_nanos: AtomicU64,
    read_nanos: AtomicU64,
    hash_nanos: AtomicU64,
    validate_nanos: AtomicU64,
    index_nanos: AtomicU64,
    swap_nanos: AtomicU64,
}

pub(crate) struct FirstPartyGenerationManager {
    current: Arc<ArcSwap<FirstPartyGeneration>>,
    refresh: Mutex<RefreshState>,
    root: PathBuf,
    limits: GenerationLimits,
    initial_seed: GenerationSeed,
    completed_build_epoch: AtomicU64,
    counters: AtomicGenerationCounters,
    next_ticket: AtomicU64,
    live_generations: Mutex<Vec<Weak<FirstPartyGeneration>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "observability seam is consumed by product wiring")]
pub(crate) struct GenerationObservability {
    pub(crate) current: u64,
    pub(crate) live_generations: usize,
    pub(crate) pinned_generations: usize,
}

impl FirstPartyGenerationManager {
    pub(crate) fn load() -> Self {
        Self::new(
            labby_runtime::lab_home().join("skills"),
            GenerationLimits::default(),
        )
    }

    pub(crate) fn new(root: PathBuf, limits: GenerationLimits) -> Self {
        Self::new_seeded(
            root,
            limits,
            GenerationSeed {
                version: 1,
                active_digest: String::new(),
            },
        )
    }

    pub(crate) fn new_seeded(
        root: PathBuf,
        limits: GenerationLimits,
        seed: GenerationSeed,
    ) -> Self {
        let initial_id = seed.version.max(1);
        let initial_seed = seed.clone();
        let (mut initial, initial_load) =
            Self::build(&root, limits, initial_id).unwrap_or_else(|(rejection, counters)| {
                tracing::error!(reason = ?rejection, "first-party Skill generation is degraded");
                let providers = FirstPartySkillProviders::from_local_skills([]);
                let (_, bytes, _, resources) = providers.admission_totals();
                (
                    Arc::new(FirstPartyGeneration {
                        id: initial_id,
                        digest: if seed.active_digest.is_empty() {
                            ResourceDigest::of_bytes(b"degraded").to_wire()
                        } else {
                            seed.active_digest.clone()
                        },
                        active_digest: seed.active_digest.clone(),
                        providers,
                        rejected: Vec::new(),
                        bytes,
                        resources,
                        degraded: Some(rejection),
                    }),
                    counters,
                )
            });
        if !seed.active_digest.is_empty() {
            Arc::get_mut(&mut initial)
                .expect("new generation is not yet shared")
                .active_digest = seed.active_digest;
        }
        let counters = AtomicGenerationCounters::default();
        counters.builds.store(1, Ordering::Relaxed);
        counters
            .scans
            .store(initial_load.directories_scanned as u64, Ordering::Relaxed);
        counters
            .files_scanned
            .store(initial_load.files_scanned as u64, Ordering::Relaxed);
        counters
            .files_read
            .store(initial_load.files_read as u64, Ordering::Relaxed);
        counters
            .bytes_read
            .store(initial_load.bytes_read as u64, Ordering::Relaxed);
        counters
            .scan_nanos
            .store(initial_load.scan_nanos, Ordering::Relaxed);
        counters
            .read_nanos
            .store(initial_load.read_nanos, Ordering::Relaxed);
        counters
            .hash_nanos
            .store(initial_load.hash_nanos, Ordering::Relaxed);
        counters
            .validate_nanos
            .store(initial_load.validate_nanos, Ordering::Relaxed);
        counters
            .index_nanos
            .store(initial_load.index_nanos, Ordering::Relaxed);
        let live_generations = Mutex::new(vec![Arc::downgrade(&initial)]);
        Self {
            current: Arc::new(ArcSwap::from(initial)),
            refresh: Mutex::new(RefreshState::default()),
            root,
            limits,
            initial_seed,
            completed_build_epoch: AtomicU64::new(1),
            counters,
            next_ticket: AtomicU64::new(1),
            live_generations,
        }
    }

    pub(crate) fn generation(&self) -> Arc<FirstPartyGeneration> {
        self.current.load_full()
    }

    pub(crate) fn generation_cell(&self) -> Arc<ArcSwap<FirstPartyGeneration>> {
        Arc::clone(&self.current)
    }

    #[allow(
        dead_code,
        reason = "runtime primitive is wired to dispatch by the next bead"
    )]
    pub(crate) fn refresh(
        &self,
        expected: Option<u64>,
    ) -> Result<RefreshDiagnostics, RefreshRejection> {
        self.refresh_with_ticket(self.begin_refresh(), expected)
    }

    pub(crate) fn begin_refresh(&self) -> RefreshTicket {
        RefreshTicket {
            generation: self.generation().id,
            epoch: self.completed_build_epoch.load(Ordering::Acquire),
            request: self.next_ticket.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub(crate) fn refresh_with_ticket(
        &self,
        ticket: RefreshTicket,
        expected: Option<u64>,
    ) -> Result<RefreshDiagnostics, RefreshRejection> {
        let mut state = self
            .refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = ticket.key(expected);
        if let Some(cached) = state.cached.get(&key) {
            return match cached.clone() {
                Ok(mut diagnostics) => {
                    diagnostics.changed = false;
                    diagnostics.coalesced = true;
                    Ok(diagnostics)
                }
                rejection => rejection,
            };
        }
        let old = self.generation();
        if let Some(expected) = expected
            && expected != old.id
        {
            let result = Err(RefreshRejection::Stale {
                expected,
                actual: old.id,
            });
            state.cache(key, result.clone());
            return result;
        }
        if ticket.generation != old.id
            || ticket.epoch != self.completed_build_epoch.load(Ordering::Acquire)
        {
            let result = Ok(self.diagnostics(&old, false, true, Vec::new(), Duration::ZERO));
            state.cache(key, result.clone());
            return result;
        }
        let started = Instant::now();
        self.counters.builds.fetch_add(1, Ordering::Relaxed);
        let attempt = Self::build(&self.root, self.limits, old.id + 1);
        let (candidate, load_counters) = match attempt {
            Ok(success) => success,
            Err((rejection, load_counters)) => {
                self.record_load(load_counters);
                self.completed_build_epoch.fetch_add(1, Ordering::Release);
                let result = Err(rejection);
                state.cache(key, result.clone());
                return result;
            }
        };
        self.record_load(load_counters);
        self.completed_build_epoch.fetch_add(1, Ordering::Release);
        let changed = candidate.digest != old.digest;
        let rejected_skills = candidate.rejected.clone();
        let swap_started = Instant::now();
        let published = if changed {
            self.current.store(Arc::clone(&candidate));
            let mut live_generations = self
                .live_generations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            live_generations.retain(|generation| generation.strong_count() > 0);
            live_generations.push(Arc::downgrade(&candidate));
            candidate
        } else {
            old
        };
        self.counters
            .swap_nanos
            .fetch_add(swap_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let result = Ok(self.diagnostics(
            &published,
            changed,
            false,
            rejected_skills,
            started.elapsed(),
        ));
        state.cache(key, result.clone());
        result
    }

    fn diagnostics(
        &self,
        generation: &FirstPartyGeneration,
        changed: bool,
        coalesced: bool,
        rejected_skills: Vec<LocalSkillRejection>,
        build_elapsed: Duration,
    ) -> RefreshDiagnostics {
        RefreshDiagnostics {
            generation: generation.id,
            changed,
            digest: generation.digest.clone(),
            rejected_skills,
            collision_rejections: generation.providers.collision_rejections().to_vec(),
            skill_count: generation.providers.discover().len(),
            resource_count: generation.resources,
            bytes: generation.bytes,
            build_elapsed,
            coalesced,
            counters: self.counters(),
        }
    }

    pub(crate) fn counters(&self) -> GenerationCounters {
        GenerationCounters {
            builds: self.counters.builds.load(Ordering::Relaxed),
            scans: self.counters.scans.load(Ordering::Relaxed),
            files_scanned: self.counters.files_scanned.load(Ordering::Relaxed),
            files_read: self.counters.files_read.load(Ordering::Relaxed),
            bytes_read: self.counters.bytes_read.load(Ordering::Relaxed),
            scan_nanos: self.counters.scan_nanos.load(Ordering::Relaxed),
            read_nanos: self.counters.read_nanos.load(Ordering::Relaxed),
            hash_nanos: self.counters.hash_nanos.load(Ordering::Relaxed),
            validate_nanos: self.counters.validate_nanos.load(Ordering::Relaxed),
            index_nanos: self.counters.index_nanos.load(Ordering::Relaxed),
            swap_nanos: self.counters.swap_nanos.load(Ordering::Relaxed),
        }
    }

    #[allow(dead_code, reason = "observability seam is consumed by product wiring")]
    /// Raw length of the weak-generation vec, without the reaping pass that
    /// `generation_observability` performs. A test asserting the vec stays
    /// bounded must not read it through an accessor that does the reaping.
    #[cfg(test)]
    pub(crate) fn tracked_generation_slots(&self) -> usize {
        self.live_generations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Test-only for now: no production surface reports generation liveness yet.
    /// Un-gate this the moment one does.
    #[cfg(test)]
    pub(crate) fn generation_observability(&self) -> GenerationObservability {
        let current = self.generation();
        let mut tracked = self
            .live_generations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tracked.retain(|generation| generation.strong_count() > 0);
        let pinned_generations = tracked
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|generation| !Arc::ptr_eq(generation, &current))
            .count();
        GenerationObservability {
            current: current.id,
            live_generations: tracked.len(),
            pinned_generations,
        }
    }

    fn record_load(&self, counters: LocalLoadCounters) {
        self.counters
            .scans
            .fetch_add(counters.directories_scanned as u64, Ordering::Relaxed);
        self.counters
            .files_scanned
            .fetch_add(counters.files_scanned as u64, Ordering::Relaxed);
        self.counters
            .files_read
            .fetch_add(counters.files_read as u64, Ordering::Relaxed);
        self.counters
            .bytes_read
            .fetch_add(counters.bytes_read as u64, Ordering::Relaxed);
        self.counters
            .scan_nanos
            .fetch_add(counters.scan_nanos, Ordering::Relaxed);
        self.counters
            .read_nanos
            .fetch_add(counters.read_nanos, Ordering::Relaxed);
        self.counters
            .hash_nanos
            .fetch_add(counters.hash_nanos, Ordering::Relaxed);
        self.counters
            .validate_nanos
            .fetch_add(counters.validate_nanos, Ordering::Relaxed);
        self.counters
            .index_nanos
            .fetch_add(counters.index_nanos, Ordering::Relaxed);
    }

    fn build(
        root: &Path,
        caps: GenerationLimits,
        id: u64,
    ) -> Result<(Arc<FirstPartyGeneration>, LocalLoadCounters), (RefreshRejection, LocalLoadCounters)>
    {
        let bundled = FirstPartySkillProviders::from_local_skills([]);
        let (bundled_skills, bundled_bytes, _, bundled_resources) = bundled.admission_totals();
        let loaded = load_local_skills_bounded(
            root,
            Some(LocalLoadLimits {
                active_skills: caps.active_skills,
                aggregate_bytes: caps.aggregate_bytes,
                per_skill_bytes: caps.per_skill_bytes,
                total_resources: caps.total_resources,
                live_candidate_bytes: caps.live_candidate_bytes,
                bundled_skills,
                bundled_bytes,
                bundled_resources,
            }),
        )
        .map_err(|limit| {
            (
                RefreshRejection::Limit {
                    kind: limit.kind,
                    limit: limit.limit,
                    actual: limit.actual,
                },
                limit.counters,
            )
        })?;
        let mut load_counters = loaded.counters;
        let index_started = Instant::now();
        let providers = FirstPartySkillProviders::from_local_skills(loaded.skills.into_values());
        load_counters.index_nanos = load_counters
            .index_nanos
            .saturating_add(index_started.elapsed().as_nanos() as u64);
        let (skills, bytes, max_skill_bytes, resources) = providers.admission_totals();
        check("active_skills", skills, caps.active_skills)
            .map_err(|error| (error, load_counters))?;
        check("aggregate_bytes", bytes, caps.aggregate_bytes)
            .map_err(|error| (error, load_counters))?;
        check("per_skill_bytes", max_skill_bytes, caps.per_skill_bytes)
            .map_err(|error| (error, load_counters))?;
        check("total_resources", resources, caps.total_resources)
            .map_err(|error| (error, load_counters))?;
        check("live_candidate_bytes", bytes, caps.live_candidate_bytes)
            .map_err(|error| (error, load_counters))?;
        let encoded = serde_json::to_vec(
            &providers
                .discover()
                .iter()
                .map(|entry| &entry.validated().entry)
                .collect::<Vec<_>>(),
        )
        .expect("validated Skill entries serialize");
        let hash_started = Instant::now();
        let digest = ResourceDigest::of_bytes(&encoded).to_wire();
        load_counters.hash_nanos = load_counters
            .hash_nanos
            .saturating_add(hash_started.elapsed().as_nanos() as u64);
        Ok((
            Arc::new(FirstPartyGeneration {
                id,
                digest: digest.clone(),
                active_digest: digest.clone(),
                providers,
                rejected: loaded.rejections,
                bytes,
                resources,
                degraded: None,
            }),
            load_counters,
        ))
    }
}

/// Build one exact immutable first-party generation from committed Artifact revisions.
pub(crate) fn project_artifact_generation(
    store: &ArtifactStore,
    snapshot: &LibrarySnapshot,
    mutation: Option<&LibraryMutation>,
    base: &FirstPartyGeneration,
) -> Result<Arc<FirstPartyGeneration>, ArtifactError> {
    let mut active = snapshot
        .records
        .values()
        .filter_map(|record| {
            if store
                .get(&record.artifact_id)
                .map_or(true, |artifact| artifact.descriptor.kind != "skill")
            {
                return None;
            }
            record.active_revision_id.as_ref().map(|revision| {
                (
                    record.artifact_id.clone(),
                    (record.name.clone(), revision.clone()),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(mutation) = mutation {
        match mutation {
            LibraryMutation::Activate {
                artifact_id,
                revision_id,
                ..
            }
            | LibraryMutation::Rollback {
                artifact_id,
                revision_id,
                ..
            } => {
                let record = snapshot
                    .records
                    .get(artifact_id)
                    .ok_or(ArtifactError::NotFound("library_record"))?;
                if store.get(artifact_id)?.descriptor.kind == "skill" {
                    active.insert(
                        artifact_id.clone(),
                        (record.name.clone(), revision_id.clone()),
                    );
                }
            }
            LibraryMutation::Deactivate { artifact_id, .. }
            | LibraryMutation::Archive { artifact_id, .. } => {
                active.remove(artifact_id);
            }
            LibraryMutation::Create { .. }
            | LibraryMutation::Save { .. }
            | LibraryMutation::SetVisibility { .. }
            | LibraryMutation::Restore { .. }
            | LibraryMutation::Refresh { .. } => {}
        }
    }

    let mut local = Vec::with_capacity(active.len());
    for (artifact_id, (name, revision_id)) in &active {
        let record = snapshot
            .records
            .get(artifact_id)
            .ok_or(ArtifactError::NotFound("library_record"))?;
        let visibility = match mutation {
            Some(LibraryMutation::SetVisibility {
                artifact_id: changed,
                visibility,
                ..
            }) if changed == artifact_id => *visibility,
            _ => record.visibility,
        };
        let revision = store.revision(artifact_id, revision_id)?;
        let mut files = Vec::with_capacity(revision.components.len());
        for component in &revision.components {
            files.push((
                component.path.clone(),
                store.read_skill_revision_file(artifact_id, revision_id, &component.path)?,
            ));
        }
        let logical = files
            .into_iter()
            .map(|(path, bytes)| {
                String::from_utf8(bytes)
                    .map(|content| labby_runtime::artifacts::LogicalSkillFile::new(path, content))
                    .map_err(|_| ArtifactError::SkillVerification)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let materialized =
            labby_runtime::artifacts::materialize_logical_skill(name, logical, Default::default())?;
        let text_files = materialized
            .resources
            .into_iter()
            .map(|(uri, bytes)| {
                String::from_utf8(bytes)
                    .map(|text| (uri, text))
                    .map_err(|_| ArtifactError::SkillVerification)
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        local.push((
            super::local::LocalSkill {
                entry: materialized.skill.entry,
                files: text_files,
            },
            ArtifactSkillAccess {
                ownership: record.ownership.clone(),
                visibility,
            },
        ));
    }
    let baseline_collisions = base.providers.collision_rejections().len();
    let providers = base.providers.with_artifact_skills(local);
    if providers.collision_rejections().len() > baseline_collisions {
        return Err(ArtifactError::Conflict("active_skill_collision"));
    }
    let (skills, bytes, max_skill_bytes, resources) = providers.admission_totals();
    let limits = GenerationLimits::default();
    for (what, actual, limit) in [
        ("active_skills", skills, limits.active_skills),
        ("aggregate_bytes", bytes, limits.aggregate_bytes),
        ("per_skill_bytes", max_skill_bytes, limits.per_skill_bytes),
        ("total_resources", resources, limits.total_resources),
        ("live_candidate_bytes", bytes, limits.live_candidate_bytes),
    ] {
        if actual > limit {
            return Err(ArtifactError::LimitExceeded {
                what,
                limit: limit as u64,
            });
        }
    }
    let active_digest = labby_runtime::artifacts::canonical_json::digest(&active)?;
    let encoded = serde_json::to_vec(
        &providers
            .discover()
            .iter()
            .map(|entry| &entry.validated().entry)
            .collect::<Vec<_>>(),
    )
    .map_err(|_| ArtifactError::SkillVerification)?;
    let digest = ResourceDigest::of_bytes(&encoded).to_wire();
    Ok(Arc::new(FirstPartyGeneration {
        id: (snapshot.version + u64::from(mutation.is_some())).max(1),
        digest: digest.clone(),
        active_digest,
        providers,
        rejected: base.rejected.clone(),
        bytes,
        resources,
        degraded: None,
    }))
}

fn check(kind: &'static str, actual: usize, limit: usize) -> Result<(), RefreshRejection> {
    if actual > limit {
        Err(RefreshRejection::Limit {
            kind,
            limit,
            actual,
        })
    } else {
        Ok(())
    }
}

static FIRST_PARTY_GENERATION_MANAGER: OnceLock<FirstPartyGenerationManager> = OnceLock::new();

fn initialize_generation_manager(
    manager: &OnceLock<FirstPartyGenerationManager>,
    root: PathBuf,
    limits: GenerationLimits,
    seed: GenerationSeed,
) -> Result<ProcessGenerationInitialization, ProcessGenerationConflict> {
    if let Some(initialized) = manager.get() {
        return compare_initialized_seed(initialized, seed);
    }
    let candidate = FirstPartyGenerationManager::new_seeded(root, limits, seed.clone());
    match manager.set(candidate) {
        Ok(()) => Ok(ProcessGenerationInitialization::Initialized),
        Err(_) => compare_initialized_seed(
            manager
                .get()
                .expect("another initializer published before set returned"),
            seed,
        ),
    }
}

fn compare_initialized_seed(
    manager: &FirstPartyGenerationManager,
    requested: GenerationSeed,
) -> Result<ProcessGenerationInitialization, ProcessGenerationConflict> {
    if manager.initial_seed == requested {
        Ok(ProcessGenerationInitialization::AlreadyInitialized)
    } else {
        Err(ProcessGenerationConflict {
            requested,
            initialized: manager.initial_seed.clone(),
        })
    }
}

/// Publishes the persisted library identity into the process-global manager.
///
/// This must run before a request first accesses [`first_party_generation_manager`].
/// Repeating the same seed is safe; attempting to replace an initialized identity
/// is rejected so request readers cannot silently change generation lineage.
#[allow(
    dead_code,
    reason = "Artifact library wiring is delivered by bead lab-2h806.6"
)]
pub(crate) fn initialize_first_party_generation_manager(
    seed: GenerationSeed,
) -> Result<ProcessGenerationInitialization, ProcessGenerationConflict> {
    initialize_generation_manager(
        &FIRST_PARTY_GENERATION_MANAGER,
        labby_runtime::lab_home().join("skills"),
        GenerationLimits::default(),
        seed,
    )
}

pub(crate) fn first_party_generation_manager() -> &'static FirstPartyGenerationManager {
    FIRST_PARTY_GENERATION_MANAGER.get_or_init(FirstPartyGenerationManager::load)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Barrier;
    use std::thread;

    fn write_skill(root: &Path, name: &str, suffix: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test {suffix}\n---\n\n{suffix}\n"),
        )
        .unwrap();
    }

    fn write_skill_with_max_resources(root: &Path, name: &str) {
        write_skill(root, name, "scale");
        let dir = root.join(name);
        for index in 1..limits::MAX_RESOURCES_PER_SKILL {
            fs::write(dir.join(format!("resource-{index:02}.txt")), b"scale").unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    fn resident_set_bytes() -> Option<u64> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kibibytes = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        Some(kibibytes.saturating_mul(1024))
    }

    #[test]
    fn seeded_manager_initialization_is_idempotent_and_rejects_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let manager = OnceLock::new();
        let seed = GenerationSeed {
            version: 42,
            active_digest: "blake3:stable".to_string(),
        };
        assert_eq!(
            initialize_generation_manager(
                &manager,
                temp.path().to_path_buf(),
                GenerationLimits::default(),
                seed.clone(),
            ),
            Ok(ProcessGenerationInitialization::Initialized)
        );
        assert_eq!(
            initialize_generation_manager(
                &manager,
                temp.path().to_path_buf(),
                GenerationLimits::default(),
                seed.clone(),
            ),
            Ok(ProcessGenerationInitialization::AlreadyInitialized)
        );
        let requested = GenerationSeed {
            version: 43,
            active_digest: "blake3:different".to_string(),
        };
        assert_eq!(
            initialize_generation_manager(
                &manager,
                temp.path().to_path_buf(),
                GenerationLimits::default(),
                requested.clone(),
            ),
            Err(ProcessGenerationConflict {
                requested,
                initialized: seed,
            })
        );
        let generation = manager.get().unwrap().generation();
        assert_eq!(generation.id, 42);
        assert_eq!(generation.active_digest, "blake3:stable");
    }

    #[test]
    fn refreshing_without_a_live_reader_does_not_grow_the_generation_vec() {
        // `live_generations` is a `Vec<Weak<_>>` appended to on every changed
        // refresh. Without the `retain`, a long-running server that reloads
        // skills repeatedly accumulates one dead `Weak` per reload forever.
        let temp = tempfile::tempdir().unwrap();
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        for revision in 0..8 {
            write_skill(temp.path(), "churn", &format!("v{revision}"));
            let report = manager
                .refresh(Some(manager.generation().id))
                .expect("refresh succeeds");
            assert!(report.changed, "revision {revision} must change the digest");
        }

        // Read the raw vec: `generation_observability` reaps on the way out, so
        // it would report a healthy number whether or not the refresh path reaps.
        //
        // Steady state is 2, not 1: the reaping pass runs before the new
        // generation is pushed, and at that moment the outgoing generation is
        // still strongly held by the swap itself. The property that matters is
        // that the count does not track the number of refreshes — without the
        // reap this would be 9 after eight reloads, and unbounded after a day.
        let slots = manager.tracked_generation_slots();
        assert!(
            slots <= 2,
            "dead generations must be reaped on refresh rather than accumulated; \
             8 refreshes left {slots} tracked slots"
        );
    }

    #[test]
    fn refresh_adds_a_local_skill_without_restart_and_pins_old_readers() {
        let temp = tempfile::tempdir().unwrap();
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let pinned = manager.generation();
        assert!(
            pinned
                .providers
                .find("skill://labby/late/SKILL.md")
                .is_none()
        );
        write_skill(temp.path(), "late", "v1");
        let report = manager.refresh(Some(pinned.id)).unwrap();
        assert!(report.changed);
        assert_eq!(report.generation, pinned.id + 1);
        let observed = manager.generation_observability();
        assert_eq!(observed.live_generations, 2);
        assert_eq!(observed.pinned_generations, 1);
        assert!(
            manager
                .generation()
                .providers
                .find("skill://labby/late/SKILL.md")
                .is_some()
        );
        assert!(
            pinned
                .providers
                .find("skill://labby/late/SKILL.md")
                .is_none()
        );
    }

    #[test]
    fn persisted_seed_restores_version_and_active_digest() {
        let temp = tempfile::tempdir().unwrap();
        let seed = GenerationSeed {
            version: 41,
            active_digest: "sha256:persisted-active-set".to_string(),
        };
        let manager = FirstPartyGenerationManager::new_seeded(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
            seed.clone(),
        );
        let generation = manager.generation();
        assert_eq!(generation.id, seed.version);
        assert_eq!(generation.active_digest, seed.active_digest);
    }

    #[test]
    fn artifact_startup_reconcile_preserves_the_loaded_operator_snapshot() {
        let local_root = tempfile::tempdir().unwrap();
        write_skill(local_root.path(), "legacy-survives", "operator");
        let manager = FirstPartyGenerationManager::new(
            local_root.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let base = manager.generation();
        let artifacts_root = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(artifacts_root.path().join("artifacts")).unwrap();
        let snapshot = store.library_snapshot().unwrap();

        let reconciled = project_artifact_generation(&store, &snapshot, None, &base).unwrap();
        let repeated = project_artifact_generation(&store, &snapshot, None, &reconciled).unwrap();

        assert!(
            reconciled
                .providers
                .find("skill://labby/legacy-survives/SKILL.md")
                .is_some()
        );
        assert!(
            reconciled
                .providers
                .artifact_access("skill://labby/legacy-survives/SKILL.md")
                .is_none()
        );
        assert_eq!(repeated.digest, reconciled.digest);
        assert!(
            repeated
                .providers
                .find("skill://labby/legacy-survives/SKILL.md")
                .is_some()
        );
    }

    #[test]
    fn invalid_and_noop_refreshes_preserve_the_last_good_generation() {
        let temp = tempfile::tempdir().unwrap();
        write_skill(temp.path(), "good", "v1");
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let initial = manager.generation();
        let noop = manager.refresh(Some(initial.id)).unwrap();
        assert!(!noop.changed);
        assert_eq!(noop.generation, initial.id);

        write_skill(temp.path(), "broken", "v1");
        fs::write(temp.path().join("broken/SKILL.md"), "not frontmatter").unwrap();
        let isolated = manager.refresh(Some(initial.id)).unwrap();
        assert!(!isolated.changed);
        assert_eq!(isolated.generation, initial.id);
        assert_eq!(isolated.rejected_skills.len(), 1);

        let rejection = manager.refresh(Some(initial.id + 10)).unwrap_err();
        assert!(matches!(rejection, RefreshRejection::Stale { .. }));
        assert!(Arc::ptr_eq(&initial, &manager.generation()));
    }

    #[test]
    fn rejected_candidate_leaves_the_published_bytes_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let mut caps = GenerationLimits::default();
        let manager = FirstPartyGenerationManager::new(temp.path().to_path_buf(), caps);
        let initial = manager.generation();
        write_skill(temp.path(), "late", "v1");
        caps.active_skills = initial.providers.discover().len();
        let constrained = FirstPartyGenerationManager {
            current: Arc::new(ArcSwap::from(Arc::clone(&initial))),
            refresh: Mutex::new(RefreshState::default()),
            root: temp.path().to_path_buf(),
            limits: caps,
            initial_seed: GenerationSeed::default(),
            completed_build_epoch: AtomicU64::new(1),
            counters: AtomicGenerationCounters::default(),
            next_ticket: AtomicU64::new(1),
            live_generations: Mutex::new(vec![Arc::downgrade(&initial)]),
        };
        assert!(matches!(
            constrained.refresh(Some(initial.id)),
            Err(RefreshRejection::Limit {
                kind: "active_skills",
                ..
            })
        ));
        assert!(Arc::ptr_eq(&initial, &constrained.generation()));
    }

    #[test]
    fn every_aggregate_bound_accepts_cap_and_rejects_cap_plus_one() {
        for kind in [
            "active_skills",
            "aggregate_bytes",
            "per_skill_bytes",
            "total_resources",
            "live_candidate_bytes",
        ] {
            assert_eq!(check(kind, 7, 7), Ok(()));
            assert_eq!(
                check(kind, 8, 7),
                Err(RefreshRejection::Limit {
                    kind,
                    limit: 7,
                    actual: 8
                })
            );
        }
    }

    #[test]
    fn concurrent_refreshes_are_serialized_monotonic_and_do_not_lose_updates() {
        let temp = tempfile::tempdir().unwrap();
        let manager = Arc::new(FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        ));
        write_skill(temp.path(), "late", "v1");
        let start = Arc::new(Barrier::new(11));
        let tickets = (0..10).map(|_| manager.begin_refresh()).collect::<Vec<_>>();
        let threads = (0..10)
            .zip(tickets)
            .map(|(_, ticket)| {
                let manager = Arc::clone(&manager);
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    manager.refresh_with_ticket(ticket, None).unwrap()
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let reports = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(reports.iter().filter(|report| report.changed).count(), 1);
        assert_eq!(reports.iter().filter(|report| report.coalesced).count(), 9);
        assert!(reports.iter().all(|report| report.generation == 2));
        assert!(
            reports
                .iter()
                .all(|report| report.digest == reports[0].digest)
        );
        assert_eq!(manager.counters().builds, 2);
        assert_eq!(manager.generation().id, 2);
        assert!(
            manager
                .generation()
                .providers
                .find("skill://labby/late/SKILL.md")
                .is_some()
        );
    }

    #[test]
    fn same_generation_epoch_failed_burst_builds_and_scans_once() {
        let temp = tempfile::tempdir().unwrap();
        let mut caps = GenerationLimits::default();
        let manager = Arc::new(FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            caps,
        ));
        let initial = manager.generation();
        write_skill(temp.path(), "too-many", "v1");
        caps.active_skills = initial.providers.discover().len();
        let constrained = Arc::new(FirstPartyGenerationManager {
            current: Arc::new(ArcSwap::from(Arc::clone(&initial))),
            refresh: Mutex::new(RefreshState::default()),
            root: temp.path().to_path_buf(),
            limits: caps,
            initial_seed: GenerationSeed::default(),
            completed_build_epoch: AtomicU64::new(1),
            counters: AtomicGenerationCounters::default(),
            next_ticket: AtomicU64::new(1),
            live_generations: Mutex::new(vec![Arc::downgrade(&initial)]),
        });
        let tickets = (0..10)
            .map(|_| constrained.begin_refresh())
            .collect::<Vec<_>>();
        assert!(
            tickets
                .windows(2)
                .all(|pair| pair[0].request != pair[1].request)
        );
        assert!(
            tickets
                .windows(2)
                .all(|pair| pair[0].key(None) == pair[1].key(None))
        );
        let expected_id = initial.id;
        let start = Arc::new(Barrier::new(11));
        let threads = tickets
            .into_iter()
            .map(|ticket| {
                let manager = Arc::clone(&constrained);
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    manager.refresh_with_ticket(ticket, Some(expected_id))
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        let first_error = results[0].as_ref().unwrap_err();
        assert!(
            results
                .iter()
                .all(|result| result.as_ref().unwrap_err() == first_error)
        );
        assert!(matches!(
            results[0],
            Err(RefreshRejection::Limit {
                kind: "active_skills",
                ..
            })
        ));
        assert_eq!(constrained.counters().builds, 1);
        assert_eq!(constrained.counters().scans, 1);
        assert_eq!(constrained.counters().files_read, 0);
    }

    #[test]
    fn expected_preconditions_are_isolated_in_both_completion_orders() {
        for stale_first in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let manager = FirstPartyGenerationManager::new(
                temp.path().to_path_buf(),
                GenerationLimits::default(),
            );
            let current = manager.generation().id;
            let stale = manager.begin_refresh();
            let valid = manager.begin_refresh();
            let (first, second) = if stale_first {
                (
                    manager.refresh_with_ticket(stale, Some(999)),
                    manager.refresh_with_ticket(valid, Some(current)),
                )
            } else {
                (
                    manager.refresh_with_ticket(valid, Some(current)),
                    manager.refresh_with_ticket(stale, Some(999)),
                )
            };
            let outcomes: [_; 2] = (first, second).into();
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|result| matches!(
                        result,
                        Err(RefreshRejection::Stale { expected: 999, .. })
                    ))
                    .count(),
                1
            );
            assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
            assert_eq!(manager.counters().builds, 2);
        }
    }

    #[test]
    #[ignore = "scale/RSS regression harness; run explicitly on a quiet host"]
    fn builds_deterministic_10_100_256_skill_corpora() {
        for count in [10usize, 100, 256] {
            let temp = tempfile::tempdir().unwrap();
            for index in 0..count {
                write_skill_with_max_resources(temp.path(), &format!("skill-{index:03}"));
            }
            let (_, bundled_bytes, _, bundled_resources) =
                FirstPartySkillProviders::from_local_skills([]).admission_totals();
            let limits = GenerationLimits {
                active_skills: 300,
                aggregate_bytes: 128 * 1024 * 1024,
                total_resources: 300 * limits::MAX_RESOURCES_PER_SKILL,
                live_candidate_bytes: 128 * 1024 * 1024,
                ..GenerationLimits::default()
            };
            #[cfg(target_os = "linux")]
            let rss_before = resident_set_bytes();
            let started = Instant::now();
            let manager = FirstPartyGenerationManager::new(temp.path().to_path_buf(), limits);
            let initial = manager.generation();
            assert_eq!(initial.providers.discover().len(), count + 2);
            assert_eq!(
                initial.resources,
                bundled_resources + count * limits::MAX_RESOURCES_PER_SKILL
            );
            assert!(initial.bytes >= bundled_bytes);
            // The unchanged rescan is the deterministic baseline: identical bytes
            // retain the digest and generation rather than publishing a new Arc.
            let baseline_digest = initial.digest.clone();
            let baseline_generation = initial.id;
            let refresh = manager.refresh(None).unwrap();
            assert!(!refresh.changed);
            assert_eq!(refresh.digest, baseline_digest);
            assert_eq!(refresh.generation, baseline_generation);
            assert!(started.elapsed() < Duration::from_secs(30));
            #[cfg(target_os = "linux")]
            if let (Some(before), Some(after)) = (rss_before, resident_set_bytes()) {
                // Allow the configured retained candidate plus a fixed allocator,
                // embedded-provider, and test-runner overhead ceiling.
                const RSS_OVERHEAD_BYTES: u64 = 128 * 1024 * 1024;
                assert!(
                    after.saturating_sub(before)
                        <= limits.live_candidate_bytes as u64 + RSS_OVERHEAD_BYTES,
                    "{count}-skill corpus exceeded the deterministic RSS ceiling"
                );
            }
        }
    }
}
