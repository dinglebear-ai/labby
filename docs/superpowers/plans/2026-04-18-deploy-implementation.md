# Deploy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new synthetic `deploy` service that builds the local `lab` release binary, resolves SSH-configured rollout targets and lab-defined groups, deploys the artifact safely to one or more devices, and exposes the same workflow over CLI, MCP, and HTTP.

**Architecture:** Implement `deploy` as a non-HTTP synthetic service in `lab-apis`, with the shared dispatch contract living in `crates/lab/src/dispatch/deploy/`. Keep CLI, MCP, and API as thin adapters over one shared action catalog and rollout execution path; store deploy preferences in `LabConfig` and derive target inventory from `~/.ssh/config`.

**Tech Stack:** Rust 2024, tokio, serde/serde_json, toml, tracing, existing `lab_apis::core::action` and dispatch helpers, local process invocation for build and transfer tools, axum API helpers, cargo-nextest/cargo test.

---

## File Structure

### New Rust files

- Create: `crates/lab-apis/src/deploy.rs`
  - Service entrypoint with `META` and module exports.
- Create: `crates/lab-apis/src/deploy/client.rs`
  - `DeployClient` orchestration for `plan`, `run`, and `verify`.
- Create: `crates/lab-apis/src/deploy/types.rs`
  - Request, plan, target, per-host result, summary, and rollout policy types.
- Create: `crates/lab-apis/src/deploy/error.rs`
  - Typed deploy errors and stage-aware failure variants.
- Create: `crates/lab-apis/src/deploy/build.rs`
  - Local build command execution and artifact discovery.
- Create: `crates/lab-apis/src/deploy/ssh_config.rs`
  - SSH alias inventory parsing for rollout targets.
- Create: `crates/lab-apis/src/deploy/transport.rs`
  - `rsync`-preferred transfer abstraction with SSH fallback.
- Create: `crates/lab-apis/src/deploy/remote.rs`
  - Per-host preflight, atomic install, restart, and verify helpers.
- Create: `crates/lab/src/dispatch/deploy.rs`
  - Directory-first dispatch entrypoint with tests.
- Create: `crates/lab/src/dispatch/deploy/catalog.rs`
  - `ACTIONS` catalog for `help`, `schema`, `targets.list`, `groups.list`, `plan`, `run`, `verify`.
- Create: `crates/lab/src/dispatch/deploy/client.rs`
  - Config loading, group expansion, and shared deploy-handle wiring.
- Create: `crates/lab/src/dispatch/deploy/params.rs`
  - All `serde_json::Value` coercion into typed deploy requests.
- Create: `crates/lab/src/dispatch/deploy/dispatch.rs`
  - `dispatch()` and `dispatch_with_client()`.
- Create: `crates/lab/src/cli/deploy.rs`
  - Typed CLI group for targets, groups, plan, run, verify.
- Create: `crates/lab/src/mcp/services/deploy.rs`
  - Thin MCP adapter forwarding to dispatch.
- Create: `crates/lab/src/api/services/deploy.rs`
  - Thin API adapter using `handle_action`.
- Create: `crates/lab/tests/deploy_dispatch.rs`
  - Shared dispatch tests beyond the entrypoint smoke tests.
- Create: `crates/lab/tests/deploy_api.rs`
  - API routing and destructive confirmation tests.
- Create: `crates/lab/tests/deploy_cli.rs`
  - CLI parsing and confirmation-gate tests.
- Create: `docs/coverage/deploy.md`
  - Coverage and live evidence matrix for the new service.
- Create: `docs/DEPLOY_SERVICE.md`
  - Operator-facing service contract for the new rollout capability.

### Existing Rust files to modify

- Modify: `crates/lab-apis/src/lib.rs`
  - Export the new `deploy` service.
- Modify: `crates/lab-apis/src/core/plugin.rs`
  - Add a non-bootstrap category if needed for deploy metadata.
- Modify: `crates/lab/Cargo.toml`
  - Add the `deploy` feature passthrough and default posture.
- Modify: `crates/lab/src/config.rs`
  - Add typed `[deploy]` config structures and parsing tests.
- Modify: `crates/lab/src/dispatch.rs`
  - Register the new dispatch module.
- Modify: `crates/lab/src/dispatch/clients.rs`
  - Add an `Option<Arc<lab_apis::deploy::DeployClient>>` field or documented equivalent shared handle.
- Modify: `crates/lab/src/dispatch/error.rs`
  - Add `impl_tool_error_from!(lab_apis::deploy::DeployError)` behind the feature gate.
- Modify: `crates/lab/src/cli.rs`
  - Register the new `deploy` CLI group.
- Modify: `crates/lab/src/registry.rs`
  - Register the `deploy` service in the runtime catalog.
- Modify: `crates/lab/src/mcp/services.rs`
  - Add `pub mod deploy;`.
- Modify: `crates/lab/src/api/services.rs`
  - Add `pub mod deploy;`.
- Modify: `crates/lab/src/api/router.rs`
  - Mount `/v1/deploy`.
- Modify: `crates/lab/src/api/state.rs`
  - Ensure the API state can access the pre-built deploy handle.

### Existing docs to modify

- Modify: `docs/README.md`
  - Link the new deploy service doc distinctly from the existing runtime topology doc.
- Modify: `docs/SERVICES.md`
  - Add `deploy` as a synthetic service and clarify feature posture.
- Modify: `docs/CONFIG.md`
  - Document `[deploy]` defaults, groups, and host overrides.
- Modify: `docs/CLI.md`
  - Document the `lab deploy` command group.
- Modify: `docs/MCP.md`
  - Document the `deploy` tool and destructive behavior.
- Modify: `docs/OBSERVABILITY.md`
  - Record rollout logging boundaries for the new service.
- Modify: `docs/SERVICE_ONBOARDING.md`
  - Only if needed to mention `deploy` as a concrete non-HTTP reference beside `extract`.

## Implementation Decisions Locked In

- `deploy` is a synthetic service implemented in `lab-apis`, not a product-local surface like `gateway`.
- `deploy` does not reuse `Category::Bootstrap`; add a new category if that is the cleanest path.
- V1 builds one local artifact with `cargo build --release --all-features`.
- Target inventory comes from `~/.ssh/config`; deploy intent comes from `[deploy]` in `config.toml`.
- `run` is destructive; `plan` and `verify` are not.
- Transfer prefers `rsync` and falls back to SSH streaming.
- Install is atomic and produces a timestamped backup when replacing an existing binary.
- Restart is optional and driven by explicit per-host config (`service`, `service_scope`, `restart`).
- Verification is shallow and deterministic: binary presence plus optional `systemctl` status.
- Online/offline presence tracking is explicitly out of scope and belongs to the follow-on `devices` capability.

## Task 1: Add The Deploy Config Model And Metadata Category

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab-apis/src/core/plugin.rs`
- Test: `crates/lab/src/config.rs` existing TOML parsing tests or add focused deploy config tests near the config module

- [ ] **Step 1: Write the failing config parsing tests**

```rust
#[test]
fn parses_deploy_defaults_groups_and_host_overrides() {
    let raw = r#"
        [deploy.defaults]
        remote_path = "/usr/local/bin/lab"
        service = "lab"
        service_scope = "system"
        restart = true
        backup = true
        verify_service = true

        [deploy.groups]
        servers = ["mini1", "mini2"]

        [deploy.hosts.mini2]
        service = "lab-worker"
        service_scope = "user"
    "#;

    let parsed: crate::config::LabConfig = toml::from_str(raw).unwrap();
    let deploy = parsed.deploy.expect("deploy config");
    assert_eq!(deploy.groups.get("servers").unwrap(), &vec!["mini1".to_string(), "mini2".to_string()]);
    assert_eq!(deploy.hosts.get("mini2").unwrap().service.as_deref(), Some("lab-worker"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab config -- --nocapture`

Expected: FAIL because `LabConfig` has no `[deploy]` block yet.

- [ ] **Step 3: Add the deploy config structures**

Implement in `crates/lab/src/config.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeployPreferences {
    #[serde(default)]
    pub defaults: Option<DeployDefaults>,
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub hosts: BTreeMap<String, DeployHostOverride>,
}
```

Add focused structs for `DeployDefaults`, `DeployHostOverride`, and a `ServiceScope` enum that can serialize as `system` or `user`.

- [ ] **Step 4: Add the category decision to plugin metadata**

Implement one of:

- add `Category::Operator` to `crates/lab-apis/src/core/plugin.rs`, or
- deliberately place `deploy` in the nearest existing category and document why in the code review notes

Prefer the explicit new category unless it causes disproportionate churn.

- [ ] **Step 5: Re-run the tests to verify the config model parses**

Run: `cargo test -p lab config -- --nocapture`

Expected: PASS for the new deploy config coverage.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/config.rs crates/lab-apis/src/core/plugin.rs
git commit -m "feat: add deploy config model"
```

## Task 2: Scaffold The `lab-apis` Deploy Service

**Files:**
- Create: `crates/lab-apis/src/deploy.rs`
- Create: `crates/lab-apis/src/deploy/client.rs`
- Create: `crates/lab-apis/src/deploy/types.rs`
- Create: `crates/lab-apis/src/deploy/error.rs`
- Create: `crates/lab-apis/src/deploy/build.rs`
- Create: `crates/lab-apis/src/deploy/ssh_config.rs`
- Create: `crates/lab-apis/src/deploy/transport.rs`
- Create: `crates/lab-apis/src/deploy/remote.rs`
- Modify: `crates/lab-apis/src/lib.rs`
- Test: focused unit tests inline in the new modules or in existing crate test locations

- [ ] **Step 1: Write the failing service-shape tests**

```rust
#[test]
fn deploy_meta_uses_expected_name() {
    assert_eq!(lab_apis::deploy::META.name, "deploy");
}

#[test]
fn deploy_request_defaults_are_non_destructive() {
    let request = lab_apis::deploy::types::DeployRequest::default();
    assert!(!request.confirm);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab-apis deploy -- --nocapture`

Expected: FAIL because the `deploy` module does not exist yet.

- [ ] **Step 3: Add the `deploy` module and metadata**

Implement `crates/lab-apis/src/deploy.rs` with:

```rust
pub mod build;
pub mod client;
pub mod error;
pub mod remote;
pub mod ssh_config;
pub mod transport;
pub mod types;

pub use client::DeployClient;
pub use error::DeployError;
pub use types::*;
```

Also define `META: PluginMeta` with no required secret env vars and optional docs/default-port fields appropriate for a synthetic rollout service.

- [ ] **Step 4: Add the typed request and result model**

In `types.rs`, define the stable core types:

- `DeployRequest`
- `DeploySelection`
- `DeployPlan`
- `DeployTarget`
- `DeployHostPolicy`
- `DeployRunSummary`
- `DeployHostResult`
- `DeployStage`
- `TransportUsed`

Keep them serializable and named for direct CLI/MCP/API reuse.

- [ ] **Step 5: Add the skeletal client API**

In `client.rs`, add:

```rust
impl DeployClient {
    pub fn new() -> Self { ... }
    pub async fn plan(&self, request: DeployRequest) -> Result<DeployPlan, DeployError> { ... }
    pub async fn run(&self, request: DeployRequest) -> Result<DeployRunSummary, DeployError> { ... }
    pub async fn verify(&self, request: DeployRequest) -> Result<DeployRunSummary, DeployError> { ... }
}
```

Use placeholder internal calls if needed, but keep the public signatures stable.

- [ ] **Step 6: Re-run the tests to verify the skeleton compiles**

Run: `cargo test -p lab-apis deploy -- --nocapture`

Expected: PASS for the basic service-shape assertions.

- [ ] **Step 7: Commit**

```bash
git add crates/lab-apis/src/lib.rs crates/lab-apis/src/deploy.rs crates/lab-apis/src/deploy
git commit -m "feat: scaffold deploy sdk service"
```

## Task 3: Implement Target Resolution And Group Expansion

**Files:**
- Modify: `crates/lab-apis/src/deploy/ssh_config.rs`
- Modify: `crates/lab-apis/src/deploy/client.rs`
- Modify: `crates/lab-apis/src/deploy/types.rs`
- Create: `crates/lab/src/dispatch/deploy/client.rs`
- Test: deploy target-resolution tests in `lab-apis` and dispatch config-resolution tests in `lab`

- [ ] **Step 1: Write the failing target-resolution tests**

```rust
#[test]
fn expands_group_to_known_ssh_aliases() {
    let ssh_hosts = vec!["mini1".to_string(), "mini2".to_string()];
    let groups = BTreeMap::from([("servers".to_string(), vec!["mini1".to_string(), "mini2".to_string()])]);

    let resolved = resolve_selection(&ssh_hosts, &groups, DeploySelection::Group("servers".into())).unwrap();
    assert_eq!(resolved, vec!["mini1".to_string(), "mini2".to_string()]);
}

#[test]
fn rejects_unknown_alias_inside_group() {
    let ssh_hosts = vec!["mini1".to_string()];
    let groups = BTreeMap::from([("servers".to_string(), vec!["mini1".to_string(), "ghost".to_string()])]);

    let err = resolve_selection(&ssh_hosts, &groups, DeploySelection::Group("servers".into())).unwrap_err();
    assert!(err.to_string().contains("ghost"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab-apis deploy::ssh_config -- --nocapture`

Expected: FAIL because the resolution helpers do not exist yet.

- [ ] **Step 3: Implement SSH inventory parsing for deploy**

Model this after the existing `extract` SSH inventory reader, but keep it focused on:

- alias
- hostname override
- user
- port

Do not infer deploy groups from comments or host patterns.

- [ ] **Step 4: Implement config-driven group expansion**

In `crates/lab/src/dispatch/deploy/client.rs`, add helpers that:

- load `LabConfig`
- access `[deploy.groups]`
- merge defaults with host overrides
- validate that all configured group members exist in the SSH inventory

- [ ] **Step 5: Re-run the tests to verify target resolution works**

Run: `cargo test -p lab-apis deploy -- --nocapture && cargo test -p lab deploy_dispatch -- --nocapture`

Expected: PASS for explicit target and group expansion cases.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/deploy/ssh_config.rs crates/lab-apis/src/deploy/client.rs crates/lab/src/dispatch/deploy/client.rs
git commit -m "feat: add deploy target resolution"
```

## Task 4: Implement Build, Preflight, Transfer, And Atomic Install Helpers

**Files:**
- Modify: `crates/lab-apis/src/deploy/build.rs`
- Modify: `crates/lab-apis/src/deploy/transport.rs`
- Modify: `crates/lab-apis/src/deploy/remote.rs`
- Modify: `crates/lab-apis/src/deploy/error.rs`
- Modify: `crates/lab-apis/src/deploy/types.rs`
- Test: focused unit tests in the same modules

- [ ] **Step 1: Write the failing helper tests**

```rust
#[test]
fn prefers_rsync_when_available() {
    let chosen = choose_transport(true, true).unwrap();
    assert_eq!(chosen, TransportUsed::Rsync);
}

#[test]
fn falls_back_to_ssh_stream_when_rsync_is_unavailable() {
    let chosen = choose_transport(false, true).unwrap();
    assert_eq!(chosen, TransportUsed::SshStream);
}

#[test]
fn atomic_install_plan_records_backup_path() {
    let plan = build_install_plan("/usr/local/bin/lab", true);
    assert!(plan.backup_path.is_some());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab-apis deploy -- --nocapture`

Expected: FAIL because the helper logic is not implemented.

- [ ] **Step 3: Implement the local build boundary**

In `build.rs`, execute:

```text
cargo build --release --all-features
```

Return a typed `BuiltArtifact` with:

- artifact path
- file size
- optional hash

Keep command invocation and output capture isolated in this file.

- [ ] **Step 4: Implement preflight and install helpers**

In `remote.rs`, add explicit stage helpers:

- `preflight_target(...)`
- `install_target(...)`
- `restart_target(...)`
- `verify_target(...)`

Represent stage failures with typed `DeployError` variants instead of stringly errors.

- [ ] **Step 5: Implement transfer selection**

In `transport.rs`, add:

- `choose_transport(...)`
- `transfer_with_rsync(...)`
- `transfer_with_ssh_stream(...)`

The actual process wiring can be thin, but selection and fallback behavior must be explicit and testable.

- [ ] **Step 6: Re-run the tests to verify the helpers behave deterministically**

Run: `cargo test -p lab-apis deploy -- --nocapture`

Expected: PASS for transport selection, install planning, and build boundary tests.

- [ ] **Step 7: Commit**

```bash
git add crates/lab-apis/src/deploy/build.rs crates/lab-apis/src/deploy/transport.rs crates/lab-apis/src/deploy/remote.rs crates/lab-apis/src/deploy/error.rs crates/lab-apis/src/deploy/types.rs
git commit -m "feat: add deploy rollout helpers"
```

## Task 5: Build The Shared Dispatch Layer

**Files:**
- Create: `crates/lab/src/dispatch/deploy.rs`
- Create: `crates/lab/src/dispatch/deploy/catalog.rs`
- Create: `crates/lab/src/dispatch/deploy/params.rs`
- Create: `crates/lab/src/dispatch/deploy/dispatch.rs`
- Modify: `crates/lab/src/dispatch.rs`
- Modify: `crates/lab/src/dispatch/error.rs`
- Test: `crates/lab/tests/deploy_dispatch.rs`

- [ ] **Step 1: Write the failing dispatch tests**

```rust
#[test]
fn deploy_catalog_includes_core_actions() {
    let names: Vec<&str> = crate::dispatch::deploy::ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"targets.list"));
    assert!(names.contains(&"groups.list"));
    assert!(names.contains(&"plan"));
    assert!(names.contains(&"run"));
    assert!(names.contains(&"verify"));
}

#[tokio::test]
async fn deploy_help_lists_run_action() {
    let value = crate::dispatch::deploy::dispatch("help", serde_json::json!({})).await.unwrap();
    assert!(value["actions"].as_array().unwrap().iter().any(|a| a["name"] == "run"));
}

#[tokio::test]
async fn deploy_unknown_action_returns_error() {
    let err = crate::dispatch::deploy::dispatch("not.a.real.action", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), "unknown_action");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab deploy_dispatch -- --nocapture`

Expected: FAIL because the dispatch module does not exist yet.

- [ ] **Step 3: Implement the directory-first dispatch entrypoint**

Use the required pattern from `docs/SERVICE_ONBOARDING.md`:

```rust
mod catalog;
mod client;
mod dispatch;
mod params;

pub use catalog::ACTIONS;
#[allow(unused_imports)]
pub use client::client_from_env;
#[allow(unused_imports)]
pub use dispatch::{dispatch, dispatch_with_client};
```

- [ ] **Step 4: Add the `ACTIONS` catalog and request coercion**

`catalog.rs` should explicitly declare:

- `help`
- `schema`
- `targets.list`
- `groups.list`
- `plan`
- `run`
- `verify`

`run` must be `destructive: true`.

`params.rs` should own coercion for:

- selection (`targets`, `group`, `all`)
- override booleans (`restart`, `backup`, `verify_service`, `dry_run`)
- optional path and service overrides

- [ ] **Step 5: Wire `DeployError` into `ToolError`**

Add:

```rust
#[cfg(feature = "deploy")]
impl_tool_error_from!(lab_apis::deploy::DeployError);
```

in `crates/lab/src/dispatch/error.rs`.

- [ ] **Step 6: Re-run the dispatch tests**

Run: `cargo test -p lab deploy_dispatch -- --nocapture`

Expected: PASS for help, schema, and unknown-action behavior.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch.rs crates/lab/src/dispatch/deploy.rs crates/lab/src/dispatch/deploy crates/lab/src/dispatch/error.rs crates/lab/tests/deploy_dispatch.rs
git commit -m "feat: add deploy dispatch layer"
```

## Task 6: Wire CLI, MCP, API, Registry, And Shared State

**Files:**
- Create: `crates/lab/src/cli/deploy.rs`
- Create: `crates/lab/src/mcp/services/deploy.rs`
- Create: `crates/lab/src/api/services/deploy.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/mcp/services.rs`
- Modify: `crates/lab/src/api/services.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/dispatch/clients.rs`
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/Cargo.toml`
- Test: `crates/lab/tests/deploy_api.rs`
- Test: `crates/lab/tests/deploy_cli.rs`

- [ ] **Step 1: Write the failing adapter tests**

```rust
#[tokio::test]
async fn deploy_api_requires_confirm_for_run() {
    // build router, post {"action":"run","params":{"all":true}}
    // expect confirmation_required
}

#[test]
fn deploy_cli_exposes_run_subcommand() {
    // clap parse of `lab deploy run --all -y`
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab deploy_api deploy_cli -- --nocapture`

Expected: FAIL because no adapters or routes exist yet.

- [ ] **Step 3: Wire the CLI group**

Implement a thin typed CLI in `crates/lab/src/cli/deploy.rs` that forwards to `crate::dispatch::deploy::dispatch()`.

Keep logic out of the CLI file:

- parse args with `clap`
- build the action + params payload
- enforce `-y` for destructive `run`
- format output with the shared output layer

- [ ] **Step 4: Wire the MCP and API shims**

Implement:

- `crates/lab/src/mcp/services/deploy.rs` as a one-line forwarder like `extract`
- `crates/lab/src/api/services/deploy.rs` using `handle_action(...)`

Make sure `/v1/deploy` is mounted and `state.clients.deploy` is available.

- [ ] **Step 5: Register the service everywhere**

Update:

- `crates/lab/Cargo.toml`
- `crates/lab/src/cli.rs`
- `crates/lab/src/mcp/services.rs`
- `crates/lab/src/api/services.rs`
- `crates/lab/src/api/router.rs`
- `crates/lab/src/registry.rs`
- `crates/lab/src/dispatch/clients.rs`

Treat missing any one of these as an incomplete rollout.

- [ ] **Step 6: Re-run the adapter tests**

Run: `cargo test -p lab deploy_api deploy_cli -- --nocapture`

Expected: PASS for CLI parsing and API destructive confirmation behavior.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/cli/deploy.rs crates/lab/src/mcp/services/deploy.rs crates/lab/src/api/services/deploy.rs crates/lab/src/cli.rs crates/lab/src/mcp/services.rs crates/lab/src/api/services.rs crates/lab/src/api/router.rs crates/lab/src/api/state.rs crates/lab/src/dispatch/clients.rs crates/lab/src/registry.rs crates/lab/Cargo.toml crates/lab/tests/deploy_api.rs crates/lab/tests/deploy_cli.rs
git commit -m "feat: expose deploy across cli mcp and api"
```

## Task 7: Finish Rollout Execution And Observability

**Files:**
- Modify: `crates/lab-apis/src/deploy/client.rs`
- Modify: `crates/lab-apis/src/deploy/build.rs`
- Modify: `crates/lab-apis/src/deploy/remote.rs`
- Modify: `crates/lab-apis/src/deploy/transport.rs`
- Modify: `crates/lab/src/dispatch/deploy/dispatch.rs`
- Modify: `docs/OBSERVABILITY.md`
- Test: extend deploy unit tests and any API/dispatch tests that assert stage-aware results

- [ ] **Step 1: Write the failing observability and result-shape assertions**

```rust
#[tokio::test]
async fn deploy_run_reports_transport_and_stage_per_host() {
    let result = /* invoke mocked run path */;
    assert_eq!(result.hosts[0].stage, DeployStage::Verify);
    assert!(matches!(result.hosts[0].transport_used, Some(TransportUsed::Rsync) | Some(TransportUsed::SshStream)));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab-apis deploy -- --nocapture && cargo test -p lab deploy_dispatch -- --nocapture`

Expected: FAIL because the orchestration path is still skeletal.

- [ ] **Step 3: Implement the full shared `run` flow**

In `crates/lab-apis/src/deploy/client.rs`, execute:

1. resolve selection
2. merge host policy
3. build artifact once
4. preflight each target
5. transfer
6. install
7. optional restart
8. verify
9. summarize results

Make the per-host failures stage-aware and non-global by default.

- [ ] **Step 4: Add rollout-scoped tracing**

Emit structured events for:

- target resolution
- build start and finish
- preflight result per host
- chosen transfer method
- install result
- restart result
- verify result

Never log raw params.

- [ ] **Step 5: Re-run the tests**

Run: `cargo test -p lab-apis deploy -- --nocapture && cargo test -p lab deploy_dispatch -- --nocapture`

Expected: PASS for deterministic rollout summary coverage.

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/deploy/client.rs crates/lab-apis/src/deploy/build.rs crates/lab-apis/src/deploy/remote.rs crates/lab-apis/src/deploy/transport.rs crates/lab/src/dispatch/deploy/dispatch.rs docs/OBSERVABILITY.md
git commit -m "feat: implement deploy rollout execution"
```

## Task 8: Update Docs, Coverage, And Final Verification

**Files:**
- Create: `docs/coverage/deploy.md`
- Create: `docs/DEPLOY_SERVICE.md`
- Modify: `docs/README.md`
- Modify: `docs/SERVICES.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CLI.md`
- Modify: `docs/MCP.md`
- Modify: `docs/superpowers/specs/2026-04-18-deploy-design.md` only if implementation-driven clarifications are necessary

- [ ] **Step 1: Write the coverage doc skeleton**

Include sections for:

- source contract
- SDK methods
- dispatch actions
- CLI commands
- MCP actions
- API route
- live test evidence

- [ ] **Step 2: Write the operator-facing deploy service doc**

Document:

- target selection
- group config
- host overrides
- destructive behavior
- transfer fallback
- verification contract
- non-goals, including the separation from `devices`

- [ ] **Step 3: Update cross-cutting docs**

Make the minimal correct edits in:

- `docs/README.md`
- `docs/SERVICES.md`
- `docs/CONFIG.md`
- `docs/CLI.md`
- `docs/MCP.md`

Keep `docs/DEPLOY.md` focused on the existing device-runtime topology and use the new doc for the deploy service itself.

- [ ] **Step 4: Run the required verification commands**

Run:

```bash
cargo fmt --all --check
cargo test --all-features
cargo build --all-features
```

Expected: PASS for the full workspace.

- [ ] **Step 5: Run live smoke tests if a safe target exists**

Examples:

```bash
lab deploy targets list
lab deploy groups list
lab deploy plan --group servers
lab deploy run --targets <safe-host> -y
lab deploy verify --targets <safe-host>
```

Record outcomes in `docs/coverage/deploy.md`.

- [ ] **Step 6: Commit**

```bash
git add docs/coverage/deploy.md docs/DEPLOY_SERVICE.md docs/README.md docs/SERVICES.md docs/CONFIG.md docs/CLI.md docs/MCP.md
git commit -m "docs: add deploy service documentation"
```

## Final Verification Gate

- [ ] Run: `cargo fmt --all --check`
- [ ] Run: `cargo test --all-features`
- [ ] Run: `cargo build --all-features`
- [ ] Run safe live smoke tests for `targets.list`, `groups.list`, `plan`, and, if possible, one non-critical `run` + `verify`
- [ ] Update `docs/coverage/deploy.md` with live evidence
- [ ] Confirm `deploy` appears in:
  - `lab help`
  - `lab://catalog`
  - MCP tool registry
  - `/v1/deploy`

## Handoff To `devices`

After this plan is accepted, the next work item is not deployment implementation yet. The next design session should create the separate `devices` plan for:

- `devices.status`
- `devices.events`
- `devices.watch`
- `devices.unreachable`

That design must keep presence tracking separate from rollout execution and reuse the approved non-goal boundary from the deploy spec.

