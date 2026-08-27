# Task 1 Implementation Report

## Status

DONE

Implementation commit: `4ba519646f0f52b89af9661fc7c56fa187414576`

## Changed files

- `crates/labby-gateway/src/gateway/palette.rs`
- `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`
- `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`
- `apps/palette-tauri/src/lib/launcherCatalog.ts`
- `apps/palette-tauri/src/lib/labbyClient.ts`
- `apps/palette-tauri/src/App.tsx`
- `apps/palette-tauri/src/lib/launcherCatalog.test.ts`
- `apps/palette-tauri/src/lib/labbyClient.test.ts`
- `apps/palette-tauri/src/lib/launcherValidation.test.ts`
- `apps/palette-tauri/src/lib/paletteAudit.test.ts`
- `apps/palette-tauri/src/lib/schemaForm.test.ts`
- `apps/palette-tauri/src-tauri/src/labby_bridge.rs`
- `crates/labby/src/api/services/palette.rs`
- `crates/labby/src/api/error.rs`

The last two adapter files plus the Tauri bridge were narrowly included by maintainer ruling so `expectedContractHash` and the receipt remain authoritative end-to-end. The three one-line TypeScript fixture changes satisfy the now-required normalized `contractHash` type.

## Design decisions

1. `CapabilityContract` v1 is the single upstream MCP capability projection. Its SHA-256 input is canonical recursive key-sorted JSON containing exactly contract version, ID, sanitized input/output schemas, four nullable typed annotation hints, and authoritative destructive classification. Descriptions and catalog revision are excluded. Arrays retain order and missing values encode as `null`.
2. Schema serialization is capped at 64 KiB per schema. The canonical contract hashes through a 160 KiB capped writer, avoiding allocation of a second complete canonical JSON tree. Oversize descriptors fail explicitly as `descriptor_unsupported`.
3. Code Mode and Palette share `execute_upstream_tool_checked`. It makes runtime ready, snapshots the published config/pool pair, re-resolves the caller-visible tool against that same pool (including subject-scoped OAuth), re-hashes, re-checks caller scope/read-only policy, validates params, then dispatches. Contract drift returns `contract_changed` with no upstream tool request.
4. Palette passes the real owner, OAuth subject, propagated caller auth, and upstream scope. Admin construction uses the supplied subject rather than the shared OAuth subject. Subject-scoped OAuth catalog enumeration and execution use the same subject connection.
5. Non-admin destructive Palette execution is forbidden. Admin desktop confirmation remains supported, but confirmed execution still passes through the checked helper. Forge callers cannot bypass the authoritative destructive classification.
6. Desktop catalog normalization preserves `contractHash`; execution rejects absent/malformed hashes before Tauri IPC. The Rust bridge independently validates and forwards the hash. A `contract_changed` response is never retried; the app refreshes once, clears cached schema/armed confirmation/selection, and requires review.
7. Receipts are created only after checked success and contain only request ID, tool ID, authoritative contract hash, opaque catalog revision, and truncation state. Telemetry records request ID, upstream/tool, subject fingerprint, hash/revision, elapsed time, and terminal kind without parameters, schemas, results, OAuth material, or raw subject.
8. Existing desktop Labby actions retain compatibility through a server-derived per-action contract hash binding ID, existing schema fingerprint, destructive flag, and admin requirement. The catalog emits it, execution compares it before dispatch, and the receipt uses only the recomputed server value. Forge descriptor exposure remains MCP-only. `contract_changed` maps to HTTP 409.
9. V1 uses the exact per-entry contract hash as the smallest opaque `catalogRevision`. A future broader catalog revision may replace it without changing what is hashed.

## TDD evidence

### RED

`cargo test -p labby-gateway palette_execute -- --nocapture`

- Exit 101 before implementation.
- Compilation failed because `CapabilityContract`, `expected_contract_hash`, compact-entry `contract_hash`, and `PaletteExecutionReceipt` did not exist.

`pnpm test -- launcherCatalog labbyClient` from `apps/palette-tauri`

- Exit 1 before implementation.
- Three intended failures: normalization dropped the contract hash; the client invoked by ID without `expectedContractHash`; and an absent hash still invoked the bridge.

During the first GREEN pass, `cargo test -p labby-gateway code_mode -- --nocapture` exposed two legacy direct-helper regressions (`111 passed; 2 failed; 1 ignored`). The direct test helper was separated from the checked production dispatch path; each failing test then passed individually, followed by the full Code Mode suite.

### GREEN and final verification

The plan's combined Cargo filter is invalid Cargo syntax, so `palette` and `code_mode` were deliberately run as separate invocations.

`cargo test -p labby-gateway palette -- --nocapture`

- Exit 0: `14 passed; 0 failed; 0 ignored; 896 filtered out`.
- Covers canonical fixed vector `f54cdd4d74a33e09b603d2856fdfeb1f706d22c08cb7c386cfb7aa8354528ddf`, description exclusion, safety drift, descriptor caps, redacted telemetry, subject isolation, credential invalidation, reload, scope, destructive drift, checked receipt, and zero upstream request on drift.

`cargo test -p labby-gateway palette_execute -- --nocapture`

- Exit 0: `6 passed; 0 failed; 0 ignored; 904 filtered out`.

`cargo test -p labby-gateway code_mode -- --nocapture`

- Exit 0: `113 passed; 0 failed; 1 ignored; 796 filtered out`.
- The ignored test is the pre-existing 4,000-tool cold-render performance budget.

`cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings`

- Exit 0: finished the dev profile with warnings denied.

`pnpm typecheck && pnpm test -- launcherCatalog labbyClient` from `apps/palette-tauri`

- Exit 0: TypeScript `tsc --noEmit` passed; Vitest reported `2 passed` files and `9 passed` tests.

`pnpm lint` from `apps/palette-tauri`

- Exit 0: Biome checked 66 files; no fixes applied.

`cargo test --manifest-path apps/palette-tauri/src-tauri/Cargo.toml validates_launcher_execute_request_shape -- --nocapture`

- Exit 0: `1 passed; 0 failed; 46 filtered out`.

`cargo test --manifest-path apps/palette-tauri/src-tauri/Cargo.toml rejects_missing_or_malformed_launcher_contract_hash -- --nocapture`

- Exit 0: `1 passed; 0 failed; 46 filtered out`.

`cargo test -p labby palette_execute -- --nocapture`

- Exit 0 after the authoritative Labby-action adapter was added: `3 passed; 0 failed; 1149 filtered out`.

`cargo test -p labby contract_changed_maps_to_conflict -- --nocapture`

- Exit 0: `1 passed; 0 failed; 1151 filtered out`.

`cargo test -p labby palette_execute_dispatches_labby_registry_action -- --nocapture`

- Exit 0: catalog-provided Labby action hash executed successfully and matched the receipt; `1 passed; 0 failed; 1151 filtered out`.

`cargo fmt --all -- --check`

- Exit 0.

`git diff --check` and `git diff --cached --check`

- Exit 0 with no whitespace errors.

## Self-review

- Verified every production upstream execution path used by Palette and Code Mode reaches the checked helper; the old direct helper is test-only.
- Verified the checked helper re-resolves visibility, OAuth subject, published priority/enabled state, schema/safety contract, caller scope, read-only trust, and validation before dispatch on one pool snapshot.
- Verified schema drift is rejected before an `upstream.request` log can occur.
- Verified non-admin destructive calls remain forbidden even if `confirmDestructive` is supplied; admin compatibility still requires explicit confirmation.
- Verified malformed/absent hashes fail in both renderer and Tauri bridge, and stale hashes are not retried.
- Verified receipts cannot contain parameters, subject, OAuth data, or results and are built only on success.
- Verified terminal telemetry uses a subject fingerprint and does not log raw parameters, schemas, results, tokens, or raw subject.
- Verified the desktop Labby-action adapter compares the server-derived hash before validation/dispatch and never echoes an arbitrary caller value into the receipt.
- Inspected the complete staged diff, ran formatting/whitespace checks, and preserved unrelated untracked plan/spec files.

## Concerns and boundaries

- `catalogRevision` is intentionally the exact per-entry contract hash for v1 because the fixed checked-helper interface returns only the tool outcome. Later descriptor/catalog work may introduce a broader opaque revision.
- The focused `labby` tests emit pre-existing warnings for an absent `apps/gateway-admin/out` bundle and unrelated unused MCP context helpers. Task-owned gateway Clippy passes with `-D warnings`.
- Full workspace tests were not run; verification was scoped to the gateway Palette/Code Mode suites, Labby API adapter/error tests, package-local Palette TypeScript tests/lint/typecheck, and focused Tauri bridge tests.
- Untracked `docs/superpowers/plans/2026-08-23-labby-app-forge-prototype.md` and `docs/superpowers/specs/2026-08-23-labby-app-forge-prototype.md` predated this work and were left untouched/uncommitted.
