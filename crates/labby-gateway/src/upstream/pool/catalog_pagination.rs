//! Bounded cursor pagination for upstream MCP catalog methods.
//!
//! rmcp's `list_all_*` conveniences have no cursor-cycle, page, byte, item, or
//! total-deadline guards. Request paths must use these helpers so an upstream
//! cannot make the gateway materialize an unbounded catalog before the public
//! catalog cap is applied.

use std::collections::HashSet;
use std::future::Future;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rmcp::RoleClient;
use rmcp::model::{PaginatedRequestParams, Prompt, Resource, ResourceTemplate, Tool};
use rmcp::service::{Peer, ServiceError};
use serde::Serialize;
use thiserror::Error;
use tokio::time::Instant;

use super::capability_call::bounded_service_error_text;
use super::helpers::classify_upstream_error;
use super::helpers::max_response_bytes;
use super::logging::is_capability_unsupported;

const MAX_CATALOG_PAGES: usize = 64;
const MAX_ITEMS_PER_PAGE: usize = 1_000;
const MAX_CURSOR_BYTES: usize = 8 * 1024;

#[derive(Debug, Error)]
pub(super) enum CatalogPaginationError {
    #[error("upstream catalog request failed: {0}")]
    Service(#[from] ServiceError),
    #[error("upstream catalog pagination timed out after {deadline_ms}ms")]
    Deadline { deadline_ms: u128 },
    #[error("upstream catalog repeated a pagination cursor")]
    RepeatedCursor,
    #[error("upstream catalog exceeded the {limit}-page pagination limit")]
    PageLimit { limit: usize },
    #[error("upstream catalog page exceeded the {limit}-item limit")]
    PageItemLimit { limit: usize },
    #[error("upstream catalogs exceeded the shared {limit}-item limit")]
    ItemLimit { limit: usize },
    #[error("upstream catalog cursor exceeded the {limit}-byte limit")]
    CursorLimit { limit: usize },
    #[error("upstream catalog exceeded the {limit}-byte serialized limit")]
    ByteLimit { limit: usize },
}

impl CatalogPaginationError {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Service(error) => classify_upstream_error(&bounded_service_error_text(error)),
            Self::Deadline { .. } => "timeout",
            Self::RepeatedCursor => "pagination_repeated_cursor",
            Self::PageLimit { .. } => "pagination_page_limit",
            Self::PageItemLimit { .. } => "pagination_page_item_limit",
            Self::ItemLimit { .. } => "pagination_item_limit",
            Self::CursorLimit { .. } => "pagination_cursor_limit",
            Self::ByteLimit { .. } => "response_too_large",
        }
    }

    pub(super) fn bounded_text(&self) -> String {
        match self {
            Self::Service(error) => bounded_service_error_text(error),
            _ => self.to_string(),
        }
    }

    /// Adapt pagination policy failures to the existing capability-call
    /// skeleton without bypassing its bulkhead, health, usage, or subject-peer
    /// eviction behavior. The original structured kind is logged at this
    /// boundary. A pagination-policy violation is a protocol failure, so the
    /// capability layer records it against the upstream circuit breaker.
    pub(super) fn into_service_error(self, upstream: &str) -> ServiceError {
        if let Self::Service(error) = self {
            return error;
        }
        tracing::warn!(
            upstream,
            kind = self.kind(),
            error = %self,
            "bounded upstream catalog pagination stopped"
        );
        ServiceError::UnexpectedResponse
    }
}

pub(super) struct SharedCatalogBudget {
    item_limit: usize,
    byte_limit: usize,
    items: AtomicUsize,
    bytes: AtomicUsize,
}

impl SharedCatalogBudget {
    pub(super) fn new(item_limit: usize, byte_limit: usize) -> Self {
        Self {
            item_limit,
            byte_limit,
            items: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
        }
    }

    fn reserve(&self, bytes: usize) -> Result<(), CatalogPaginationError> {
        self.items
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < self.item_limit).then_some(current + 1)
            })
            .map_err(|_| CatalogPaginationError::ItemLimit {
                limit: self.item_limit,
            })?;
        if self
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|next| *next <= self.byte_limit)
            })
            .is_err()
        {
            self.items.fetch_sub(1, Ordering::AcqRel);
            return Err(CatalogPaginationError::ByteLimit {
                limit: self.byte_limit,
            });
        }
        Ok(())
    }
}

struct ByteCounter(usize);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialized_len<T: Serialize>(value: &T) -> usize {
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value).map_or(usize::MAX, |()| counter.0)
}

async fn collect_bounded<T, F, Fut>(
    deadline: Duration,
    item_limit: usize,
    fetch: F,
) -> Result<Vec<T>, CatalogPaginationError>
where
    T: Serialize,
    F: FnMut(Option<PaginatedRequestParams>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), ServiceError>>,
{
    collect_bounded_with_budget(deadline, item_limit, None, fetch).await
}

async fn collect_bounded_with_budget<T, F, Fut>(
    deadline: Duration,
    item_limit: usize,
    shared_budget: Option<&SharedCatalogBudget>,
    mut fetch: F,
) -> Result<Vec<T>, CatalogPaginationError>
where
    T: Serialize,
    F: FnMut(Option<PaginatedRequestParams>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), ServiceError>>,
{
    let started = Instant::now();
    let deadline_at = started + deadline;
    let byte_limit = max_response_bytes();
    let mut items = Vec::with_capacity(item_limit.min(128));
    let mut total_bytes = 0usize;
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();

    for page_index in 0..MAX_CATALOG_PAGES {
        let params = Some(PaginatedRequestParams::default().with_cursor(cursor.clone()));
        let (page, next_cursor) = tokio::time::timeout_at(deadline_at, fetch(params))
            .await
            .map_err(|_| CatalogPaginationError::Deadline {
                deadline_ms: deadline.as_millis(),
            })??;

        if page.len() > MAX_ITEMS_PER_PAGE {
            return Err(CatalogPaginationError::PageItemLimit {
                limit: MAX_ITEMS_PER_PAGE,
            });
        }
        for item in page {
            if items.len() >= item_limit {
                tracing::warn!(
                    item_limit,
                    page_count = page_index + 1,
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream catalog pagination stopped at item limit"
                );
                return Ok(items);
            }
            let item_bytes = serialized_len(&item);
            total_bytes = total_bytes.saturating_add(item_bytes);
            if total_bytes > byte_limit {
                return Err(CatalogPaginationError::ByteLimit { limit: byte_limit });
            }
            if let Some(shared_budget) = shared_budget {
                shared_budget.reserve(item_bytes)?;
            }
            items.push(item);
        }

        let Some(next) = next_cursor else {
            return Ok(items);
        };
        if next.len() > MAX_CURSOR_BYTES {
            return Err(CatalogPaginationError::CursorLimit {
                limit: MAX_CURSOR_BYTES,
            });
        }
        total_bytes = total_bytes.saturating_add(next.len());
        if total_bytes > byte_limit {
            return Err(CatalogPaginationError::ByteLimit { limit: byte_limit });
        }
        if !seen_cursors.insert(next.clone()) {
            return Err(CatalogPaginationError::RepeatedCursor);
        }
        cursor = Some(next);
    }

    Err(CatalogPaginationError::PageLimit {
        limit: MAX_CATALOG_PAGES,
    })
}

/// `tools/list`, tolerating an upstream that implements no tools at all.
///
/// A server advertises its capabilities during initialize, and a client is only
/// supposed to call what was advertised. A server that exposes only resources —
/// or only skills, via the `io.modelcontextprotocol/skills` extension — answers
/// `tools/list` with `-32601 Method not found` and is behaving correctly.
///
/// Treating that as an error made such an upstream permanently unusable rather
/// than merely tool-less: `connect.rs` and `connect_stdio.rs` turn it into a
/// failed connection, and `probe.rs` marks the heartbeat unhealthy, so the
/// upstream never joins the catalog however healthy it actually is. Discovered
/// against a skills-over-MCP server that serves `skills/list` and
/// `resources/list` and declares no `tools` capability.
///
/// `capability.rs` has always applied this same tolerance to `resources` and
/// `prompts`; tools were the outlier. It belongs here rather than at each call
/// site because "no tools capability" and "an empty tool list" are the same
/// thing to every caller — the catalog gets no tools from this upstream either
/// way — and spreading the check invites the next call site to forget it.
pub(super) async fn list_tools(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
) -> Result<Vec<Tool>, CatalogPaginationError> {
    let result = collect_bounded(deadline, item_limit, |params| async move {
        let result = peer.list_tools(params).await?;
        Ok((result.tools, result.next_cursor))
    })
    .await;

    tools_or_empty_when_unsupported(result)
}

/// The tolerance described on `list_tools`, split out so it can be tested
/// without standing up a peer.
///
/// Narrow on purpose: only an unsupported-capability reply becomes an empty
/// catalog. A timeout, a pagination-bound breach, or any other transport or
/// protocol failure stays an error — silently reporting "no tools" for an
/// upstream that is actually broken would hide the outage behind a healthy
/// upstream serving nothing.
fn tools_or_empty_when_unsupported(
    result: Result<Vec<Tool>, CatalogPaginationError>,
) -> Result<Vec<Tool>, CatalogPaginationError> {
    match result {
        Err(CatalogPaginationError::Service(ref error)) if is_capability_unsupported(error) => {
            Ok(Vec::new())
        }
        other => other,
    }
}

pub(super) async fn list_prompts(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
) -> Result<Vec<Prompt>, CatalogPaginationError> {
    collect_bounded(deadline, item_limit, |params| async move {
        let result = peer.list_prompts(params).await?;
        Ok((result.prompts, result.next_cursor))
    })
    .await
}

pub(super) async fn list_prompts_with_budget(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
    budget: &SharedCatalogBudget,
) -> Result<Vec<Prompt>, CatalogPaginationError> {
    collect_bounded_with_budget(deadline, item_limit, Some(budget), |params| async move {
        let result = peer.list_prompts(params).await?;
        Ok((result.prompts, result.next_cursor))
    })
    .await
}

pub(super) async fn list_resources(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
) -> Result<Vec<Resource>, CatalogPaginationError> {
    collect_bounded(deadline, item_limit, |params| async move {
        let result = peer.list_resources(params).await?;
        Ok((result.resources, result.next_cursor))
    })
    .await
}

pub(super) async fn list_resources_with_budget(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
    budget: &SharedCatalogBudget,
) -> Result<Vec<Resource>, CatalogPaginationError> {
    collect_bounded_with_budget(deadline, item_limit, Some(budget), |params| async move {
        let result = peer.list_resources(params).await?;
        Ok((result.resources, result.next_cursor))
    })
    .await
}

pub(super) async fn list_resource_templates(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
) -> Result<Vec<ResourceTemplate>, CatalogPaginationError> {
    collect_bounded(deadline, item_limit, |params| async move {
        let result = peer.list_resource_templates(params).await?;
        Ok((result.resource_templates, result.next_cursor))
    })
    .await
}

pub(super) async fn list_resource_templates_with_budget(
    peer: &Peer<RoleClient>,
    deadline: Duration,
    item_limit: usize,
    budget: &SharedCatalogBudget,
) -> Result<Vec<ResourceTemplate>, CatalogPaginationError> {
    collect_bounded_with_budget(deadline, item_limit, Some(budget), |params| async move {
        let result = peer.list_resource_templates(params).await?;
        Ok((result.resource_templates, result.next_cursor))
    })
    .await
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::model::Tool;

    use super::*;

    #[tokio::test]
    async fn repeated_cursor_is_rejected_after_two_bounded_requests() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let result = collect_bounded(Duration::from_secs(1), 10, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, ServiceError>((Vec::<Tool>::new(), Some("again".to_string()))) }
        })
        .await;

        assert!(matches!(
            result,
            Err(CatalogPaginationError::RepeatedCursor)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn item_limit_stops_fetching_before_an_endless_catalog_materializes() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let result = collect_bounded(Duration::from_secs(1), 3, move |_| {
            let page = observed.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok::<_, ServiceError>((
                    vec![
                        Tool::new(
                            format!("tool-{page}-a"),
                            "",
                            Arc::new(serde_json::Map::new()),
                        ),
                        Tool::new(
                            format!("tool-{page}-b"),
                            "",
                            Arc::new(serde_json::Map::new()),
                        ),
                    ],
                    Some(format!("cursor-{page}")),
                ))
            }
        })
        .await
        .expect("bounded catalog");

        assert_eq!(result.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn shared_budget_rejects_before_cross_upstream_item_materialization() {
        let tool = |name: &str| Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()));
        let budget = SharedCatalogBudget::new(3, usize::MAX);
        let first =
            collect_bounded_with_budget(Duration::from_secs(1), 10, Some(&budget), |_| async {
                Ok::<_, ServiceError>((vec![tool("a"), tool("b")], None))
            })
            .await
            .unwrap();
        assert_eq!(first.len(), 2);

        let second =
            collect_bounded_with_budget(Duration::from_secs(1), 10, Some(&budget), |_| async {
                Ok::<_, ServiceError>((vec![tool("c"), tool("must-not-materialize")], None))
            })
            .await;

        assert!(matches!(
            second,
            Err(CatalogPaginationError::ItemLimit { limit: 3 })
        ));
    }

    #[tokio::test]
    async fn total_deadline_cancels_a_stalled_page() {
        let result = collect_bounded::<Tool, _, _>(Duration::from_millis(10), 10, |_| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<_, ServiceError>((Vec::new(), None))
        })
        .await;

        assert!(matches!(
            result,
            Err(CatalogPaginationError::Deadline { .. })
        ));
    }

    #[tokio::test]
    async fn oversized_cursor_is_rejected_before_it_is_retained() {
        let result = collect_bounded::<Tool, _, _>(Duration::from_secs(1), 10, |_| async {
            Ok::<_, ServiceError>((Vec::new(), Some("x".repeat(MAX_CURSOR_BYTES + 1))))
        })
        .await;

        assert!(matches!(
            result,
            Err(CatalogPaginationError::CursorLimit { .. })
        ));
    }

    #[test]
    fn an_upstream_without_a_tools_capability_yields_an_empty_catalog() {
        // A skills- or resources-only server answers `tools/list` with -32601.
        // That is a conformant answer to a method it never advertised, so it
        // must leave the upstream usable rather than failing its connection.
        let result = tools_or_empty_when_unsupported(Err(CatalogPaginationError::Service(
            ServiceError::McpError(rmcp::model::ErrorData::method_not_found::<
                rmcp::model::CallToolRequestMethod,
            >()),
        )));

        assert!(matches!(result, Ok(tools) if tools.is_empty()));
    }

    #[test]
    fn a_real_failure_is_not_disguised_as_an_empty_catalog() {
        // The dangerous shape of this fix: reporting "no tools" for an upstream
        // that is actually broken would present an outage as a healthy upstream
        // serving nothing.
        let result = tools_or_empty_when_unsupported(Err(CatalogPaginationError::Service(
            ServiceError::McpError(rmcp::model::ErrorData::internal_error(
                "upstream on fire",
                None,
            )),
        )));

        assert!(matches!(
            result,
            Err(CatalogPaginationError::Service(ServiceError::McpError(_)))
        ));
    }

    #[test]
    fn a_bounds_breach_is_still_an_error_not_an_empty_catalog() {
        let result = tools_or_empty_when_unsupported(Err(CatalogPaginationError::PageLimit {
            limit: MAX_CATALOG_PAGES,
        }));

        assert!(matches!(
            result,
            Err(CatalogPaginationError::PageLimit { .. })
        ));
    }

    #[test]
    fn a_successful_listing_passes_through_untouched() {
        let result =
            tools_or_empty_when_unsupported(Ok(vec![super::super::testsupport::test_tool("echo")]));

        assert!(matches!(result, Ok(tools) if tools.len() == 1));
    }

    #[test]
    fn service_error_text_is_bounded_and_keeps_classification() {
        let error = CatalogPaginationError::Service(ServiceError::McpError(
            rmcp::model::ErrorData::internal_error("x".repeat(1024 * 1024), None),
        ));

        assert_eq!(error.kind(), "connection_error");
        assert!(error.bounded_text().len() < 5_000);
    }
}
