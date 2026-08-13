//! Shared vocabulary for catalog-change notification sources.
//!
//! `notifications/tools/list_changed` is emitted from several independent
//! sites — gateway reconcile (two paths), enrichment hint apply, and the
//! per-call MCP snapshot diffs — that all funnel into one peer fanout. Without
//! attribution the fanout log cannot answer the only question that matters
//! when clients report tool "flapping": *which* site keeps invalidating the
//! catalog.
//!
//! These labels are the stable attribution vocabulary. They live here rather
//! than in either surface because the emitting sites are split across
//! `labby-gateway` (reconcile, enrichment) and `labby` (per-call MCP paths),
//! and one vocabulary is the point.
//!
//! Treat them like log field names, not an error taxonomy: stable enough to
//! build an alert on, and adding or renaming one is a `docs/dev/OBSERVABILITY.md`
//! change in the same commit.

/// Gateway reconcile that kept the live pool and selectively reconciled
/// newly added upstreams.
pub const SOURCE_GATEWAY_RELOAD_SELECTIVE: &str = "gateway.reload.selective";

/// Gateway reconcile that rebuilt the upstream pool.
pub const SOURCE_GATEWAY_RELOAD_FULL: &str = "gateway.reload.full";

/// `gateway.code_mode.set` changed the visible Code Mode contract without an
/// upstream-pool reconcile, such as toggling the explicit MCP App UI.
pub const SOURCE_GATEWAY_CODE_MODE_SET: &str = "gateway.code_mode.set";

/// `gateway.enrich.hint.apply` writing a `code_mode_hint`, which is rendered
/// into the visible `codemode` tool description.
pub const SOURCE_GATEWAY_ENRICH_HINT: &str = "gateway.enrich.hint_apply";

/// Post-run catalog delta observed by a `codemode` tool call. Emitted while
/// the caller's turn is still open, so this is the source most likely to
/// invalidate a client binding mid-turn.
pub const SOURCE_MCP_CALL_CODEMODE: &str = "mcp.call.codemode";

/// Catalog delta produced by the text-only `mcp_app` control tool.
pub const SOURCE_MCP_CALL_MCP_APP: &str = "mcp.call.mcp_app";

/// Post-call catalog delta observed by a raw upstream proxy call. Same
/// mid-turn caveat as [`SOURCE_MCP_CALL_CODEMODE`].
pub const SOURCE_MCP_CALL_UPSTREAM: &str = "mcp.call.upstream";

/// A live upstream subscription reported a scoped catalog change.
pub const SOURCE_UPSTREAM_SUBSCRIPTION: &str = "upstream.subscription";

/// The upstream event receiver lagged and reconciled all authoritative
/// catalogs before conservatively signalling downstream peers.
pub const SOURCE_UPSTREAM_NOTIFICATION_LAG: &str = "upstream.notification_lag";

/// Several emitters converged on one net visible change and were delivered as
/// a single notification. The contributing emitters are listed on the
/// `catalog.notify.flush` event.
pub const SOURCE_COALESCED: &str = "coalesced";

/// Fallback for a notification that reached the fanout without attribution.
/// Seeing this in logs means a new emitter was added without a source label.
pub const SOURCE_UNKNOWN: &str = "unknown";

/// Every known source label. Ordered as declared; used by tests and by
/// operator docs that enumerate what can appear in the `source` field.
pub const SOURCES: &[&str] = &[
    SOURCE_GATEWAY_RELOAD_SELECTIVE,
    SOURCE_GATEWAY_RELOAD_FULL,
    SOURCE_GATEWAY_CODE_MODE_SET,
    SOURCE_GATEWAY_ENRICH_HINT,
    SOURCE_MCP_CALL_CODEMODE,
    SOURCE_MCP_CALL_MCP_APP,
    SOURCE_MCP_CALL_UPSTREAM,
    SOURCE_UPSTREAM_SUBSCRIPTION,
    SOURCE_UPSTREAM_NOTIFICATION_LAG,
    SOURCE_COALESCED,
    SOURCE_UNKNOWN,
];

#[cfg(test)]
mod tests {
    use super::{SOURCE_UNKNOWN, SOURCES};

    #[test]
    fn source_labels_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for source in SOURCES {
            assert!(seen.insert(*source), "duplicate source label `{source}`");
        }
        assert_eq!(seen.len(), SOURCES.len());
    }

    #[test]
    fn source_labels_are_dotted_lowercase() {
        // Log field values operators filter on; keep them shell/grep friendly
        // and consistent with the `action` naming convention.
        for source in SOURCES {
            assert!(
                source
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
                "source label `{source}` must be lowercase dotted/underscored ascii"
            );
        }
        assert!(!SOURCE_UNKNOWN.contains('.'));
    }
}
