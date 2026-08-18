# Code Mode Retained Results

**Status:** Proposed, not implemented on current main as of 2026-08-18.
**Tracking issue:** #274, "Retain and page oversized Code Mode results by handle."

## Purpose

Code Mode already expects model-authored code to reduce large upstream results inside the sandbox before returning them. Issue #274 proposes an optional fallback for expensive, rate-limited, non-idempotent, or otherwise unsafe-to-repeat results that still exceed the final response budget.

The proposed behavior is:

1. retain the complete over-budget serialized value in a bounded host-side store;
2. return the normal preview plus an opaque handle;
3. allow a later Code Mode execution to fetch metadata or select a bounded path/range from that retained value;
4. avoid re-running the original upstream tool merely to recover data truncated from the first response.

## Required Safety Properties

Any implementation must preserve the acceptance criteria in issue #274:

- handles are opaque and scoped to the originating authorization context;
- retained items have bounded TTL, item count, and byte usage with deterministic eviction;
- malformed, expired, evicted, or unauthorized handles return structured errors without leaking handle existence across callers;
- hard truncation remains the fallback when retention is disabled or unavailable;
- small and cheaply reproducible results are not retained by default;
- metrics cover stored bytes, fetches, misses, expirations, and evictions.

## Current Main

Current main has Code Mode result shaping/truncation, artifact retention, source retention, and step journaling, but it does **not** expose the proposed retained-result handle API such as codemode.fetch(handle) or codemode.slice(handle, ...). Those adjacent retention mechanisms are separate features and must not be treated as implementation of #274.

## Implementation Note

The previous detailed design bundle was retired on 2026-08-18 because it was grounded in an older main commit and carried obsolete branch, worktree, bead, and file/line coordinates. Before implementation begins, refresh the design against current labby-codemode, labby-gateway, surface ownership, authorization context, result-shaping code, and the live issue acceptance criteria.

Until then, issue #274 is the authoritative feature tracker and this document is only the current repository-facing summary.
