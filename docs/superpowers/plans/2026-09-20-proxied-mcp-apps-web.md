# Proxied MCP Apps Web Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render direct and Code Mode-nested upstream MCP Apps inline in the Labby Web Phoenix assistant while preserving the ordinary tool timeline and text/result fallback.

**Architecture:** Keep upstream discovery and resource ownership in `labby-gateway`, expose one authenticated Phoenix read action that forwards only valid `ui://` resource reads through the current gateway pool, and make the web client derive app render descriptors from retained completed `mcpToolCall` events. Extract the existing sandboxed Code Mode iframe into a generic MCP App resource panel so both the inspector and Phoenix share load/failure behavior; Phoenix keeps the original event timeline beside the progressively enhanced app.

**Tech Stack:** Rust 1.97.1, rmcp models, Axum service actions, TypeScript, React 19, Next.js 16 static export, Node test runner, happy-dom, pnpm 9.

**Spec:** GitHub issue `dinglebear-ai/labby#731` and the acceptance requirements supplied in the implementation request; protocol references are the MCP Apps overview, `RESOURCE_URI_META_KEY` compatibility documentation, OpenAI plugin reference, and Codex App Server item contract.

## Global Constraints

- Prefer modern `_meta.ui.resourceUri`; accept legacy `_meta["ui/resourceUri"]` and older App Server `mcpAppResourceUri` compatibility shapes.
- Accept only non-empty `ui://` resource URIs; malformed or non-UI metadata must leave the ordinary tool timeline/result fallback unchanged.
- A rendered app is progressive enhancement: keep the normal MCP tool event node and model-facing content/result visible even when resource loading or iframe rendering fails.
- Reuse the generic gateway resource owner/read path and existing Code Mode trace parser; do not add upstream names, Connexin identifiers, restart behavior, or per-server branches.
- Render HTML in a sandboxed iframe without same-origin authority; do not grant host DOM, cookies, or storage access.
- Retained completed events are authoritative after streaming, React rerender, and session reload; started/progress events must not create duplicate app instances.
- Preserve Phoenix event redaction, byte bounds, caller identity, platform-administrator authorization, route scope, upstream OAuth subject behavior, and CSRF policy.
- Do not edit generated docs by hand or touch `docs/sessions/` or historical `docs/superpowers/` material outside this requested plan.
- Do not push or create/merge a pull request.

## Review Focus

- A completed direct MCP call whose `appContext.resourceUri` is `ui://…` renders once, while the existing tool activity node and fallback text remain present.
- A completed `codemode` call whose result wraps a `code_mode_execute_trace` under `structuredContent`, `structured_content`, `toolOutput`, `output`, `result`, or JSON text finds every valid nested `calls[].ui.resourceUri` without treating ordinary calls as apps.
- Legacy `mcpAppResourceUri` and `_meta["ui/resourceUri"]` compatibility values are accepted only when they are valid `ui://` strings; empty, HTTP, object, and malformed values fall back without a fetch.
- A resource response with no HTML content, a rejected read, or an iframe load error shows a bounded unavailable state and never removes the normal tool result/timeline.
- Replaying the same retained completed event during polling/rerender/reload yields a stable descriptor key and one app panel; a different item id or nested call id remains independently renderable.

---

### Task 1: Gateway-Owned Phoenix UI Resource Read

**Files:**
- Modify: `crates/labby-gateway/src/gateway/manager/resource_discovery.rs`
- Modify: `crates/labby/src/dispatch/phoenix.rs`
- Modify: `crates/labby/src/api/services/phoenix.rs`
- Test: `crates/labby/src/api/services/phoenix.rs`
- Test: `crates/labby-gateway/src/gateway/manager/tests/code_mode.rs`

**Interfaces:**
- Consumes: `GatewayManager::current_pool_sync() -> Option<Arc<UpstreamPool>>` and `UpstreamPool::read_upstream_ui_resource(&str) -> Option<Result<ReadResourceResult, String>>`.
- Produces: `GatewayManager::read_mcp_app_resource(&self, uri: &str) -> Result<rmcp::model::ReadResourceResult, labby_runtime::error::ToolError>` and Phoenix action `phoenix.mcp_app.read` with `{ uri: string }`, returning the serialized MCP `ReadResourceResult`.

- [x] **Step 1: Add failing manager tests for exact generic UI reads**

  Extend the existing manager Code Mode test fixture with a `ui://fixture/app.html` resource and assert that `read_mcp_app_resource("ui://fixture/app.html")` returns its HTML, while `https://example.test/app.html`, an unknown UI owner, and an unavailable pool return stable `invalid_param`, `not_found`, and `executor_unavailable` errors without upstream-specific logic.

- [x] **Step 2: Run the focused manager tests and verify red**

  Run: `cargo test -p labby-gateway gateway::manager::tests::code_mode::phoenix_mcp_app_resource`

  Expected: FAIL because `GatewayManager::read_mcp_app_resource` does not exist.

- [x] **Step 3: Implement the bounded manager method**

  In `resource_discovery.rs`, validate `uri.starts_with("ui://")` and reject empty authorities/paths through a small private parser; obtain the published pool without constructing or restarting an upstream; call `read_upstream_ui_resource`; translate `None` to `not_found`, gateway strings through the existing SDK error style, and return the untouched `ReadResourceResult` so content MIME metadata survives.

- [x] **Step 4: Run the focused manager tests and verify green**

  Run: `cargo test -p labby-gateway gateway::manager::tests::code_mode::phoenix_mcp_app_resource`

  Expected: PASS for successful HTML, invalid URI, missing owner, and unavailable-pool cases.

- [x] **Step 5: Add failing Phoenix HTTP adapter tests**

  Add `phoenix.mcp_app.read` to the shared Phoenix action catalog and write adapter tests proving: the action is read-only/CSRF-exempt; a platform administrator can read a generic `ui://` resource through an injected test gateway manager; a non-admin is denied before the read; and invalid/missing resources retain structured API errors.

- [x] **Step 6: Run the focused Phoenix adapter tests and verify red**

  Run: `cargo test -p labby api::services::phoenix::tests`

  Expected: FAIL because the action is not routed to the gateway manager.

- [x] **Step 7: Route the read action through authenticated server state**

  In `api/services/phoenix.rs`, clone `state.gateway_manager` under the `gateway` feature and dispatch `phoenix.mcp_app.read` to `GatewayManager::read_mcp_app_resource`; fail closed when gateway management is unavailable. Keep all session/turn actions on `PhoenixRuntime::dispatch`, mark this read action CSRF-exempt, and add a no-gateway feature branch that returns `executor_unavailable` rather than changing feature contracts.

- [x] **Step 8: Run the focused Rust tests and verify green**

  Run: `cargo test -p labby api::services::phoenix::tests && cargo test -p labby-gateway gateway::manager::tests::code_mode::phoenix_mcp_app_resource`

  Expected: PASS.

### Task 2: Shared MCP App Detection Contract

**Files:**
- Create: `apps/gateway-admin/lib/mcp-app/event-apps.ts`
- Create: `apps/gateway-admin/lib/mcp-app/event-apps.test.ts`
- Modify: `apps/gateway-admin/lib/code-mode-app/trace.ts`

**Interfaces:**
- Consumes: `PhoenixEvent`, completed App Server `mcpToolCall` items, and `parseCodeModeTrace(value: unknown) -> CodeModeTrace | null`.
- Produces: `McpAppRenderDescriptor { key: string; resourceUri: string; itemId: string; callId?: string; appName?: string; toolResult?: unknown }` and `mcpAppsForPhoenixEvent(event: PhoenixEvent): McpAppRenderDescriptor[]`.

- [x] **Step 1: Write failing direct-app and compatibility parser tests**

  Cover one completed direct call with `appContext.resourceUri`, the deprecated top-level `mcpAppResourceUri`, a result `_meta.ui.resourceUri`, and legacy result `_meta["ui/resourceUri"]`. Assert modern `appContext` wins conflicts, keys are deterministic (`itemId:resourceUri`), and the original `result` is retained for later iframe delivery/fallback.

- [x] **Step 2: Write failing invalid/non-app parser tests**

  Cover `item/started`, a non-`mcpToolCall`, incomplete status, missing ids, empty strings, object-valued metadata, `https://`, malformed `ui://`, and an ordinary successful tool result. Assert every case returns `[]` and never throws.

- [x] **Step 3: Write failing Code Mode nested-app tests**

  Feed a completed `codemode` item whose `result` wraps `code_mode_execute_trace` in each existing supported envelope and whose calls contain two valid UI links, one normal call, and one invalid UI link. Assert two ordered descriptors keyed by item id plus nested call id, with no direct-tool special case and no duplicate when the same URI appears twice for the same call.

- [x] **Step 4: Run parser tests and verify red**

  Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/mcp-app/event-apps.test.ts`

  Expected: FAIL because the module and extraction function do not exist.

- [x] **Step 5: Implement strict generic extraction**

  Add small record/string guards and `validUiResourceUri` using `new URL(uri)` plus `protocol === "ui:"`, requiring a hostname/authority. Extract only authoritative completed items. For direct calls, check `appContext.resourceUri`, top-level compatibility, modern result metadata, then legacy result metadata. For Code Mode, call `parseCodeModeTrace(item.result)` and map execute-trace calls with valid `ui.resourceUri`; do not parse tool names or upstream names beyond recognizing the generic `codemode` trace kind.

- [x] **Step 6: Run parser tests and verify green**

  Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/mcp-app/event-apps.test.ts lib/code-mode-app/trace.test.ts`

  Expected: PASS, including all direct, invalid, legacy, and nested Code Mode cases.

### Task 3: Reusable Sandboxed MCP App Panel

**Files:**
- Create: `apps/gateway-admin/components/mcp-app/mcp-app-resource-panel.tsx`
- Create: `apps/gateway-admin/components/mcp-app/mcp-app-resource-panel.test.tsx`
- Modify: `apps/gateway-admin/components/code-mode-app/code-mode-inspector.tsx`
- Modify: `apps/gateway-admin/components/code-mode-app/code-mode-inspector.test.tsx`
- Modify: `apps/gateway-admin/lib/api/phoenix-client.ts`
- Modify: `apps/gateway-admin/lib/api/phoenix-client.test.ts`

**Interfaces:**
- Consumes: `resourceUri`, optional `toolResult`, and `readResource(params: { uri: string }, signal?: AbortSignal) -> Promise<McpResourceReadResult>`.
- Produces: `McpAppResourcePanel` with stable loading/ready/unavailable/error/iframe-error states; `McpResourceReadResult` and `phoenixApi.readMcpAppResource(uri, signal)`; Code Mode inspector continues consuming its host-provided `ResourceReader` through the same panel.

- [x] **Step 1: Add failing Phoenix client contract test**

  Assert `readMcpAppResource("ui://fixture/app.html")` POSTs `phoenix.mcp_app.read` with the exact URI and returns `contents` without renaming `mimeType`, `mime_type`, text, URI, or `_meta` fields.

- [x] **Step 2: Add failing panel lifecycle tests**

  In JSDOM, assert valid HTML becomes one iframe with `sandbox="allow-scripts allow-forms allow-popups allow-downloads"` and no `allow-same-origin`; non-HTML content shows unavailable; read rejection shows failure; `onError` changes a ready iframe to failure; and changing/unmounting the URI aborts or ignores the stale read.

- [x] **Step 3: Add failing rerender stability test**

  Render the panel, resolve HTML, rerender with the same `resourceUri`, reader, and result but changed surrounding props, and assert it does not issue a second read or replace the iframe. Rerender with a new URI and assert exactly one new read.

- [x] **Step 4: Run client/panel tests and verify red**

  Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/api/phoenix-client.test.ts components/mcp-app/mcp-app-resource-panel.test.tsx`

  Expected: FAIL because the API method and shared panel do not exist.

- [x] **Step 5: Implement the client method and shared panel**

  Add the exact resource response types to `phoenix-client.ts`. Move the Code Mode panel’s Aurora shell, HTML selection, sandbox, cancellation flag, and load states into `McpAppResourcePanel`. Cache the resolved HTML inside the mounted component keyed by URI, pass the complete tool result as panel data for future bridge-compatible delivery without suppressing text fallback, and render concise failure copy without exposing backend details.

- [x] **Step 6: Replace the inspector-local renderer with the shared panel**

  Adapt the existing `ExtApps.App.readServerResource` callback to the shared `readResource` signature. Preserve inspector minimize/restore behavior, existing labels, and tests; delete only the now-duplicated local panel/helper code.

- [x] **Step 7: Run shared and inspector tests and verify green**

  Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/api/phoenix-client.test.ts components/mcp-app/mcp-app-resource-panel.test.tsx components/code-mode-app/code-mode-inspector.test.tsx`

  Expected: PASS with the same Code Mode inspector behavior and the new failure/rerender coverage.

### Task 4: Inline Phoenix Rendering and Retained-State Semantics

**Files:**
- Modify: `apps/gateway-admin/components/console/phoenix-conversation.tsx`
- Modify: `apps/gateway-admin/components/console/phoenix-conversation.test.tsx`
- Modify: `apps/gateway-admin/components/console/phoenix-event-timeline.tsx`
- Modify: `apps/gateway-admin/components/console/phoenix-event-timeline.test.tsx`

**Interfaces:**
- Consumes: `mcpAppsForPhoenixEvent`, `McpAppResourcePanel`, and `phoenixApi.readMcpAppResource`.
- Produces: event chunks that render the unchanged `PhoenixEventTimeline` plus zero or more keyed inline app panels at the completed call’s chronological position.

- [x] **Step 1: Add failing direct-inline conversation test**

  Render `text delta -> completed direct MCP App -> text delta`, assert chronological order, one app panel/iframe placeholder, the original `labby · tool` event node, and both assistant text chunks. The test reader returns generic HTML and asserts the exact `ui://` URI.

- [x] **Step 2: Add failing Code Mode nested conversation test**

  Render a completed `codemode` event with two nested trace apps and a normal nested call. Assert two panels in call order, one unchanged `codemode` tool activity node, and no panel for the normal call.

- [x] **Step 3: Add failing fallback and retained-state tests**

  Cover invalid metadata (zero reads/panels, normal event node remains), read failure (failure copy plus normal node/result detail remains), duplicate started/completed events (only completed creates one panel), rerender with the identical retained event array contents (stable panel identity/no duplicate read), and a fresh render representing session reload (panel is reconstructed from the retained event without requiring a live stream).

- [x] **Step 4: Run conversation tests and verify red**

  Run: `cd apps/gateway-admin && pnpm exec tsx --test components/console/phoenix-conversation.test.tsx components/console/phoenix-event-timeline.test.tsx`

  Expected: FAIL because event chunks do not render app descriptors.

- [x] **Step 5: Render progressive app enhancement in event chunks**

  Add an injectable `readMcpAppResource` prop defaulting to the Phoenix API method for testability. For every event chunk, render the existing timeline first and then descriptors from completed events using their deterministic keys. Keep app rendering outside the collapsed event detail so load failures cannot hide the normal tool node.

- [x] **Step 6: Run conversation tests and verify green**

  Run: `cd apps/gateway-admin && pnpm exec tsx --test components/console/phoenix-conversation.test.tsx components/console/phoenix-event-timeline.test.tsx`

  Expected: PASS for direct, Code Mode, invalid, failure, chronology, deduplication, rerender, and reload scenarios.

### Task 5: Document and Qualify the Contract

**Files:**
- Modify: `docs/services/PHOENIX_ASSISTANT.md`
- Modify: `docs/services/GATEWAY.md` only if the implemented manager behavior changes the existing upstream MCP App contract wording.
- Modify: `docs/superpowers/plans/2026-09-20-proxied-mcp-apps-web.md`

**Interfaces:**
- Consumes: the completed implementation and exact test commands.
- Produces: current product documentation for direct/nested app detection, retained rendering, fallback, and sandbox/resource-read behavior.

- [x] **Step 1: Update canonical Phoenix documentation**

  Document `mcpToolCall.appContext.resourceUri`, legacy compatibility, Code Mode nested trace projection, authenticated generic `ui://` resource reads, stable retained-event reconstruction, iframe sandbox limitations, and the invariant that text/tool fallback remains visible on all failures. State explicitly that the implementation has no named-upstream branch and does not restart upstreams.

- [x] **Step 2: Run all focused tests**

  Run: `cargo test -p labby-gateway gateway::manager::tests::code_mode::phoenix_mcp_app_resource`

  Run: `cargo test -p labby api::services::phoenix::tests`

  Run: `cd apps/gateway-admin && pnpm exec tsx --test lib/mcp-app/event-apps.test.ts lib/code-mode-app/trace.test.ts lib/api/phoenix-client.test.ts components/mcp-app/mcp-app-resource-panel.test.tsx components/code-mode-app/code-mode-inspector.test.tsx components/console/phoenix-conversation.test.tsx components/console/phoenix-event-timeline.test.tsx`

  Expected: all PASS.

- [x] **Step 3: Run repository-prescribed relevant gates**

  Run: `cargo test -p labby-gateway`

  Run: `cargo test -p labby --all-features`

  Run: `cargo check --workspace --all-features --all-targets --locked`

  Run: `cargo clippy -p labby-gateway --all-features --all-targets -- -D warnings`

  Run: `cargo clippy -p labby --all-features --all-targets -- -D warnings`

  Run: `cargo fmt --all -- --check`

  Run: `cd apps/gateway-admin && pnpm lint`

  Run: `cd apps/gateway-admin && pnpm test`

  Run: `cd apps/gateway-admin && pnpm test:browser`

  Run: `cd apps/gateway-admin && pnpm build`

  Run: `just docs-check`

  Expected: all gates PASS; record any unchanged baseline failure separately and fix every failure attributable to this change.

- [x] **Step 4: Review the final diff against acceptance**

  Run: `git diff --check && git diff --stat && git status --short`

  Verify each acceptance item maps to an automated test, there are no `Connexin`/named-upstream strings, no manual generated-file edits, no unrelated changes, no weakened assertions, and no push/PR operation.

- [x] **Step 5: Mark the plan complete and commit**

  Check every completed box in this file, close Beads issue `lab-3ftkh`, then run:

  ```bash
  git add crates/labby-gateway/src/gateway/manager/resource_discovery.rs \
    crates/labby-gateway/src/gateway/manager/tests/code_mode.rs \
    crates/labby/src/dispatch/phoenix.rs \
    crates/labby/src/api/services/phoenix.rs \
    apps/gateway-admin/lib/mcp-app/event-apps.ts \
    apps/gateway-admin/lib/mcp-app/event-apps.test.ts \
    apps/gateway-admin/lib/code-mode-app/trace.ts \
    apps/gateway-admin/lib/api/phoenix-client.ts \
    apps/gateway-admin/lib/api/phoenix-client.test.ts \
    apps/gateway-admin/components/mcp-app/mcp-app-resource-panel.tsx \
    apps/gateway-admin/components/mcp-app/mcp-app-resource-panel.test.tsx \
    apps/gateway-admin/components/code-mode-app/code-mode-inspector.tsx \
    apps/gateway-admin/components/code-mode-app/code-mode-inspector.test.tsx \
    apps/gateway-admin/components/console/phoenix-conversation.tsx \
    apps/gateway-admin/components/console/phoenix-conversation.test.tsx \
    apps/gateway-admin/components/console/phoenix-event-timeline.tsx \
    apps/gateway-admin/components/console/phoenix-event-timeline.test.tsx \
    docs/services/PHOENIX_ASSISTANT.md \
    docs/services/GATEWAY.md \
    docs/superpowers/plans/2026-09-20-proxied-mcp-apps-web.md
  git commit -m "feat: render proxied MCP apps in web assistant"
  ```

  Expected: one local commit on `feat/issue-731-web-mcp-apps`; do not push.
