# Service Layer UniFi Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate UniFi onto `crates/lab/src/services` so CLI, MCP, and HTTP API all use the same shared execution path instead of routing through MCP-owned dispatch.

**Architecture:** Follow the ByteStash pattern already established in the repo, but preserve UniFi's existing action-group split so the service layer stays readable. The shared `services::unifi` module becomes the single owner of the UniFi action catalog, client resolution, param helpers, and dispatch routing; MCP, CLI, HTTP API, registry, and health checks become thin adapters over it.

**Tech Stack:** Rust 2024, `serde_json`, `clap`, `axum`, `tracing`, `lab-apis`, existing `ActionSpec` / `ParamSpec`, existing `ToolError`

---

## File Structure

This slice should mirror the current UniFi MCP decomposition instead of collapsing everything into one large file.

**Create:**
- `crates/lab/src/services/unifi.rs`
- `crates/lab/src/services/unifi/helpers.rs`
- `crates/lab/src/services/unifi/acl.rs`
- `crates/lab/src/services/unifi/clients.rs`
- `crates/lab/src/services/unifi/devices.rs`
- `crates/lab/src/services/unifi/dns.rs`
- `crates/lab/src/services/unifi/firewall.rs`
- `crates/lab/src/services/unifi/hotspot.rs`
- `crates/lab/src/services/unifi/misc.rs`
- `crates/lab/src/services/unifi/networks.rs`
- `crates/lab/src/services/unifi/switching.rs`
- `crates/lab/src/services/unifi/traffic.rs`
- `crates/lab/src/services/unifi/wifi.rs`

**Modify:**
- `crates/lab/src/services.rs`
- `crates/lab/src/mcp/services/unifi.rs`
- `crates/lab/src/cli/unifi.rs`
- `crates/lab/src/api/services/unifi.rs`
- `crates/lab/src/mcp/registry.rs`
- `crates/lab/src/cli/health.rs`
- `docs/coverage/unifi.md`
- `docs/SERVICE_LAYER_MIGRATION.md`

**Do not modify in this slice unless a test forces it:**
- `crates/lab/src/cli/radarr.rs`
- `crates/lab/src/api/services/radarr.rs`
- `crates/lab/src/mcp/services/radarr.rs`

## Constraints

- Do not move upstream request building or response parsing out of `lab-apis`.
- Do not rewrite UniFi CLI UX in this slice; keep action-style CLI.
- Do not let UniFi CLI call `crate::mcp::services::unifi::dispatch` after this slice.
- Do not let UniFi HTTP API call `crate::mcp::services::unifi::dispatch` after this slice.
- Preserve the current UniFi action names and action count.
- Keep the MCP module as a forwarding adapter, not a second source of truth.
- Keep `ACTIONS` ownership in `services::unifi`, then re-export from MCP.
- Preserve the current HTTP `handle_action` confirmation gate behavior.
- Do not log params in any new dispatch logging.

## Task 1: Create the Shared UniFi Service Skeleton

**Files:**
- Create: `crates/lab/src/services/unifi.rs`
- Create: `crates/lab/src/services/unifi/helpers.rs`
- Modify: `crates/lab/src/services.rs`
- Test: `crates/lab/src/services/unifi.rs`

- [ ] **Step 1: Write failing service-layer skeleton tests**

Add focused tests in `crates/lab/src/services/unifi.rs` for:

- `help` includes representative actions such as `system.info`, `sites.list`, `clients.list`, `wifi.broadcasts.list`
- the combined action count remains `72`
- there are no duplicate action names
- unknown action returns `ToolError::UnknownAction` with a populated `valid` list

Suggested assertions:

```rust
#[tokio::test]
async fn help_lists_core_actions() {
    let value = dispatch("help", serde_json::json!({})).await.unwrap();
    let actions = value["actions"].as_array().unwrap();
    assert!(actions.iter().any(|a| a["name"] == "system.info"));
    assert!(actions.iter().any(|a| a["name"] == "clients.list"));
}

#[test]
fn action_count_preserved() {
    assert_eq!(ACTIONS.len(), 72);
}
```

- [ ] **Step 2: Run the targeted tests to verify they fail**

Run:

```bash
cargo test -p lab services::unifi -- --nocapture
```

Expected:

- failure because `services::unifi` does not exist yet

- [ ] **Step 3: Create the module skeleton**

Implement:

- `pub mod unifi;` in `crates/lab/src/services.rs` behind `#[cfg(feature = "unifi")]`
- `crates/lab/src/services/unifi.rs` as the entry point
- `crates/lab/src/services/unifi/helpers.rs` by moving the current shared UniFi helper logic out of MCP ownership

At this stage:

- define `pub use helpers::client_from_env;`
- define `pub fn actions() -> &'static [ActionSpec]`
- define `pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError>`
- keep the action-group split identical to the current MCP UniFi structure

- [ ] **Step 4: Run the targeted tests to verify the skeleton passes**

Run:

```bash
cargo test -p lab services::unifi -- --nocapture
```

Expected:

- the new service-layer tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/services.rs crates/lab/src/services/unifi.rs crates/lab/src/services/unifi/helpers.rs
git commit -m "refactor: add unifi service dispatch skeleton"
```

## Task 2: Move UniFi Action Groups from MCP Ownership to Services Ownership

**Files:**
- Create: `crates/lab/src/services/unifi/acl.rs`
- Create: `crates/lab/src/services/unifi/clients.rs`
- Create: `crates/lab/src/services/unifi/devices.rs`
- Create: `crates/lab/src/services/unifi/dns.rs`
- Create: `crates/lab/src/services/unifi/firewall.rs`
- Create: `crates/lab/src/services/unifi/hotspot.rs`
- Create: `crates/lab/src/services/unifi/misc.rs`
- Create: `crates/lab/src/services/unifi/networks.rs`
- Create: `crates/lab/src/services/unifi/switching.rs`
- Create: `crates/lab/src/services/unifi/traffic.rs`
- Create: `crates/lab/src/services/unifi/wifi.rs`
- Modify: `crates/lab/src/services/unifi.rs`
- Test: `crates/lab/src/services/unifi.rs`

- [ ] **Step 1: Write one focused failing test for delegated routing**

Add a test proving one action from each of these families resolves through the new service layer without falling into `unknown_action`:

- `devices.list`
- `clients.list`
- `networks.list`
- `wifi.broadcasts.list`
- `firewall.zones.list`

Do not require live controller credentials. The assertion is that the action is accepted by routing. Accept either:

- a config error such as `internal_error` due to missing env, or
- a real SDK error if env is present

Example:

```rust
#[tokio::test]
async fn devices_list_is_routed_by_service_layer() {
    let err = dispatch("devices.list", serde_json::json!({"site_id": "default"}))
        .await
        .unwrap_err();
    assert_ne!(err.kind(), "unknown_action");
}
```

- [ ] **Step 2: Run the targeted test to verify it fails**

Run:

```bash
cargo test -p lab services::unifi::tests::devices_list_is_routed_by_service_layer -- --nocapture
```

Expected:

- failure until the service-layer submodules are wired

- [ ] **Step 3: Copy the action-group modules into `services/unifi/`**

Move the implementation ownership for:

- `acl`
- `clients`
- `devices`
- `dns`
- `firewall`
- `hotspot`
- `misc`
- `networks`
- `switching`
- `traffic`
- `wifi`

For each copied module:

- replace imports of `crate::mcp::services::unifi::helpers::*` with `super::helpers::*`
- keep `pub const ACTIONS` intact
- keep dispatch signatures as `pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError>`
- do not change action names or param metadata

- [ ] **Step 4: Update `services/unifi.rs` to assemble the combined action catalog**

Mirror the current MCP logic:

- prepend built-in `help`
- append each action-group `ACTIONS`
- route dispatch by action prefix to the matching group module

At the end of this step, the service-layer module must own the full action table and the full dispatch tree.

- [ ] **Step 5: Run the focused service-layer test set**

Run:

```bash
cargo test -p lab services::unifi -- --nocapture
```

Expected:

- all UniFi service-layer tests pass
- action count remains `72`
- no duplicate action names

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/services/unifi.rs crates/lab/src/services/unifi
git commit -m "refactor: move unifi dispatch ownership to services"
```

## Task 3: Make MCP a Thin Adapter Over `services::unifi`

**Files:**
- Modify: `crates/lab/src/mcp/services/unifi.rs`
- Modify: `crates/lab/src/mcp/registry.rs`
- Test: `crates/lab/src/mcp/services/unifi.rs`

- [ ] **Step 1: Write failing MCP adapter tests**

Add tests that lock in the forwarding behavior:

- `actions()` exposes the same count as `crate::services::unifi::actions()`
- `dispatch("help", ...)` still works via the MCP module
- `client_from_env()` is still available for existing callers until they migrate

Suggested assertions:

```rust
#[test]
fn mcp_actions_forward_to_services_catalog() {
    assert_eq!(actions().len(), crate::services::unifi::actions().len());
}
```

- [ ] **Step 2: Run the targeted tests to verify current behavior fails the new adapter contract**

Run:

```bash
cargo test -p lab mcp::services::unifi -- --nocapture
```

Expected:

- at least one new test fails before forwarding is implemented

- [ ] **Step 3: Replace MCP-owned dispatch with re-export / forwarding**

Make `crates/lab/src/mcp/services/unifi.rs` match the ByteStash adapter pattern as closely as practical:

- keep any small compatibility wrapper needed by registry callers
- re-export `client_from_env` from `crate::services::unifi`
- forward `actions()` and `dispatch()` to `crate::services::unifi`
- remove service-semantic ownership from the MCP module

- [ ] **Step 4: Point registry wiring at the service layer**

In `crates/lab/src/mcp/registry.rs`, change UniFi registration to use:

- `crate::services::unifi::actions()`
- `crate::services::unifi::dispatch`

This makes MCP registration reflect the real source of truth.

- [ ] **Step 5: Run the MCP-focused tests**

Run:

```bash
cargo test -p lab mcp::services::unifi -- --nocapture
cargo test -p lab registry -- --nocapture
```

Expected:

- UniFi MCP adapter tests pass
- registry tests, if present, remain green

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/mcp/services/unifi.rs crates/lab/src/mcp/registry.rs
git commit -m "refactor: make unifi mcp adapter use services layer"
```

## Task 4: Make CLI and HTTP API Use `services::unifi`

**Files:**
- Modify: `crates/lab/src/cli/unifi.rs`
- Modify: `crates/lab/src/api/services/unifi.rs`
- Modify: `crates/lab/src/cli/health.rs`
- Test: `crates/lab/src/cli/unifi.rs`
- Test: `crates/lab/src/api/services/unifi.rs`
- Test: `crates/lab/src/cli/health.rs`

- [ ] **Step 1: Write failing adapter tests**

Add focused tests for:

- CLI module no longer references `crate::mcp::services::unifi::dispatch`
- API handler passes `crate::services::unifi::actions()` and `crate::services::unifi::dispatch`
- health check resolves client via `crate::services::unifi::client_from_env()`

If a direct unit test for symbol references is awkward, add narrow behavior tests plus a text grep check in the command step.

- [ ] **Step 2: Run the targeted checks and tests to verify the old coupling is still present**

Run:

```bash
cargo test -p lab unifi -- --nocapture
rtk rg -n "mcp::services::unifi::(dispatch|client_from_env)" crates/lab/src
```

Expected:

- tests either fail or grep shows the old forbidden coupling

- [ ] **Step 3: Switch the adapters**

Make these changes:

- in `crates/lab/src/cli/unifi.rs`, call `crate::services::unifi::dispatch`
- in `crates/lab/src/api/services/unifi.rs`, pass `crate::services::unifi::actions()` and dispatch through `crate::services::unifi::dispatch`
- in `crates/lab/src/cli/health.rs`, use `crate::services::unifi::client_from_env()`

Do not change CLI UX or HTTP route structure.

- [ ] **Step 4: Run the targeted tests and grep verification**

Run:

```bash
cargo test -p lab unifi -- --nocapture
rtk rg -n "mcp::services::unifi::(dispatch|client_from_env)" crates/lab/src
```

Expected:

- tests pass
- grep returns no UniFi CLI/API/health call sites

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/cli/unifi.rs crates/lab/src/api/services/unifi.rs crates/lab/src/cli/health.rs
git commit -m "refactor: route unifi cli and api through services"
```

## Task 5: Verification and Documentation

**Files:**
- Modify: `docs/coverage/unifi.md`
- Modify: `docs/SERVICE_LAYER_MIGRATION.md`

- [ ] **Step 1: Update the coverage doc**

In `docs/coverage/unifi.md`:

- update the implementation note so UniFi is described as owned by `crates/lab/src/services/unifi.rs`
- update MCP / CLI / HTTP wording to say they are adapters over the shared service layer
- do not claim live verification unless it actually happened against a controller

- [ ] **Step 2: Update the migration checklist**

In `docs/SERVICE_LAYER_MIGRATION.md`, mark the UniFi checklist items complete only if this slice fully lands:

- create `services/unifi.rs`
- move operation matching and validation there
- make MCP wrap `services::unifi`
- make CLI wrap `services::unifi`
- make HTTP API wrap `services::unifi`
- verify behavior stays stable

Do not change the architecture contract in this doc during the implementation slice.

- [ ] **Step 3: Run the focused verification suite**

Run:

```bash
cargo test -p lab services::unifi -- --nocapture
cargo test -p lab mcp::services::unifi -- --nocapture
cargo test -p lab unifi -- --nocapture
cargo test -p lab api::services::helpers -- --nocapture
```

Then run the coupling checks:

```bash
rtk rg -n "mcp::services::unifi::dispatch" crates/lab/src
rtk rg -n "mcp::services::unifi::client_from_env" crates/lab/src
```

Expected:

- all targeted tests pass
- remaining UniFi MCP references, if any, are only within the MCP adapter itself

- [ ] **Step 4: Run a broader compile check**

Run:

```bash
cargo check -p lab --features unifi
```

Expected:

- compile success for the `lab` crate with UniFi enabled

- [ ] **Step 5: Commit**

```bash
git add docs/coverage/unifi.md docs/SERVICE_LAYER_MIGRATION.md
git commit -m "docs: mark unifi service migration complete"
```

## Follow-On Slice

After this plan lands, the next slice should be Radarr:

- create `crates/lab/src/services/radarr.rs`
- move Radarr machine-facing dispatch ownership there
- point MCP and HTTP API at `services::radarr`
- map the typed CLI onto the same shared execution path

That follow-on should be a separate plan because the typed CLI mapping is a distinct concern from UniFi's action-style migration.
