//! Bounded pagination for upstream catalog listing RPCs.
//!
//! rmcp's `Peer::list_all_*` helpers follow `nextCursor` in an unbounded loop,
//! so a malicious or buggy upstream (e.g. one whose `nextCursor` points back
//! at itself) could stream pages into gateway memory for the entire listing
//! timeout window before the `MAX_UPSTREAM_*` item caps apply. These wrappers
//! fetch at most [`MAX_LIST_PAGES`] pages per upstream per listing pass and
//! stop early on a repeated cursor, truncating with a WARN and a process-wide
//! truncation counter instead of looping.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::RoleClient;
use rmcp::model::{PaginatedRequestParams, Prompt, Resource, ResourceTemplate};
use rmcp::service::{Peer, ServiceError};

/// Page budget per upstream per listing pass. Mirrors the `MAX_LIST_PAGES`
/// budget adopted for the skills extension (epic lab-cainq).
pub(super) const MAX_LIST_PAGES: usize = 16;

/// Process-wide count of truncated listing passes (page cap hit or cursor
/// loop detected). Included in every truncation WARN so operators can see a
/// misbehaving upstream accumulating truncations across refreshes.
static LIST_TRUNCATIONS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(super) fn list_truncation_count() -> u64 {
    LIST_TRUNCATIONS.load(Ordering::Relaxed)
}

/// Bounded replacement for `Peer::list_all_resources`.
pub(super) async fn list_resources_bounded(
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<Vec<Resource>, ServiceError> {
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
) -> Result<Vec<ResourceTemplate>, ServiceError> {
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

/// Bounded replacement for `Peer::list_all_prompts`.
pub(super) async fn list_prompts_bounded(
    peer: &Peer<RoleClient>,
    upstream_name: &str,
) -> Result<Vec<Prompt>, ServiceError> {
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
/// failing the whole merge.
async fn paginate_bounded<T, F, Fut>(
    upstream_name: &str,
    method: &'static str,
    mut fetch_page: F,
) -> Result<Vec<T>, ServiceError>
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
            return Ok(items);
        };
        // The cursor value itself is upstream-controlled and unbounded, so it
        // is tracked but never logged.
        if !seen_cursors.insert(next.clone()) {
            warn_truncated(upstream_name, method, "cursor_loop", page, items.len());
            return Ok(items);
        }
        cursor = Some(next);
    }
    warn_truncated(
        upstream_name,
        method,
        "page_cap",
        MAX_LIST_PAGES,
        items.len(),
    );
    Ok(items)
}

fn warn_truncated(
    upstream_name: &str,
    method: &'static str,
    reason: &'static str,
    pages: usize,
    items: usize,
) {
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
        let items = paginate_bounded(
            "finite",
            "resources/list",
            counting_pages(&calls, |page| (page < 3).then(|| format!("page-{page}"))),
        )
        .await
        .expect("finite pagination succeeds");

        assert_eq!(items, vec![1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn stops_on_a_self_referencing_cursor() {
        let before = list_truncation_count();
        let calls = Arc::new(AtomicUsize::new(0));
        let items = paginate_bounded(
            "looping",
            "resources/list",
            counting_pages(&calls, |_| Some("loop".to_string())),
        )
        .await
        .expect("looping pagination truncates instead of failing");

        // Page 1 introduces the cursor, page 2 repeats it — no third fetch.
        assert_eq!(items, vec![1, 2]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(list_truncation_count(), before + 1);
    }

    #[tokio::test]
    async fn stops_at_the_page_cap_when_cursors_never_repeat() {
        let before = list_truncation_count();
        let calls = Arc::new(AtomicUsize::new(0));
        let items = paginate_bounded(
            "endless",
            "prompts/list",
            counting_pages(&calls, |page| Some(format!("page-{page}"))),
        )
        .await
        .expect("endless pagination truncates instead of failing");

        assert_eq!(items.len(), MAX_LIST_PAGES);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_LIST_PAGES);
        assert_eq!(list_truncation_count(), before + 1);
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
}
