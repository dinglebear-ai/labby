# Labby Operator Apps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-class operator app registry, launcher, shared host bridge, and richer server-log app behavior.

**Architecture:** Keep `ActionSpec` as the source of truth for callable action metadata. Add a Labby-layer `AppSpec` registry that references `service + action` bindings, then serialize app manifests by resolving those bindings against the live `ToolRegistry`; UI routing stays in Labby, while action names/scopes/descriptions come from `ActionSpec`.

**Tech Stack:** Rust 2024, axum, serde, embedded HTML/JS, Playwright smoke checks.

## Global Constraints

- Do not reintroduce the removed syslog/fleet `logs` product slice.
- Do not add UI metadata fields to `labby-primitives::ActionSpec`; keep primitive action metadata transport-neutral.
- Browser data routes must use existing `/v1` auth behavior.
- The ChatGPT `ui://` resource must keep working when the hosted browser URL exists.

---

### Task 1: ActionSpec-Backed App Manifest

**Files:**
- Create: `crates/labby/src/app_manifest.rs`
- Modify: `crates/labby/src/lib.rs`
- Modify: `crates/labby/src/api/router.rs`

**Interfaces:**
- Consumes: `ToolRegistry::service(name)` and `RegisteredService.actions`
- Produces: `app_manifest::manifest_for_registry(&ToolRegistry) -> AppsManifest`

- [ ] Add static app specs that bind app data routes to `server_logs/server_logs.query`.
- [ ] Serialize required scopes from the bound `ActionSpec.requires_admin` field.
- [ ] Add tests that fail if an app references a missing action.

### Task 2: Hosted App Routes And Launcher

**Files:**
- Modify: `crates/labby/src/api/router.rs`

**Interfaces:**
- Consumes: `app_manifest::manifest_for_registry`
- Produces: `GET /v1/apps/manifest`, `GET /apps`, and `GET /apps/server-logs`

- [ ] Serve a small `/apps` launcher page.
- [ ] Serve `/v1/apps/manifest` behind existing `/v1` auth.
- [ ] Keep `/apps/server-logs` protected when auth is configured.

### Task 3: Shared Browser/ChatGPT Host Bridge

**Files:**
- Modify: `crates/labby/src/app_assets.rs`
- Modify: `crates/labby/src/api/router.rs`
- Modify: `crates/labby/src/mcp/assets/server_logs_app.html`

**Interfaces:**
- Produces: `window.LabbyAppHost.callAction(service, action, params, options)`

- [ ] Add a bridge that chooses `window.openai.callTool` in ChatGPT and `/v1` HTTP in browser mode.
- [ ] Serve the bridge at `/apps/assets/labby-app-host.js`.
- [ ] Keep an inline fallback so MCP `ui://` rendering does not depend on a public asset URL.

### Task 4: Server Log App UX

**Files:**
- Modify: `crates/labby/src/mcp/assets/server_logs_app.html`

**Interfaces:**
- Consumes: app manifest, bridge, URL query params, localStorage saved views

- [ ] Add deep-link URL state for filters.
- [ ] Add saved views stored in browser localStorage.
- [ ] Add operator chrome: mode, auth/scope hint, refresh state, last updated.
- [ ] Add cross-app drilldown links when fields include `request_id`, `trace_id`, `service`, or `action`.

### Task 5: Verification

**Files:**
- Modify tests near the changed modules.

- [ ] Run `cargo fmt --all --check`.
- [ ] Run `git diff --check`.
- [ ] Run focused app/server-log/router tests.
- [ ] Run architecture boundary tests.
- [ ] Run a Playwright smoke for `/apps` and `/apps/server-logs`.
