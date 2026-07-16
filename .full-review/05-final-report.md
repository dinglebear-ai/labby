# Comprehensive Full-Project Review — Final Report

Review target: `e6d761f91466905b435b253497b5d4077882fba8`
Scope: entire tracked Lab project (11,090 files), excluding ignored build/cache/local state
Review phases: code quality, architecture, security, performance, testing, documentation/API contracts, framework/language practices, CI/CD, and DevOps

## Executive summary

The review validated **49 unique issues** after cross-phase deduplication:

| Priority | Count |
|---|---:|
| P0 | 1 |
| P1 | 10 |
| P2 | 31 |
| P3 | 7 |
| Total | 49 |

The repository's normal baseline is strong but incomplete: `just lint` passed; 2,035/2,035 runnable Rust tests passed; Gateway Admin passed 336 unit and 2 installer tests. Those gates did not cover the highest-risk authorization, cancellation, secret, and publication boundaries found here.

Remediation and PR validation subsequently surfaced thirteen additional delivery
issues outside the original 49-finding snapshot. All were fixed before merge:
same-resource MCP UI refreshes re-minimizing restored inspectors (React and
embedded hosts), persisted iframe heights preventing shrink, Windows ACL
hardening inheriting an incompatible PowerShell 7 module path, an unpinned
cargo-deny CLI breaking its CI invocation, obsolete Node action runtimes, an
invalid distrobuilder version probe, loopback reverse proxies satisfying the
unauthenticated bootstrap capability, a zero-limit usage-page panic, and the
CLI exposing offset pagination without the replacement cursor control.
The final review pass also found that the accepted local bootstrap capability
was not propagated through the shared API admin gate; that capability is now
scoped to bootstrap and covered by regression tests.
Release rollback now preserves pre-existing releases and container versions,
and Release Please creates an explicit stable tag with a private draft so the
validated tag workflow can publish that draft only after every gate succeeds.

## P0 — Immediate security remediation

### F-01 — Active and reusable credentials are published in the public repository

Evidence: credential material is present in `docs/sessions/2026-05-04-acp-session-persistence-chat-polish.md:98-100` and `docs/superpowers/plans/2026-04-12-backup-node-live-test-services.md:30-53`; `.gitleaksignore:1-11` suppresses the detections. A non-printing equality check confirmed one published HMAC value still matches active `LAB_ACP_HMAC_SECRET` configuration. No secret value is reproduced in this report.

Required remediation: rotate/revoke every affected credential, redact the current tree, remove permissive baselines, scan current tree and history, purge published plaintext where operationally safe, and verify the running service accepts only rotated material.

## P1 — High-priority correctness, security, and release integrity

### F-02 — Non-admin Setup actions can rewrite auth/config state and mutate the host

Setup bootstrap, draft, plugin, finalize, and secret/config paths are marked non-admin; arbitrary environment keys are accepted; bootstrap can mint and return a bearer. Make every mutation/secret read admin-only and destructive where appropriate, restrict first-run bootstrap to a local capability, enforce schema-backed environment-key grammar, and registry-test all callers/surfaces.

### F-03 — MCP authorization omits Doctor's admin-only action

`doctor.oauth.relay.check` is marked `requires_admin`, but MCP uses a hard-coded service allowlist that omits Doctor. Resolve action metadata for every registered service, fail closed, and registry-test every admin action across MCP and HTTP.

### F-04 — Doctor/health advertises probes that cannot probe anything

Doctor's known/configured service lists and client set are empty, yet CLI, action catalog, operations docs, and audit output promise service health. Rebuild from the live registry/gateway health model or remove the obsolete surface; empty coverage must warn/fail. Add successful, failing, unknown, and multi-instance end-to-end tests.

### F-05 — Gateway and protected-route saves are partially committed

The UI saves gateway state, closes through its parent callback, then mutates the protected route. Add one backend transaction under the config lock or tested compensation; close/succeed only after the complete operation. Test add/update/remove failure, abort, rollback, and retry.

### F-06 — Canceled full gateway reloads can leave the runtime offline

Reload publishes no runtime and drains the live pool before the replacement is built and probed; HTTP timeout cancellation can abandon that future. Build beside the live pool, atomically swap only when ready, then drain. Test dropped futures, timeouts, and blocked probes while the old pool stays available.

### F-07 — Code Mode step/journal state is unbounded and cancellation leaks request state

Steps and per-step journal payloads have no aggregate count/byte budget; normal-return cleanup is bypassed by cancellation. Add hard budgets, release completed ordinals, and use a drop-safe execution guard. Stress-test boundaries and abort-after-first-step cleanup.

### F-08 — Generated API contracts omit action-level admin requirements

JSON/Markdown/MCP/OpenAPI projections omit `requires_admin`, `lab:admin`, and 403 semantics even though runtime enforcement relies on them. Project the metadata into every contract, document transport differences, and test generated policy parity.

### F-09 — Required `ci-gate` omits Code Mode and MCP regression jobs

`codemode-runner-smoke` and `mcp-regressions` can fail while the sole required aggregate check succeeds. Add both to `needs` and the result predicate; add a policy test that every intended gate participates.

### F-10 — Release publication advances public surfaces before validation completes

Versioned/`latest` GHCR images and a Release Please GitHub release can become public before binary/Incus validation and artifact attachment finish, with no rollback. Build/smoke privately, then publish images and promote a draft release only in one final job dependent on every gate.

### F-11 — MCP publisher is mutable/unverified and receives the registry signing key

Release downloads `releases/latest`, extracts and executes it without checksum/signature verification, then exposes `MCP_PRIVATE_KEY`. Pin a specific release and verified digest/signature, rotate the signing key, prefer short-lived/OIDC publishing, and policy-test executable downloads.

## P2 — Required medium-priority remediation

### F-12 — Gateway-test `AbortSignal` is dropped

Thread the signal through the mutation hook and API client; test close, unmount, and superseding-request cancellation.

### F-13 — `services.status` converts Claude CLI failure into false absence

Propagate typed errors or model explicit unknown state; test spawn, nonzero exit, timeout, and malformed JSON.

### F-14 — Draft commit ignores secret-bearing draft deletion failure

Return a structured partial-commit state/error and preserve diagnostics; securely clear the draft and test deletion/read permission failures.

### F-15 — Corrupt/unreadable gateway runtime state is silently overwritten

Default only on `NotFound`; quarantine or fail visibly on I/O/decode failure and preserve forensic evidence. Add malformed and permission-denied tests.

### F-16 — MCP production code depends on the API adapter

Import neutral `labby_auth` types directly and enforce cross-surface import boundaries.

### F-17 — CLI gateway construction depends on MCP internals

Inject a neutral connector at the product composition root and add a layer guard.

### F-18 — Stable error kinds fall through to HTTP 500

Map `audit_timeout`, `merge_write_conflict`, `workspace_not_configured`, and every active stable kind through a complete typed status table with endpoint tests.

### F-19 — `setup.plugin_connectivity` is a blind SSRF oracle

Restrict it to local admin use, validate the canonical allowed origin or apply full SSRF checks to resolved/connected IPs, disable redirects, and test loopback/LAN/metadata/redirect cases.

### F-20 — JWT signing uses RSA affected by the Marvin timing advisory

Migrate to Ed25519/P-256 or another constant-time backend, remove direct affected RSA and the false advisory exception, rotate keys, and test token interoperability.

### F-21 — Palette locks vulnerable `serde_with` 3.20.0

Upgrade to at least 3.21.0, test renderer/Tauri surfaces, and include Palette's independent lockfile in advisory automation.

### F-22 — Windows secret/auth files lack owner-only ACL enforcement

Atomically create and validate restrictive DACLs for env/drafts/backups, auth DB/WAL/SHM, private keys, and usage/journal stores. Add Windows ACL tests and truthful Doctor/docs behavior.

### F-23 — Healthy upstream reprobes form an unbounded synchronized herd

Use a shared semaphore, stable per-upstream jitter on every interval, and a clamped concurrency limit; test peak concurrency deterministically.

### F-24 — Usage pagination performs deep OFFSET scans and full recounts

Adopt `(ts_unix,id)` keyset pagination/indexing and optional/cached totals; benchmark million-row behavior.

### F-25 — Gateway routes eagerly ship the closed dialog and CodeMirror

Dynamic-import the dialog and lazy-load the editor only when selected; enforce compressed route bundle budgets.

### F-26 — Generated documentation is stale

Resolve the `snippets.promote.confirm` contract, regenerate action catalog/MCP help/OpenAPI, and require `just docs-check` before merge.

### F-27 — OpenWiki is materially stale and contains broken commands/links/contracts

Regenerate from current code and add link, command, environment-name, and protocol-contract validation to its workflow.

### F-28 — Palette tests/build/audit are absent from repository CI

Add renderer tests/coverage/typecheck/Vite build, Tauri Rust tests, independent lockfile audit, and Windows smoke jobs with path routing.

### F-29 — Gateway browser tests are excluded and opaque on startup failure

Use a free port, capture early exit/stderr, build once, and run the five-test suite in CI.

### F-30 — Post-pivot gateway tests remain ignored for missing fixtures

Add a test-only registered service with required/secret metadata and restore the affected tests to CI.

### F-31 — Performance risks have no regression budgets

Add large-row pagination benchmarks, reprobe concurrency tests, Code Mode memory limits, OAuth map churn tests, async responsiveness tests, and compressed bundle thresholds.

### F-32 — ACP installation blocks Tokio workers

Move extraction, recursive validation, copy/fsync, and blocking process work into bounded blocking/async subprocess paths; test responsiveness and cleanup.

### F-33 — Gateway package-cache repair blocks Tokio workers

Run recursive npm/uv cache repair in bounded blocking work and test a large cache tree alongside concurrent requests.

### F-34 — Provision timeout handling blocks a Tokio worker

Use async process-group signals and `tokio::time::sleep`, or isolate blocking work; test SIGTERM/SIGKILL ordering and cancellation.

### F-35 — Palette persistence blocks UI/async workers and holds a credential mutex

Cache settings, move durable I/O off async/UI threads, and persist OAuth data outside the global mutex with generation checks.

### F-36 — Setup API/MCP performs durable synchronous transactions inline

Execute each complete atomic transaction within one `spawn_blocking` closure and test concurrent request responsiveness.

### F-37 — Cargo manifest/lock/toolchain changes skip full Rust test suites

Route `rust_test` for manifest, lockfile, toolchain, and build-script changes; add classifier tests.

### F-38 — Mutable external action tags remain in release/secret-bearing workflows

Pin every external action to a reviewed full SHA and enforce that policy automatically.

### F-39 — Release tags lack strict early preflight

Before any build/publication, enforce stable SemVer, ancestry from `main`, and Cargo/npm/registry version equality; protect release tags.

### F-40 — Rolling Incus publication can expose mixed source and assets

Stage uniquely versioned assets, verify them, then move the rolling alias last; do not conflate the rolling channel with semantic latest.

### F-41 — Release artifacts lack independent provenance and signatures

Attest archives/images, publish an SBOM, keylessly sign the container digest, and expose immutable digests.

### F-42 — Critical filesystem/subprocess failure paths lack injection coverage

Add injectable seams and explicit failure tests for Setup command execution, draft clearing, corrupt runtime state, process signaling, and durable-write partial failures.

## P3 — Required low-priority remediation

### F-43 — `GatewayFormDialog` is an oversized effect/ref state machine

Introduce a typed reducer and extract OAuth, protected-route, config-editor, and service/custom-form units while preserving behavior tests.

### F-44 — Four large production modules combine unrelated responsibilities

Mechanically split Code Mode driver handlers, configuration domains, API router/middleware, and auth SQLite domains into modern sibling modules with unchanged contracts and tests.

### F-45 — OAuth per-IP rate-limit maps never evict

Add bounded/LRU idle-TTL eviction and address-churn tests.

### F-46 — Rust coverage has no trend or critical-module gate

Publish `cargo llvm-cov` artifacts and initially gate security/dispatch/runtime-critical modules while reporting the test pyramid.

### F-47 — `docs/CHANGELOG.md` is a stale duplicate

Remove it or redirect readers to the release-managed root changelog.

### F-48 — Dev mockup handlers synchronously scan/read files per request

Use async/bounded blocking I/O or a cached newest-file index and test large-directory concurrency.

### F-49 — CI/image tooling floats outside lockfiles

Pin Actionlint and distrobuilder/tool versions or digests, record build versions, and update through reviewed automation.

## Remediation and verification contract

Every F-01 through F-49 is in scope. Fix work must preserve the intentional `marketplace-no-mcp` branch, update generated docs/contracts, add the missing tests described above, and run the all-features workspace gates plus Gateway Admin, Palette, browser, security/history scan, CI policy, and live secret/runtime checks before merge.

Cross-phase findings were consolidated rather than discarded: authorization-test gaps are attached to F-02/F-03; Doctor tests/docs to F-04; partial-failure tests to F-05; cancellation tests to F-06/F-07/F-12; secret-scan policy to F-01; Windows docs/tests to F-22; error tests to F-18; and performance budgets to F-23-F-25/F-31-F-36/F-45/F-48.
