# Phase 4 — Framework, Language, CI/CD, and DevOps Review

Review target: `e6d761f91466905b435b253497b5d4077882fba8`
Scope: entire tracked Lab project

## Framework and language findings

No new P0 or P1 framework findings were validated. The gateway-save, cancellation, and oversized-component findings from Phase 1 were independently corroborated and remain in the final ledger.

### P2 — ACP installation blocks Tokio workers with synchronous extraction and filesystem work

`crates/labby-apis/src/acp_registry/installer.rs:206-242,471-537,541-619,657-711,721-781` performs extraction, recursive validation, copy, fsync, and process work inline in async paths.

Fix: move post-download transactions into bounded `spawn_blocking` work, use asynchronous child processes with timeout/termination, and test runtime responsiveness and failure cleanup.

### P2 — Gateway package-cache repair blocks async workers

`crates/labby-gateway/src/upstream/pool/cache_repair.rs:33-45,83-173,190-239` recursively scans and mutates npm/uv caches from async functions.

Fix: run repair as bounded blocking work and test a large cache tree alongside a concurrent Tokio ticker.

### P2 — Provision timeout handling launches and sleeps synchronously

`crates/labby/src/dispatch/setup/provision.rs:750-790,823-840` synchronously invokes `kill` and sleeps for 500 ms on a Tokio worker.

Fix: use async process-group signaling and `tokio::time::sleep`, or isolate blocking work; test SIGTERM/SIGKILL ordering and responsiveness.

### P2 — Palette persistence blocks UI callbacks and async workers

`apps/palette-tauri/src-tauri/src/lib.rs:71-97`, `persistence.rs`, `window_events.rs`, and OAuth persistence paths perform synchronous durable I/O; OAuth can persist while holding the global credential mutex.

Fix: cache settings in managed state, move durable I/O to `spawn_blocking`, persist outside the mutex with generation checking, and test concurrent UI/OAuth responsiveness.

### P2 — Setup API/MCP executes synchronous durable transactions inline

`crates/labby/src/dispatch/setup/dispatch.rs` and `crates/labby/src/config/env_merge.rs` perform backup/atomic-write/fsync transactions directly in async request paths.

Fix: keep each atomic transaction intact inside one `spawn_blocking` closure and test concurrent request latency.

### P3 — Dev mockup handlers synchronously scan files per request

`crates/labby/src/api/router.rs:1239-1305` scans, stats, and reads mockup files inline.

Fix: use async filesystem access, bounded blocking work, or a cached newest-file index; test a large mockup directory concurrently.

## CI/CD and DevOps findings

### P1 — The required aggregate gate omits two regression jobs

`codemode-runner-smoke` and `mcp-regressions` can fail, but neither appears in `ci-gate.needs` or its success predicate (`.github/workflows/ci.yml:197-218,370-398,915-967`). Branch protection is designed to require only `ci-gate`.

Impact: focused Code Mode or MCP regressions can fail while the merge-required check succeeds.

Fix: add both jobs to the aggregate gate and add a workflow-contract test enumerating every intended gate.

### P1 — Public release publication is non-atomic

`.github/workflows/release.yml:140-273` pushes versioned and `latest` GHCR images before binary/Incus validation and final release attachment complete. Release Please can also create an empty public release before artifacts succeed. There is no rollback of `latest`.

Fix: build and smoke without publishing, retain the OCI artifact, then publish the image tags and promote a draft release only in one final job dependent on every validation job. Test that an injected Incus failure advances no public pointer.

### P1 — MCP publisher executes an unverified mutable binary with a signing key

`.github/workflows/release.yml:321-380` downloads from `releases/latest`, pipes directly into `tar`, executes the result, and later exposes `MCP_PRIVATE_KEY` to it without version or digest verification.

Fix: pin a publisher release and verified checksum/signature, rotate the current key, and prefer short-lived/OIDC publication when supported. Add policy tests rejecting executable `/latest/` downloads.

### P2 — Manifest and lockfile changes skip full Rust test suites

`scripts/ci/changed_paths.py:70-125` sets `rust_compile` for manifests but sets `rust_test` only for Rust source paths.

Fix: set `rust_test = rust_sources or rust_manifests` for Cargo manifests/lockfile, toolchain, and build scripts. Add classifier tests for manifest-only and lockfile-only PRs.

### P2 — Mutable external-action tags remain in sensitive workflows

`.github/actions/build-gateway-admin/action.yml` uses mutable major tags in release-producing builds. `.github/workflows/openwiki-update.yml` uses mutable tags with repository write permission, an API secret, and a persistent self-hosted runner.

Fix: pin every external action to a reviewed full commit SHA and enforce this through workflow lint policy.

### P2 — Release tags lack an early strict preflight

The broad `v*.*.*` trigger publishes before verifying strict stable SemVer, reachability from `main`, and workspace/npm/registry version equality.

Fix: add an initial full-history preflight enforcing `^v[0-9]+\.[0-9]+\.[0-9]+$`, main ancestry, and every version surface; make all build/publish jobs depend on it and protect release tags.

### P2 — Rolling Incus publication can expose mixed source and assets

`.github/workflows/build-incus-image.yml:112-145` force-moves the rolling tag before release metadata and assets are replaced.

Fix: upload uniquely versioned assets to a temporary/draft release, verify checksums, and move the rolling alias last. Do not make the rolling channel GitHub's semantic latest release.

### P2 — Release artifacts lack independent provenance

Archives and images have same-workflow checksums but no build attestations, signed provenance, SBOM, or container signature.

Fix: attest archives/images, publish an SBOM, keylessly sign the container digest, and include immutable digests in release notes.

### P3 — CI and image tools float outside project lockfiles

CI resolves Actionlint at `latest`; Incus jobs install current apt and classic Snap tool versions.

Fix: pin tool versions/digests, record them in build metadata, and update through reviewed dependency PRs.
