//! Churn accounting for `notifications/tools/list_changed`.
//!
//! A single catalog notification is normal. The failure mode operators actually
//! hit is *repetition*: clients discard and rebuild their connector namespace
//! on every notification, so a burst of them invalidates tool bindings mid-turn
//! and calls fail before reaching Labby. That is indistinguishable from healthy
//! behavior in a per-event log — you need the rate, and you need to know whether
//! a tool call was open at the time.
//!
//! This module owns both:
//!
//! * a rolling count of notifications in a recent window, so a burst is one
//!   `WARN` instead of N indistinguishable `INFO`s, and
//! * an in-flight tool-call gauge, so a notification emitted *during* a call —
//!   the case that actually breaks a turn — is visible as such.
//!
//! State is process-global because there is exactly one MCP server per process
//! and the emitters are spread across two crates; threading a handle from the
//! gateway reconcile through the mpsc fanout to the per-call MCP paths would be
//! plumbing with no added fidelity.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Notifications within [`churn_window`] at or above this count are reported as
/// churn. Four bursts in a minute is already past anything a settled gateway
/// does; the default errs toward reporting rather than staying quiet.
const DEFAULT_CHURN_THRESHOLD: usize = 4;
const DEFAULT_CHURN_WINDOW_SECS: u64 = 60;
/// Bounds on operator-supplied overrides. A window under a few seconds cannot
/// hold a meaningful burst, and one over an hour stops being actionable.
const MIN_CHURN_WINDOW_SECS: u64 = 5;
const MAX_CHURN_WINDOW_SECS: u64 = 3_600;
const MIN_CHURN_THRESHOLD: usize = 2;
/// Hard cap on retained timestamps so a pathological notification storm cannot
/// grow the window buffer without bound between prunes.
const MAX_WINDOW_SAMPLES: usize = 4_096;

/// Monotonic process clock. `Instant` rather than wall time so a clock step
/// (NTP, suspend/resume) cannot fabricate or hide a burst.
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);

static IN_FLIGHT_TOOL_CALLS: AtomicUsize = AtomicUsize::new(0);
static NOTIFY_TOTAL: AtomicUsize = AtomicUsize::new(0);
static WINDOW: LazyLock<Mutex<VecDeque<Duration>>> =
    LazyLock::new(|| Mutex::new(VecDeque::with_capacity(16)));

fn churn_window() -> Duration {
    static WINDOW_SECS: LazyLock<u64> = LazyLock::new(|| {
        env_parsed("LABBY_MCP_CATALOG_CHURN_WINDOW_SECS")
            .map(|secs: u64| secs.clamp(MIN_CHURN_WINDOW_SECS, MAX_CHURN_WINDOW_SECS))
            .unwrap_or(DEFAULT_CHURN_WINDOW_SECS)
    });
    Duration::from_secs(*WINDOW_SECS)
}

fn churn_threshold() -> usize {
    static THRESHOLD: LazyLock<usize> = LazyLock::new(|| {
        env_parsed("LABBY_MCP_CATALOG_CHURN_THRESHOLD")
            .map(|threshold: usize| threshold.max(MIN_CHURN_THRESHOLD))
            .unwrap_or(DEFAULT_CHURN_THRESHOLD)
    });
    *THRESHOLD
}

fn env_parsed<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.trim().parse().ok()
}

/// RAII gauge for an in-flight MCP tool call.
///
/// Held for the whole `call_tool` dispatch so any notification emitted while it
/// lives is flagged as landing mid-call. Decrements on drop, including on the
/// early-return and error paths, which is the reason this is a guard rather
/// than a pair of counter calls.
#[derive(Debug)]
pub(crate) struct InFlightToolCall {
    _private: (),
}

impl InFlightToolCall {
    pub(crate) fn enter() -> Self {
        IN_FLIGHT_TOOL_CALLS.fetch_add(1, Ordering::Relaxed);
        Self { _private: () }
    }
}

impl Drop for InFlightToolCall {
    fn drop(&mut self) {
        // `fetch_update` rather than `fetch_sub` so a double-drop bug can never
        // wrap the gauge to `usize::MAX` and make every later notification look
        // like it landed mid-call.
        let _ =
            IN_FLIGHT_TOOL_CALLS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(1))
            });
    }
}

pub(crate) fn in_flight_tool_calls() -> usize {
    IN_FLIGHT_TOOL_CALLS.load(Ordering::Relaxed)
}

/// One notification's churn context, as of the moment it was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChurnSample {
    /// Notifications emitted since process start, including this one.
    pub(crate) total: usize,
    /// Notifications emitted within the churn window, including this one.
    pub(crate) window_count: usize,
    /// Gap since the previous notification. `None` for the first one.
    pub(crate) since_last_ms: Option<u128>,
    /// Tool calls open at emission time. Non-zero means this notification can
    /// invalidate a binding a caller is mid-way through using.
    pub(crate) in_flight_tool_calls: usize,
    pub(crate) window_secs: u64,
    pub(crate) threshold: usize,
}

impl ChurnSample {
    /// Whether this notification is part of a burst worth reporting.
    pub(crate) fn is_churning(self) -> bool {
        self.window_count >= self.threshold
    }
}

/// Record a notification and return its churn context.
///
/// Call once per emission, at the fanout choke point — recording at the
/// individual emitters would double-count a diff that fans out to many peers.
pub(crate) fn record_notification() -> ChurnSample {
    let window = churn_window();
    let now = PROCESS_START.elapsed();
    let total = NOTIFY_TOTAL.fetch_add(1, Ordering::Relaxed) + 1;

    let mut samples = lock_window();
    let since_last_ms = samples
        .back()
        .map(|previous| now.saturating_sub(*previous).as_millis());
    let cutoff = now.checked_sub(window).unwrap_or_default();
    while samples.front().is_some_and(|stamp| *stamp < cutoff) {
        samples.pop_front();
    }
    samples.push_back(now);
    if samples.len() > MAX_WINDOW_SAMPLES {
        samples.pop_front();
    }
    let window_count = samples.len();

    ChurnSample {
        total,
        window_count,
        since_last_ms,
        in_flight_tool_calls: in_flight_tool_calls(),
        window_secs: window.as_secs(),
        threshold: churn_threshold(),
    }
}

fn lock_window() -> MutexGuard<'static, VecDeque<Duration>> {
    // A panic while holding this lock would only have left a timestamp deque in
    // a valid-but-stale state; churn accounting must never take the process
    // down with it.
    WINDOW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) fn reset_notification_history_for_test() {
    NOTIFY_TOTAL.store(0, Ordering::Relaxed);
    lock_window().clear();
}

#[cfg(test)]
mod tests {
    use super::{
        ChurnSample, InFlightToolCall, in_flight_tool_calls, record_notification,
        reset_notification_history_for_test,
    };

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_support::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn first_notification_has_no_predecessor() {
        let _guard = serial();
        reset_notification_history_for_test();

        let sample = record_notification();

        assert_eq!(sample.total, 1);
        assert_eq!(sample.window_count, 1);
        assert_eq!(sample.since_last_ms, None);
        assert!(!sample.is_churning(), "one notification is not churn");
    }

    #[test]
    fn repeated_notifications_accumulate_into_the_window() {
        let _guard = serial();
        reset_notification_history_for_test();

        let samples: Vec<ChurnSample> = (0..5).map(|_| record_notification()).collect();

        assert_eq!(samples[4].total, 5);
        assert_eq!(samples[4].window_count, 5);
        assert!(
            samples[1].since_last_ms.is_some(),
            "every notification after the first reports a gap"
        );
        // Default threshold is 4, so the burst reports churn — and reports it
        // from the 4th onward, not only at the end.
        assert!(!samples[2].is_churning());
        assert!(samples[3].is_churning());
        assert!(samples[4].is_churning());
    }

    #[test]
    fn in_flight_gauge_tracks_guard_lifetime() {
        let _guard = serial();
        reset_notification_history_for_test();

        // `serial()` orders the churn tests against each other, but the gauge is
        // a process-global that production `call_tool` also drives, so a tool
        // call from a test running in parallel can be open the whole time. Only
        // this test's own deltas are meaningful; absolute counts are not.
        let base = in_flight_tool_calls();
        {
            let _call = InFlightToolCall::enter();
            assert_eq!(in_flight_tool_calls(), base + 1);
            {
                let _nested = InFlightToolCall::enter();
                assert_eq!(in_flight_tool_calls(), base + 2);
            }
            assert_eq!(in_flight_tool_calls(), base + 1);

            // The field that matters: a notification emitted here is marked as
            // landing while a caller's turn is open.
            assert_eq!(record_notification().in_flight_tool_calls, base + 1);
        }
        assert_eq!(in_flight_tool_calls(), base);
        assert_eq!(record_notification().in_flight_tool_calls, base);
    }

    #[test]
    fn gauge_never_wraps_below_zero() {
        let _guard = serial();
        reset_notification_history_for_test();

        let base = in_flight_tool_calls();
        drop(InFlightToolCall::enter());
        drop(InFlightToolCall::enter());

        assert_eq!(
            in_flight_tool_calls(),
            base,
            "saturating decrement, no wrap"
        );
    }

    #[test]
    fn resetting_notification_history_does_not_steal_live_call_ownership() {
        let _guard = serial();
        reset_notification_history_for_test();
        let base = in_flight_tool_calls();
        let older_call = InFlightToolCall::enter();

        // A history reset may happen while an unrelated request is active. It
        // must not zero the RAII gauge owned by that request.
        reset_notification_history_for_test();
        let current_call = InFlightToolCall::enter();
        drop(older_call);
        assert_eq!(
            in_flight_tool_calls(),
            base + 1,
            "dropping the older call must leave the current call accounted for"
        );

        drop(current_call);
        assert_eq!(in_flight_tool_calls(), base);
    }
}
