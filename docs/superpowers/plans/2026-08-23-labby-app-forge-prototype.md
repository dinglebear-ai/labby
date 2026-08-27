# Labby App Forge Prototype Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Generate and run a portable one-tool Livebook App from Labby's caller-visible MCP catalog with caller-consistent, contract-checked execution.

**Architecture:** Extend the existing Palette projection with one bounded descriptor, then repair Palette execution so descriptor verification, scope, OAuth subject, validation, and dispatch share one live contract. Build a small independent `kino_labby` package whose `Kino.JS.Live` form collects bounded inputs while Elixir owns execution, cancellation, rendering, and deterministic notebook generation.

**Tech Stack:** Rust 1.97.1, Axum, rmcp, serde, SHA-256, Elixir 1.18, Req 0.5, Kino 0.19, Livebook `.livemd`

**Spec:** `docs/superpowers/specs/2026-08-23-labby-app-forge-prototype.md`

## Global Constraints

- Fix Palette subject propagation and contract TOCTOU before the Livebook client.
- V1 refuses destructive tools and never treats browser confirmation as authority.
- Least privilege is `mcp:read mcp:write` plus `gateway:<upstream>`; `lab:admin` is only an operator shortcut.
- Descriptor caps: 64 KiB per schema, 160 KiB aggregate, depth 64, description 2,048 characters.
- Client caps: redirects disabled, HTTPS except explicit loopback development, 5-second connect, 30-second total deadline, 10 MiB response, no execute retries.
- Form caps: 100 fields, 100 enum options, 512-character labels/descriptions, 64 KiB JSON fallback.
- Preview caps: 1,000 rows, 100 fields per row, depth 16, 4,096 characters per string, 1 MiB rendered JSON.
- Never expose credentials, params, schemas, bodies, raw subjects, or OAuth material through logs, browser data, errors, AppSpecs, or notebooks.
- Use a local path or approved pinned Git SHA until Hex publication is separately authorized.
- Generate docs with repository commands; never hand-edit `docs/generated/`.

---

## File map

### Labby

- `crates/labby-gateway/src/gateway/palette.rs` — bounded `CapabilityContract`, descriptor lookup, expected-hash execute request, telemetry.
- `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs` — reusable caller/subject-aware checked dispatch.
- `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs` — identity, scope, drift, bounds, concurrency tests.
- `crates/labby/src/api/services/palette.rs` — descriptor route, least-privilege caller mapping, HTTP tests.
- `crates/labby/tests/fixtures/stdio_mcp_fixture.rs` — hermetic Forge tools, delays, errors, drift, subject responses, large results, and invocation counts.
- `crates/labby-gateway/tests/fixtures/capability-contract-v1.json` — canonical cross-language descriptor/hash vectors.
- `apps/palette-tauri/src/lib/launcherCatalog.ts`, `labbyClient.ts`, and tests — carry the catalog contract hash into existing Palette execution.
- `apps/palette-tauri/src/App.tsx` — bind the selected entry/hash to execution and surface `contract_changed` by refreshing once.
- `docs/surfaces/HTTP_API.md` and generated docs — contract documentation.

### `/home/jmagar/workspace/kino_labby`

- `mix.exs` — Kino, Req, Jason.
- `lib/kino_labby/client.ex` — redacted bounded fixed-route client.
- `lib/kino_labby/schema.ex` — schema compiler and params encoder.
- `lib/kino_labby/tool_form.ex` plus `tool_form/main.js` — bounded live form.
- `lib/kino_labby/result.ex` — bounded frame rendering.
- `lib/kino_labby/app_spec.ex`, `notebook.ex`, `app.ex` — artifact and runtime.
- `lib/kino_labby/app_spec/migrations.ex` — version-dispatched pure one-step migration registry.
- `lib/kino_labby/forge.ex`, `livebooks/app_forge.livemd` — Forge.
- `test/fixtures/capability-contract-v1.json` — byte-identical copy of the Rust golden vectors.
- `test/support/fake_labby.ex`, fixtures, and focused tests — failure evidence.

---

### Task 1: Make Palette execution caller-consistent and contract-checked

**Files:**
- Modify: `crates/labby-gateway/src/gateway/palette.rs:128-346`
- Modify: `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs:71-165,580-650`
- Modify: `apps/palette-tauri/src/lib/launcherCatalog.ts`
- Modify: `apps/palette-tauri/src/lib/labbyClient.ts`
- Modify: `apps/palette-tauri/src/App.tsx`
- Test: `apps/palette-tauri/src/lib/launcherCatalog.test.ts`, `labbyClient.test.ts`
- Test: `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs:786-875`

**Interfaces:**
- Consumes: `PaletteCaller`, `UpstreamTool`, `ToolScope`, checked Code Mode dispatch, existing Tauri Palette catalog/execute client.
- Produces: `CapabilityContract::from_upstream_tool`, compact-entry `contract_hash`, required `expected_contract_hash`, caller-aware `palette_execute`, and `PaletteExecutionReceipt`.

- [ ] **Step 1: Write failing subject-isolation and TOCTOU tests**

Create subject-scoped fixture connections for Alice and Bob. Assert Alice reaches only Alice. Change the schema between preview and execute and assert:

```rust
let error = manager
    .palette_execute(
        &alice,
        PaletteExecuteRequest {
            id: "mcp:github::search_issues".into(),
            params: json!({"query": "bug"}),
            expected_contract_hash: "old-hash".into(),
        },
    )
    .await
    .expect_err("changed contract must fail closed");
assert_eq!(error.kind(), "contract_changed");
assert_eq!(upstream_call_count.load(Ordering::SeqCst), 0);
```

Test credential invalidation, reload, destructive reclassification, and cross-upstream scope denial.

- [ ] **Step 2: Run and verify failure**

```bash
cargo test -p labby-gateway palette_execute -- --nocapture
```

Expected: subject and expected-hash assertions fail against the current shared-subject path.

- [ ] **Step 3: Add one pure capability projection**

Define `CapabilityContract` with version, ID, sanitized schemas, four typed annotation booleans, authoritative destructive flag, and hash. Canonicalize through a capped writer so hashing does not allocate a second full JSON tree. Do not include description or revision in the hash.

- [ ] **Step 4: Extract caller-aware checked dispatch**

Create this shared helper and route both Code Mode and Palette through it:

```rust
async fn execute_upstream_tool_checked(
    &self,
    upstream: &str,
    tool: &str,
    params: Value,
    owner: &UpstreamRuntimeOwner,
    oauth_subject: Option<&str>,
    caller_auth: Option<PropagatedCallerAuth>,
    caller_scope: Option<PropagatedCallerUpstreamScope>,
    expected_contract_hash: &str,
) -> Result<ToolCallOutcome, ToolError>
```

Resolve, hash, validate, classify, and invoke against one coherent config/pool revision. Palette passes the real owner, subject, auth, and scope; it never calls a helper that hardcodes `SHARED_GATEWAY_OAUTH_SUBJECT`.

- [ ] **Step 5: Refuse destructive Forge-capable calls**

Non-admin Palette execution rejects authoritative `destructive == true`. Preserve desktop Palette confirmation compatibility, but run its approved call through the same checked helper. App Forge sends no confirmation field.

- [ ] **Step 6: Update the existing Tauri Palette consumer**

Add `contractHash` to the TypeScript compact entry, preserve it through search/selection, and require `executeLauncherEntry(entry, params)` to send `expectedContractHash: entry.contractHash`. On `contract_changed`, refresh the catalog once, clear the armed destructive confirmation, and require the user to select/review the current entry again. Add tests proving no execution occurs with an absent/stale hash and other request fields remain byte-compatible.

- [ ] **Step 7: Add redacted telemetry tests**

For success, hidden tool, drift, validation, OAuth, timeout, and upstream error, assert logs contain request ID, upstream, tool, safe subject fingerprint, hash, elapsed time, and kind; assert absence of token canaries, raw subject, params, schema, and result.

- [ ] **Step 8: Return and test safe execution receipts**

Add `PaletteExecutionReceipt { request_id, tool_id, contract_hash, catalog_revision, truncated }` beside the result. Build it only after checked success. Assert it identifies the exact invoked contract and contains no token, params, subject, OAuth data, or result content.

- [ ] **Step 9: Verify and commit**

```bash
cargo test -p labby-gateway palette code_mode
cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings
pnpm --filter palette-tauri test -- launcherCatalog labbyClient
git add crates/labby-gateway/src/gateway/palette.rs crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs crates/labby-gateway/src/gateway/manager/tests/code_mode.rs apps/palette-tauri/src
git commit -m "fix(palette): bind execution to caller contract"
```

### Task 2: Add bounded descriptors and least-privilege HTTP scopes

**Files:**
- Modify: `crates/labby-gateway/src/gateway/palette.rs`
- Modify: `crates/labby/src/api/services/palette.rs:34-215`
- Modify: `docs/surfaces/HTTP_API.md`
- Regenerate: `docs/generated/`
- Test: Palette gateway/API test modules

**Interfaces:**
- Consumes: `CapabilityContract`.
- Produces: `GET /v1/palette/descriptor`, `CapabilityDescriptor`, scoped `mcp:write` execution.

- [ ] **Step 1: Write failing contract, bounds, and scope tests**

Test full descriptor, auth failure, browse-only denial, scoped write success, cross-upstream denial, admin-action denial, hidden/unknown equivalence, exact and over-limit schemas, depth 65, aggregate overflow, and typed annotation canaries. Over-limit cases must return `descriptor_unsupported`, never `inputSchema: null`.

- [ ] **Step 2: Run and verify failure**

```bash
cargo test -p labby --all-features palette_descriptor -- --nocapture
```

Expected: descriptor route is absent and scoped write is denied.

- [ ] **Step 3: Implement DTO and route**

Add `CapabilityDescriptor` with version, revision, identity, description, schemas, typed annotations, destructive, and hash. Mount `.route("/descriptor", get(descriptor))`. Accept only canonical MCP IDs and reuse current auth/request identity/error envelopes.

- [ ] **Step 4: Implement least-privilege mapping**

Map `mcp:read` to browse. Map `mcp:read mcp:write` to execute only explicitly named `gateway:<name>` scopes. Preserve `lab:admin`. Prove no `gateway:*` widening, no cross-upstream call, and no Labby action execution.

- [ ] **Step 5: Define revision coherence without N+1 calls**

Return compact catalog fingerprint as `catalogRevision`. Descriptor is authoritative. On revision mismatch the client refreshes search once and retries selection once. Replace the process-global one-slot Palette cache with a caller-keyed cache capped at 32 entries, two-second TTL, and least-recently-used eviction; cache keys retain manager identity, subject, and sorted scopes. Test alternating callers cannot evict each other into repeated refresh storms, reload/OAuth refresh between search and selection, 100 search rows cause zero descriptor calls, and one selection causes exactly one.

- [ ] **Step 6: Bound and profile projection**

Use capped serialization for schemas and aggregate descriptor. Benchmark 100, 500, and 1,000 tools with 1/16/64 KiB schemas. Record p95 warm search and descriptor latency; add no persistent index unless warm search exceeds 250 ms.

- [ ] **Step 7: Add canonical cross-language golden vectors**

Create `crates/labby-gateway/tests/fixtures/capability-contract-v1.json` with reordered keys, null schemas, Unicode, integers/floats, all annotations, destructive changes, optional-property additions, required-property additions, output changes, and identity changes. Each case includes normalized input, canonical JSON, expected SHA-256, and expected additive-input compatibility. Rust regenerates and compares every value.

- [ ] **Step 8: Document, verify, commit**

```bash
just docs-generate
just docs-check
cargo test -p labby --all-features palette
cargo fmt --all -- --check
cargo clippy -p labby --all-features --all-targets -- -D warnings
git add crates/labby-gateway/src/gateway/palette.rs crates/labby-gateway/tests/fixtures/capability-contract-v1.json crates/labby/src/api/services/palette.rs docs/surfaces/HTTP_API.md docs/generated
git commit -m "feat(api): expose bounded capability descriptors"
```

### Task 3: Scaffold `kino_labby` and implement a hardened client

**Files:**
- Create: `/home/jmagar/workspace/kino_labby/mix.exs`
- Create: `/home/jmagar/workspace/kino_labby/lib/kino_labby/client.ex`
- Create: `/home/jmagar/workspace/kino_labby/test/support/fake_labby.ex`
- Create: fixtures and `test/kino_labby/client_test.exs`

**Interfaces:**
- Consumes: Task 2 routes and canonical golden vectors.
- Produces: `from_env/1`, `search/3`, `descriptor/2`, `execute/4`, `%ExecutionReceipt{}`, returning tagged tuples.

- [ ] **Step 1: Establish the repository safely**

Use `superpowers:using-git-worktrees` at execution. Confirm the target path is absent or intentionally empty before `mix new`. Record owner, license, remote decision, and contract version; do not publish or push without authorization.

- [ ] **Step 2: Pin dependencies and write happy-contract tests**

Use Elixir `~> 1.18`, Kino `~> 0.19`, Req `~> 0.5`, Jason `~> 1.4`. Test fixed routes, bearer header, request ID, descriptor decoding, expected-hash execution, and stable errors.

- [ ] **Step 3: Write transport/security failure tests**

Cover missing env, malformed URL, userinfo/query/fragment/path, non-loopback HTTP, redirects, DNS/TLS/connect, slow headers/body, timeout, reset, 401, 429, 5xx, invalid JSON/content type, truncation, and over-10-MiB body. Put canaries in token, URL, params, and upstream error; assert none leak through inspect, errors, logs, or browser data.

- [ ] **Step 4: Implement non-raising construction**

`from_env/1` returns `{:ok, client} | {:error, error}`. Permit HTTPS or explicit loopback-development HTTP. Disable redirects, redact inspect, enforce 5-second connect and 30-second total deadlines. Retry search/descriptor transport once at most; never retry execute.

- [ ] **Step 5: Implement bounded operations**

Search requires explicit two-character query. Descriptor validates all types and the 160 KiB cap. Execute requires expected hash, decodes the safe receipt, verifies receipt tool/hash/request-ID consistency, and preserves stable redacted Labby errors. Copy the canonical fixture byte-for-byte into `test/fixtures/`; ExUnit asserts its SHA-256 matches the Labby fixture and every hash/compatibility vector agrees.

- [ ] **Step 6: Verify and commit**

```bash
mix format --check-formatted
mix compile --warnings-as-errors
mix test
git add mix.exs mix.lock lib test README.md LICENSE
git commit -m "feat: add hardened Labby capability client"
```

### Task 4: Build bounded schema forms and cancellable operations

**Files:**
- Create: `lib/kino_labby/schema.ex`
- Create: `lib/kino_labby/tool_form.ex`
- Create: `lib/kino_labby/tool_form/main.js`
- Create: `lib/kino_labby/result.ex`
- Test: corresponding ExUnit files

**Interfaces:**
- Consumes: descriptor and execute client calls.
- Produces: neutral fields, `ToolForm.new/3`, monitored operations, bounded frame rendering, collapsed execution details.

- [ ] **Step 1: Write schema-boundary tests**

Cover primitives, lexical order, required/optional encoding, integer/number, min/max, enums, dates, booleans, defaults/null, unknown keys, NaN/infinity, 100/101 fields/options, length caps, malformed events, and 64 KiB fallback JSON object validation.

- [ ] **Step 2: Implement compiler and encoder**

Return fields only when every property is supported and bounded; otherwise return explicit JSON fallback. Reject over-limit schemas visibly rather than partially rendering them.

- [ ] **Step 3: Write concurrency/security component tests**

With `Kino.Test`, assert initialization excludes client/token/raw schema. Test duplicate Run, timeout, task crash, disconnect, late completion, selection change, malformed/oversized event, reconnect, and drift. Assert one upstream call and controls re-enable.

- [ ] **Step 4: Implement monitored live component**

Use `Kino.JS.Live` only for fields. Store one active operation ID/task in Elixir, cancel on termination/selection, ignore stale messages, and send expected hash every Run. Refuse destructive descriptors.

- [ ] **Step 5: Implement safe browser assets and bounded result frames**

Use DOM APIs without eval, dynamic imports, network, Markdown, or raw HTML. Emit bounded typed params only. Elixir keeps one canonical result, applies preview caps, and records sizes/timing without content. Large results expose a JSON download. Render the safe receipt in collapsed plain-text “Execution details” showing only request ID, tool ID, hash, revision, and truncation.

- [ ] **Step 6: Verify and commit**

```bash
mix test test/kino_labby/schema_test.exs test/kino_labby/tool_form_test.exs test/kino_labby/result_test.exs
mix format --check-formatted
mix compile --warnings-as-errors
git add lib/kino_labby test/kino_labby
git commit -m "feat: add bounded MCP tool forms"
```

### Task 5: Add AppSpec and deterministic notebook generation

**Files:**
- Create: `lib/kino_labby/app_spec.ex`
- Create: `lib/kino_labby/app_spec/migrations.ex`
- Create: `lib/kino_labby/notebook.ex`
- Create: `lib/kino_labby/app.ex`
- Test: corresponding ExUnit files

**Interfaces:**
- Consumes: Client, ToolForm, Result, descriptor hash and normalized contract snapshot.
- Produces: validated AppSpec, `decode/1`, `compatible?/2`, one-step migration registry, deterministic `.livemd`.

- [ ] **Step 1: Write validation, adversarial, and version tests**

Reject blank/long titles, non-MCP IDs, invalid hashes, destructive descriptors, unknown compatibility modes, unsupported versions/renderers. Snapshot fences, interpolation, quotes, newlines, HTML, dangerous URLs, Unicode. Assert artifact excludes token, URL, params, result, and JavaScript; its bounded normalized contract snapshot contains only public descriptor fields.

- [ ] **Step 2: Implement fixed serialization**

Serialize validated fields and bounded normalized contract snapshot into a fixed template with Elixir escaping, LF, one trailing newline, and local-path or approved pinned-SHA dependency. Do not emit `~> 0.1`.

- [ ] **Step 3: Implement exact and additive-input compatibility**

`:exact` accepts only equal hashes. `:additive_input` requires identical version, ID, output schema, annotations, destructive flag, existing input properties, and `required`; it permits only new optional top-level input properties. Test every golden vector plus nested/combinator/additionalProperties changes, which fail unless byte-identical.

- [ ] **Step 4: Add versioned decoding and migration infrastructure**

`AppSpec.decode/1` dispatches integer versions. `Migrations.migrate/1` applies registered pure transforms exactly one version at a time and rejects unknown, skipped, cycling, or non-incrementing migrations. Register no fake product v2. Use a private test-only 7-to-8 transform to prove sequencing and golden before/after behavior while production version 2 remains unsupported.

- [ ] **Step 5: Test runtime failures**

Cover missing env/tool, destructive tool, render/run drift, timeout, 401, malformed response, oversized result. Missing configuration returns plain setup guidance; run drift makes zero upstream calls.

- [ ] **Step 6: Implement App composition**

Create non-raising client, fetch descriptor, evaluate explicit compatibility against the stored normalized contract, and render only a compatible non-destructive contract. Exact uses the stored hash; accepted additive input uses the current live hash on every call and shows the compatibility decision in execution details.

- [ ] **Step 7: Verify and commit**

```bash
mix test test/kino_labby/app_spec_test.exs test/kino_labby/notebook_test.exs test/kino_labby/app_test.exs
mix test
mix format --check-formatted
mix compile --warnings-as-errors
git add lib/kino_labby test/kino_labby test/fixtures/capability-contract-v1.json
git commit -m "feat: compile safe Livebook app specs"
```

### Task 6: Build Forge and prove the vertical slice

**Files:**
- Create: `lib/kino_labby/forge.ex`
- Create: `livebooks/app_forge.livemd`
- Create: `test/kino_labby/forge_test.exs`
- Modify: `README.md`

**Interfaces:**
- Consumes: Tasks 3-5.
- Produces: explicit-search Forge and downloadable one-tool notebook.

- [ ] **Step 1: Write Forge race/request-count tests**

Test explicit search, two-character minimum, request IDs, stale rejection, one descriptor per selection, reselection deduplication by ID/hash, revision refresh-once, selection cancellation, and release of obsolete result/download state.

- [ ] **Step 2: Implement derived state**

Keep current query/results, selected row, descriptor, active operation, bounded preview, title, renderer, and one notebook binary. Preview is encouraged but not required; generation requires a current non-destructive descriptor.

- [ ] **Step 3: Implement download and entrypoint**

Generate safe lowercase ASCII filename with single hyphens and `labby-app.livemd` fallback. Use Elixir-owned download bytes. Development notebook installs the local path and calls `Forge.new()`.

- [ ] **Step 4: Run package gates**

```bash
mix test
mix format --check-formatted
mix compile --warnings-as-errors
```

Do not run Hex build as a portability claim.

- [ ] **Step 5: Extend the hermetic stdio MCP fixture**

Extend `crates/labby/tests/fixtures/stdio_mcp_fixture.rs` with deterministic Forge tools selected by fixture arguments: safe primitives, unsupported nested schema, destructive classification, delay, structured error, bounded-large result, subject-specific response, and mutable schema revision. Add an invocation counter readable through `fixture://forge-status`. Keep it offline and credential-free.

- [ ] **Step 6: Run fresh-browser functional and scale smoke**

Using the hermetic fixture and `testing:web-app-testing`, verify connection, search, one descriptor, form, safe execution, receipt, download, reopen, exact drift rejection, accepted optional-input addition, rejected required/output/destructive changes, and unknown AppSpec version. Exercise 100 rows, 100 fields, long enums/labels, 1,000 result rows, rapid selection, reconnect, Alice/Bob responses, delayed/error tools, and mobile. Capture invocation counts, timing, bytes, DOM nodes, console/page/network failures, and unexpected external requests. GitHub is an optional manual realism smoke only.

- [ ] **Step 7: Commit**

```bash
git -C /home/jmagar/workspace/kino_labby add lib/kino_labby/forge.ex livebooks/app_forge.livemd test/kino_labby/forge_test.exs README.md
git -C /home/jmagar/workspace/kino_labby commit -m "feat: add one-tool Labby App Forge"
git -C /home/jmagar/workspace/labby add crates/labby/tests/fixtures/stdio_mcp_fixture.rs
git -C /home/jmagar/workspace/labby commit -m "test: extend hermetic MCP fixture for App Forge"
```

### Task 7: Cross-repository closeout

**Files:**
- Verify only; modify only failures caused by Tasks 1-6.

**Interfaces:**
- Consumes: final Labby and `kino_labby` heads.
- Produces: exact-head automated/fresh-client evidence.

- [ ] **Step 1: Verify clean intended state and exact heads**

```bash
git -C /home/jmagar/workspace/labby status --short --branch
git -C /home/jmagar/workspace/kino_labby status --short --branch
git -C /home/jmagar/workspace/labby log -1 --oneline
git -C /home/jmagar/workspace/kino_labby log -1 --oneline
```

- [ ] **Step 2: Run Labby gates**

```bash
cd /home/jmagar/workspace/labby
just check
just test
just lint
just docs-check
```

- [ ] **Step 3: Run Elixir gates**

```bash
cd /home/jmagar/workspace/kino_labby
mix deps.unlock --check-unused
mix format --check-formatted
mix compile --warnings-as-errors
mix test
```

- [ ] **Step 4: Repeat exact-head proof**

Record both SHAs, URL without credentials, scope class, subject fingerprint, tool ID/hash/revision, descriptor/upstream call counts, renderer/truncation, filename, timing/bytes/DOM, and console/page/network failures. Re-prove Alice/Bob isolation and zero calls on drift.

- [ ] **Step 5: Produce handoff**

Report implemented/deferred scope, exact heads, tests, browser evidence, limits, and unverified publication/deployment. Do not claim Hex or production release.

---

## Engineering Review Synthesis

### Architecture

- Strength: Labby/BEAM ownership split and Palette reuse are sound.
- Applied: caller-aware dispatch, atomic expected hash, authoritative destructive state, revision behavior, and explicit Kino frame/task composition.

### Simplicity

- Applied: no parallel Forge catalog, Markdown, Hex packaging, or mandatory preview state.
- Retained intentionally: detailed descriptor and `Kino.JS.Live` because full schemas/annotations/hash and a live schema form are explicit requirements.

### Security

- Applied: subject isolation, least privilege, no destructive V1, typed metadata, URL/redirect controls, redaction, plain rendering, server-side contract binding.

### Performance

- Applied: explicit search, one descriptor per selection, capped hashing, size limits, monitored single-flight operations, stale rejection, bounded rendering, scale smoke.

### Failure Modes

| Codepath | Failure | Rescued | Tested | User sees | Logged |
| --- | --- | --- | --- | --- | --- |
| checked execute | wrong subject or drift | yes | yes | stable error | redacted event |
| descriptor | hidden, stale, malformed, large | yes | yes | retry/unavailable | kind/revision |
| client | config, redirect, timeout, malformed body | yes | yes | setup/retry | route/kind/request ID |
| form | unsupported, duplicate, stale, disconnect | yes | yes | fallback/error | operation outcome |
| result | large or hostile content | yes | yes | preview/download | sizes only |
| notebook | injection, missing tool, drift | yes | yes | fail-closed | correlated |
| Forge | request storm, late selection, retained state | yes | yes | current state | operation ID |

No critical gap remains where failure is unrescued, untested, silent, and unlogged.

### Not in Scope

- Destructive confirmation — needs server-held one-shot confirmation/elicitation.
- Persistent catalog index — profile first.
- Hex publication — needs release authorization.
- Markdown/upstream MCP Apps — needs a separate threat model.
- Multi-tool graphs/AI AppSpecs — depend on one-tool stability.

### Recommendation Checklist

- [x] 1. Preserve caller subject/scope through final dispatch.
- [x] 2. Enforce expected hash atomically server-side.
- [x] 3. Include authoritative destructive state and exclude it from V1.
- [x] 4. Add least-privilege browse/execute scopes.
- [x] 5. Use one bounded capability projection.
- [x] 6. Define revision behavior and avoid descriptor N+1s.
- [x] 7. Make client construction non-raising and bounded.
- [x] 8. Harden URL, redirect, error, log, and rendering boundaries.
- [x] 9. Define deterministic schema coercion and limits.
- [x] 10. Use monitored single-flight Kino tasks.
- [x] 11. Bound descriptor, result, browser, and retained state.
- [x] 12. Defer Hex and use local path or pinned SHA.
- [x] 13. Add exact-head identity, drift, failure, and scale proof.
- [x] 14. Simplify renderers/state without dropping explicit requirements.
- [x] 15. Add byte-identical cross-language canonical hash golden vectors.
- [x] 16. Make a hermetic stdio MCP upstream the automated acceptance target.
- [x] 17. Return and render safe correlated execution receipts.
- [x] 18. Add exact and narrowly defined additive-input compatibility modes.
- [x] 19. Add version-dispatched AppSpec decoding and one-step migration infrastructure.
- [SKIPPED: full descriptors are an explicit prototype requirement] 20. Remove the descriptor endpoint and use only the legacy input-schema fingerprint.
- [SKIPPED: a Kino.JS.Live schema form is an explicit prototype requirement] 21. Replace the custom live form entirely with native controls.

**Applied:** 19. **Skipped with reasons:** 2. **Critical gaps remaining in the plan:** 0.


