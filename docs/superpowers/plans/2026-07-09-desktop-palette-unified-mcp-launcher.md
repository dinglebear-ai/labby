# Desktop Palette Unified MCP Launcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Tauri desktop palette into a unified launcher that searches Labby actions and connected upstream MCP tools, validates tool input with schemas, and executes through the existing gateway backend.

**Architecture:** Add a palette-facing backend contract over existing Labby catalog and gateway Code Mode/upstream internals. The catalog path may use cached/non-cold discovery, but execution must re-resolve the live upstream tool, re-check exposure/auth/destructive policy, validate current schema, and call the existing upstream pool. The Tauri renderer stays network-free: renderer -> Tauri command -> fixed Labby HTTP route.

**Tech Stack:** Rust 2024, Axum, rmcp, labby-gateway, labby-codemode schema validator, Tauri v2, React 19, TypeScript, Vitest, Ajv for renderer JSON Schema validation, Zod-style catalog decoding if already available or a small local decoder if not.

## Global Constraints

- Epic: `lab-z9xbx` with child beads `lab-z9xbx.1` through `lab-z9xbx.4`.
- First milestone is unified launcher/search/execution; full gateway management parity is future scope.
- Do not make Tauri or the renderer speak MCP directly.
- Renderer must not receive or store bearer/OAuth tokens.
- Search/catalog results are not authorization; execution must re-check live exposure, auth/scope, destructive policy, and schema.
- `/v1/palette/*` must mount only when API auth is configured and a gateway manager is present, matching the safety posture of `/v1/gateway`.
- Palette backend must define `AuthContext + request_id -> PaletteCaller -> CodeModeCaller + ToolScope + UpstreamRuntimeOwner + oauth_subject`; fail closed when mapping is absent.
- Catalog discovery must not cold-connect every upstream on palette open.
- Schema exposure is auth-sensitive and must be scope-gated.
- Palette schema projection must redact `default`, `examples`, secret-looking enum values, and overlarge schema fragments before schemas reach the renderer. Full schema remains server-side for authoritative validation.
- Preserve existing `/v1/catalog`, `/v1/gateway`, and product action dispatch behavior.
- Use `CLAUDE.md` as agent-memory source of truth; `AGENTS.md` and `GEMINI.md` should remain symlinks if present.
- Default verification should include all-features Rust checks where feasible.

---

## File Structure

- Create `crates/labby-gateway/src/gateway/palette.rs`: gateway-owned launcher DTOs, caller/scope adapter types, scope-aware catalog cache helpers, redacted schema projection, and direct upstream tool execution.
- Modify `crates/labby-gateway/src/gateway.rs`: expose the new `palette` module.
- Modify `crates/labby-gateway/src/gateway/code_mode.rs` or `code_mode/search.rs`: expose a narrow helper for building sanitized tool descriptors without widening the unscoped render cache unsafely.
- Modify `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`: extract the current direct upstream call block into a shared helper so palette execution and Code Mode use the same validation/call semantics.
- Create `crates/labby/src/api/services/palette.rs`: fixed HTTP route for launcher catalog and execution.
- Modify `crates/labby/src/api/services.rs` and `crates/labby/src/api/router.rs`: mount the palette route only when gateway manager/auth prerequisites are satisfied.
- Modify or add backend tests near `crates/labby/src/api/services/palette.rs` and `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`.
- Create `apps/palette-tauri/src/lib/launcherCatalog.ts`: discriminated union, catalog fetch/normalization, search helpers, and stale catalog guards.
- Create `apps/palette-tauri/src/lib/launcherValidation.ts`: Ajv-backed JSON Schema validation, schema-fingerprint memoization, and conservative `{}` fallback params.
- Modify `apps/palette-tauri/src/lib/labbyClient.ts`: typed launcher catalog/execute client wrappers.
- Modify `apps/palette-tauri/src/lib/invoke.ts`: browser fallback stubs for new Tauri commands.
- Modify `apps/palette-tauri/src-tauri/src/labby_bridge.rs` and `src-tauri/src/main.rs`: add fixed Tauri commands for launcher catalog and execution.
- Modify `apps/palette-tauri/src/App.tsx`: consume launcher entries and add request-id stale result guard.
- Modify `apps/palette-tauri/src/components/palette/ActionList.tsx` and `ActionIcon.tsx`: show kind/source/upstream labels.
- Modify `apps/palette-tauri/package.json` and `pnpm-lock.yaml`: add `ajv` unless the package already appears in the lockfile.
- Modify `apps/palette-tauri/README.md` and `apps/palette-tauri/CLAUDE.md`: remove stale Axon instructions.

---

### Task 1: Backend Palette Catalog and Direct Tool Execution

**Files:**
- Create: `crates/labby-gateway/src/gateway/palette.rs`
- Modify: `crates/labby-gateway/src/gateway.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode/search.rs`
- Modify: `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`
- Create: `crates/labby/src/api/services/palette.rs`
- Modify: `crates/labby/src/api/services.rs`
- Modify: `crates/labby/src/api/router.rs`
- Test: `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`
- Test: `crates/labby/src/api/services/palette.rs`

**Interfaces:**
- Produces: `LauncherCatalogView { entries: Vec<LauncherEntryView>, fingerprint: String }`
- Produces: `LauncherEntryView` with `kind`, `id`, `label`, `description`, `source`, `destructive`, redacted `input_schema`, schema fingerprint, and execution target fields.
- Produces: fixed API route, preferred shape `GET /v1/palette/catalog` and `POST /v1/palette/execute`.
- Consumes: existing `GatewayManager`, `CodeModeCaller`, `ToolScope`, `validate_code_mode_params_against_schema`, and upstream pool call path.
- Produces: `PaletteCaller` adapter mapping `AuthContext` into Code Mode caller/scope/owner/OAuth subject without introducing a `labby-gateway -> labby` dependency.

- [ ] **Step 1: Write failing gateway catalog projection test**

Add a test that installs fixture tools for one upstream and asserts the palette catalog returns an MCP tool entry with a stable `mcp:<upstream>::<tool>` id, sanitized schema, source label, and no cold-connect requirement.

Also add negative tests:
- `/v1/palette/catalog` is not mounted or rejects when API auth is not configured.
- admin catalog cache warm-up cannot make non-admin catalog include admin-only/hidden schema data.
- palette catalog fingerprint changes when visible tool ids or redacted schema fingerprints change.

Run: `cargo test -p labby-gateway palette_catalog`
Expected: FAIL because `palette` module/API does not exist.

- [ ] **Step 2: Implement minimal launcher DTOs and catalog projection**

Create `gateway/palette.rs` with DTOs:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LauncherCatalogView {
    pub fingerprint: String,
    pub entries: Vec<LauncherEntryView>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LauncherEntryView {
    LabbyAction(LabbyActionLauncherEntry),
    McpTool(McpToolLauncherEntry),
}
```

Use existing catalog/action metadata for Labby actions and Code Mode/Upstream tool metadata for MCP tools. Do not use the unscoped Code Mode render cache for scoped callers unless the fingerprint includes scope.

Implement palette catalog projection directly from scoped healthy/exposed `UpstreamTool` records, using `sanitize_tool_text` and a new palette schema projection. The scope-aware cache key must include caller class, OAuth subject/scope bucket, allowed upstream set, schema exposure policy, visible tool ids, and schema fingerprints. Add tracing fields: `entry_count`, `schema_bytes`, `fingerprint`, `cache_hit`, and `elapsed_ms`.

- [ ] **Step 3: Write failing direct execution tests**

Cover:
- unknown launcher id -> stable `not_found`
- hidden/priority-zero tool id -> rejected at execution even if caller supplies id
- invalid params -> `missing_param` or `invalid_param`
- valid fixture tool -> upstream result returned
- destructive current-live tool without `confirmDestructive: true` -> `confirmation_required`
- no-auth or read-only caller outside scope -> `forbidden`

Run: `cargo test -p labby-gateway palette_execute`
Expected: FAIL until execution helper exists.

- [ ] **Step 4: Implement direct execution helper**

Add a gateway manager helper that:
1. Parses only `mcp:<upstream>::<tool>` ids for tool execution.
2. Resolves the current live tool with the same policy as Code Mode.
3. Blocks destructive tools unless the caller is allowed to execute them and `/v1/palette/execute` includes fresh `confirmDestructive: true`.
4. Calls `validate_code_mode_params_against_schema`.
5. Dispatches through a single shared `GatewayManager::execute_upstream_tool` helper also used by Code Mode, preserving structured content, text JSON fallback, UI metadata, timeout, response-size cap, and error normalization.

Do not generate JavaScript snippets.

- [ ] **Step 5: Add fixed HTTP route**

Add `api/services/palette.rs` with:
- `GET /v1/palette/catalog`
- `POST /v1/palette/execute`

The route must use the authenticated API context, gateway manager, and existing error envelope mapping. Unknown routes and missing gateway manager must fail visibly.

Mount this route only when API auth is configured and a gateway manager is present. Add router tests for unauthenticated/no-auth-configured behavior.

- [ ] **Step 6: Run backend tests**

Run:
```bash
cargo test -p labby-gateway palette
cargo test -p labby --all-features palette
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/labby-gateway/src/gateway* crates/labby/src/api
git commit -m "feat(palette): add gateway-backed launcher catalog"
```

---

### Task 2: Tauri Bridge and Typed Client

**Files:**
- Modify: `apps/palette-tauri/src-tauri/src/labby_bridge.rs`
- Modify: `apps/palette-tauri/src-tauri/src/main.rs`
- Modify: `apps/palette-tauri/src/lib/labbyClient.ts`
- Modify: `apps/palette-tauri/src/lib/invoke.ts`
- Test: `apps/palette-tauri/src/lib/labbyClient.test.ts`

**Interfaces:**
- Consumes: `GET /v1/palette/catalog` and `POST /v1/palette/execute`.
- Produces: Tauri commands `fetch_launcher_catalog` and `execute_launcher_entry`.
- Produces: TypeScript functions `fetchLauncherCatalog(etag?)` and `executeLauncherEntry(id, params, options?)`.

- [ ] **Step 1: Write failing TypeScript wrapper tests**

Add tests that assert:
- `fetchLauncherCatalog` returns decoded entries.
- `executeLauncherEntry` posts `{ id, params }`.
- Browser fallback supports both new commands.
- Browser fallback execution returns `unsupported_surface`, not success.
- HTTP errors return the stable result payload rather than throwing.

Run: `pnpm --filter palette-tauri test -- labbyClient`
Expected: FAIL.

- [ ] **Step 2: Add Tauri commands**

In Rust bridge, add fixed commands:

```rust
#[tauri::command]
pub(crate) async fn fetch_launcher_catalog(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    etag: Option<String>,
) -> Result<LabbyHttpResult, String> {
    // Build fixed `{server}/v1/palette/catalog` request and send with reauth.
}

#[tauri::command]
pub(crate) async fn execute_launcher_entry(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, crate::oauth::OauthState>,
    request: LauncherExecuteRequest,
) -> Result<LabbyHttpResult, String> {
    // Build fixed `{server}/v1/palette/execute` request and send with reauth.
}
```

Both must use `send_with_reauth`. Neither may accept arbitrary URL/path fragments from renderer input.

Add Rust-side DTO validation before sending:
- `id` length <= 512 bytes
- id matches `mcp:<upstream>::<tool>` or `labby:<service>::<action>`
- `params` is a JSON object
- serialized `params` size <= 256 KiB
- nesting depth <= 32
- `confirmDestructive` is a boolean when present

- [ ] **Step 3: Register commands**

Register the new commands in `src-tauri/src/main.rs` alongside existing palette bridge commands.

- [ ] **Step 4: Add typed client wrappers**

In `labbyClient.ts`, add typed functions and keep existing `fetchCatalog`/`dispatchAction` intact until Task 3 migration is complete.

- [ ] **Step 5: Update browser fallback**

In `invoke.ts`, add stubs:
- `fetch_launcher_catalog` -> empty catalog
- `execute_launcher_entry` -> `{ ok: false, status: 501, payload: { kind: "unsupported_surface", message: "Launcher execution is only available in the desktop app" } }`

- [ ] **Step 6: Run bridge/client tests**

Run:
```bash
pnpm --filter palette-tauri test -- labbyClient
cargo test --manifest-path apps/palette-tauri/src-tauri/Cargo.toml
```

Expected: PASS or document if the Tauri crate is not independently testable.

- [ ] **Step 7: Commit**

```bash
git add apps/palette-tauri/src-tauri/src apps/palette-tauri/src/lib
git commit -m "feat(palette): add launcher bridge client"
```

---

### Task 3: Renderer Unified Entry Model and Schema Validation

**Files:**
- Create: `apps/palette-tauri/src/lib/launcherValidation.ts`
- Create: `apps/palette-tauri/src/lib/launcherCatalog.ts`
- Modify: `apps/palette-tauri/src/App.tsx`
- Modify: `apps/palette-tauri/src/components/palette/ActionList.tsx`
- Modify: `apps/palette-tauri/src/components/palette/ActionIcon.tsx`
- Modify: `apps/palette-tauri/src/lib/runState.ts`
- Modify: `apps/palette-tauri/package.json`
- Modify: `apps/palette-tauri/pnpm-lock.yaml`
- Test: `apps/palette-tauri/src/lib/launcherCatalog.test.ts`
- Test: `apps/palette-tauri/src/lib/launcherValidation.test.ts`
- Test: `apps/palette-tauri/src/App.test.tsx` if an app-level test pattern exists.

**Interfaces:**
- Consumes: `fetchLauncherCatalog` and `executeLauncherEntry`.
- Produces: `LauncherEntry` discriminated union with `kind: "labby_action" | "mcp_tool"`.
- Produces: `launcherEntryMatches(entry, query)` from `launcherCatalog.ts`.
- Produces: `validateLauncherParams(entry, params)`.

- [ ] **Step 1: Write failing entry/search tests**

Test:
- Labby action and MCP tool entries normalize to stable ids.
- Search matches action/tool name, upstream/source, description, category/kind.
- Duplicate visible names remain distinct by id.

Run: `pnpm --filter palette-tauri test -- launcherCatalog`
Expected: FAIL.

- [ ] **Step 2: Implement `launcherCatalog.ts`**

Implement a discriminated union and search helper. Precompute a `searchText` field per entry so filtering remains cheap with large catalogs.

- [ ] **Step 3: Write failing schema validation tests**

Test:
- no schema -> valid JSON object
- required field missing -> invalid message
- wrong primitive type -> invalid message
- validators are memoized by `entry.id + schemaFingerprint`, not just entry id
- unsupported schema keywords do not hide backend errors; frontend reports best-effort validation only

Run: `pnpm --filter palette-tauri test -- launcherValidation`
Expected: FAIL.

- [ ] **Step 4: Add Ajv validation**

Add `ajv` to `apps/palette-tauri/package.json` when it is absent from the lockfile. Compile schemas lazily and memoize by `entry.id + schemaFingerprint`. Return concise user-facing validation messages while preserving backend validation as final authority.

Do not generate sample params by walking arbitrary schemas in v1. Use `{}` for unknown schemas and only surface explicit defaults/examples if they survived backend redaction.

- [ ] **Step 5: Replace catalog hook**

Add `useLauncherCatalog` that fetches unified entries and preserves the current active-flag stale write guard. Keep old `useActionCatalog` only if tests or compatibility still need it.

- [ ] **Step 6: Refactor `App.tsx` to launcher entries**

Replace `PaletteAction` state with `LauncherEntry`. Preserve:
- browse and argument modes
- destructive double-enter confirmation
- result view
- retry
- keyboard navigation

Add request-id or abort-based stale result guard so an old tool result cannot overwrite a newer run state. Preserve the active entry by id across catalog refresh; do not reset argument mode unless the active id disappeared.

Show long-running execution as a visible connecting/running state and map upstream timeout/connect errors to retryable messages without implying cancellation stopped server-side effects.

- [ ] **Step 7: Protect secret params**

Implement `redactLauncherParams(value)` and use it before `lastParamsRef`, retry state, any debug/result metadata, copied result text where params are included, and future persisted history. Redact nested object keys and arrays; treat keys containing `token`, `secret`, `password`, `apiKey`, `authorization`, or `key` as sensitive.

- [ ] **Step 8: Update rows/icons**

Show source/upstream on MCP tool rows and keep Labby action labels readable. Do not let text overflow compact palette rows.

- [ ] **Step 9: Run frontend tests**

Run:
```bash
pnpm --filter palette-tauri test
pnpm --filter palette-tauri build
```

Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add apps/palette-tauri/src apps/palette-tauri/package.json apps/palette-tauri/pnpm-lock.yaml
git commit -m "feat(palette): unify launcher entries"
```

---

### Task 4: Documentation and Verification

**Files:**
- Modify: `apps/palette-tauri/README.md`
- Modify: `apps/palette-tauri/CLAUDE.md`
- Verify symlink only: `apps/palette-tauri/AGENTS.md`
- Verify symlink only: `apps/palette-tauri/GEMINI.md`
- Modify: `docs/contracts/palette-launcher.md` if the backend route needs a stable API contract beyond the app README.

**Interfaces:**
- Consumes: final backend route names and payload shapes from Tasks 1-3.
- Produces: current Labby palette documentation and agent instructions.

- [ ] **Step 1: Refresh README**

Replace Axon references with Labby palette behavior:
- Labby server URL/auth
- unified launcher catalog
- Tauri bridge security boundary
- JSON/schema validation MVP
- future gateway management parity

- [ ] **Step 2: Refresh nested `CLAUDE.md`**

Describe current files and rules:
- `labbyClient.ts`
- `launcherCatalog.ts`
- `launcherValidation.ts`
- `labby_bridge.rs`
- no direct renderer networking
- use Labby backend contracts, not Axon/OpenAPI generated clients

Do not edit `AGENTS.md` or `GEMINI.md` directly if they are symlinks.

- [ ] **Step 3: Run full verification**

Run:
```bash
pnpm --filter palette-tauri test
pnpm --filter palette-tauri build
cargo test -p labby-gateway palette
cargo test -p labby --all-features palette
just check
```

If a command is blocked by missing dependencies or unrelated pre-existing failures, capture exact output and fix if in touched scope.

- [ ] **Step 4: Final commit**

```bash
git add apps/palette-tauri/README.md apps/palette-tauri/CLAUDE.md docs
git commit -m "docs(palette): document unified launcher"
```

---

## Review Checklist

- [ ] `/v1/palette/*` is mounted only with API auth configured and gateway manager present.
- [ ] Palette caller/scope mapping is explicit and tested for admin, read-only, no-auth, and restricted scope.
- [ ] Backend catalog does not cold-connect upstreams on palette open.
- [ ] Backend catalog cache key is scope-aware and schema-exposure-aware.
- [ ] Backend execution re-resolves live tool and does not trust cached ids.
- [ ] Schema exposure is scope-gated and hidden tools do not leak.
- [ ] Redacted schema projection removes defaults/examples/secret-looking enum values before renderer exposure.
- [ ] Destructive upstream tools fail closed and require palette-specific `confirmDestructive` plus UI confirmation where exposed.
- [ ] Renderer uses fixed Tauri commands, not direct HTTP/MCP.
- [ ] Renderer has stale-result guard for long-running upstream calls.
- [ ] Secret-looking params are not retained for retry/history.
- [ ] Docs no longer mention Axon current files/env vars.
