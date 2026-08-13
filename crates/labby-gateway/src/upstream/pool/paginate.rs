//! Bounded pagination for upstream catalog listing RPCs.
//!
//! rmcp's `Peer::list_all_*` helpers follow `nextCursor` in an unbounded loop,
//! so a malicious or buggy upstream (e.g. one whose `nextCursor` points back
//! at itself) could stream pages into gateway memory for the entire listing
//! timeout window — or indefinitely, on the untimed prompt/template passes —
//! before the `MAX_UPSTREAM_*` item caps apply. These wrappers fetch at most
//! [`MAX_LIST_PAGES`] pages per upstream per listing pass and stop early on a
//! repeated cursor, truncating with a WARN and a [`ListTruncation`] report
//! instead of looping. Callers that publish a catalog entry surface that
//! report — refresh passes via `UpstreamPool::record_listing_success_for`,
//! connect-time discovery via the entry's `tool_last_error` — so
//! `gateway.status` does not present a truncated catalog as complete.
//! Subject-scoped per-subject listings have no shared entry to annotate and
//! rely on the truncation WARN alone.
//!
//! The page cap bounds RPC *count*, not bytes: worst-case memory per listing
//! pass is still `MAX_LIST_PAGES` × the per-response byte cap, per upstream,
//! concurrently across upstreams. Wall clock is bounded per call site: the
//! catalog fan-outs use [`listing_catalog_timeout`], discovery/refresh paths
//! use their `DISCOVERY_TIMEOUT`-family budgets, and subject-scoped calls run
//! under `timed_capability_call` request budgets or [`with_listing_timeout`].

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rmcp::RoleClient;
use rmcp::model::{PaginatedRequestParams, Prompt, Resource, ResourceTemplate, Tool};
use rmcp::service::{Peer, ServiceError};

/// Page budget per upstream per listing pass. Mirrors the `MAX_LIST_PAGES`
/// budget adopted for the skills extension (epic lab-cainq).
pub(super) const MAX_LIST_PAGES: usize = 16;

/// Wall-clock cap for one upstream catalog listing pass (resources, resource
/// templates, or prompts). Catalog fan-outs wrap each per-upstream bounded
/// listing in `min(request_timeout, LISTING_CATALOG_TIMEOUT)` so one stalled
/// upstream cannot hold the merged listing open. The subject-scoped resource
/// pass also reuses this budget for its connection acquisition.
pub(super) const LISTING_CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn listing_catalog_timeout(request_timeout: Duration) -> Duration {
    request_timeout.min(LISTING_CATALOG_TIMEOUT)
}

/// Wrap one bounded listing pass in its wall-clock budget, mapping expiry to
/// the same `ServiceError::Timeout` the caller's error arm already classifies.
pub(super) async fn with_listing_timeout<T>(
    timeout: Duration,
    listing: impl Future<Output = Result<T, ServiceError>>,
) -> Result<T, ServiceError> {
    tokio::time::timeout(timeout, listing)
        .await
        .unwrap_or_else(|_| Err(ServiceError::Timeout { timeout }))
}

/// Process-wide count of truncated listing passes (page cap hit or cursor
/// loop detected), across all upstreams and list methods. Included in every
/// truncation WARN so accumulating truncations are visible across refreshes;
/// per-upstream attribution comes from the WARN's `upstream` field.
static LIST_TRUNCATIONS: AtomicU64 = AtomicU64::new(0);

/// Why and where a bounded listing pass stopped early. Returned alongside the
/// partial items so callers can surface the truncation in the status channel
/// instead of recording the pass as an ordinary full success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListTruncation {
    method: &'static str,
    reason: &'static str,
    pages: usize,
}

impl ListTruncation {
    #[cfg(test)]
    pub(super) fn for_tests(method: &'static str, reason: &'static str, pages: usize) -> Self {
        Self {
            method,
            reason,
            pages,
        }
    }

    /// Operator-facing note for the capability's `last_error` status field.
    ///
    /// Must not match the `is_nonessential_capability_error` prefixes in
    /// `gateway/projection.rs` (mirrored by the doctor finding filter in
    /// `crates/labby/src/dispatch/doctor/gateway.rs`), or `gateway.status`
    /// and doctor would filter it out.
    pub(crate) fn status_note(&self) -> String {
        format!(
            "{} truncated ({}) after {} pages — upstream catalog is partial",
            self.method, self.reason, self.pages
        )
    }
}

/// Bounded replacement for `Peer::list_all_resources`.
pub(super) async fn list_resources_bounded(
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<(Vec<Resource>, Option<ListTruncation>), ServiceError> {
    paginate_bounded(upstream_name, "resources/list", |cursor| async move {
        let result = peer
            .list_resources(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await?;
        Ok((result.resources, result.next_cursor))
    })
    .await
}

/// Bounded replacement for `Peer::list_all_resource_templates`.
pub(super) async fn list_resource_templates_bounded(
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<(Vec<ResourceTemplate>, Option<ListTruncation>), ServiceError> {
    paginate_bounded(
        upstream_name,
        "resources/templates/list",
        |cursor| async move {
            let result = peer
                .list_resource_templates(Some(
                    PaginatedRequestParams::default().with_cursor(cursor),
                ))
                .await?;
            Ok((result.resource_templates, result.next_cursor))
        },
    )
    .await
}

/// Bounded replacement for `Peer::list_all_tools`.
pub(super) async fn list_tools_bounded(
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<(Vec<Tool>, Option<ListTruncation>), ServiceError> {
    paginate_bounded(upstream_name, "tools/list", |cursor| async move {
        let result = peer
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await?;
        Ok((result.tools, result.next_cursor))
    })
    .await
}

/// Bounded replacement for `Peer::list_all_prompts`.
pub(super) async fn list_prompts_bounded(
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<(Vec<Prompt>, Option<ListTruncation>), ServiceError> {
    paginate_bounded(upstream_name, "prompts/list", |cursor| async move {
        let result = peer
            .list_prompts(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await?;
        Ok((result.prompts, result.next_cursor))
    })
    .await
}

/// Drive `fetch_page` until the upstream stops returning a `next_cursor`, a
/// cursor repeats (self-referencing or cyclic pagination), or the page budget
/// is spent — whichever comes first. Truncation keeps the items already
/// fetched: catalog listings deliberately degrade to partial data rather than
/// failing the whole merge, and the returned [`ListTruncation`] tells the
/// caller the partial state so it can be surfaced in status.
async fn paginate_bounded<T, F, Fut>(
    upstream_name: &str,
    method: &'static str,
    mut fetch_page: F,
) -> Result<(Vec<T>, Option<ListTruncation>), ServiceError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), ServiceError>>,
{
    let mut items = Vec::new();
    let mut seen_cursors: HashSet<String> = HashSet::new();
    let mut cursor: Option<String> = None;
    for page in 1..=MAX_LIST_PAGES {
        let (page_items, next_cursor) = fetch_page(cursor.take()).await?;
        items.extend(page_items);
        let Some(next) = next_cursor else {
            return Ok((items, None));
        };
        // The cursor value itself is upstream-controlled and unbounded, so it
        // is tracked but never logged.
        if !seen_cursors.insert(next.clone()) {
            let truncation =
                warn_truncated(upstream_name, method, "cursor_loop", page, items.len());
            return Ok((items, Some(truncation)));
        }
        cursor = Some(next);
    }
    let truncation = warn_truncated(
        upstream_name,
        method,
        "page_cap",
        MAX_LIST_PAGES,
        items.len(),
    );
    Ok((items, Some(truncation)))
}

fn warn_truncated(
    upstream_name: &str,
    method: &'static str,
    reason: &'static str,
    pages: usize,
    items: usize,
) -> ListTruncation {
    let truncations_total = LIST_TRUNCATIONS.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::warn!(
        upstream = %upstream_name,
        method,
        reason,
        pages,
        items,
        page_cap = MAX_LIST_PAGES,
        truncations_total,
        "upstream listing pagination truncated"
    );
    ListTruncation {
        method,
        reason,
        pages,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use rmcp::model::ErrorData;

    use super::*;

    fn counting_pages(
        calls: &Arc<AtomicUsize>,
        next_cursor_for: fn(usize) -> Option<String>,
    ) -> impl FnMut(
        Option<String>,
    ) -> std::future::Ready<Result<(Vec<usize>, Option<String>), ServiceError>> {
        let calls = Arc::clone(calls);
        move |_cursor| {
            let page = calls.fetch_add(1, Ordering::SeqCst) + 1;
            std::future::ready(Ok((vec![page], next_cursor_for(page))))
        }
    }

    #[tokio::test]
    async fn stops_when_the_upstream_returns_no_cursor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (items, truncation) = paginate_bounded(
            "finite",
            "resources/list",
            counting_pages(&calls, |page| (page < 3).then(|| format!("page-{page}"))),
        )
        .await
        .expect("finite pagination succeeds");

        assert_eq!(items, vec![1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(truncation, None);
    }

    // The process-wide `LIST_TRUNCATIONS` total is deliberately not asserted
    // in these tests: it is shared across every test in the process, so exact
    // deltas race under plain `cargo test` (in-process parallelism). The
    // returned `ListTruncation` is produced only by `warn_truncated`, so
    // asserting it pins the same path.
    #[tokio::test]
    async fn stops_on_a_self_referencing_cursor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (items, truncation) = paginate_bounded(
            "looping",
            "resources/list",
            counting_pages(&calls, |_| Some("loop".to_string())),
        )
        .await
        .expect("looping pagination truncates instead of failing");

        // Page 1 introduces the cursor, page 2 repeats it — no third fetch.
        assert_eq!(items, vec![1, 2]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let note = truncation
            .expect("cursor loop reports truncation")
            .status_note();
        assert_eq!(
            note,
            "resources/list truncated (cursor_loop) after 2 pages — upstream catalog is partial"
        );
    }

    #[tokio::test]
    async fn stops_at_the_page_cap_when_cursors_never_repeat() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (items, truncation) = paginate_bounded(
            "endless",
            "prompts/list",
            counting_pages(&calls, |page| Some(format!("page-{page}"))),
        )
        .await
        .expect("endless pagination truncates instead of failing");

        assert_eq!(items.len(), MAX_LIST_PAGES);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_LIST_PAGES);
        let note = truncation
            .expect("page cap reports truncation")
            .status_note();
        assert_eq!(
            note,
            "prompts/list truncated (page_cap) after 16 pages — upstream catalog is partial"
        );
    }

    #[tokio::test]
    async fn listing_that_ends_exactly_at_the_page_cap_is_not_truncated() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (items, truncation) = paginate_bounded(
            "exact",
            "resources/list",
            counting_pages(&calls, |page| {
                (page < MAX_LIST_PAGES).then(|| format!("page-{page}"))
            }),
        )
        .await
        .expect("a listing spanning exactly the page budget succeeds");

        assert_eq!(items.len(), MAX_LIST_PAGES);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_LIST_PAGES);
        assert_eq!(truncation, None);
    }

    #[test]
    fn listing_catalog_timeout_caps_the_general_upstream_budget() {
        assert_eq!(
            listing_catalog_timeout(Duration::from_mins(1)),
            Duration::from_secs(10)
        );
        assert_eq!(
            listing_catalog_timeout(Duration::from_millis(25)),
            Duration::from_millis(25)
        );
    }

    #[tokio::test]
    async fn propagates_page_fetch_errors() {
        let error = paginate_bounded("broken", "prompts/list", |_cursor| {
            std::future::ready(Err::<(Vec<usize>, Option<String>), _>(
                ServiceError::McpError(ErrorData::internal_error("listing failed", None)),
            ))
        })
        .await
        .expect_err("page errors propagate");

        assert!(matches!(error, ServiceError::McpError(_)));
    }

    #[tokio::test]
    async fn mid_pagination_errors_discard_accumulated_pages() {
        // Deliberate all-or-error semantics per upstream (matches rmcp's
        // `list_all_*`): a page-2 failure fails the whole listing so the
        // caller's error arm records the failure — partial-data degradation
        // applies only to truncation, not to transport/protocol errors.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = Arc::clone(&calls);
        let error = paginate_bounded("flaky", "resources/list", move |_cursor| {
            let page = calls_in.fetch_add(1, Ordering::SeqCst) + 1;
            std::future::ready(if page == 1 {
                Ok((vec![page], Some("page-2".to_string())))
            } else {
                Err(ServiceError::McpError(ErrorData::internal_error(
                    "second page failed",
                    None,
                )))
            })
        })
        .await
        .expect_err("a mid-pagination error fails the listing");

        assert!(matches!(error, ServiceError::McpError(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
