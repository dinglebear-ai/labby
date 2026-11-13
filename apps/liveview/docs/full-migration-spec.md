# Complete Labby LiveView migration

## Required outcome

Implement the entire native Labby mock in the real product, at the same or better
visual quality. Use TDD, regression tests and Playwright coverage for every
reasonable workflow. After functional and visual parity, embed a Phoenix release
inside Labby, extract it to a versioned application directory, supervise it and
proxy HTTP/WebSocket through Labby's existing address. Run the requested full
review over every touched PR file, fix every finding and live-test the result.
No smaller subset satisfies completion.

## Architecture

Reuse the separate `phx-book` mock's authored HEEx, styles and interaction patterns
as the visual reference. Replace preview/sample reducers with per-caller API
adapters and real Rust-owned state. Pure presentation reducers may remain in the
console; domain validation, authorization and persistence remain in Rust.
Unsupported data must remain explicitly unavailable rather than invented. A
generic action directory is not a substitute for the mock's designed workflows.

The current eleven read-model routes remain available while mock-shaped pages
are migrated. Shared transport admits explicit service/action pairs, forwards the
current caller's cookie and CSRF, bounds bytes/deadlines, preserves meaningful
structured failure and uncertain-side-effect state, and never retries mutations
automatically. Forbidden actions do not revoke otherwise valid sessions.

Keep dispatch rejection distinct from transport ambiguity, retaining the gateway's
bounded kind/context/recovery/side-effect metadata. Apply a 32 KiB action request
budget by default (larger domain payloads require separately reviewed bounded
transport), 64 KiB response budget, 5-second complete operation deadline (12 seconds
for the existing host sample), and at most four concurrent operations per session
and 32 globally. Admission rejects excess work visibly rather than queueing it
without bounds. A failed or canceled write after dispatch has uncertain outcome
until reconciled. Existing read paths must share the 401/403 distinction.

Scope/request generations bind asynchronous completions to the initiating caller,
Team, Project and operation. Sign-out/revocation, scope changes and replacement
requests invalidate stale replies. Treat browser workspace selection as a caller-
isolated UI preference, distinct from `projects.activate`, which restores an
archived Project. Reconcile authoritative active lists and explicitly fetched
archived snapshots without inventing records or retaining obsolete permissions.

The reference has eighteen named areas: Overview, Gateway, Observe, Discover,
Create, Library, Settings, Stash, Tasks, Projects, Agents, Access, Browser,
Diagnostics, Setup, Depot, Tools and Environments. Include their dialogs, tabs,
inspectors, filters, pagination, keyboard/mobile behavior and connected flows.
The source contract inventory additionally covers dispatch and dedicated HTTP
boundaries, including uploads, credentials and transfers.

## Acceptance evidence

Each workflow needs a reference source/hash, owning Rust contract, test-first
regression, live browser journey, persistence/error/permission checks, and visual
comparison at desktop and narrow mobile widths. A fixture-only result cannot
close a live acceptance item. External integrations need an authorized real
provider target when the workflow depends on one; lack of credentials is an open
acceptance item, not a reason to substitute fake success.

Shared UI acceptance includes menus/dialog focus and Escape, browser history,
long/empty/error data, isolated concurrent callers, refresh/reconnect, loading,
expired authority and responsive overflow. Browser runs must have explicit
request/resource/time budgets and task-owned cleanup.

## Packaging, after parity

Build an OTP release with ERTS for each supported native target. Embed its archive
and manifest into Rust. Extract into an immutable versioned application directory
using digest verification, bounded safe paths, atomic publication and concurrent
startup locking. Persist secrets through existing supported secret storage.
Supervise only owned child processes, with readiness and bounded crash recovery;
stop/reap on shutdown. Proxy HTTP and WebSocket through the existing authenticated
gateway address without exposing an unauthenticated loopback bypass. Preserve
existing CLI/MCP/API behavior and installation/update/rollback guarantees.

Prove first launch, warm launch, tampering, interrupted/concurrent extraction,
path attacks, crashes/restarts, signal cleanup, upgrades/rollback, HTTP assets,
WebSocket events and auth isolation using the packaged native artifact itself.
Source-level tests cannot qualify the distribution artifact.

## Final review

After implementation and packaging, run `vibin:full-review` on the complete PR
diff. Preserve its immutable scope, all specialist coverage, every finding and
disposition, validated report and sealed evidence. Address all surfaced issues;
rerun affected live acceptance against the final revision before completion.
