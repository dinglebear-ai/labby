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
    current: std::sync::Mutex<Option<Arc<GoogleRefreshGeneration>>>,
}

#[cfg(feature = "http-axum")]
#[derive(Default)]
struct GoogleRefreshGeneration {
    completed: OnceLock<Result<crate::google::GoogleExchange, crate::error::AuthError>>,
    notify: tokio::sync::Notify,
}

#[cfg(feature = "http-axum")]
struct GoogleRefreshOwner {
    generation: Arc<GoogleRefreshGeneration>,
    completed: bool,
}

#[cfg(feature = "http-axum")]
impl GoogleRefreshOwner {
    fn finish(mut self, result: Result<crate::google::GoogleExchange, crate::error::AuthError>) {
        self.generation
            .completed
            .set(result)
            .expect("google refresh generation completes only once");
        self.completed = true;
        self.generation.notify.notify_waiters();
    }
}

#[cfg(feature = "http-axum")]
impl Drop for GoogleRefreshOwner {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        drop(
            self.generation
                .completed
                .set(Err(crate::error::AuthError::Server(
                    "shared google refresh owner was cancelled".to_string(),
                ))),
        );
        self.generation.notify.notify_waiters();
    }
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
    run_shared_flight(flight, operation, || {}).await
}

#[cfg(feature = "http-axum")]
async fn run_shared_flight<F, Fut, W>(
    flight: Arc<GoogleRefreshFlight>,
    operation: F,
    waiting: W,
) -> (
    bool,
    Result<crate::google::GoogleExchange, crate::error::AuthError>,
)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<crate::google::GoogleExchange, crate::error::AuthError>>,
    W: FnOnce(),
{
    // Every owner publishes into a distinct generation. A waiter retains that
    // generation even if a later caller starts and completes the next flight.
    let (generation, is_owner) = {
        let mut current = flight
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(generation) = current
            .as_ref()
            .filter(|generation| generation.completed.get().is_none())
        {
            (Arc::clone(generation), false)
        } else {
            let generation = Arc::new(GoogleRefreshGeneration::default());
            *current = Some(Arc::clone(&generation));
            (generation, true)
        }
    };
    if is_owner {
        let owner = GoogleRefreshOwner {
            generation,
            completed: false,
        };
        let result = operation().await;
        owner.finish(result.clone());
        return (true, result);
    }

    // `notify_waiters` does not retain a permit. Enable before checking the
    // immutable result slot: completion either already exists or wakes us.
    let notified = generation.notify.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if generation.completed.get().is_none() {
        waiting();
        notified.await;
    }
    let result = generation.completed.get().cloned().unwrap_or_else(|| {
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

    #[cfg(feature = "http-axum")]
    fn exchange() -> crate::google::GoogleExchange {
        exchange_with_token("access-token")
    }

    #[cfg(feature = "http-axum")]
    fn exchange_with_token(access_token: &str) -> crate::google::GoogleExchange {
        crate::google::GoogleExchange {
            subject: "subject".to_string(),
            email: None,
            email_verified: None,
            hosted_domain: None,
            access_token: access_token.to_string(),
            refresh_token: None,
            expires_in: Some(3_600),
            granted_scopes: Vec::new(),
            id_token: None,
        }
    }

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

    #[cfg(feature = "http-axum")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn waiter_cannot_miss_completion_between_condition_check_and_await() {
        let flight = Arc::new(super::GoogleRefreshFlight::default());
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let owner_flight = Arc::clone(&flight);
        let owner = tokio::spawn(async move {
            super::run_shared_flight(
                owner_flight,
                || async move {
                    started_tx.send(()).expect("test awaits owner start");
                    finish_rx.await.expect("test releases owner");
                    Ok(exchange())
                },
                || {},
            )
            .await
        });
        started_rx.await.expect("owner starts");

        let condition_checked = Arc::new(std::sync::Barrier::new(2));
        let allow_await = Arc::new(std::sync::Barrier::new(2));
        let waiter_flight = Arc::clone(&flight);
        let waiter_condition_checked = Arc::clone(&condition_checked);
        let waiter_allow_await = Arc::clone(&allow_await);
        let waiter = tokio::spawn(async move {
            super::run_shared_flight(
                waiter_flight,
                || async {
                    Err(crate::error::AuthError::Server(
                        "waiter unexpectedly owned the active flight".to_string(),
                    ))
                },
                || {
                    waiter_condition_checked.wait();
                    waiter_allow_await.wait();
                },
            )
            .await
        });

        condition_checked.wait();
        finish_tx.send(()).expect("owner still waiting");
        let (was_owner, owner_result) = owner.await.expect("owner task");
        assert!(was_owner);
        assert!(owner_result.is_ok());
        allow_await.wait();

        let (was_owner, waiter_result) =
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("enabled waiter observes the already-delivered notification")
                .expect("waiter task");
        assert!(!was_owner);
        assert_eq!(waiter_result.expect("shared result"), exchange());
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn prior_waiter_cannot_observe_a_later_generation_result() {
        let flight = Arc::new(super::GoogleRefreshFlight::default());
        let (first_started_tx, first_started_rx) = tokio::sync::oneshot::channel();
        let (finish_first_tx, finish_first_rx) = tokio::sync::oneshot::channel();
        let first_flight = Arc::clone(&flight);
        let first = tokio::spawn(async move {
            super::run_shared_flight(
                first_flight,
                || async move {
                    first_started_tx.send(()).expect("test awaits first owner");
                    finish_first_rx.await.expect("test releases first owner");
                    Ok(exchange_with_token("first-generation"))
                },
                || {},
            )
            .await
        });
        first_started_rx.await.expect("first owner starts");

        let waiter_registered = Arc::new(std::sync::Barrier::new(2));
        let release_waiter = Arc::new(std::sync::Barrier::new(2));
        let waiter_flight = Arc::clone(&flight);
        let waiter_registered_task = Arc::clone(&waiter_registered);
        let release_waiter_task = Arc::clone(&release_waiter);
        let waiter = tokio::spawn(async move {
            super::run_shared_flight(
                waiter_flight,
                || async {
                    Err(crate::error::AuthError::Server(
                        "waiter unexpectedly owned the first flight".to_string(),
                    ))
                },
                || {
                    waiter_registered_task.wait();
                    release_waiter_task.wait();
                },
            )
            .await
        });
        waiter_registered.wait();

        finish_first_tx.send(()).expect("first owner still waiting");
        let (was_first_owner, first_result) = first.await.expect("first owner task");
        assert!(was_first_owner);
        assert_eq!(
            first_result.expect("first result").access_token,
            "first-generation"
        );

        let (was_second_owner, second_result) = super::run_shared_flight(
            Arc::clone(&flight),
            || async { Ok(exchange_with_token("second-generation")) },
            || {},
        )
        .await;
        assert!(was_second_owner);
        assert_eq!(
            second_result.expect("second result").access_token,
            "second-generation"
        );

        release_waiter.wait();
        let (was_waiter_owner, waiter_result) = waiter.await.expect("waiter task");
        assert!(!was_waiter_owner);
        assert_eq!(
            waiter_result.expect("first shared result").access_token,
            "first-generation"
        );
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancelling_owner_wakes_waiter_and_allows_a_later_owner() {
        let flight = Arc::new(super::GoogleRefreshFlight::default());
        let (owner_started_tx, owner_started_rx) = tokio::sync::oneshot::channel();
        let owner_flight = Arc::clone(&flight);
        let owner = tokio::spawn(async move {
            super::run_shared_flight(
                owner_flight,
                || async move {
                    owner_started_tx.send(()).expect("test awaits start");
                    std::future::pending().await
                },
                || {},
            )
            .await
        });
        owner_started_rx.await.expect("owner starts");

        let waiter_ready = Arc::new(std::sync::Barrier::new(2));
        let release_waiter = Arc::new(std::sync::Barrier::new(2));
        let waiter_flight = Arc::clone(&flight);
        let waiter_ready_task = Arc::clone(&waiter_ready);
        let release_waiter_task = Arc::clone(&release_waiter);
        let waiter = tokio::spawn(async move {
            super::run_shared_flight(
                waiter_flight,
                || async {
                    Err(crate::error::AuthError::Server(
                        "waiter unexpectedly owned the active flight".to_string(),
                    ))
                },
                || {
                    waiter_ready_task.wait();
                    release_waiter_task.wait();
                },
            )
            .await
        });
        waiter_ready.wait();
        owner.abort();
        assert!(owner.await.expect_err("owner is cancelled").is_cancelled());
        release_waiter.wait();

        let (_, waiter_result) = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("cancelled owner wakes waiter")
            .expect("waiter task");
        assert!(matches!(
            waiter_result,
            Err(crate::error::AuthError::Server(message))
                if message == "shared google refresh owner was cancelled"
        ));

        let (was_owner, retry_result) =
            super::run_shared_flight(flight, || async { Ok(exchange()) }, || {}).await;
        assert!(was_owner);
        assert!(retry_result.is_ok());
    }
}
