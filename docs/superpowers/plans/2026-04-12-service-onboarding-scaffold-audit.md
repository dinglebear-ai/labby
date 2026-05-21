# Service Onboarding Scaffold And Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add first-class `lab scaffold service` and `lab audit onboarding` commands that generate and verify service onboarding work against the current repo contract, then expose the audit (but not scaffold writes) through an internal MCP tool without pretending they are external services.

**Architecture:** Keep scaffold and audit in the `lab` crate as internal product capabilities, not `lab-apis` services and not synthetic services like `extract`. Implement reusable core modules for scaffolding, static onboarding checks, and live verification; wire them into CLI first; then expose the audit operation through a single internal MCP tool such as `lab_admin`. The `service.scaffold` action **must not** be exposed over MCP until MCP elicitation for destructive actions is implemented — file-system write operations require a human-facing confirmation gate that the MCP surface cannot currently enforce.

**Tech Stack:** Rust, Cargo, `serde`, `serde_json`, `tokio`, `tracing`, existing `lab` CLI/MCP/API infrastructure, repo docs under `docs/`

**Security baseline (applies to every task below):**
- Service name must pass `^[a-z][a-z0-9_]{1,63}$` before any path construction or subprocess call.
- After joining the name onto a base path, assert `canonicalize(result).starts_with(canonicalize(base))` — fail closed if not (path traversal guard).
- All subprocess invocations (`lab` binary, `curl`) must use explicit `tokio::process::Command` arg arrays — never `sh -c` string interpolation.
- All templates must be embedded via `include_str!()` — no runtime filesystem template loading.
- Cargo.toml patchers must use the `toml` crate to read/modify/write — never raw string manipulation.

---

## Full Module Layout

No `mod.rs` files anywhere — repo hard rule. Every module `foo` is declared in `foo.rs` sibling to its `foo/` directory.

```
crates/lab/src/
│
├── scaffold.rs                         # mod service; mod templates; mod patcher;
│                                       # pub fn scaffold_service(config, dry_run) -> Result<ScaffoldResult>
├── scaffold/
│   ├── service.rs                      # ScaffoldConfig, ScaffoldResult, FileOp, ScaffoldError
│   │                                   # validate_service_name(), validate_scaffold_target()
│   ├── templates.rs                    # mod lab_apis; mod dispatch; mod adapters; mod docs;
│   ├── templates/
│   │   ├── lab_apis.rs                 # include_str! for: <service>.rs, client.rs, types.rs, error.rs
│   │   ├── dispatch.rs                 # include_str! for: <service>.rs, catalog.rs, client.rs, dispatch.rs, params.rs
│   │   ├── adapters.rs                 # include_str! for: cli/<service>.rs, mcp/services/<service>.rs, api/services/<service>.rs
│   │   └── docs.rs                     # include_str! for: docs/coverage/<service>.md
│   ├── patcher.rs                      # mod toml; mod source;
│   │                                   # pub fn compute_patches(name, repo_root) -> Result<Vec<FileOp>>
│   └── patcher/
│       ├── toml.rs                     # TOML patchers using the toml crate (lab-apis/Cargo.toml, lab/Cargo.toml)
│       └── source.rs                   # Rust source patchers (lib.rs, cli.rs, registry.rs, router.rs, state.rs, tui/metadata.rs, mcp/services.rs, api/services.rs)
│
├── audit.rs                            # mod types; mod checks; mod onboarding;
├── audit/
│   ├── types.rs                        # CheckResult (Pass|Fail|Skip), ServiceReport, AuditReport
│   ├── onboarding.rs                   # pub fn audit_service(name, shared_ctx) -> ServiceReport
│   │                                   # pub fn audit_services(names, repo_root) -> AuditReport
│   │                                   # SharedContext (preloaded registration files)
│   ├── checks.rs                       # mod files; mod registration; mod dispatch; mod tests; mod docs;
│   └── checks/
│       ├── files.rs                    # required file/dir existence (lab-apis files, dispatch files, adapter files)
│       ├── registration.rs             # feature registration (Cargo.toml, lib.rs) + adapter registration (7 source files)
│       ├── dispatch.rs                 # dispatch layout: re-exports ACTIONS/client_from_env/dispatch/dispatch_with_client + client helpers
│       ├── tests.rs                    # test file/block existence (SDK, dispatch, MCP adapter, API adapter)
│       └── docs.rs                     # docs/coverage/<service>.md existence
│
└── mcp/services/
    └── lab_admin.rs                    # onboarding.audit action only; no service contract; opt-in via LAB_ADMIN_ENABLED
```

Template source files (referenced by `include_str!()` — compiled into the binary):
```
crates/lab/src/scaffold/templates/
    lab_apis_service.tpl
    lab_apis_client.tpl
    lab_apis_types.tpl
    lab_apis_error.tpl
    dispatch_entrypoint.tpl
    dispatch_catalog.tpl
    dispatch_client.tpl
    dispatch_dispatch.tpl
    dispatch_params.tpl
    adapter_cli.tpl
    adapter_mcp.tpl
    adapter_api.tpl
    coverage_doc.tpl
```

**One responsibility per file:**

| File | Owns | Does NOT own |
|------|------|--------------|
| `scaffold.rs` | public API, two-phase orchestration | template content, patcher logic |
| `scaffold/service.rs` | types + name validation | file I/O, templates |
| `scaffold/templates/lab_apis.rs` | lab-apis layer template strings | dispatch/adapter templates |
| `scaffold/templates/dispatch.rs` | dispatch layer template strings | lab-apis/adapter templates |
| `scaffold/templates/adapters.rs` | CLI/MCP/API adapter template strings | lab-apis/dispatch templates |
| `scaffold/templates/docs.rs` | coverage doc template string | all other templates |
| `scaffold/patcher/toml.rs` | TOML file patching via `toml` crate | Rust source patching |
| `scaffold/patcher/source.rs` | Rust source file patching | TOML patching |
| `audit.rs` | public audit API | check implementations |
| `audit/types.rs` | CheckResult, ServiceReport, AuditReport | check logic |
| `audit/onboarding.rs` | orchestration, SharedContext, per-service assembly | individual check implementations |
| `audit/checks/files.rs` | file/dir existence checks | registration, dispatch, test, doc checks |
| `audit/checks/registration.rs` | feature + adapter registration checks | other check categories |
| `audit/checks/dispatch.rs` | dispatch layout + client helper checks | registration, test, doc checks |
| `audit/checks/tests.rs` | test file/block existence checks | all other check categories |
| `audit/checks/docs.rs` | coverage doc existence check | all other check categories |

---

### Task 1: Lock The Product Shape And Command Contract

**Files:**
- Create: `docs/superpowers/specs/2026-04-12-service-onboarding-scaffold-audit-design.md`
- Modify: `docs/SERVICE_ONBOARDING.md`
- Modify: `docs/README.md`
- Modify: `CLAUDE.md`
- Test: `rtk rg -n 'lab audit onboarding|lab scaffold service|lab_admin|synthetic service' docs CLAUDE.md -g '*.md'`

- [ ] Write a short design note capturing the final decision that scaffold and audit are core `lab` capabilities, not standalone services.
- [ ] Define the public command surface in the design note:
  - `lab scaffold service <service> [--kind http|non-http] [--dry-run] [--yes/-y]`
  - `lab audit onboarding <service...> [--json]`
- [ ] Define the MCP shape in the design note as one internal tool `lab_admin`, with actions:
  - `onboarding.audit` (MCP-safe: read-only)
  - `service.scaffold` is CLI-only until MCP elicitation is implemented; note this explicitly
- [ ] Document the security posture in the design note: `lab_admin` requires `LAB_ADMIN_ENABLED=1` to register; `service.scaffold` is `destructive: true` and requires `--yes` on CLI; scaffold writes are not reachable over MCP in this iteration.
- [ ] Update [SERVICE_ONBOARDING.md](/home/jmagar/workspace/lab/docs/SERVICE_ONBOARDING.md) so it references scaffold and audit as the preferred enforcement path.
- [ ] Update [docs/README.md](/home/jmagar/workspace/lab/docs/README.md) with links to the new audit/scaffold capabilities.
- [ ] Update [CLAUDE.md](/home/jmagar/workspace/lab/CLAUDE.md) with the rule that onboarding work should prefer scaffold first, audit second, and all-features verification last.
- [ ] Run `rtk rg -n 'lab audit onboarding|lab scaffold service|lab_admin|synthetic service' docs CLAUDE.md -g '*.md'`.

### Task 2: Create The Core Scaffold Module Structure

**Files:**
- Create: `crates/lab/src/scaffold.rs`
- Create: `crates/lab/src/scaffold/service.rs`
- Create: `crates/lab/src/scaffold/templates.rs`
- Create: `crates/lab/src/scaffold/templates/lab_apis.rs`
- Create: `crates/lab/src/scaffold/templates/dispatch.rs`
- Create: `crates/lab/src/scaffold/templates/adapters.rs`
- Create: `crates/lab/src/scaffold/templates/docs.rs`
- Create: `crates/lab/src/scaffold/patcher.rs`
- Create: `crates/lab/src/scaffold/patcher/toml.rs`
- Create: `crates/lab/src/scaffold/patcher/source.rs`
- Modify: `crates/lab/src/main.rs`
- Test: `cargo check --manifest-path crates/lab/Cargo.toml --all-features`

- [ ] Add `mod scaffold;` to `crates/lab/src/main.rs`.
- [ ] Create `crates/lab/src/scaffold/service.rs` — owns `ScaffoldConfig`, `ScaffoldResult`, `FileOp`, and `ScaffoldError`. Also owns `validate_service_name()` and `validate_scaffold_target()` since they operate on the types here. No file I/O, no templates.
- [ ] Create `crates/lab/src/scaffold/templates/lab_apis.rs` — `include_str!()` for the four lab-apis file templates. Exposes one render function per template that accepts a service name string and returns the file body.
- [ ] Create `crates/lab/src/scaffold/templates/dispatch.rs` — `include_str!()` for the five dispatch file templates (entrypoint, catalog, client, dispatch, params). Same render-function pattern.
- [ ] Create `crates/lab/src/scaffold/templates/adapters.rs` — `include_str!()` for the three thin adapter templates (cli, mcp/services, api/services).
- [ ] Create `crates/lab/src/scaffold/templates/docs.rs` — `include_str!()` for the coverage doc template.
- [ ] Create `crates/lab/src/scaffold/templates.rs` — declares `mod lab_apis; mod dispatch; mod adapters; mod docs;`. Re-exports the render functions for use by `scaffold.rs`.
- [ ] Create `crates/lab/src/scaffold/patcher/toml.rs` — TOML patchers only. Uses the `toml` crate. Owns patching for `lab-apis/Cargo.toml` and `lab/Cargo.toml`. Returns `FileOp` entries; does not write.
- [ ] Create `crates/lab/src/scaffold/patcher/source.rs` — Rust source patchers only. Owns patching for `lib.rs`, `cli.rs`, `mcp/services.rs`, `registry.rs`, `api/services.rs`, `router.rs`, `state.rs`, `tui/metadata.rs`. Returns `FileOp` entries; does not write.
- [ ] Create `crates/lab/src/scaffold/patcher.rs` — declares `mod toml; mod source;`. Exposes `compute_patches(name, repo_root) -> Result<Vec<FileOp>>` which calls both sub-modules and collects their ops.
- [ ] Create `crates/lab/src/scaffold.rs` — declares `mod service; mod templates; mod patcher;`. Exposes `scaffold_service(config: ScaffoldConfig, dry_run: bool) -> Result<ScaffoldResult>`. Contains the two-phase execution model:
  - **Plan phase:** validate name, render all templates via `templates::*`, compute all patches via `patcher::compute_patches`. Collect into `Vec<FileOp>`. Any failure here writes nothing.
  - **Execute phase:** iterate `Vec<FileOp>`, write each atomically (temp file + rename). Only reached when `dry_run == false`.
- [ ] Run `cargo check --manifest-path crates/lab/Cargo.toml --all-features`.

### Task 3: Implement HTTP Service Scaffolding And Repo Patchers

**Files:**
- Modify: `crates/lab/src/scaffold/service.rs`
- Modify: `crates/lab/src/scaffold/templates/lab_apis.rs`
- Modify: `crates/lab/src/scaffold/templates/dispatch.rs`
- Modify: `crates/lab/src/scaffold/templates/adapters.rs`
- Modify: `crates/lab/src/scaffold/templates/docs.rs`
- Modify: `crates/lab/src/scaffold/patcher/toml.rs`
- Modify: `crates/lab/src/scaffold/patcher/source.rs`
- Modify: `crates/lab/src/scaffold.rs`
- Create: `crates/lab/src/scaffold/templates/lab_apis_service.tpl`
- Create: `crates/lab/src/scaffold/templates/lab_apis_client.tpl`
- Create: `crates/lab/src/scaffold/templates/lab_apis_types.tpl`
- Create: `crates/lab/src/scaffold/templates/lab_apis_error.tpl`
- Create: `crates/lab/src/scaffold/templates/dispatch_entrypoint.tpl`
- Create: `crates/lab/src/scaffold/templates/dispatch_catalog.tpl`
- Create: `crates/lab/src/scaffold/templates/dispatch_client.tpl`
- Create: `crates/lab/src/scaffold/templates/dispatch_dispatch.tpl`
- Create: `crates/lab/src/scaffold/templates/dispatch_params.tpl`
- Create: `crates/lab/src/scaffold/templates/adapter_cli.tpl`
- Create: `crates/lab/src/scaffold/templates/adapter_mcp.tpl`
- Create: `crates/lab/src/scaffold/templates/adapter_api.tpl`
- Create: `crates/lab/src/scaffold/templates/coverage_doc.tpl`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml scaffold --all-features -- --nocapture`

**Service name validation (implement first in `service.rs`):**
- [ ] Implement `validate_service_name(name: &str) -> Result<(), ScaffoldError>` — enforces `^[a-z][a-z0-9_]{1,63}$`. Structured error if it fails.
- [ ] Implement `validate_scaffold_target(name: &str, base: &Path) -> Result<PathBuf, ScaffoldError>` — joins name, canonicalizes, asserts result is under `base`. Fail closed.
- [ ] Both validators are called as the first step in the plan phase, before any template or patcher code runs.

**Template files (`*.tpl` — embedded via `include_str!()`, compiled into binary):**
- [ ] Write `lab_apis_service.tpl` — `<service>.rs` module declaration with `pub mod client; pub mod types; pub mod error; pub const META: PluginMeta = ...`.
- [ ] Write `lab_apis_client.tpl` — `<service>/client.rs` with `<Service>Client` struct, `new()`, and a minimal health-check stub.
- [ ] Write `lab_apis_types.tpl` — `<service>/types.rs` with placeholder request/response type stubs.
- [ ] Write `lab_apis_error.tpl` — `<service>/error.rs` with `<Service>Error` using `thiserror`, wrapping `ApiError`.
- [ ] Write `dispatch_entrypoint.tpl` — dispatch module declaration re-exporting `ACTIONS`, `client_from_env`, `dispatch`, `dispatch_with_client`.
- [ ] Write `dispatch_catalog.tpl` — `ACTIONS` constant with `help` and `schema` entries.
- [ ] Write `dispatch_client.tpl` — `client_from_env()`, `require_client()`, `not_configured_error()`.
- [ ] Write `dispatch_dispatch.tpl` — `dispatch()` and `dispatch_with_client()` stubs routing to `help`/`schema`.
- [ ] Write `dispatch_params.tpl` — param extraction helpers stub.
- [ ] Write `adapter_cli.tpl` — thin CLI shim calling dispatch.
- [ ] Write `adapter_mcp.tpl` — thin MCP shim calling dispatch.
- [ ] Write `adapter_api.tpl` — thin HTTP handler shim calling dispatch.
- [ ] Write `coverage_doc.tpl` — docs/coverage stub with service name and empty action table.

**Patcher implementations:**
- [ ] In `patcher/toml.rs`: implement `patch_lab_apis_cargo(name, content) -> Result<String>` and `patch_lab_cargo(name, content) -> Result<String>`. Use the `toml` crate. Check for existing feature token before inserting (no-op if present).
- [ ] In `patcher/source.rs`: implement one patcher function per target file (`patch_lib_rs`, `patch_cli_rs`, `patch_mcp_services_rs`, `patch_mcp_registry_rs`, `patch_api_services_rs`, `patch_api_router_rs`, `patch_api_state_rs`, `patch_tui_metadata_rs`). Each takes `(name, content) -> Result<String>`. Insert at a deterministic location (alphabetically sorted within the relevant block, or before a sentinel comment). Check for existing token — no-op if already present.
- [ ] In `patcher/source.rs`: define a sentinel comment convention for each target file so insertion location is stable across `rustfmt` runs.
- [ ] In `patcher.rs`: `compute_patches` reads all 10 target files, calls the appropriate sub-patcher for each, and returns a `Vec<FileOp>`. Any patcher returning `Err` causes the entire function to fail — zero files are staged.

**Scaffold orchestration:**
- [ ] Write a failing scaffold test for `sampleaudit` before implementing.
- [ ] In `scaffold.rs`: implement the execute phase — iterate `Vec<FileOp>`, write each via temp-file-then-rename (follow `extract.apply` contract). Log each write at `INFO` with the target path.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml scaffold --all-features -- --nocapture`.

### Task 4: Add Scaffold CLI Surface And Snapshot-Like Tests

**Files:**
- Create: `crates/lab/src/cli/scaffold.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/output.rs`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml cli::scaffold --all-features -- --nocapture`

- [ ] Add `scaffold` to the root CLI tree in `crates/lab/src/cli.rs`.
- [ ] Implement `lab scaffold service <service>` in `crates/lab/src/cli/scaffold.rs`. This file is a thin shim — calls `scaffold::scaffold_service()`, formats output. No patcher or template logic here.
- [ ] Require `--yes`/`-y` for non-dry-run invocations (scaffold is `destructive: true`).
- [ ] Add human-readable output for created files and modified files.
- [ ] Add `--json` output so automation can consume scaffold results.
- [ ] Add tests that:
  - Verify the generated file list and the patched registry targets.
  - Assert that a dry-run produces zero filesystem writes (check file mtime or file non-existence).
  - Assert that a second scaffold of the same service name is a no-op (idempotency — all patchers return no-op for an already-registered name).
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml cli::scaffold --all-features -- --nocapture`.

### Task 5: Create The Static Onboarding Audit Core

**Files:**
- Create: `crates/lab/src/audit.rs`
- Create: `crates/lab/src/audit/types.rs`
- Create: `crates/lab/src/audit/onboarding.rs`
- Create: `crates/lab/src/audit/checks.rs`
- Create: `crates/lab/src/audit/checks/files.rs`
- Create: `crates/lab/src/audit/checks/registration.rs`
- Create: `crates/lab/src/audit/checks/dispatch.rs`
- Create: `crates/lab/src/audit/checks/tests.rs`
- Create: `crates/lab/src/audit/checks/docs.rs`
- Modify: `crates/lab/src/main.rs`
- Test: `cargo check --manifest-path crates/lab/Cargo.toml --all-features`

- [ ] Add `mod audit;` to `crates/lab/src/main.rs`.
- [ ] Create `crates/lab/src/audit/types.rs` — owns `CheckResult` (`Pass | Fail(String) | Skip(String)` — three variants only), `ServiceReport`, and `AuditReport`. No check logic here.
- [ ] Create `crates/lab/src/audit/checks/files.rs` — owns file and directory existence checks: required lab-apis files (`<service>.rs`, `client.rs`, `types.rs`, `error.rs`), dispatch files (`<service>.rs`, `catalog.rs`, `client.rs`, `dispatch.rs`, `params.rs`), adapter files (`cli/<service>.rs`, `mcp/services/<service>.rs`, `api/services/<service>.rs`). Returns `Vec<(check_name, CheckResult)>`.
- [ ] Create `crates/lab/src/audit/checks/registration.rs` — owns feature registration checks (Cargo.toml feature entries, lib.rs `pub mod`) and adapter registration checks (token presence in `cli.rs`, `mcp/services.rs`, `registry.rs`, `api/services.rs`, `router.rs`, `state.rs`, `tui/metadata.rs`). Reads exclusively from `SharedContext` (preloaded map) — zero filesystem reads.
- [ ] Create `crates/lab/src/audit/checks/dispatch.rs` — owns dispatch layout checks: entrypoint re-exports `ACTIONS`, `client_from_env`, `dispatch`, `dispatch_with_client`; and client helper checks: `client_from_env()`, `require_client()`, `not_configured_error()` present in `dispatch/<service>/client.rs`.
- [ ] Create `crates/lab/src/audit/checks/tests.rs` — owns test file/block existence checks: SDK test file, dispatch test block, MCP adapter test block, API adapter test block.
- [ ] Create `crates/lab/src/audit/checks/docs.rs` — owns the single coverage doc check: `docs/coverage/<service>.md` exists and is non-empty.
- [ ] Create `crates/lab/src/audit/checks.rs` — declares `pub mod files; pub mod registration; pub mod dispatch; pub mod tests; pub mod docs;`. All regex patterns used across these modules declared as `std::sync::LazyLock<Regex>` here at module scope — compile once, reuse across service iterations.
- [ ] Create `crates/lab/src/audit/onboarding.rs` — owns `SharedContext` (the preloaded registration file map), `audit_service(name, &SharedContext, repo_root) -> ServiceReport`, and `audit_services(names, repo_root) -> AuditReport`. Calls each `checks::*` module in sequence. No check logic itself.
- [ ] Create `crates/lab/src/audit.rs` — declares `mod types; mod checks; mod onboarding;`. Re-exports `audit_services` as the public API.
- [ ] Run `cargo check --manifest-path crates/lab/Cargo.toml --all-features`.

### Task 6: Implement Static Checks Against The Current Docs Contract

**Files:**
- Modify: `crates/lab/src/audit/onboarding.rs`
- Modify: `crates/lab/src/audit/checks/files.rs`
- Modify: `crates/lab/src/audit/checks/registration.rs`
- Modify: `crates/lab/src/audit/checks/dispatch.rs`
- Modify: `crates/lab/src/audit/checks/tests.rs`
- Modify: `crates/lab/src/audit/checks/docs.rs`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml audit --all-features -- --nocapture`

**`SharedContext` (implement first in `onboarding.rs`):**
- [ ] Define `SharedContext { files: HashMap<PathBuf, String> }` and `SharedContext::load(repo_root) -> Result<SharedContext>` which reads all 10 shared registration files once before any per-service checks run. All `registration.rs` checks receive `&SharedContext` — zero additional filesystem reads for shared files. This collapses O(N × M) file reads to O(N + M).
- [ ] Shared files to preload: `lab-apis/Cargo.toml`, `lab/Cargo.toml`, `lab-apis/src/lib.rs`, `lab/src/cli.rs`, `lab/src/mcp/services.rs`, `lab/src/mcp/registry.rs`, `lab/src/api/services.rs`, `lab/src/api/router.rs`, `lab/src/api/state.rs`, `lab/src/tui/metadata.rs`.

**`checks/files.rs` — implement all file/dir existence checks:**
- [ ] Required lab-apis files: `<service>.rs`, `<service>/client.rs`, `<service>/types.rs`, `<service>/error.rs`.
- [ ] Required dispatch files: `dispatch/<service>.rs`, `dispatch/<service>/catalog.rs`, `dispatch/<service>/client.rs`, `dispatch/<service>/dispatch.rs`, `dispatch/<service>/params.rs`.
- [ ] Required adapter files: `cli/<service>.rs`, `mcp/services/<service>.rs`, `api/services/<service>.rs`.

**`checks/registration.rs` — implement all registration checks (reads from SharedContext only):**
- [ ] Feature registration: `<service> = [...]` in `lab-apis/Cargo.toml`, `<service> = ["lab-apis/<service>"]` in `lab/Cargo.toml`, `#[cfg(feature = "<service>")] pub mod <service>;` in `lab-apis/src/lib.rs`.
- [ ] Adapter registration: service token presence in `cli.rs`, `mcp/services.rs`, `registry.rs`, `api/services.rs`, `router.rs`, `state.rs`, `tui/metadata.rs`.

**`checks/dispatch.rs` — implement dispatch layout + client helper checks:**
- [ ] Dispatch re-exports: `ACTIONS`, `client_from_env`, `dispatch`, `dispatch_with_client` present in `dispatch/<service>.rs`.
- [ ] Client helpers: `client_from_env()`, `require_client()`, `not_configured_error()` present in `dispatch/<service>/client.rs`.

**`checks/tests.rs` — implement test existence checks:**
- [ ] SDK test: `crates/lab-apis/tests/<service>_client.rs` exists, or `#[cfg(test)]` block in `lab-apis/src/<service>/client.rs`.
- [ ] Dispatch test: `#[cfg(test)]` block in `dispatch/<service>/dispatch.rs`.
- [ ] MCP adapter test: `#[cfg(test)]` block in `mcp/services/<service>.rs`.
- [ ] API adapter test: `#[cfg(test)]` block in `api/services/<service>.rs`.

**`checks/docs.rs` — implement coverage doc check:**
- [ ] `docs/coverage/<service>.md` exists and is non-empty.

**Do not add:**
- [ ] **No heuristic pattern checks** (`surface="api"` grep, AppState reuse grep, catalog handling grep, error type placement grep). These produce false positives on comments and doc strings, are not CI-safe, and add maintenance burden. The exact structural checks above cover what matters.

- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml audit --all-features -- --nocapture`.

### Task 7: Add Audit CLI Surface And Machine-Readable Output

**Files:**
- Create: `crates/lab/src/cli/audit.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/output.rs`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml cli::audit --all-features -- --nocapture`

- [ ] Add `audit` to the root CLI tree in `crates/lab/src/cli.rs`.
- [ ] Implement `lab audit onboarding <service...>` in `crates/lab/src/cli/audit.rs`. Thin shim — calls `audit::audit_services()`, formats output. No check logic here.
- [ ] Add a human-readable report that lists each check with its result.
- [ ] Add `--json` output using the `types.rs` report schema.
- [ ] Make the command exit non-zero when any audited service has at least one `Fail`. Propagate failure count through to `ExitCode::FAILURE` (same pattern as `lab doctor`).
- [ ] Add tests for:
  - one fully passing static report fixture
  - one failing fixture
  - JSON output shape
  - exit code is non-zero when any `Fail` is present
- [ ] Update [SERVICE_ONBOARDING.md](/home/jmagar/workspace/lab/docs/SERVICE_ONBOARDING.md) with concrete command examples for scaffold and audit.
- [ ] Update [docs/README.md](/home/jmagar/workspace/lab/docs/README.md) so the commands are discoverable from the docs index.
- [ ] Update `.claude/skills/lab-service-onboarding/SKILL.md` so it tells operators to scaffold first and audit before claiming alignment.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml cli::audit --all-features -- --nocapture`.
- [ ] Run `rtk rg -n 'lab scaffold service|lab audit onboarding|lab_admin' .claude/skills/lab-service-onboarding/SKILL.md docs -g '*.md'`.

### Task 8: Expose Audit Through An Internal MCP Tool

> **Scope:** Only `onboarding.audit` is exposed over MCP. `service.scaffold` is CLI-only until MCP elicitation for destructive actions is implemented. See Task 1 design note.
>
> **Registration:** `lab_admin` appears in the MCP registry but NOT in the HTTP router (file-write operations must not be reachable over HTTP). The existing `registry_and_router_service_sets_are_identical` test must be updated to add `lab_admin` to a named exemption set with a comment explaining why it is MCP-only. Add this exemption before the first CI run.
>
> **No service contract:** No `lab-apis` backing, no `PluginMeta`, no feature flag, no `client_from_env`/`require_client`/`not_configured_error` helpers (explicitly waived — there is no external service). Register manually using the inline pattern (same as `extract`), not via `register_service!`.
>
> **Opt-in only:** Register only when `LAB_ADMIN_ENABLED=1` env var is set. Do not register in `build_default_registry()` unconditionally.

**Files:**
- Create: `crates/lab/src/mcp/services/lab_admin.rs`
- Modify: `crates/lab/src/mcp/services.rs`
- Modify: `crates/lab/src/mcp/registry.rs`
- Modify: `crates/lab/src/catalog.rs`
- Test: `cargo test --manifest-path crates/lab/Cargo.toml mcp::services::lab_admin --all-features -- --nocapture`

- [ ] Add a new internal MCP tool named `lab_admin` in `mcp/services/lab_admin.rs`. This file is a thin dispatch shim — calls `audit::audit_services()`. No check logic here.
- [ ] Implement `onboarding.audit` action. This is the only action exposed over MCP in this iteration.
- [ ] Register `lab_admin` in the MCP registry only when `LAB_ADMIN_ENABLED=1` is set.
- [ ] Add `lab_admin` to the named exemption set in `registry_and_router_service_sets_are_identical` with an explanatory comment.
- [ ] Add the tool to the shared catalog (it should appear in `lab help` output).
- [ ] Add MCP adapter tests for `onboarding.audit`.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml mcp::services::lab_admin --all-features -- --nocapture`.

### Task 9: End-To-End Verification

**Files:** Test only

- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml scaffold --all-features -- --nocapture`.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml audit --all-features -- --nocapture`.
- [ ] Run `cargo test --manifest-path crates/lab/Cargo.toml mcp::services::lab_admin --all-features -- --nocapture`.
- [ ] Run `cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features`.
- [ ] Run `cargo check --manifest-path crates/lab/Cargo.toml --all-features`.
- [ ] Run `lab scaffold service sampleaudit --dry-run --json` and assert zero files written.
- [ ] Run `lab scaffold service sampleaudit --yes --json` and verify generated file list.
- [ ] Run `lab scaffold service sampleaudit --yes --json` a second time and verify all operations are no-ops (idempotency).
- [ ] Run `lab audit onboarding gotify bytestash tei --json`.
- [ ] Verify exit code is non-zero when an audited service has a failing check.

---

### Deferred (do not implement in this plan)

The following items were considered and explicitly deferred. Implement only if a concrete use case arises.

- **`--verify-live` / live verification checks:** Duplicates `just test-integration`. Live checks require running services, spawn 3 subprocesses per service, and carry subprocess injection risk. The existing integration test suite owns this problem space. If live verification is needed in the audit tool later, add it as a separate plan.
- **`service.scaffold` over MCP (`lab_admin`):** Scaffold writes source files — it is `destructive: true`. MCP elicitation for destructive actions is a documented known gap. Do not expose scaffold writes over MCP until elicitation is implemented.
- **Heuristic pattern checks in audit:** Regex-over-source for `surface="api"`, AppState reuse, catalog handling, and error type placement are fragile, produce false positives on comments, and are not CI-safe. Drop permanently unless a concrete gap is found that exact checks cannot cover.
- **`warn` severity level in audit results:** Unactionable in CI (does it block the merge or not?). Start with `Pass`/`Fail`/`Skip`. Revisit if policy evolves.

---

### Notes For The Implementer

- Treat [SERVICE_ONBOARDING.md](/home/jmagar/workspace/lab/docs/SERVICE_ONBOARDING.md) as the primary spec and cross-check it against:
  - [DISPATCH.md](/home/jmagar/workspace/lab/docs/DISPATCH.md)
  - [OBSERVABILITY.md](/home/jmagar/workspace/lab/docs/OBSERVABILITY.md)
  - [ERRORS.md](/home/jmagar/workspace/lab/docs/ERRORS.md)
  - [SERIALIZATION.md](../../design/SERIALIZATION.md)
- Do not model scaffold or audit as synthetic services and do not add them to `lab-apis`.
- `extract` remains a service-like capability because it manages external bootstrap discovery and env synthesis; scaffold and audit do not fit that contract.
- All verification is authoritative only in `--all-features` builds.
- The `extract.apply` implementation is the reference for patcher design: read → dedupe check → deterministic insert → atomic write (temp file + rename).
- Every generated template must be embedded via `include_str!()` — not loaded from a runtime path.
- Service name validation (`^[a-z][a-z0-9_]{1,63}$` + canonicalize check) must be the first step executed in every scaffold code path that touches the filesystem.
- Thin shim rule: `cli/scaffold.rs`, `cli/audit.rs`, and `mcp/services/lab_admin.rs` call the core modules and format output. Zero check logic, zero patcher logic, zero template logic in these files.
