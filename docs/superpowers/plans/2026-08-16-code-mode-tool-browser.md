# Code Mode Tool Browser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a root-administrator-only `/tools` page that lexically searches and describes the live caller-visible upstream-tool catalog without executing JavaScript or invoking tools.

**Architecture:** After `lab-837l6.2` lands and a measurement gate justifies native discovery, add neutral lexical scoring and visibility-safe target resolution to `labby-codemode`. A narrow GatewayManager adapter acquires one live render for an authenticated root administrator, while API-private handlers enforce auth and response caps; the static Web UI uses abortable component-local requests cleared by a monotonic authentication epoch.

**Tech Stack:** Rust 1.97.1, Tokio, Serde, Axum 0.8, `labby-codemode`, `labby-gateway`, Next.js 16 static export, React 19, TypeScript 5.7, Aurora/shadcn components, Node test runner, Playwright.

**Spec:** `docs/superpowers/specs/2026-08-16-code-mode-tool-browser.md`

## Global Constraints

- Do not begin implementation until `lab-837l6.2` is closed and its required cache/safety code is present.
- Do not reactivate `lab-837l6.3` until Task 1 records a passing measurement decision.
- V1 is API-private, Web-only, root `lab:admin`, lexical-only, and upstream-tool-only.
- No shared `ActionSpec`, MCP/CLI surface, broker API, semantic/TEI hook, snippet support, command palette, or execution control.
- Live dispatch remains authoritative; discovery never grants execution.
- Filter before scoring or resolving; hidden and unknown targets remain indistinguishable.
- Caller authority is created by the API handler and never deserialized.
- Reuse the render owner, safety DTO, identity model, and single-flight delivered by `lab-837l6.2`.
- Search/describe never call an upstream tool.
- Render all upstream text as text, never HTML.
- Generated docs are generator-owned.
- Default verification is workspace all-features.

---

## File Structure

### New files

- `crates/labby-codemode/src/discovery.rs` — neutral lexical scoring, bounded response DTOs, and target resolution.
- `crates/labby-codemode/src/tests_discovery.rs` — fixture-driven lexical/security tests.
- `crates/labby-codemode/tests/fixtures/discovery-v1.json` — deterministic activation/performance fixture.
- `crates/labby-gateway/src/gateway/manager/code_mode_discovery.rs` — one-render root-admin orchestration and projection caps.
- `apps/gateway-admin/components/tools/tool-browser.tsx` — search, results, states, and detail drawer.
- `apps/gateway-admin/components/tools/tool-browser.test.tsx` — component and race tests.
- `apps/gateway-admin/app/(admin)/tools/page.tsx` — route shell.

### Modified files

- `crates/labby-codemode/src/lib.rs`, `types.rs`, `preamble.rs` — exports, `CodeModeSurface::Api`, and shared fixture tests.
- `crates/labby-gateway/src/gateway/manager.rs`, `manager/core.rs`, `manager/tests/inspection.rs` — manager module and tests.
- `crates/labby/src/api/services/gateway.rs` — API-private search/describe handlers and admin gate.
- `crates/labby/src/api/services/gateway/tests.rs` if the existing inline test module must be split to remain reviewable.
- `apps/gateway-admin/lib/api/gateway-config.ts`, `gateway-client.ts` — private endpoint URLs and typed fetch functions.
- `apps/gateway-admin/lib/auth/session-store.ts` and existing auth tests — monotonic `authEpoch`.
- `apps/gateway-admin/components/app-sidebar.tsx` and test — Tools navigation.
- `docs/dev/CODE_MODE.md` — native browser-discovery contract.

---

### Task 1: Prove the Activation Gates

**Files:**
- Create: `crates/labby-codemode/tests/fixtures/discovery-v1.json`
- Create: `crates/labby-gateway/src/gateway/manager/tests/discovery_measurement.rs`
- Modify: `docs/superpowers/specs/2026-08-16-code-mode-tool-browser.md` only to record measured values, without weakening its thresholds.

**Interfaces:**
- Consumes: completed `lab-837l6.2` render/safety/single-flight implementation.
- Produces: a checked-in measurement report and explicit go/no-go decision for `lab-837l6.3`.

- [ ] **Step 1: Verify the prerequisite mechanically**

Run:

```bash
bd show lab-837l6.2 --json | jq -e '.[0].status == "closed"'
rg -n "single.flight|singleflight|render.*identity|embedding.*identity" crates/labby-gateway/src/gateway/code_mode crates/labby-gateway/src/gateway/manager
```

Expected: the bead-status command exits zero and the code search locates the landed render and embedding identity/single-flight implementation. If either check fails, stop; leave `.3` deferred.

- [ ] **Step 2: Write the failing deterministic measurement test**

Build one 4,000-tool fixture and compare:

1. the least-expensive safe Web alternative that starts a Code Mode run only to execute `codemode.search`/`describe`;
2. one direct manager render acquisition followed by a placeholder lexical projection.

Record structural counters, not wall-clock assertions:

```rust
assert_eq!(native.tool_calls, 0);
assert_eq!(native.runner_starts, 0);
assert_eq!(native.render_acquisitions, 1);
assert!(native.serialized_response_bytes <= 256 * 1024);
assert!(javascript.runner_starts >= 1);
```

The report must include render acquisitions, runner starts, DTS generations, full-catalog serialization bytes, and response bytes for both paths.

- [ ] **Step 3: Run and verify the test fails before the native projection exists**

```bash
cargo nextest run -p labby-gateway discovery_native_activation_measurement
```

Expected: FAIL because the direct native projection has not been implemented.

- [ ] **Step 4: Implement only the measurement harness seam**

Add test-only counters around the isolated fixture/manager. Do not add production search APIs in this task. Use barriers and counters; no TEI, external upstream, sleep, or wall-clock pass/fail assertion.

- [ ] **Step 5: Record and apply the decision**

Reactivate only if native discovery eliminates the runner start or another material full-catalog operation while preserving the API auth boundary:

```bash
bd update lab-837l6.3 --defer "" --status open
bd comments add lab-837l6.3 "DECISION: Activation gates passed. lab-837l6.2 is closed; the 4,000-tool measurement shows native Web discovery removes the recorded runner/catalog work while preserving API-only root-admin authority."
```

If it does not, add a `DECISION` comment with the measured values and stop the plan with `.3` still deferred.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/tests/fixtures/discovery-v1.json crates/labby-gateway/src/gateway/manager/tests/discovery_measurement.rs docs/superpowers/specs/2026-08-16-code-mode-tool-browser.md
git commit -m "test(codemode): measure native tool browser trigger"
```

---

### Task 2: Add Neutral Bounded Lexical Discovery

**Files:**
- Create: `crates/labby-codemode/src/discovery.rs`
- Create: `crates/labby-codemode/src/tests_discovery.rs`
- Modify: `crates/labby-codemode/src/lib.rs`
- Modify: `crates/labby-codemode/src/types.rs`
- Modify: `crates/labby-codemode/src/preamble.rs`

**Interfaces:**
- Consumes: `ToolDescriptor`, `ToolScope`, `discovery_entry_visible`, and `CodeModeSafetyFacts` from `.2`.
- Produces: `search_visible_tools`, `describe_visible_tool`, `CodeModeSearchResponse`, and `CodeModeDescribeResponse`.

- [ ] **Step 1: Write failing lexical and disclosure tests**

```rust
#[test]
fn broad_search_owns_only_the_final_fifty_results() {
    let fixture = fixture_tools(4_000);
    let response = search_visible_tools(&fixture, &ToolScope::default(), "tool", 50).unwrap();
    assert_eq!(response.results.len(), 50);
    assert_eq!(response.total, 4_000);
    assert!(serde_json::to_vec(&response).unwrap().len() <= SEARCH_RESPONSE_MAX_BYTES);
    assert!(!serde_json::to_string(&response).unwrap().contains("typescript"));
}

#[test]
fn hidden_and_random_describe_are_publicly_identical() {
    let fixture = scoped_fixture();
    let hidden = describe_visible_tool(&fixture, &tool_scope(), "admin::rotate_key").unwrap_err();
    let random = describe_visible_tool(&fixture, &tool_scope(), "missing::tool").unwrap_err();
    assert_eq!(hidden.public_envelope(), random.public_envelope());
}

#[test]
fn oversized_typescript_is_omitted_whole() {
    let response = describe_visible_tool(&oversized_dts_fixture(), &ToolScope::default(), "github::search").unwrap();
    assert_eq!(response.typescript, None);
    assert_eq!(response.typescript_omitted.as_deref(), Some("size_limit"));
}
```

- [ ] **Step 2: Run and verify failure**

```bash
cargo nextest run -p labby-codemode tests_discovery
```

Expected: FAIL because the module and DTOs do not exist.

- [ ] **Step 3: Define exact DTOs and caps**

Implement the spec’s 1,024-byte query, 4,096-byte target, 4 KiB description, 8 KiB signature, 32×256-byte tags, 64 KiB DTS, 256 KiB search response, and 128 KiB describe response caps.

```rust
pub struct CodeModeSearchHit {
    pub path: String,
    pub id: String,
    pub kind: CodeModeCatalogKind,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub signature: String,
    pub tags: Vec<String>,
    pub score: u32,
    pub safety: Option<CodeModeSafetyFacts>,
}

pub struct CodeModeDescribeResponse {
    pub path: String,
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub helper: String,
    pub signature: String,
    pub tags: Vec<String>,
    pub safety: Option<CodeModeSafetyFacts>,
    pub typescript: Option<String>,
    pub typescript_omitted: Option<String>,
}
```

Use omission-aware Serde attributes. Do not syntactically truncate TypeScript.

- [ ] **Step 4: Implement borrowed scoring and bounded selection**

Normalize and score borrowed descriptors. Store candidate `(entry_index, score)` pairs, maintain `total`, use bounded top-k selection for at most 50 winners, sort only those winners by score descending then path ascending, and clone fields only when constructing final DTOs.

Exclude snippets before scoring. Do not generate, clone, or serialize DTS in search.

- [ ] **Step 5: Implement visibility-safe describe**

Validate target before catalog work. Iterate only entries where `kind == Tool` and `discovery_entry_visible(entry, scope)`. Resolve exact ID/path/helper or one bare name. Produce visible ambiguity candidates only. Select the existing `entry.dts` after resolution and apply the complete-omission size rule.

- [x] **Step 6: Lock the native lexical contract in Rust table tests**

Rust table tests cover normalization, lexical weights, coverage, ties, limits, Unicode bounds, and hidden/unknown equivalence. The engineering review rejected permanent exact JS/Rust scoring parity; sandbox search remains an independent contract and may use semantic ranking.

- [ ] **Step 7: Add `CodeModeSurface::Api` exhaustiveness tests**

`tag()` returns `api`. Audit every exhaustive `CodeModeSurface` match; API discovery must never inherit CLI trusted-local/destructive behavior.

- [ ] **Step 8: Run tests and commit**

```bash
cargo nextest run -p labby-codemode tests_discovery
cargo nextest run -p labby-codemode
git add crates/labby-codemode/src/discovery.rs crates/labby-codemode/src/tests_discovery.rs crates/labby-codemode/src/lib.rs crates/labby-codemode/src/types.rs crates/labby-codemode/src/preamble.rs
git commit -m "feat(codemode): add bounded lexical tool discovery"
```

---

### Task 3: Add One-Render Gateway Orchestration

**Files:**
- Create: `crates/labby-gateway/src/gateway/manager/code_mode_discovery.rs`
- Modify: `crates/labby-gateway/src/gateway/manager.rs`
- Modify: `crates/labby-gateway/src/gateway/manager/core.rs`
- Modify: `crates/labby-gateway/src/gateway/manager/tests/inspection.rs`

**Interfaces:**
- Consumes: Task 2 functions and completed `.2` render owner.
- Produces: `AdminToolBrowserContext`, `GatewayManager::search_admin_tools`, and `GatewayManager::describe_admin_tool`.

- [ ] **Step 1: Write failing manager tests**

```rust
#[tokio::test]
async fn admin_search_acquires_one_render_and_calls_zero_tools() {
    let manager = measured_manager(fixture_tools(4_000));
    let response = manager.search_admin_tools(admin_context("operator"), "issues", 50).await.unwrap();
    assert_eq!(manager.render_acquisitions(), 1);
    assert_eq!(manager.upstream_tool_calls(), 0);
    assert_eq!(manager.warm_dts_generations(), 0);
    assert!(response.results.len() <= 50);
}

#[tokio::test]
async fn describe_re_resolves_after_catalog_churn() {
    let manager = changing_manager();
    manager.search_admin_tools(admin_context("operator"), "issues", 50).await.unwrap();
    manager.remove_tool("github::search_issues").await;
    assert_eq!(manager.describe_admin_tool(admin_context("operator"), "github::search_issues").await.unwrap_err().kind(), "unknown_tool");
}
```

- [ ] **Step 2: Run and verify failure**

```bash
cargo nextest run -p labby-gateway admin_tool_browser
```

Expected: FAIL because manager methods are absent.

- [ ] **Step 3: Define the non-serializable context**

```rust
#[derive(Debug, Clone)]
pub struct AdminToolBrowserContext {
    subject: Option<String>,
}

impl AdminToolBrowserContext {
    pub(crate) fn from_authenticated_admin(subject: Option<String>) -> Self {
        Self { subject }
    }
}
```

Do not derive `Deserialize` and do not expose a public constructor outside the authenticated adapter boundary.

- [ ] **Step 4: Implement one-render manager methods**

Create a scoped admin caller with `can_read=true`, `can_execute=true`, `can_use_snippets=false`, `is_admin=true`, `CodeModeSurface::Api`, and root `ToolScope::default()`. Call the existing `CodeModeHost::list_tools` once with snippets disabled, then call Task 2 functions.

Do not call `semantic_rank`, warm query embeddings, call upstream tools, or create background work. Search/describe use the render returned by `.2`; a warm request must regenerate zero DTS.

- [ ] **Step 5: Add structural 4,000-tool evidence**

On a fresh isolated manager/cache, record raw catalog projections, fingerprint bytes, render builds, DTS count/bytes, full serialization count, returned DTO count, and response bytes. Normal CI asserts structural counts and absolute caps only. Put latency/peak-allocation measurements in an ignored benchmark.

- [ ] **Step 6: Run tests and commit**

```bash
cargo nextest run -p labby-gateway admin_tool_browser
cargo nextest run -p labby-gateway code_mode
git add crates/labby-gateway/src/gateway/manager/code_mode_discovery.rs crates/labby-gateway/src/gateway/manager.rs crates/labby-gateway/src/gateway/manager/core.rs crates/labby-gateway/src/gateway/manager/tests/inspection.rs
git commit -m "feat(gateway): add root-admin tool discovery"
```

---

### Task 4: Add API-Private Authenticated Endpoints

**Files:**
- Modify: `crates/labby/src/api/services/gateway.rs`
- Modify: `crates/labby/src/api/services/gateway/tests.rs` if splitting the inline tests is needed.
- Modify: `crates/labby/src/api/router.rs` only if route nesting requires explicit registration.

**Interfaces:**
- Consumes: Task 3 manager methods.
- Produces: `POST /v1/gateway/codemode/tools/search` and `/describe`.

- [ ] **Step 1: Write failing API boundary tests**

Cover unauthenticated, `lab:read`, `lab:admin`, authority-field injection, hidden/random equivalence, oversized input, catalog failure, and response-byte caps.

```rust
assert_eq!(post_search(no_auth(), json!({"query":"issues"})).await.status(), StatusCode::UNAUTHORIZED);
assert_eq!(post_search(read_auth(), json!({"query":"issues"})).await.status(), StatusCode::FORBIDDEN);
assert_eq!(post_search(admin_auth(), json!({"query":"issues","scope":{}})).await.status(), StatusCode::UNPROCESSABLE_ENTITY);
assert_eq!(hidden_describe_response().await.into_body_bytes(), random_describe_response().await.into_body_bytes());
```

Assert failed auth performs zero catalog acquisitions.

- [ ] **Step 2: Run and verify failure**

```bash
cargo nextest run -p labby --all-features api_admin_tool_browser
```

Expected: FAIL because routes do not exist.

- [ ] **Step 3: Add strict request DTOs**

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolSearchRequest { query: String, #[serde(default = "default_limit")] limit: usize }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDescribeRequest { target: String }
```

Validate byte caps before manager/catalog work. Reject `caller`, `surface`, `scope`, `subject`, `route`, and every other unknown field.

- [ ] **Step 4: Implement API-private handlers**

Extend `routes()` with the two nested POST routes. Require `AuthContext` and an exact `lab:admin` scope before building `AdminToolBrowserContext`. The `(admin)` frontend route is irrelevant to server authorization.

Return existing structured `ToolError`/`ApiError` envelopes. Search/describe remain absent from `gateway::ACTIONS`, MCP help, CLI, and generic dispatch.

- [ ] **Step 5: Apply observability and response policies**

Use the existing API dispatch boundary for action timing and error kind; do not add a duplicate manager timer. Record fixed action names `tools.search`/`tools.describe`, elapsed time, request ID, result count, and serialized bytes. Captured-log tests must prove query, target, IDs, descriptions, DTS, subject, scopes, and allowlists are absent for success, validation, not-found, cancellation, and catalog failure.

Add `Referrer-Policy: no-referrer` to these responses or the authenticated app shell.

- [ ] **Step 6: Run tests and commit**

```bash
cargo nextest run -p labby --all-features api_admin_tool_browser
git add crates/labby/src/api/services/gateway.rs crates/labby/src/api/services/gateway/tests.rs crates/labby/src/api/router.rs
git commit -m "feat(api): add private admin tool discovery"
```

Stage only files that exist and changed; if tests remain inline, omit the nonexistent split test file from `git add`.

---

### Task 5: Build an Auth-Epoch-Safe Tools Page

**Files:**
- Create: `apps/gateway-admin/components/tools/tool-browser.tsx`
- Create: `apps/gateway-admin/components/tools/tool-browser.test.tsx`
- Create: `apps/gateway-admin/app/(admin)/tools/page.tsx`
- Modify: `apps/gateway-admin/lib/api/gateway-config.ts`
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
- Modify: `apps/gateway-admin/lib/auth/session-store.ts`
- Modify: existing auth session-store tests
- Modify: `apps/gateway-admin/components/app-sidebar.tsx`
- Modify: `apps/gateway-admin/components/app-sidebar.test.tsx`

**Interfaces:**
- Consumes: Task 4 endpoints.
- Produces: abortable typed requests, monotonic `authEpoch`, `/tools`, result list, and text-only detail drawer.

- [ ] **Step 1: Write failing auth-transition and race tests**

Use controllable deferred promises, not timers:

```tsx
const adminSearch = deferred<SearchResponse>()
render(<ToolBrowser authEpoch={1} request={requestUsing(adminSearch)} />)
submitQuery('issues')
rerender(<ToolBrowser authEpoch={2} request={requestUsing(readOnlySearch)} />)
adminSearch.resolve(secretAdminResults)
expect(screen.queryByText('admin::rotate_key')).not.toBeInTheDocument()

selectTool('github::first')
selectTool('github::second')
secondDescribe.resolve(secondTool)
firstDescribe.resolve(firstTool)
expect(screen.getByRole('dialog')).toHaveTextContent('github::second')
```

Also test logout, 401, 403, 5xx with request ID, no matches, oversized-DTS omission, and literal rendering of `<script>alert(1)</script>`.

- [ ] **Step 2: Run and verify failure**

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/tools/tool-browser.test.tsx
```

Expected: FAIL because the component and epoch do not exist.

- [ ] **Step 3: Add typed endpoint functions without global caching**

Add exact DTOs beside the existing gateway client and two functions accepting `AbortSignal`. Use a bounded body reader before `JSON.parse`; reject a search body above 256 KiB and describe above 128 KiB with the existing typed API error carrying request ID.

Do not add SWR keys or module-global result caches.

- [ ] **Step 4: Add monotonic auth epoch**

Extend browser session state with `authEpoch: number`. Increment it on login, logout, refresh, subject replacement, and scope replacement. Do not derive it from or expose raw subject/scope values. Test both transition orders.

- [ ] **Step 5: Implement submit-based page state**

Search and selection remain component-local so catalog terms and internal IDs are not retained in browser history. Every new query, target, or `authEpoch` aborts prior requests, clears stale results/details, and checks the captured request identity before publishing a response.

Use local component state only. Do not persist catalog queries or tool identifiers in URL/history.

- [ ] **Step 6: Render accessible text-only UI**

Use existing Aurora `Input`, `Badge`, `Alert`, `Skeleton`, `ScrollArea`, and `Sheet`/`Drawer`. Upstream description/signature/tags/DTS render as React text or `<pre><code>` without Markdown, HTML insertion, or executable syntax-highlighter output.

Provide distinct initial guidance, loading, no-match, invalid, sign-in, forbidden, unavailable/request-ID, stale-selection, and TypeScript-size-omission states.

- [ ] **Step 7: Add sidebar navigation only**

Add `{ title: 'Tools', url: '/tools', icon: Wrench }`. Do not add command-palette integration in v1.

- [ ] **Step 8: Run frontend tests and commit**

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/tools/tool-browser.test.tsx components/app-sidebar.test.tsx
cd apps/gateway-admin && pnpm test
git add apps/gateway-admin/components/tools/tool-browser.tsx apps/gateway-admin/components/tools/tool-browser.test.tsx apps/gateway-admin/app/'(admin)'/tools/page.tsx apps/gateway-admin/lib/api/gateway-config.ts apps/gateway-admin/lib/api/gateway-client.ts apps/gateway-admin/lib/auth/session-store.ts apps/gateway-admin/components/app-sidebar.tsx apps/gateway-admin/components/app-sidebar.test.tsx
git commit -m "feat(web): add admin Code Mode tool browser"
```

Add the existing auth test file to the commit when modified.

---

### Task 6: Browser Proof, Documentation, and Full Gates

**Files:**
- Extend: `apps/gateway-admin/components/tools/tool-browser.test.tsx`
- Modify: `docs/dev/CODE_MODE.md`
- Modify: generated API-route/OpenAPI docs only through `just docs-generate`.

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: end-to-end proof and current operator documentation.

- [ ] **Step 1: Write the failing browser test**

Use deterministic local fixtures and assert:

```ts
await page.goto('/tools?q=issues')
await page.getByRole('button', { name: 'Search tools' }).click()
await expect(page.getByText('github::search_issues')).toBeVisible()
await page.getByText('github::search_issues').click()
await expect(page.getByRole('dialog')).toContainText('declare function')
await expect(page.getByRole('complementary', { name: 'Tool details' })).toContainText('github::search_issues')
```

Add direct unauthenticated endpoint denial, admin→logout→new-session without reload, reverse-order response completion, hidden/random identical not-found, literal hostile DTS rendering, and no embedded production fixture.

- [ ] **Step 2: Run and verify failure**

```bash
cd apps/gateway-admin && node --import tsx --test components/tools/tool-browser.test.tsx
```

Expected: FAIL until browser fixtures and route are wired.

- [ ] **Step 3: Add deterministic fixtures and production-bundle assertion**

Mock only the two private endpoints. Add a bundle/static-output search proving fixture-only tool IDs/descriptions do not appear in production route assets. Use deferred responses/barriers; no real TEI, upstream, sleep, or fixed animation timeout.

- [ ] **Step 4: Update canonical documentation**

Document API-private root-admin lexical discovery, the `.2` prerequisite, focused disclosure of existing DTS, authority boundary, response caps, auth epoch, and explicit v1 deferrals. State that sandbox search remains JavaScript and may include semantic ranking; browser v1 promises only the shared lexical subset.

- [ ] **Step 5: Regenerate API docs and verify no shared action drift**

```bash
just docs-generate
just docs-check
rg -n "gateway.codemode.search|gateway.codemode.describe" docs/generated/action-catalog.json docs/generated/mcp-help.json
```

Expected: docs checks pass and the final search returns no matches because these are API-private routes, not shared actions.

- [ ] **Step 6: Run browser, frontend, Rust, and repository gates**

```bash
cd apps/gateway-admin && node --import tsx --test components/tools/tool-browser.test.tsx
just web-build
cargo nextest run -p labby-codemode
cargo nextest run -p labby-gateway
cargo nextest run -p labby --all-features api_admin_tool_browser
just check
just lint
just test
just docs-check
```

Expected: all commands PASS. Report unrelated pre-existing failures separately; feature-specific failures block completion.

- [ ] **Step 7: Run final boundary scans**

```bash
git diff --check
rg -n "requiresApproval|resume_token|confirm|pause|rollback" crates/labby-codemode/src/discovery.rs crates/labby-gateway/src/gateway/manager/code_mode_discovery.rs apps/gateway-admin/components/tools docs/dev/CODE_MODE.md
rg -n "caller|surface|scope|subject" apps/gateway-admin/lib/api/gateway-client.ts
rg -n "dangerouslySetInnerHTML" apps/gateway-admin/components/tools
```

Expected: no forbidden lifecycle vocabulary, no client-supplied authority fields, and no HTML injection path.

- [ ] **Step 8: Commit**

```bash
git add apps/gateway-admin/components/tools/tool-browser.test.tsx docs/dev/CODE_MODE.md docs/generated
git commit -m "docs(codemode): document private admin tool discovery"
```

---

## Self-Review Results

- **Spec coverage:** Tasks 1–6 cover activation gates, root-admin authority, lexical search, focused disclosure, caps, browser races, auth transitions, text safety, observability, and full verification.
- **Placeholder scan:** No implementation placeholder remains. Conditional activation has an explicit measurable pass/fail rule and stop behavior.
- **Type consistency:** Rust and TypeScript response fields match the spec, including `typescript_omitted`; no task references the removed broker, shared-action, semantic, snippet, or SWR interfaces.
- **Review completeness:** All architecture, simplicity, security, and performance recommendations are either implemented in the active tasks or named under the spec’s explicit deferrals.
