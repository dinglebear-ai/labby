//! Bounded admission shared by discovery, qualification, detail, and probes.
//! No spawned tasks or background maintenance; dropping futures releases slots.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

const RECEIPT_BUDGET: Duration = Duration::from_secs(5);

/// Work not attempted because of admission or deadline is pending, not failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Depot work is pending capacity or a fresh request budget")]
pub struct Pending;

struct ActorSlots {
    active: Arc<Semaphore>,
    total: Arc<Semaphore>,
}

pub struct Scheduler {
    active: Arc<Semaphore>,
    total: Arc<Semaphore>,
    calls: Arc<Semaphore>,
    probe: Arc<Semaphore>,
    actors: Mutex<BTreeMap<String, Weak<ActorSlots>>>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            active: Arc::new(Semaphore::new(4)),
            total: Arc::new(Semaphore::new(20)),
            calls: Arc::new(Semaphore::new(16)),
            probe: Arc::new(Semaphore::new(1)),
            actors: Mutex::new(BTreeMap::new()),
        }
    }
}

/// One instance belongs to one provider incarnation and all of its callers.
#[derive(Clone)]
pub struct ProviderAdmission(Arc<Semaphore>);

impl Default for ProviderAdmission {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(2)))
    }
}

/// Keeps the federation/actor slots alive through merge and serialization.
pub struct Admission {
    deadline: Instant,
    calls: Arc<Semaphore>,
    local_calls: Arc<Semaphore>,
    _held: Vec<OwnedSemaphorePermit>,
    _actor: Option<Arc<ActorSlots>>,
}

/// All permits live exactly as long as the upstream operation, including decode.
pub struct CallPermit {
    _provider: OwnedSemaphorePermit,
    _local: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

impl Scheduler {
    /// `actor` must come from verified browser authority, never a request field.
    pub async fn admit(&self, actor: &str, receipt: Instant) -> Result<Admission, Pending> {
        if actor.is_empty() || actor.len() > 256 {
            return Err(Pending);
        }
        let deadline = deadline(receipt)?;
        // Reserve the bounded global envelope before allocating actor state.
        let global_total = self
            .total
            .clone()
            .try_acquire_owned()
            .map_err(|_| Pending)?;
        let actor = {
            let mut actors = self
                .actors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            actors.retain(|_, slots| slots.strong_count() > 0);
            if let Some(slots) = actors.get(actor).and_then(Weak::upgrade) {
                slots
            } else {
                let slots = Arc::new(ActorSlots {
                    active: Arc::new(Semaphore::new(1)),
                    total: Arc::new(Semaphore::new(3)),
                });
                actors.insert(actor.to_owned(), Arc::downgrade(&slots));
                slots
            }
        };
        let actor_total = actor
            .total
            .clone()
            .try_acquire_owned()
            .map_err(|_| Pending)?;
        let actor_active = acquire(actor.active.clone(), deadline).await?;
        let global_active = acquire(self.active.clone(), deadline).await?;
        if Instant::now() >= deadline {
            return Err(Pending);
        }
        Ok(Admission {
            deadline,
            calls: self.calls.clone(),
            local_calls: Arc::new(Semaphore::new(4)),
            _held: vec![global_total, actor_total, actor_active, global_active],
            _actor: Some(actor),
        })
    }

    pub fn probe(&self, receipt: Instant) -> Result<Admission, Pending> {
        let deadline = deadline(receipt)?;
        let permit = self
            .probe
            .clone()
            .try_acquire_owned()
            .map_err(|_| Pending)?;
        Ok(Admission {
            deadline,
            calls: self.calls.clone(),
            local_calls: Arc::new(Semaphore::new(1)),
            _held: vec![permit],
            _actor: None,
        })
    }

    #[cfg(test)]
    pub(super) fn retained_actors(&self) -> usize {
        self.actors.lock().unwrap().len()
    }
}

impl Admission {
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Never wait with any call permit held: unavailable providers are skipped.
    pub fn try_call(&self, provider: &ProviderAdmission) -> Result<CallPermit, Pending> {
        if Instant::now() >= self.deadline {
            return Err(Pending);
        }
        let provider = provider
            .0
            .clone()
            .try_acquire_owned()
            .map_err(|_| Pending)?;
        let local = self
            .local_calls
            .clone()
            .try_acquire_owned()
            .map_err(|_| Pending)?;
        let global = self
            .calls
            .clone()
            .try_acquire_owned()
            .map_err(|_| Pending)?;
        Ok(CallPermit {
            _provider: provider,
            _local: local,
            _global: global,
        })
    }
}

fn deadline(receipt: Instant) -> Result<Instant, Pending> {
    let now = Instant::now();
    let deadline = receipt + RECEIPT_BUDGET;
    if receipt > now || deadline <= now {
        Err(Pending)
    } else {
        Ok(deadline)
    }
}

async fn acquire(
    semaphore: Arc<Semaphore>,
    deadline: Instant,
) -> Result<OwnedSemaphorePermit, Pending> {
    tokio::time::timeout_at(deadline, semaphore.acquire_owned())
        .await
        .map_err(|_| Pending)?
        .map_err(|_| Pending)
}
