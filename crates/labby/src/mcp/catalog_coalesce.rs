//! Coalescing and turn-aware deferral for `tools/list_changed`.
//!
//! Emitting a catalog notification is not free: a client reacts by discarding
//! and rebuilding its connector namespace. Two properties follow, and neither
//! is expressible at the individual emitters — they only make sense where every
//! emitter converges.
//!
//! **One net change, one notification.** A single operator action can fan into
//! several triggers — a reload swaps the pool, an enrichment apply writes a
//! hint, the next tool call observes a delta. Per-peer contract diffing already
//! drops triggers that move nothing, but a *burst* of individually-real changes
//! (three upstreams hydrating in sequence) still produces three notifications
//! for what a client experiences as one. Waiting for the burst to settle
//! collapses them.
//!
//! **Never mid-turn.** A notification delivered while a tool call is open
//! invalidates the binding that call is using, which is the failure clients
//! actually report: the call fails before reaching Labby, carrying no trace.
//! Holding the notification until in-flight calls drain means the client's
//! bindings stay valid for the duration of the call it is making, and it learns
//! about the change immediately afterwards.
//!
//! Deferral is bounded. A gateway under continuous load would otherwise never
//! reach a quiet moment, so `max_hold` forces delivery even with calls still in
//! flight — a late notification is a nuisance, a lost one is a bug.
//!
//! Coalescing is by *settling*, not by a fixed timer: the debounce restarts on
//! each new trigger, so an ongoing burst is delivered once when it ends rather
//! than sliced on an arbitrary boundary. What is finally sent is recomputed
//! per peer at flush time (see `catalog_notifications::notify_catalog_peers`),
//! so the delivered notification reflects the settled state, never a stale
//! intermediate one.

use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crate::mcp::catalog_churn::in_flight_tool_calls;
use crate::mcp::catalog_notifications::{CatalogNotificationChanges, notify_catalog_peers};
use crate::mcp::peers::PeerRegistry;

/// How long the trigger stream must be quiet before a flush runs. Long enough
/// that a reconcile's follow-on triggers land in the same batch, short enough
/// that a client's tool list is never meaningfully stale.
const DEFAULT_COALESCE_MS: u64 = 250;
/// Upper bound on total deferral, including waiting for in-flight calls to
/// drain. A busy gateway must still be told.
const DEFAULT_MAX_HOLD_MS: u64 = 5_000;
/// Poll interval while waiting for in-flight tool calls to drain.
const DRAIN_POLL_MS: u64 = 50;

const MIN_COALESCE_MS: u64 = 1;
const MAX_COALESCE_MS: u64 = 10_000;
const MIN_MAX_HOLD_MS: u64 = 100;
const MAX_MAX_HOLD_MS: u64 = 120_000;

fn coalesce_window() -> Duration {
    static MS: LazyLock<u64> = LazyLock::new(|| {
        env_parsed("LABBY_MCP_CATALOG_COALESCE_MS")
            .map(|ms: u64| ms.clamp(MIN_COALESCE_MS, MAX_COALESCE_MS))
            .unwrap_or(DEFAULT_COALESCE_MS)
    });
    Duration::from_millis(*MS)
}

fn max_hold() -> Duration {
    static MS: LazyLock<u64> = LazyLock::new(|| {
        env_parsed("LABBY_MCP_CATALOG_MAX_HOLD_MS")
            .map(|ms: u64| ms.clamp(MIN_MAX_HOLD_MS, MAX_MAX_HOLD_MS))
            .unwrap_or(DEFAULT_MAX_HOLD_MS)
    });
    Duration::from_millis(*MS)
}

fn env_parsed<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.trim().parse().ok()
}

/// Triggers accumulated since the last flush.
#[derive(Default)]
struct Pending {
    changes: Option<CatalogNotificationChanges>,
    /// Every emitter that contributed, so the flush log names all of them
    /// rather than crediting the batch to whichever fired last.
    sources: BTreeSet<&'static str>,
    /// Bumped on every trigger; the flusher uses it to tell "the burst is
    /// still growing" from "nothing has happened since I last looked".
    epoch: u64,
    flush_running: bool,
}

static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(|| Mutex::new(Pending::default()));

fn pending() -> MutexGuard<'static, Pending> {
    // A poisoned lock here would only mean a previous flush panicked mid-update;
    // catalog bookkeeping must not take the process down with it.
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Record a catalog change and ensure exactly one flush is pending.
///
/// Returns immediately — the caller is usually finishing a reconcile or a tool
/// call and must not block on peer IO. Every emitter should call this rather
/// than `notify_catalog_peers` directly, so bursts collapse and nothing is
/// delivered into an open turn.
pub(crate) fn schedule_catalog_notification(
    peers: &PeerRegistry,
    changes: CatalogNotificationChanges,
    source: &'static str,
) {
    if !changes.any() {
        return;
    }

    let start_flush = {
        let mut pending = pending();
        pending.changes = Some(match pending.changes {
            Some(existing) => existing.merged_with(changes),
            None => changes,
        });
        pending.sources.insert(source);
        pending.epoch = pending.epoch.wrapping_add(1);
        if pending.flush_running {
            false
        } else {
            pending.flush_running = true;
            true
        }
    };

    if start_flush {
        let peers = std::sync::Arc::clone(peers);
        tokio::spawn(async move { flush_when_settled(peers).await });
    }
}

/// Wait for the trigger burst to settle and for open turns to drain, then
/// deliver one notification for everything accumulated.
async fn flush_when_settled(peers: PeerRegistry) {
    let coalesce = coalesce_window();
    let max_hold = max_hold();
    let deadline = tokio::time::Instant::now() + max_hold;

    // Phase 1: settle. Restart the window whenever a new trigger arrives, so an
    // ongoing burst is delivered once at its end.
    loop {
        let epoch_before = pending().epoch;
        tokio::time::sleep(coalesce).await;
        let settled = pending().epoch == epoch_before;
        if settled || tokio::time::Instant::now() >= deadline {
            break;
        }
    }

    // Phase 2: stay out of open turns. A notification here would invalidate a
    // binding a caller is actively using.
    let mut deferred_for_calls_ms = 0u128;
    let drain_started = tokio::time::Instant::now();
    while in_flight_tool_calls() > 0 {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(DRAIN_POLL_MS)).await;
        deferred_for_calls_ms = drain_started.elapsed().as_millis();
    }

    let (changes, sources, epoch) = {
        let mut pending = pending();
        pending.flush_running = false;
        let changes = pending.changes.take();
        let sources = std::mem::take(&mut pending.sources);
        (changes, sources, pending.epoch)
    };

    let Some(changes) = changes else {
        return;
    };

    let source_list: Vec<&str> = sources.iter().copied().collect();
    let in_flight_at_flush = in_flight_tool_calls();
    tracing::debug!(
        surface = "mcp",
        service = "peers",
        action = "catalog.notify.flush",
        subsystem = "mcp_server",
        sources = ?source_list,
        source_count = source_list.len(),
        trigger_epoch = epoch,
        deferred_for_calls_ms,
        // Non-zero means the hold expired with calls still running: the
        // notification is being delivered mid-turn deliberately, because
        // withholding it indefinitely would be worse.
        in_flight_tool_calls = in_flight_at_flush,
        "flushing coalesced catalog change"
    );

    // Attribute the batch to a single source when there is one, so the existing
    // `source` field keeps its meaning; otherwise mark it as coalesced.
    let source = match source_list.as_slice() {
        [only] => sources.iter().copied().next().unwrap_or(only),
        _ => labby_runtime::catalog_notify::SOURCE_COALESCED,
    };
    notify_catalog_peers(&peers, changes, source).await;
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    let mut pending = pending();
    *pending = Pending::default();
}

#[cfg(test)]
mod tests {
    use std::sync::MutexGuard;

    use super::{Pending, coalesce_window, max_hold, pending, reset_for_test};

    fn serial_catalog() -> MutexGuard<'static, ()> {
        crate::test_support::CATALOG_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    use crate::mcp::catalog_notifications::CatalogNotificationChanges;

    #[test]
    fn merging_accumulates_every_changed_kind() {
        let _catalog_lock = serial_catalog();
        // A batch must not lose a kind because a later trigger only moved a
        // different one.
        let tools = CatalogNotificationChanges::new(true, false, false);
        let prompts = CatalogNotificationChanges::new(false, false, true);

        let merged = tools.merged_with(prompts);

        assert!(merged.tools_changed);
        assert!(merged.prompts_changed);
        assert!(!merged.resources_changed);
    }

    #[test]
    fn windows_are_bounded_and_ordered() {
        let _catalog_lock = serial_catalog();
        // The hold must exceed the settle window, or a burst could never settle
        // before delivery is forced.
        assert!(
            max_hold() > coalesce_window(),
            "max hold must leave room for at least one settle window"
        );
    }

    #[test]
    fn pending_starts_empty() {
        let _catalog_lock = serial_catalog();
        reset_for_test();
        let pending: &Pending = &pending();
        assert!(pending.changes.is_none());
        assert!(pending.sources.is_empty());
        assert!(!pending.flush_running);
    }
}
