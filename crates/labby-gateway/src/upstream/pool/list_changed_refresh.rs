//! Per-upstream, coalesced `list_changed` refresh workers.
//!
//! The pool's notification bus has one consumer. If that consumer awaited an
//! upstream re-list inline, a slow or chatty upstream would stall delivery of
//! every other upstream's events and could push the broadcast channel into
//! lag. Instead the consumer only records which catalog kinds an exact
//! upstream reported as changed; a worker task per upstream performs the
//! re-list off the consumer loop and forwards the downstream notification for
//! that upstream once its refresh completed. Peers therefore never observe a
//! forwarded `list_changed` while the cached catalog is still stale, and one
//! upstream's refresh never delays another upstream's events.
//!
//! The forwarder is an injected callback so this module stays surface-neutral:
//! the MCP peer fanout lives with the peer registry, not here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::task::JoinHandle;

use super::UpstreamPool;

/// How long a worker waits at the start of each iteration so a burst of
/// `list_changed` signals from one upstream collapses into a single re-list.
pub const LIST_CHANGED_COALESCE_WINDOW: Duration = Duration::from_millis(250);

/// Which catalog families one upstream reported as changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListChangedKinds {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

impl ListChangedKinds {
    pub const TOOLS: Self = Self {
        tools: true,
        resources: false,
        prompts: false,
    };
    pub const RESOURCES: Self = Self {
        tools: false,
        resources: true,
        prompts: false,
    };
    pub const PROMPTS: Self = Self {
        tools: false,
        resources: false,
        prompts: true,
    };

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !(self.tools || self.resources || self.prompts)
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            tools: self.tools || other.tools,
            resources: self.resources || other.resources,
            prompts: self.prompts || other.prompts,
        }
    }
}

/// Called after one upstream's refresh completed, with the kinds it covered.
/// It must return promptly: it runs on the worker task and is expected to
/// schedule downstream delivery rather than perform peer IO itself.
type ListChangedForwarder = Arc<dyn Fn(&str, ListChangedKinds) + Send + Sync>;

struct Worker {
    /// Kinds signalled since the worker last took its batch.
    pending: ListChangedKinds,
    handle: JoinHandle<()>,
}

type Workers = Arc<Mutex<HashMap<String, Worker>>>;

/// Owns the per-upstream refresh workers for one pool. Dropping it aborts
/// every worker, so its lifetime should match the notification consumer that
/// feeds it.
pub struct ListChangedRefresher {
    pool: Arc<UpstreamPool>,
    workers: Workers,
    forward: ListChangedForwarder,
    coalesce_window: Duration,
}

impl ListChangedRefresher {
    pub fn new(
        pool: Arc<UpstreamPool>,
        forward: impl Fn(&str, ListChangedKinds) + Send + Sync + 'static,
    ) -> Self {
        Self {
            pool,
            workers: Arc::default(),
            forward: Arc::new(forward),
            coalesce_window: LIST_CHANGED_COALESCE_WINDOW,
        }
    }

    /// Record `kinds` as pending for `upstream` and make sure a worker will
    /// refresh them. Returns immediately.
    ///
    /// While a worker exists for the upstream, the kinds merge into its
    /// pending set and it runs another iteration after the current one; the
    /// worker exits only once it observes an empty pending set, and it removes
    /// itself from the task set under the same lock so no signal is lost
    /// between the check and the removal.
    pub fn schedule(&self, upstream: &str, kinds: ListChangedKinds) {
        if kinds.is_empty() {
            return;
        }
        let mut workers = lock_workers(&self.workers);
        if let Some(worker) = workers.get_mut(upstream) {
            worker.pending = worker.pending.union(kinds);
            return;
        }
        let handle = tokio::spawn(run_worker(
            Arc::clone(&self.pool),
            Arc::clone(&self.workers),
            Arc::clone(&self.forward),
            upstream.to_string(),
            self.coalesce_window,
        ));
        workers.insert(
            upstream.to_string(),
            Worker {
                pending: kinds,
                handle,
            },
        );
    }

    #[cfg(test)]
    pub(super) fn active_worker_count_for_test(&self) -> usize {
        lock_workers(&self.workers).len()
    }
}

impl Drop for ListChangedRefresher {
    fn drop(&mut self) {
        for (_, worker) in lock_workers(&self.workers).drain() {
            worker.handle.abort();
        }
    }
}

fn lock_workers(workers: &Workers) -> MutexGuard<'_, HashMap<String, Worker>> {
    // A worker that panicked mid-update leaves nothing inconsistent behind:
    // the map only holds plain flags and handles.
    workers.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn run_worker(
    pool: Arc<UpstreamPool>,
    workers: Workers,
    forward: ListChangedForwarder,
    upstream: String,
    coalesce_window: Duration,
) {
    loop {
        tokio::time::sleep(coalesce_window).await;
        let kinds = {
            let mut workers = lock_workers(&workers);
            let Some(worker) = workers.get_mut(&upstream) else {
                return;
            };
            let kinds = std::mem::take(&mut worker.pending);
            if kinds.is_empty() {
                workers.remove(&upstream);
                return;
            }
            kinds
        };
        pool.refresh_after_list_changed(&upstream, kinds).await;
        forward(&upstream, kinds);
    }
}

impl UpstreamPool {
    /// Run the refreshes one exact upstream needs after reporting the given
    /// `list_changed` kinds. Tool and resource re-lists run concurrently;
    /// prompts need no refresh because `prompts/list` is served live.
    ///
    /// A resource refresh that does not publish a snapshot is logged, not
    /// surfaced: the failed re-list removed the cached source, so what peers
    /// see did change and the caller forwards `list_changed` either way.
    pub async fn refresh_after_list_changed(&self, upstream: &str, kinds: ListChangedKinds) {
        let tools = async {
            if kinds.tools {
                self.refresh_tools_after_list_changed(upstream).await;
            }
        };
        let resources = async {
            if kinds.resources && !self.refresh_resources_after_list_changed(upstream).await {
                tracing::warn!(
                    action = "catalog.resources.list_changed",
                    upstream,
                    "resources/list_changed refresh did not publish a snapshot; forwarding list_changed with the upstream's rows withheld until the next successful listing"
                );
            }
        };
        tokio::join!(tools, resources);
    }
}
