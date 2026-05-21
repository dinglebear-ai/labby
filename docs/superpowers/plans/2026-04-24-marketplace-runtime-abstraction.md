# Marketplace Runtime Abstraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor `marketplace` into a runtime-agnostic service that preserves current Claude behavior, adds Codex read support first, and leaves a clean backend seam for Gemini.

**Architecture:** Keep one public `marketplace` service and split the current implementation into a shared service layer plus runtime-specific backends. The service owns stable action semantics, type normalization, and backend selection; each backend owns filesystem layout, manifest discovery, install/cache state, and CLI command wrappers.

**Tech Stack:** Rust workspace, existing `lab` dispatch patterns, serde JSON/TOML parsing, local filesystem I/O, shell-out wrappers for runtime CLIs.

---

## Scope

Phase 1 only:

- preserve existing Claude marketplace behavior
- extract a runtime/backend abstraction
- add Codex read-only support
- add semantic plugin/component metadata so UI and API clients stop reverse-engineering artifacts
- document the runtime-aware model

Out of scope for this plan:

- Codex write flows (`plugin.workspace`, `plugin.save`, `plugin.deploy*`)
- Gemini backend implementation
- official Codex marketplace publishing

## File Structure

### Shared marketplace domain and orchestration

- Modify: `crates/lab/src/dispatch/marketplace.rs`
  - declare the new submodules and keep the dispatch module tree compiling
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
  - thin action router that delegates to the service layer
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
  - stable action catalog plus new read-only component action
- Modify: `crates/lab-apis/src/marketplace/types.rs`
  - runtime-aware domain types used by CLI/API/MCP/frontend
- Create: `crates/lab/src/dispatch/marketplace/runtime.rs`
  - `MarketplaceRuntime` enum and selection helpers
- Create: `crates/lab/src/dispatch/marketplace/backend.rs`
  - `MarketplaceBackend` trait and backend capability contracts
- Create: `crates/lab/src/dispatch/marketplace/service.rs`
  - orchestration, runtime selection, normalized read operations
- Create: `crates/lab/src/dispatch/marketplace/package.rs`
  - manifest parsing and semantic component extraction

### Runtime backends

- Create: `crates/lab/src/dispatch/marketplace/backends/claude.rs`
  - extracted current Claude implementation
- Create: `crates/lab/src/dispatch/marketplace/backends/codex.rs`
  - Codex marketplace/config/cache readers for read-only support
- Modify: `crates/lab/src/dispatch/marketplace/client.rs`
  - keep only shared filesystem sync helpers and shell helpers, or reduce to backend-agnostic utilities

### Documentation

- Modify: `docs/MARKETPLACE.md`
  - make the service runtime-aware

### Existing consumer migration

- Modify: `crates/lab/src/tui/marketplace.rs`
  - stop duplicating runtime-specific marketplace loading rules once the shared backend exists
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`
  - preserve compatibility with existing plugin shapes while adding new runtime-aware fields/actions

### Tests

- Modify: existing marketplace dispatch/backend tests in `crates/lab/src/dispatch/marketplace/dispatch.rs` or split into backend-specific test modules if needed
- Create: targeted tests for package parsing and Codex path discovery near the new backend/modules

## Public Contract Changes

### Keep stable

- `sources.list`
- `plugins.list`
- `plugin.get`
- `plugin.artifacts`
- `plugin.workspace`
- `plugin.save`
- `plugin.deploy.preview`
- `plugin.deploy`
- `plugin.install`
- `plugin.uninstall`

### Add in Phase 1

- `plugin.components`
  - returns semantic plugin component metadata derived from manifests and file layout

### Parameter policy

- add optional `runtime` parameter to read actions:
  - `sources.list`
  - `plugins.list`
  - `plugin.get`
  - `plugin.artifacts`
  - `plugin.components`
- if omitted, service auto-detects with this priority:
  1. explicit local repo Codex marketplace files
  2. Codex home marketplace/config
  3. Claude home marketplace files
- if multiple runtimes are present and auto-detection is ambiguous, return a structured error asking for explicit `runtime`

### Capability policy for Phase 1

Write-capable actions remain supported for Claude exactly as they work today.

For `runtime="codex"` in Phase 1, these actions are explicitly unsupported:

- `plugin.workspace`
- `plugin.save`
- `plugin.deploy.preview`
- `plugin.deploy`
- `plugin.install`
- `plugin.uninstall`

Return a stable structured error rather than guessing behavior. Use the existing error vocabulary from `docs/ERRORS.md`; do not invent a new runtime-local envelope shape.

## Type Design

Extend `crates/lab-apis/src/marketplace/types.rs` with focused domain types.

Add:

- `MarketplaceRuntime`
- `PluginSummary`
- `PluginDetails`
- `PluginManifestSummary`
- `PluginComponent`
- `PluginComponentKind`
- `PluginInstallState`

Recommended shape:

- `PluginSummary`
  - `id`
  - `name`
  - `runtime`
  - `marketplace`
  - `version`
  - `description`
  - `tags`
  - `installed`
  - `enabled`
  - `installed_at`
  - `updated_at`
- `PluginDetails`
  - `summary`
  - `manifest`
  - `components`
  - `install_state`
  - `source_path`
  - `cache_path`
- `PluginManifestSummary`
  - normalized fields gathered from runtime-specific manifests
- `PluginComponent`
  - `kind`
  - `path`
  - `name`
  - optional metadata map

Do not remove the current `Artifact` type in Phase 1. Keep `plugin.artifacts` as a low-level file view.

Compatibility rule:

- preserve the existing serialized plugin fields consumed by current clients:
  - `mkt`
  - `ver`
  - `desc`
  - `installedAt`
  - `updatedAt`
- add new fields compatibly rather than replacing the current shape in Phase 1
- `plugin.components` should be additive; do not force existing clients to migrate in the same change

## Backend Contract

Create `MarketplaceBackend` in `crates/lab/src/dispatch/marketplace/backend.rs`.

Required methods:

- `fn runtime(&self) -> MarketplaceRuntime`
- `fn is_available(&self) -> bool`
- `fn list_sources(&self) -> Result<Vec<Marketplace>, ToolError>`
- `fn list_plugins(&self, filter: PluginFilter) -> Result<Vec<PluginSummary>, ToolError>`
- `fn get_plugin(&self, id: &PluginId) -> Result<PluginDetails, ToolError>`
- `fn list_artifacts(&self, id: &PluginId) -> Result<Vec<Artifact>, ToolError>`
- `fn list_components(&self, id: &PluginId) -> Result<Vec<PluginComponent>, ToolError>`

Do not put write operations on the trait yet unless you are prepared to implement them cleanly for both runtimes. Phase 1 should focus on read operations.

Important: this backend contract should become the single source of truth for marketplace runtime discovery. Do not leave `crates/lab/src/tui/marketplace.rs` with a second independent implementation of Claude/Codex/Gemini file-layout rules once the shared backend is available.

## Package Parsing Layer

Create `package.rs` for semantic parsing independent of runtime storage.

Responsibilities:

- parse Claude marketplace plugin metadata into normalized summaries
- parse Codex `.codex-plugin/plugin.json`
- extract component references:
  - skills
  - apps
  - mcp servers
  - commands
  - agents
  - assets
  - hooks when present
- infer additional components from directory layout when manifest pointers are absent

Do not let backends hand-craft component objects inline. That logic belongs in `package.rs`.

## Claude Backend Rules

Move the current implementation into `backends/claude.rs`.

Preserve:

- `~/.claude/plugins/known_marketplaces.json`
- installed state from `~/.claude/plugins/installed_plugins.json`
- marketplace manifest resolution from:
  - `<installLocation>/.claude-plugin/marketplace.json`
  - `<installLocation>/marketplace.json`

Phase 1 success condition:

- existing Claude read behavior remains unchanged after the refactor

## Codex Backend Rules

Implement read-only support in `backends/codex.rs`.

Support these sources:

- repo marketplace: `$REPO_ROOT/.agents/plugins/marketplace.json`
- compatibility marketplace: `$REPO_ROOT/.claude-plugin/marketplace.json`
- personal marketplace: `~/.agents/plugins/marketplace.json`
- install cache root: `~/.codex/plugins/cache/`
- enabled/disabled state: `~/.codex/config.toml`

Read behavior:

- `sources.list`
  - return available local/personal marketplace sources
- `plugins.list`
  - return plugin summaries with runtime=`codex`
- `plugin.get`
  - include parsed manifest/components and install/cache metadata
- `plugin.components`
  - return semantic components
- `plugin.artifacts`
  - return file artifacts from authoritative source when available, otherwise cache

Do not implement source mutation, install/uninstall, or deploy in Phase 1.

## Existing TUI Runtime Logic

The repo already contains runtime-specific marketplace discovery in `crates/lab/src/tui/marketplace.rs` for Claude, Codex, and Gemini.

Phase 1 must not leave that logic as a parallel source of truth.

Required outcome:

- either move the shared parsing/loading rules out of the TUI into the marketplace backend layer and have the TUI consume the shared layer
- or explicitly make the TUI call the same normalized marketplace service/backend helpers

Do not duplicate:

- plugin ID normalization
- installed-state rules
- Codex marketplace path discovery
- Gemini extension discovery

## Error Policy

Keep existing marketplace error envelopes from `docs/ERRORS.md`.

Add only if needed:

- `invalid_param` for unsupported runtime names
- `not_found` when a plugin exists in one runtime but not the selected runtime
- `conflict` or `invalid_param` when runtime auto-detection is ambiguous

Do not create runtime-specific error vocabularies.

## Testing Strategy

Write tests first for each extraction step.

### Task 1: Add runtime and domain types

**Files:**
- Modify: `crates/lab-apis/src/marketplace/types.rs`
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`
- Test: `crates/lab/src/dispatch/marketplace/dispatch.rs` tests or a new `types`-adjacent test module if already established nearby

- [ ] **Step 1: Write failing serialization tests for new runtime-aware types**

Add tests that assert:

- `MarketplaceRuntime` serializes predictably
- `PluginDetails` includes manifest/components/install-state fields
- existing `Artifact` serialization remains unchanged
- existing plugin list/get payloads remain backward-compatible for gateway-admin consumers

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace -- --nocapture
```

Expected:

- compile or assertion failures because the new types do not exist yet

- [ ] **Step 3: Implement the new domain types**

Add the minimal structs/enums in `crates/lab-apis/src/marketplace/types.rs`.

- [ ] **Step 3a: Preserve existing frontend-facing plugin aliases**

Keep the serialized compatibility fields used by the current admin client while adding new normalized fields.

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace -- --nocapture
```

Expected:

- new type serialization tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/lab-apis/src/marketplace/types.rs apps/gateway-admin/lib/api/marketplace-client.ts
git commit -m "feat: add runtime-aware marketplace domain types"
```

### Task 2: Extract runtime selection and backend trait

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace.rs`
- Create: `crates/lab/src/dispatch/marketplace/runtime.rs`
- Create: `crates/lab/src/dispatch/marketplace/backend.rs`
- Create: `crates/lab/src/dispatch/marketplace/service.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`

- [ ] **Step 1: Write failing tests for runtime parsing and backend selection**

Add tests covering:

- explicit `runtime="claude"` selects Claude backend
- explicit `runtime="codex"` selects Codex backend
- invalid runtime returns `invalid_param`
- ambiguous auto-detect returns structured error
- unsupported write actions against `runtime="codex"` return a stable structured error

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- failures because runtime selection does not exist yet

- [ ] **Step 3: Implement runtime enum, backend trait, and service selection**

Keep `dispatch.rs` thin. Move decision logic into `service.rs`.

- [ ] **Step 3a: Wire the new submodules into `marketplace.rs`**

Declare the new modules so the crate compiles as the implementation is split.

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- runtime selection tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/marketplace.rs crates/lab/src/dispatch/marketplace/runtime.rs crates/lab/src/dispatch/marketplace/backend.rs crates/lab/src/dispatch/marketplace/service.rs crates/lab/src/dispatch/marketplace/dispatch.rs
git commit -m "refactor: add marketplace runtime and backend abstraction"
```

### Task 3: Extract package parsing into a shared module

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/package.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`

- [ ] **Step 1: Write failing tests for plugin manifest parsing**

Cover:

- Claude-style marketplace plugin metadata normalization
- Codex `.codex-plugin/plugin.json` parsing
- component extraction for `skills`, `apps`, and `mcpServers`
- directory-layout fallback extraction when component arrays are absent

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace::package -- --nocapture
```

Expected:

- parser tests fail because `package.rs` does not exist

- [ ] **Step 3: Implement shared package parsing**

Keep parsing focused and data-only. No filesystem discovery in this module beyond interpreting already-located file trees or JSON values.

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace::package -- --nocapture
```

Expected:

- parser tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/package.rs crates/lab/src/dispatch/marketplace/dispatch.rs
git commit -m "refactor: add shared marketplace package parsing"
```

### Task 4: Move current behavior into Claude backend

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/backends/claude.rs`
- Modify: `crates/lab/src/dispatch/marketplace/client.rs`
- Modify: `crates/lab/src/dispatch/marketplace/service.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Modify: `crates/lab/src/tui/marketplace.rs`

- [ ] **Step 1: Write failing regression tests for current Claude read actions**

Cover:

- `sources.list`
- `plugins.list`
- `plugin.get`
- `plugin.artifacts`

Use existing test fixtures and the current test-only plugin root override pattern.

- [ ] **Step 2: Run the targeted tests to verify they fail once the backend seam is introduced**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- Claude regression tests fail until backend extraction is wired up

- [ ] **Step 3: Implement `ClaudeMarketplaceBackend`**

Move existing filesystem logic into the backend without changing behavior.

- [ ] **Step 3a: Start collapsing duplicated TUI Claude marketplace loading**

Do not leave Claude runtime discovery in both the TUI and the new backend as independent implementations.

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- existing Claude read behavior passes unchanged

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/backends/claude.rs crates/lab/src/dispatch/marketplace/client.rs crates/lab/src/dispatch/marketplace/service.rs crates/lab/src/dispatch/marketplace/dispatch.rs crates/lab/src/tui/marketplace.rs
git commit -m "refactor: extract claude marketplace backend"
```

### Task 5: Add Codex read-only backend

**Files:**
- Create: `crates/lab/src/dispatch/marketplace/backends/codex.rs`
- Modify: `crates/lab/src/dispatch/marketplace/service.rs`
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab/src/tui/marketplace.rs`

- [ ] **Step 1: Write failing tests for Codex source discovery and plugin listing**

Cover:

- repo marketplace discovery from `.agents/plugins/marketplace.json`
- fallback compatibility discovery from `.claude-plugin/marketplace.json`
- personal marketplace discovery from `~/.agents/plugins/marketplace.json`
- installed/cache discovery from `~/.codex/plugins/cache/...`
- enabled state from `~/.codex/config.toml`
- explicit unsupported-action behavior for Codex Phase 1 write operations

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace::codex -- --nocapture
```

Expected:

- Codex backend tests fail because backend logic does not exist

- [ ] **Step 3: Implement `CodexMarketplaceBackend` read operations**

Implement:

- `sources.list`
- `plugins.list`
- `plugin.get`
- `plugin.artifacts`
- `plugin.components`

- [ ] **Step 3a: Remove duplicated Codex marketplace discovery from the TUI**

Make the TUI consume the shared backend/service logic for Codex rather than keeping a second filesystem parser.

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace::codex -- --nocapture
```

Expected:

- Codex backend tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/backends/codex.rs crates/lab/src/dispatch/marketplace/service.rs crates/lab/src/dispatch/marketplace/catalog.rs crates/lab/src/tui/marketplace.rs
git commit -m "feat: add codex marketplace read backend"
```

### Task 6: Expose semantic components in the public API

**Files:**
- Modify: `crates/lab/src/dispatch/marketplace/catalog.rs`
- Modify: `crates/lab/src/dispatch/marketplace/dispatch.rs`
- Modify: `crates/lab/src/dispatch/marketplace/service.rs`
- Modify: `apps/gateway-admin/lib/api/marketplace-client.ts`

- [ ] **Step 1: Write failing tests for `plugin.components`**

Cover:

- Claude plugin component extraction
- Codex plugin component extraction
- empty component lists return `[]`, not null
- legacy plugin list/get consumers still deserialize without code breakage

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- failures because `plugin.components` is not exposed

- [ ] **Step 3: Add the new action and wire it through the service**

Update the catalog and dispatch routing. Keep output stable and machine-friendly.

- [ ] **Step 3a: Add additive frontend support for `plugin.components`**

Keep existing marketplace list/detail flows working while allowing the UI to consume semantic components when available.

- [ ] **Step 4: Run the targeted tests to verify they pass**

Run:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- `plugin.components` tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/marketplace/catalog.rs crates/lab/src/dispatch/marketplace/dispatch.rs crates/lab/src/dispatch/marketplace/service.rs apps/gateway-admin/lib/api/marketplace-client.ts
git commit -m "feat: expose marketplace plugin components"
```

### Task 7: Update documentation

**Files:**
- Modify: `docs/MARKETPLACE.md`

- [ ] **Step 1: Write the documentation updates**

Document:

- runtime-aware marketplace model
- backend concept
- Codex read-only Phase 1 support
- `plugin.components`
- runtime selection and ambiguity behavior
- explicit note that Codex write flows are deferred
- explicit note that Phase 1 preserves current payload compatibility for existing clients
- explicit note that the TUI runtime loader has been unified with the shared marketplace backend or service layer

- [ ] **Step 2: Review the doc for drift against code paths and action names**

Check:

- action names match the catalog
- parameter names match dispatch parsing
- Claude and Codex storage descriptions are accurate

- [ ] **Step 3: Commit**

```bash
git add docs/MARKETPLACE.md
git commit -m "docs: describe runtime-aware marketplace service"
```

## Verification Checklist

Run these after implementation:

- [ ] Claude regression coverage:

```bash
cargo test --manifest-path crates/lab/Cargo.toml dispatch::marketplace::dispatch::tests -- --nocapture
```

Expected:

- existing Claude read behavior still passes

- [ ] Codex backend coverage:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace::codex -- --nocapture
```

Expected:

- Codex source/plugin/component tests pass

- [ ] Shared package parser coverage:

```bash
cargo test --manifest-path crates/lab/Cargo.toml marketplace::package -- --nocapture
```

Expected:

- component extraction tests pass

- [ ] Focused compile check:

```bash
cargo check --manifest-path crates/lab/Cargo.toml --tests
```

Expected:

- compile succeeds for the touched marketplace modules and tests

## Design Constraints

- keep one `marketplace` service; do not create separate `claude_marketplace` and `codex_marketplace` services
- do not hardcode Codex assumptions into shared service/domain code
- do not expose raw runtime-specific file shapes directly to clients when a normalized type will do
- do not implement Codex deploy semantics in Phase 1
- do not pre-bake Gemini logic without real filesystem/manifest/CLI facts

## Handoff Notes

After this plan lands, the next implementation plan should be Phase 2:

- Codex workspace mirror
- Codex save/deploy preview/deploy semantics against authoritative source
- optional refresh/reinstall action
- backend capability flags for write operations

Gemini should be planned only after confirming:

- marketplace file locations
- plugin package manifest format
- install/cache layout
- enable/disable storage
- management CLI behavior
