# Native Skills and Reduced SDK Fork Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Execute the accepted migration recommendation inline, with one fresh review before publication.

**Goal:** Migrate Labby to native RMCP Skills, retire the fork's typed/raw-response APIs, and submit the remaining SDK changes upstream.

**Architecture:** Keep Labby's Skills operation owner and transport budget wrappers. Use upstream native Skills request/handler types, retaining only OAuth Send, strict Origin validation, configurable Unix response bounds, workload JWT grants, bounded SSE retries, repeated authentication challenges, the optional DPoP/conformance support, and the additive Events capability in the SDK delta.

**Tech Stack:** Rust 1.97.1 for Labby, the SDK's pinned Rust toolchain, Tokio, RMCP 3.9, existing serde and native transport fixtures.

**Spec:** [Accepted fork assessment](rmcp-fork-assessment.md). The user accepted its migration/upstreaming recommendation on 2026-11-08.

## Global Constraints

- Preserve the current 10 MiB ordinary and 24 MiB Skills response budgets, including tighter operator settings.
- Preserve existing authorization, route isolation, bounded pagination, cancellation, typed errors, headers, and manifest validation.
- Keep Unix HTTP framing and SSE inside RMCP; add no competing transport implementation.
- Unknown cache hints must never discard a usable skill. Native wire adapters may conservatively advertise `private` when the SDK cannot express a future hint; record and test that conversion.
- Preserve private instructions, foreign worktrees, and canonical dirty files. Build output belongs to the claimed task checkout.
- Upstream PR submission is authorized by the accepted recommendation. Labby publication and all PR merges remain separate actions.
- This plan executes the SDK/migration prerequisite; the two delivery adapters remain in their existing plan.

## Review Focus

- Native method interception must preserve legacy and current MCP lifecycle behavior and current caller/route policy.
- SDK removal must leave no transport adapter relying on fork-only raw response types or hooks.
- A valid Skills response above 16 MiB must fit the configured 24 MiB limit, while configured-limit overflow fails before deserialization.
- OAuth JWT-enabled client credentials must produce Send futures; Origin hardening must reject duplicate/malformed origins and unintended ports.
- Unknown cache hints must preserve usable Skills and avoid broadening cache visibility.

### Task 1: Reduce the SDK delta and add its bounded extension points

**Files:** SDK `crates/rmcp/src/model/capabilities.rs`, `transport/auth.rs`, `transport/common/unix_socket.rs`, `transport/streamable_http_server/tower.rs`, their focused tests and changelog. Restore upstream implementations for the removed typed/raw-response patch and remove only obsolete tests for removed APIs.

**Interfaces:** Native upstream Skills APIs. Add `ServerCapabilities.events: Option<JsonObject>` and builder `enable_events()`. Retain `UnixSocketHttpClient::with_max_response_bytes(usize)` as an explicit JSON cap override; omitted uses 16 MiB, error bodies remain bounded by the smaller of the configured cap and 64 KiB.

- [x] Write and run failing capability and explicit-large-JSON-bound regressions.
- [x] Merge verified upstream main into the task branch, remove the obsolete raw-response patch, and retain the used fork behavior.
- [x] Implement the capability and bounded explicit override, then run focused native SDK tests.
- [x] Run full all-feature/no-local SDK tests, minimal-feature tests, strict Clippy, and the repository formatter; commit only the task changes.

### Task 2: Move Labby to native Skills and remove raw-response adapters

**Files:** Six gateway modules named in the assessment, MCP `server.rs`/`skills.rs`, native Skills fixtures, both RMCP manifests, lockfile and conformance pins.

**Interfaces:** Preserve Labby's independent Skills DTOs. Translate native client request/result and server handler types at the existing boundaries. Convert unknown outbound cache hints to `private` rather than rejecting the skill. Keep current budgets and policy owners.

- [x] Add native wire, metadata/pagination/error, cache-hint and 16–24 MiB transport tests; observe failure against the current adapters or SDK.
- [x] Change the dependency through a command-local path patch during development, migrate every affected adapter/fixture, and remove obsolete raw-response hooks.
- [x] Verify focused gateway, MCP, CLI/feature-slice behavior and the existing real-daemon Skills target.
- [x] After the reviewed SDK revision is published, pin that exact available revision in every production manifest/conformance projection and regenerate the lockfile.

### Task 3: Qualify the complete migration

**Files:** Existing native tests, current Skills/upstream contracts and migration evidence in the task workspace.

- [x] Run repository-owned compilation, tests, lint, feature slices, documentation and conformance checks appropriate to the dependency change; preserve and classify any environment failures.
- [x] Run a fresh independent review of the SDK delta and Labby migration; fix confirmed important findings and rerun affected checks.
- [x] Commit the reviewed local Labby migration and update the existing delivery plan with the resulting SDK prerequisite revision and completed migration state.

### Task 4: Submit independent upstream SDK changes

**Files:** Independent SDK branches based on freshly fetched upstream main, each limited to one fix/capability and its tests.

- [x] Prepare separate OAuth Send, Origin validation, configurable Unix bounds, Events, SSE retry, workload JWT, repeated challenge, and DPoP changes with no obsolete typed/raw APIs.
- [x] Validate each against its real upstream base and perform fresh non-destructive conflict checks.
- [ ] Push the authorized SDK branches and create reviewable upstream PRs with concrete behavior, compatibility considerations and actual validation.
- [x] Verify remote revisions and report exact published candidate links. Leave merges to a human and keep the reviewed combined fork revision available for Labby's pin.


Execution status: the local migration and SDK candidate are qualified. Eight
upstream candidates are published; creation of upstream PRs is blocked by the
current GitHub credentials. The [migration record](native-skills-migration.md)
contains their exact revisions and comparison links. The SDK integration is
[fork PR #9](https://github.com/dinglebear-ai/rust-sdk/pull/9). No main branch was changed.
