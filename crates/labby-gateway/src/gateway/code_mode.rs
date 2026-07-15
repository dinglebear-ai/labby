//! Gateway adapter over the extracted `labby-codemode` crate.
//!
//! The Code Mode JS execution kernel, broker, result-shaping helpers, and
//! snippet engine now live in `labby-codemode`. This module is the gateway's
//! thin adapter: it re-exports the crate's public surface under
//! `crate::gateway::code_mode::*` import paths, owns the host-side render
//! caches, and hosts `impl CodeModeHost for GatewayManager`
//! (`code_mode_host.rs`) plus the upstream→`ToolDescriptor` catalog projection
//! (`search.rs`) and the one-shot CLI catalog cache (`catalog_cache.rs`).

pub(crate) mod catalog_cache;
pub(crate) mod code_mode_host;
pub(crate) mod embeddings;
mod search;

// ── Re-exports of the crate's neutral public surface ────────────────────────
//
pub use labby_codemode::run_code_mode_runner_stdio;
pub use labby_codemode::{
    CodeModeBroker, CodeModeCaller, CodeModeCallerCapabilities, CodeModeHistory,
    CodeModeHistoryEntry, CodeModeHistoryKind, CodeModeSourceLookup, CodeModeSourceStore,
    CodeModeSurface, RunnerPool, code_mode_execute_trace, validate_code_mode_params_against_schema,
};
#[cfg(any(test, feature = "testkit"))]
pub use labby_codemode::{CodeModeExecutedCall, CodeModeExecutionResponse};
pub use labby_codemode::{CodeModeExecutionSource, ToolDescriptor, ToolScope};

pub(crate) use labby_codemode::split_namespaced_id;

pub use code_mode_host::JournalOwner;

// ── Host-side render caches (gateway-owned, keyed on the live tool set) ──────

/// Cached rendered Code Mode discovery catalog.
///
/// Keyed by a fingerprint string (sorted `upstream::tool` ids joined with `\n`
/// plus the snippet fingerprint). When the pool's healthy tool set has not
/// changed between lookups, this avoids re-running `generate_tool_types` and
/// re-serializing the catalog JSON. It does NOT avoid re-generating the
/// discovery/proxy JS strings themselves (`generate_discovery_js` /
/// `generate_js_proxy_from_catalog`) — those are rebuilt from `entries` on
/// every `codemode` execution regardless of cache hit/miss (see
/// `crates/labby-codemode/src/execute.rs`'s `build_code_mode_proxy`).
///
/// Bead `lab-5cgrz` investigated converting `search`/`describe` to host-RPC
/// and rejected it at the time: the injection cost was negligible at then-current
/// scale, the evaluated approach (the `local_provider.rs`/`LOCAL_PROVIDER_LOCK`
/// pattern) would have serialized repeated calls behind the global local-provider
/// lock, and reusing `local_providers_allowed()` verbatim would have wrongly
/// restricted `search`/`describe` to admin-only. `describe()` alone (not
/// `search()`) was later revisited and converted to host-RPC once catalog sizes
/// grew large enough for the injection cost to matter (benchmarked: ~3x smaller
/// injected payload, ~1.8x faster parse at 4,000 tools) — using the
/// `__lab_internal::*` reserved-namespace `tool_call` mechanism `semantic_rank`
/// already used instead of the rejected `local_provider.rs` pattern, which
/// avoids both structural objections above by construction. `entries`/
/// `catalog_json` being `Arc`-wrapped (not `Vec`/`String`) is a direct
/// consequence: `describe()` now calls `list_tools()` per invocation instead of
/// once per execution, and cloning the whole catalog per call would otherwise
/// make `describe()`-heavy scripts slower than before that change at large
/// catalog sizes.
///
/// This cache is a single slot (`Mutex<Option<CatalogRenderCache>>` on
/// `GatewayManager`) with NO caller/scope component in its fingerprint —
/// **and every caller reaches it**, not only the unscoped CLI path.
/// `catalog_from_tools` (`code_mode/search.rs`) reads/writes this cache
/// unconditionally; `use_cache` only selects where `raw_tools` is sourced
/// from (`code_mode_catalog_tools_cached` vs `code_mode_catalog_tools_allowed`),
/// it does not gate access to this cache. (An earlier version of this comment
/// claimed the cache was reached exclusively through the unscoped CLI path —
/// that was never true; corrected once the `describe_types` internal call
/// below made the cache's actual reach relevant to trace precisely.)
///
/// Sharing this single slot across every caller/scope is safe because the
/// fingerprint is derived from the *content* of `raw_tools` (sorted
/// `upstream::tool::shape_digest` triples), which itself already reflects
/// whatever namespace filtering (`allowed_upstreams`, i.e.
/// `scope.allowed_namespaces()`) produced that particular `raw_tools`. Two
/// callers with different namespace grants almost always produce different
/// `raw_tools` and thus different fingerprints — so in practice they don't
/// share a cache hit at all; they just take turns evicting the single slot
/// (a perf/thrash concern, not a leak). A genuine cache hit means the
/// requesting caller's namespace-filtered tool set is byte-identical to what's
/// cached, so serving the cached render is content-equivalent to rebuilding it.
///
/// What this fingerprint-content argument does **not** cover: the
/// namespace-level filter (`allowed_upstreams`) is coarser than `ToolScope`'s
/// full grant — it says nothing about the finer-grained per-tool `scope.tools`
/// restriction (`ToolScope::allows`). `CatalogRenderCache.entries`/`ToolsRender.entries`
/// can therefore always contain tools a caller's `scope.tools` should exclude,
/// even on a correctly-scoped cache hit. The cache is not the security
/// boundary — `discovery_entry_visible(entry, scope)` is. Every consumer of
/// `.entries` (`build_code_mode_proxy`, `semantic_rank`'s host impl in
/// `code_mode_host.rs`, and the `describe_types` internal call in
/// `labby-codemode`'s `execute.rs`) MUST apply that filter itself before using
/// an entry; nothing upstream of them enforces it. A new consumer that skips
/// it is a real information-disclosure bug, not a style issue (this is
/// exactly what `describe_types` shipped with initially and had to be fixed).
pub(crate) struct CatalogRenderCache {
    /// Fingerprint of the healthy tool list when this cache was built.
    pub fingerprint: String,
    /// Rendered catalog entries (includes `.signature` / `.dts`). A boxed
    /// slice behind an `Arc`, not `Arc<Vec<_>>` — mirrors `catalog_json`
    /// below and `ToolsRender.entries` in `labby-codemode`: nothing ever
    /// mutates this in place, so there's no reason to carry `Vec`'s
    /// spare-capacity bookkeeping. `Arc`-wrapped so a cache hit is a refcount
    /// bump, not a deep clone — `codemode.describe()` now calls `list_tools()`
    /// again per invocation (see `labby-codemode`'s `execute.rs`
    /// `describe_types` dispatch), so this is read far more than once per
    /// execution.
    pub entries: std::sync::Arc<[ToolDescriptor]>,
    /// `serde_json::to_string(&entries)` — the `const tools = ...` payload.
    /// Same `Arc` rationale as `entries`.
    pub catalog_json: std::sync::Arc<str>,
    /// Serialized catalog size in bytes (for the tracing log).
    pub serialized_size: usize,
}

/// Cached snippet metadata for Code Mode discovery.
///
/// Keyed by cheap directory metadata plus the caller visibility policy. Stores
/// metadata only; executable snippet source is resolved lazily per execution
/// when `codemode.run()` asks the host for it.
pub(crate) struct SnippetMetadataCache {
    pub fingerprint: String,
    pub entries: Vec<labby_codemode::snippet::store::SnippetInfo>,
}

/// Cached catalog embedding vectors, keyed by the same fingerprint used for
/// `CatalogRenderCache` (see `search.rs`'s `catalog_from_tools`). One vector
/// per catalog entry id, computed via one or more batched TEI calls.
pub(crate) struct CatalogEmbeddingCache {
    pub fingerprint: String,
    /// `(entry.id, embedding_vector)` pairs. Callers should look up by id,
    /// not by index.
    pub vectors: Vec<(String, Vec<f32>)>,
}
