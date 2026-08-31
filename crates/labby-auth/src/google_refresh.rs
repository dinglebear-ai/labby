//! Process-wide single-flight coordination for central Google credentials.

#[cfg(feature = "http-axum")]
use std::future::Future;
use std::sync::{Arc, OnceLock};
#[cfg(feature = "http-axum")]
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;

const MAX_GOOGLE_REFRESH_LOCKS: usize = 2_048;

struct GoogleRefreshLocks {
    locks: DashMap<String, Arc<Mutex<()>>>,
    overflow: Arc<Mutex<()>>,
    maintenance: std::sync::Mutex<()>,
}

static GOOGLE_PROVIDER_REFRESH_LOCKS: OnceLock<GoogleRefreshLocks> = OnceLock::new();
#[cfg(feature = "http-axum")]
const SHARED_FAILURE_TTL: Duration = Duration::from_secs(2);
#[cfg(feature = "http-axum")]
static GOOGLE_REFRESH_FLIGHT_MAINTENANCE: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "http-axum")]
#[derive(Default)]
pub(crate) struct GoogleRefreshFlight {
    running: Mutex<bool>,
    completed:
        std::sync::Mutex<Option<Result<crate::google::GoogleExchange, crate::error::AuthError>>>,
    notify: tokio::sync::Notify,
}

#[cfg(feature = "http-axum")]
pub(crate) async fn run_shared<F, Fut>(
    state: &crate::state::AuthState,
    subject: &str,
    operation: F,
) -> (
    bool,
    Result<crate::google::GoogleExchange, crate::error::AuthError>,
)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<crate::google::GoogleExchange, crate::error::AuthError>>,
{
    let flight = {
        let _maintenance = GOOGLE_REFRESH_FLIGHT_MAINTENANCE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = state.google_refresh_flights.get(subject) {
            existing.value().clone()
        } else {
            if state.google_refresh_flights.len() >= MAX_GOOGLE_REFRESH_LOCKS {
                let idle = state
                    .google_refresh_flights
                    .iter()
                    .find(|entry| Arc::strong_count(entry.value()) == 1)
                    .map(|entry| entry.key().clone());
                if let Some(idle) = idle {
                    state.google_refresh_flights.remove(&idle);
                } else {
                    return (
                        false,
                        Err(crate::error::AuthError::Server(
                            "google refresh coordination capacity exhausted".to_string(),
                        )),
                    );
                }
            }
            state
                .google_refresh_flights
                .entry(subject.to_string())
                .or_insert_with(|| Arc::new(GoogleRefreshFlight::default()))
                .clone()
        }
    };
    let notified = flight.notify.notified();
    {
        let mut running = flight.running.lock().await;
        if !*running {
            *running = true;
            drop(running);
            let result = operation().await;
            *flight
                .completed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result.clone());
            *flight.running.lock().await = false;
            flight.notify.notify_waiters();
            return (true, result);
        }
    }
    notified.await;
    let result = flight
        .completed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| {
            Err(crate::error::AuthError::Server(
                "shared refresh result missing".to_string(),
            ))
        });
    (false, result)
}

/// Return the process-wide mutex for one stable Google provider subject.
///
/// Inbound Labby token rotation, outbound Google MCP refresh, status probes, and
/// explicit revocation all use this same lock so one central refresh credential
/// is never refreshed or deleted concurrently by separate product surfaces.
pub(crate) fn lock(subject: &str) -> Arc<Mutex<()>> {
    let registry = GOOGLE_PROVIDER_REFRESH_LOCKS.get_or_init(|| GoogleRefreshLocks {
        locks: DashMap::new(),
        overflow: Arc::new(Mutex::new(())),
        maintenance: std::sync::Mutex::new(()),
    });
    let _maintenance = registry
        .maintenance
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = registry.locks.get(subject) {
        return existing.value().clone();
    }
    if registry.locks.len() >= MAX_GOOGLE_REFRESH_LOCKS {
        let idle = registry
            .locks
            .iter()
            .find(|entry| Arc::strong_count(entry.value()) == 1)
            .map(|entry| entry.key().clone());
        if let Some(idle) = idle {
            registry.locks.remove(&idle);
        } else {
            return Arc::clone(&registry.overflow);
        }
    }
    registry
        .locks
        .entry(subject.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[cfg(feature = "http-axum")]
pub(crate) fn recent_transient_failure(
    state: &crate::state::AuthState,
    subject: &str,
) -> Option<crate::error::AuthError> {
    let failures = &state.google_refresh_failures;
    let entry = failures.get(subject)?;
    if entry.value().2.elapsed() >= SHARED_FAILURE_TTL {
        drop(entry);
        failures.remove(subject);
        return None;
    }
    let (network, message, _) = entry.value();
    Some(if *network {
        crate::error::AuthError::Network(message.clone())
    } else {
        crate::error::AuthError::Server(message.clone())
    })
}

#[cfg(feature = "http-axum")]
pub(crate) fn record_transient_failure(
    state: &crate::state::AuthState,
    subject: &str,
    error: &crate::error::AuthError,
) {
    let (network, message) = match error {
        crate::error::AuthError::Network(message) => (true, message.clone()),
        crate::error::AuthError::Server(message) => (false, message.clone()),
        _ => return,
    };
    let failures = &state.google_refresh_failures;
    failures.retain(|_, (_, _, at)| at.elapsed() < SHARED_FAILURE_TTL);
    if failures.len() >= MAX_GOOGLE_REFRESH_LOCKS
        && let Some(oldest) = failures
            .iter()
            .max_by_key(|entry| entry.value().2.elapsed())
            .map(|entry| entry.key().clone())
    {
        failures.remove(&oldest);
    }
    failures.insert(subject.to_string(), (network, message, Instant::now()));
}

#[cfg(feature = "http-axum")]
pub(crate) fn clear_transient_failure(state: &crate::state::AuthState, subject: &str) {
    state.google_refresh_failures.remove(subject);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::lock;

    #[test]
    fn lock_is_shared_per_google_subject() {
        let left = lock("google-subject-lock-test");
        let right = lock("google-subject-lock-test");
        let other = lock("different-google-subject-lock-test");

        assert!(Arc::ptr_eq(&left, &right));
        assert!(!Arc::ptr_eq(&left, &other));
    }

    #[test]
    fn active_subjects_use_bounded_overflow_lock_after_registry_cap() {
        let held = (0..super::MAX_GOOGLE_REFRESH_LOCKS)
            .map(|index| lock(&format!("bounded-subject-{index}")))
            .collect::<Vec<_>>();
        let overflow_one = lock("bounded-overflow-one");
        let overflow_two = lock("bounded-overflow-two");
        let registry = super::GOOGLE_PROVIDER_REFRESH_LOCKS
            .get()
            .expect("refresh registry initialized");
        assert!(registry.locks.len() <= super::MAX_GOOGLE_REFRESH_LOCKS);
        assert!(Arc::ptr_eq(&overflow_one, &overflow_two));
        drop(held);
    }
}
