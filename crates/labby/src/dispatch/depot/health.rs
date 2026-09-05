//! Local observations only. Reading health never contacts a provider.
use super::provider::ProviderError;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Unauthorized,
    Incompatible,
    Configuration,
    Transient,
    NotFound,
    SnapshotChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    #[default]
    Unknown,
    Healthy,
    Unauthorized,
    Incompatible,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Qualification,
    List,
    Get,
    Probe,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub state: HealthState,
    pub observed_at: Option<u64>,
    pub provenance: Option<Provenance>,
    pub retry_not_before: Option<u64>,
}

#[derive(Default)]
struct Observation {
    view: HealthView,
    blocked: Option<Failure>,
    retry: Option<Instant>,
}

#[derive(Default)]
pub struct Health(Mutex<Observation>);

impl Health {
    pub fn view(&self) -> HealthView {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .view
            .clone()
    }
    pub fn admit(&self, manual: bool) -> Result<(), ProviderError> {
        if manual {
            return Ok(());
        }
        let observation = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(failure) = observation.blocked {
            return Err(ProviderError::Failed(failure));
        }
        if observation
            .retry
            .is_some_and(|retry| retry > Instant::now())
        {
            return Err(ProviderError::Pending);
        }
        Ok(())
    }
    pub fn record(&self, result: Result<(), Failure>, provenance: Provenance) {
        let mut observation = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if observation.blocked.is_some() && provenance != Provenance::Probe {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let (state, blocked) = match result {
            Ok(()) | Err(Failure::NotFound) => (HealthState::Healthy, None),
            Err(Failure::SnapshotChanged) => (HealthState::Unknown, None),
            Err(Failure::Unauthorized) => (HealthState::Unauthorized, Some(Failure::Unauthorized)),
            Err(Failure::Incompatible) => (HealthState::Incompatible, Some(Failure::Incompatible)),
            Err(Failure::Configuration) => (HealthState::Unavailable, Some(Failure::Configuration)),
            Err(Failure::Transient) => (HealthState::Unavailable, None),
        };
        let cooldown = matches!(result, Err(Failure::Transient))
            .then(|| 1 + u64::from(uuid::Uuid::new_v4().as_bytes()[0] % 30));
        *observation = Observation {
            view: HealthView {
                state,
                observed_at: Some(now),
                provenance: Some(provenance),
                retry_not_before: cooldown.map(|seconds| now + seconds),
            },
            blocked,
            retry: cooldown.map(|seconds| Instant::now() + Duration::from_secs(seconds)),
        };
    }
}
