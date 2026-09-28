# Upstream MCP runtime

This directory owns surface-neutral HTTP, Unix-socket, and stdio upstream connections. Product dispatch/upstream.rs is a compatibility shim. Never import product CLI/API/MCP handlers; use the injected InProcessConnector seam. Runner mechanics belong in labby-codemode, not this pool.

For implementation rationale and a source map, consult [Upstream Runtime Maintenance Notes](../../../../docs/dev/UPSTREAM_INTERNALS.md) on demand. Do not automatically import that document. Source files, not copied tables, own numerical limits and current symbols.

## Discovery and publication

- Use catalog_pagination.rs and the bounded listing helpers, never rmcp Peer::list_all_*. Preserve page/item/cursor/byte limits, repeated-cursor detection, and the original deadline. A bounds breach is an error, not an empty successful catalog.
- Preserve CatalogWriteGuard publication and independent generations for tools/resources/templates/prompts. Resource admission failure withholds the offending upstream, not the fleet; incarnation violations and whole-projection limits retain their stricter contract. Do not publish UI, synthetic, or subject-private rows as regular fleet capabilities.
- Coalesce list_changed per upstream and refresh before forwarding downstream notification. A blocked upstream must not delay unrelated refreshes. Resource listing remains cache-backed with bounded single-flight warm-up; do not make reads wait on subscription handshakes.
- Keep native ui:// resource identities intact. Preserve upstream tool annotations, typed MCP errors, and input_required rather than applying product-local normalization.

## Authorization and lifecycle

- Enforce expose_tools/resources/prompts on discovery AND direct calls, reads, gets, and completion. Catalog, OAuth-subject, and relay paths use the same compiled policy. Admin inspection snapshots may intentionally retain excluded rows; they are not execution authority.
- OAuth replacement, refresh, clear, and shared-provider revocation go through oauth_invalidation.rs. Close subject, relay, and retained-task peers before success. Fence asynchronous publication against the credential lifecycle epoch so late discovery cannot restore invalidated state. Never log raw subjects or credentials.
- Relay cache ownership includes upstream, session, subject, and capability fingerprint. Preserve generation coexistence for in-flight calls, downstream rebinding, bounded eviction, and task authorization snapshots. Schema caches and catalog freshness do not confer permission to execute.
- Stdio transport is the single child-exit observer. Classify lifecycle compatibility from protocol_error(), never diagnostic text containing stderr. A dead child or an ordinary "Method not found" log line is not evidence for downgrading and respawning.

## Cancellation, isolation, and recovery

- Every caller-attributed RPC uses the upstream bulkhead and downstream cancellation token. Fan-out discovery has separate concurrency/deadline limits and explicit partial-result diagnostics. Do not add bypass paths or duplicate cancellable APIs.
- Keep biased cancellation selection: an already-cancelled caller must not dispatch. Propagate pooled/relay cancellation and bound detached delivery/cleanup; dropping a local await alone does not stop remote execution. Cancellation is not a breaker failure; actual timeouts still are.
- HEADER_MISMATCH recovery is typed, pre-dispatch, exact-peer, and single-shot under the original deadline. Refresh that peer's schema before replay. Keep relay recovery boxed to avoid nested-future stack growth. Do not retry arbitrary failures or synthesize parameter headers from arbitrary arguments.
- Unix-socket transport remains a thin policy wrapper around rmcp's client. RPC and cancellation must share framing, auth, header, SSE, and response-budget behavior. Preserve SSRF and spawn guards; the basename-only executable allowlist relies on trusted admin configuration, not executable identity verification.
- Keep environment reads in the existing helpers/connect boundaries. New pool files should stay below the existing 500-line target, including tests; legacy oversized modules are not a precedent.

## Verification

Run targeted regressions for changed behavior, then `cargo nextest run -p labby-gateway --all-features` and relevant product MCP/API tests. Especially preserve exposure parity, stale-epoch rejection, list-change independence, early-cancel non-dispatch, bounded relay cleanup, boxed header recovery, and stdio protocol-versus-stderr classification. Run `just module-reachability` after module moves; an orphan test file is not compiled.
