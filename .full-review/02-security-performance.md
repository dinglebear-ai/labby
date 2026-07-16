# Phase 2: Security & Performance Review

Review commit: `e6d761f91466905b435b253497b5d4077882fba8`

## Summary

| Area | Critical | High | Medium | Low | Total |
|---|---:|---:|---:|---:|---:|
| Security | 0 | 3 | 6 | 1 | 10 |
| Performance | 0 | 2 | 3 | 1 | 6 |
| Combined phase count | 0 | 5 | 9 | 2 | 16 |

Some security findings validate Phase 1 defects from a threat perspective. They remain cross-referenced here and will be deduplicated in the final report.

## Security Findings

### High

#### S-H1: Non-admin Setup actions can rewrite authentication/config state and mutate the host

- CVSS 8.8; CWE-862/CWE-20.
- Evidence: `dispatch/setup/catalog.rs:58-127,205-227,321-379` marks bootstrap, draft mutations, plugin mutations, and finalize non-admin; `dispatch/setup/params.rs:8-41` accepts arbitrary keys; `dispatch/setup/dispatch.rs:635-708` permits unknown keys and merges them into `~/.labby/.env`; `dispatch/setup/bootstrap.rs:42-55` returns a newly generated static bearer. API/MCP gates rely on `requires_admin`.
- Attack: any ordinary authenticated caller can stage/commit arbitrary auth/bind/security env values, mint a bearer on first run, discard operator drafts, or mutate plugins. Invalid/newline env keys are not rejected.
- Fix: make all setup mutation and secret/config reads admin-only; mark bootstrap/draft writes destructive; restrict bootstrap to explicit local first-run capability; allow only registered schema keys matching a strict env-key grammar; add cross-surface authorization tests.

#### S-H2: Real credentials are committed to the public repository and one HMAC secret remains active

- CVSS 7.5; CWE-798/CWE-200.
- Evidence: `docs/sessions/2026-05-04-acp-session-persistence-chat-polish.md:98-100` contains a full HMAC secret; a non-printing equality check confirmed it matches the active `LAB_ACP_HMAC_SECRET`. `docs/superpowers/plans/2026-04-12-backup-node-live-test-services.md:30-53` contains multiple service credentials. `.gitleaksignore:1-11` baselines these while the repository is public.
- Attack: public readers can forge HMAC-protected material with the active key; documented snapshot credentials become usable when those golden instances are restored.
- Fix: redact current docs, rotate the active HMAC and all published credentials/tokens, invalidate snapshot secrets, narrow `.gitleaksignore` to proven fixtures with rationale, add a current-tree scan, then consider history rewriting after rotation.

#### S-H3: MCP omits an explicitly admin-only Doctor action

- CVSS 6.5; CWE-862. Security validation of Phase 1 A-H1.
- Evidence: `dispatch/doctor/catalog.rs:107-120` marks `oauth.relay.check` admin-only; `mcp/context.rs:172-203` omits Doctor from its hard-coded allowlist; `mcp/call_tool.rs:369-385` relies on that helper.
- Attack: a non-admin MCP caller can trigger relay readiness/internal target probes despite the declared admin boundary.
- Fix: resolve metadata for every service and fail closed; test every admin action on API and MCP.

### Medium

#### S-M1: `setup.plugin_connectivity` is an authenticated blind SSRF oracle

- CVSS 5.4; CWE-918.
- Evidence: `dispatch/setup/catalog.rs:237-247` accepts a caller URL without admin; `dispatch/setup/dispatch.rs:153-159` forwards it; `dispatch/setup/plugin_hook.rs:276-335` appends `/health` and uses a default redirect-following client without URL/IP/peer validation.
- Attack: a low-privilege caller can scan loopback/LAN/metadata endpoints and follow redirects, observing status/timing.
- Fix: restrict to local admin use, parse/allowlist the Labby origin or apply canonical SSRF validation, disable redirects, and validate resolved/connected IPs.

#### S-M2: Remote JWT signing uses RSA versions affected by Marvin timing attack

- CVSS 5.9; CVE-2023-49092/RUSTSEC-2023-0071; CWE-208.
- Evidence: live `cargo audit` reports `rsa 0.9.10` and direct `rsa 0.10.0-rc.16`; `labby-auth/src/jwt.rs:61-105` signs issued access tokens and `token.rs:417` invokes it. `deny.toml:4-10` suppresses the advisory on a now-false claim that RSA signing is not performed.
- Attack: a remote client collecting many signing timings may recover the private key; complexity is high.
- Fix: migrate signing to Ed25519/P-256 or another constant-time backend, remove direct RSA and the stale exception; until then rate-limit issuance and rotate/isolate keys with a removal deadline.

#### S-M3: Palette Tauri ships a vulnerable `serde_with` lockfile

- GitHub Medium; GHSA-7gcf-g7xr-8hxj/CWE-20.
- Evidence: live Dependabot alert #77; `apps/palette-tauri/src-tauri/Cargo.lock:3054-3077` pins 3.20.0 via Tauri, patched in 3.21.0.
- Attack: affected `KeyValueMap` serialization can panic on crafted empty sequence/map entries if that path is reached.
- Fix: update to `serde_with >=3.21.0`, test Palette, and include its independent lockfile in automated advisory scans.

#### S-M4: Windows secret/token/auth files are not hardened with owner-only ACLs

- CVSS 5.5; CWE-732.
- Evidence: `config/env_merge.rs:462-475` and `labby-auth/src/util.rs:41-71` are permission no-ops on Windows; the latter protects JWT keys and auth DB usage in `jwt.rs:61-98` and `sqlite.rs:1150-1292`. Windows is a release target.
- Attack: another local account/process allowed by inherited ACLs can read bearer tokens, OAuth/session data, or signing keys.
- Fix: atomically create/validate owner-only DACLs for env/drafts/backups, auth DB/WAL/SHM, signing keys, and usage/journal DBs; add Windows ACL tests.

#### S-M5: Secret-bearing draft deletion failures are ignored

- CVSS 4.7; CWE-459/CWE-391. Security validation of Phase 1 Q-M3.
- Evidence: `dispatch/setup/dispatch.rs:696-738` ignores `remove_file` failure and reports success; `draft.rs:15-19` also converts all read errors to empty state.
- Fix: distinguish NotFound, return partial status/error, preserve diagnostics, and test read/deletion failures.

#### S-M6: Gateway and protected-route policy use separate transactions

- CVSS 5.3; CWE-670. Security validation of Phase 1 Q-H2.
- Evidence: `gateway-form-dialog.tsx:867-916` saves gateway then route; list/detail parent callbacks close on the first request.
- Fix: one backend atomic transaction or explicit compensation, with failure-injection tests.

### Low

#### S-L1: Corrupt runtime state is silently treated as empty and overwritten

- CWE-391. Security validation of Phase 1 Q-M4.
- Evidence: `labby-gateway/src/gateway/runtime.rs:72-93,148-194` defaults on all read/decode errors.
- Fix: default only on NotFound, quarantine/fail on other errors, preserve forensic evidence, and test corruption/permissions.

## Performance Findings

### High

#### P-H1: A canceled full gateway reload can leave runtime offline

- Evidence: `gateway/manager/pool_lifecycle.rs:208-221` publishes `None` and drains the old pool before building/probing; `:243-340` installs the replacement only after all discovery; per-attempt timeout/default concurrency are 15 seconds/3 (`upstream/pool/helpers.rs:32-41`); HTTP timeout is 30 seconds (`labby/src/api/router.rs:1699-1705`).
- Impact: 20 slow upstreams require about seven waves (~105 seconds); HTTP cancellation can drop the future after runtime removal, leaving the gateway unavailable.
- Fix: build beside the live pool, atomically swap only when ready, drain afterward; test dropped reload futures with blocked probes.

#### P-H2: Code Mode step state is unbounded and canceled executions leak journals

- Evidence: `labby-codemode/src/runner_drive.rs:1199-1204` inserts every step; `:1249-1253` does not remove results; no step/aggregate budget exists. `gateway/code_mode/code_mode_host.rs:126-175,340-386` stores up to ~68 KiB per step and removes only on explicit flush; MCP flush occurs only after execute returns (`mcp/call_tool_codemode.rs:394-405,439-443,480-483`).
- Impact: 10,000 steps can retain roughly 650 MiB plus overhead; canceled requests leak execution entries for daemon lifetime.
- Fix: hard step/count/byte budgets, remove completed ordinals, and use a drop-safe execution guard; add stress and cancellation tests.

### Medium

#### P-M1: Healthy upstream probes form a synchronized herd

- Evidence: `upstream/pool/probe.rs:33-120` creates one task per upstream and uses identical 30-second intervals, resetting healthy attempts to zero; loops do not share bulk discovery semaphore; configured concurrency lacks a maximum (`helpers.rs:223-235`).
- Impact: startup/reload synchronizes periodic discovery spikes across all upstreams.
- Fix: shared reprobe semaphore, stable per-upstream jitter on every interval, concurrency clamp, and peak-concurrency tests.

#### P-M2: Usage pagination performs deep offset scans plus full recount

- Evidence: unrestricted offset in `gateway/params.rs:166-177`, passed through `manager/usage.rs:94-106`; `usage/store.rs:323-375` runs `COUNT(*)` plus ordered `LIMIT/OFFSET`.
- Impact: million-row retention tables produce progressively slower pages and connection occupancy.
- Fix: `(ts_unix,id)` keyset cursor/index, optional/cached totals, and million-row benchmarks.

#### P-M3: Gateway routes eagerly ship closed dialog and CodeMirror

- Evidence: list/detail pages eagerly import/render `GatewayFormDialog`; it statically imports `TextSurface`, which imports CodeMirror. Current `/gateways` artifacts reference ~1.83 MB raw/~562 KiB gzip JS, with ~706 KB raw/~233 KiB gzip in the CodeMirror-bearing chunk.
- Impact: every gateway visit pays editor parse/download cost even when never opened.
- Fix: dynamically import the dialog and lazily load editor on drawer selection; add compressed route bundle budgets.

### Low

#### P-L1: OAuth per-IP rate-limit maps never evict

- Evidence: `labby-auth/src/state.rs:58-100` documents and implements two permanent per-address maps.
- Impact: rotating IPv4/IPv6 sources cause monotonic daemon memory growth.
- Fix: idle TTL plus bounded/LRU eviction and address-churn tests.

## Tooling and Positive Controls

- Main workspace `cargo deny` passed advisories, bans, licenses, and sources; its RSA exception is invalid and surfaced only through direct `cargo audit` validation.
- Root JavaScript audit reported no advisories; live GitHub Dependabot found the separate Palette lockfile alert.
- Gitleaks investigation excluded build/generated/test-fixture noise and validated the first-party secret documents without reproducing secret values.
- Positive controls include blocking SQLite work via `spawn_blocking`, WAL pools, bounded telemetry writes, runner overflow and response/artifact limits, bounded subject caches, and upstream body limits.

## Critical Issues for Phase 3 Context

No Critical-severity Phase 2 findings were found. Phase 3 must verify test coverage for all High findings, especially authorization metadata, Setup mutations, gateway reload cancellation, Code Mode cancellation/step bounds, secret scanning/rotation hygiene, and atomic config/policy mutation. Documentation review must reconcile the false RSA advisory rationale and Windows permission caveat, and identify any other live secrets or stale security claims.
