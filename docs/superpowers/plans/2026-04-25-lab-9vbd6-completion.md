# lab-9vbd.6 Completion Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Finish bead `lab-9vbd.6` by caching sorted unique action names in the MCP registry and restoring prompt argument completion to use that cache.

**Architecture:** `ToolRegistry` owns the startup-built service catalog, so it will maintain a sorted, deduplicated action-name cache as services are registered. `LabMcpServer::complete` will advertise and handle MCP prompt argument completions, using the registry cache for the `run-action.action` argument and lightweight service-name completion for prompt service arguments.

**Tech Stack:** Rust 2024, rmcp 1.4 `ServerHandler::complete`, `lab_apis::core::action::ActionSpec`, existing `ToolRegistry` and `LabMcpServer`.

---

### File Structure

- Modify `crates/lab/src/registry.rs`: add cached sorted unique action names to `ToolRegistry`, expose cached action-name accessors, and add focused tests for cache order, deduplication, old-output equivalence, and empty-prefix timing.
- Modify `crates/lab/src/mcp/server.rs`: advertise completion capability, implement `ServerHandler::complete`, route prompt argument completions, and add tests proving action completion uses the cached list.
- Create `docs/superpowers/plans/2026-04-25-lab-9vbd6-completion.md`: this implementation plan.
- Create `docs/sessions/2026-04-25-lab-9vbd6-completion.md`: final factual session report.

### Task 1: Add registry action-name cache tests

**Files:**
- Modify: `crates/lab/src/registry.rs`

- [x] **Step 1: Add tests before production changes**

Add tests in `crates/lab/src/registry.rs` that:
- Register services with unsorted and duplicate action names.
- Assert `ToolRegistry::action_names()` is sorted and deduplicated.
- Assert `ToolRegistry::action_name_completions("")` equals the previous per-request algorithm of collect/sort/dedup.
- Assert `build_default_registry().action_name_completions("")` returns all cached actions in under 1ms.

- [x] **Step 2: Run focused registry tests and confirm RED**

Run: `cargo test -p lab --lib registry::tests::action --all-features`

Expected: FAIL because `ToolRegistry::action_names` and `ToolRegistry::action_name_completions` do not exist yet.

### Task 2: Implement registry action-name cache

**Files:**
- Modify: `crates/lab/src/registry.rs`

- [x] **Step 1: Add cache field and accessors**

Add `action_names: Vec<&'static str>` to `ToolRegistry`, initialize it in `new()`, and expose:

```rust
pub fn action_names(&self) -> &[&'static str]
pub fn action_name_completions(&self, prefix: &str) -> Vec<String>
```

- [x] **Step 2: Maintain sorted unique cache during registration**

When `register()` accepts a non-duplicate service, insert each `service.actions[*].name` into the cache with `binary_search()` and `Vec::insert()` only when absent.

- [x] **Step 3: Run focused registry tests and confirm GREEN**

Run: `cargo test -p lab --lib registry::tests::action --all-features`

Expected: PASS.

### Task 3: Restore MCP prompt completion handler

**Files:**
- Modify: `crates/lab/src/mcp/server.rs`

- [x] **Step 1: Add completion handler tests before production changes**

Add tests in `crates/lab/src/mcp/server.rs` that prove:
- Server capabilities advertise completions.
- Completing `run-action.action` with an empty prefix returns the registry cached action list.
- Completing `run-action.action` with a prefix returns only matching cached action names.
- Completing `run-action.service` and `service-discover.service` returns matching service names.

- [x] **Step 2: Run focused MCP completion tests and confirm RED**

Run: `cargo test -p lab --lib mcp::server::tests::completion --all-features`

Expected: FAIL because completions are not advertised and the completion helper/handler is absent.

- [x] **Step 3: Implement rmcp completion support**

Import rmcp completion types, add `.enable_completions()` to `get_info()`, implement `async fn complete(...)`, and route prompt references as follows:
- `run-action.action`: use `registry.action_name_completions(prefix)`.
- `run-action.service` and `service-discover.service`: return service names matching the prefix.
- Other prompts, arguments, and resource references: return an empty completion result.

- [x] **Step 4: Run focused MCP completion tests and confirm GREEN**

Run: `cargo test -p lab --lib mcp::server::tests::completion --all-features`

Expected: PASS.

### Task 4: Verify bead completion

**Files:**
- No additional source edits expected.

- [x] **Step 1: Run focused tests**

Run:

```bash
cargo test -p lab --lib registry::tests::action --all-features
cargo test -p lab --lib mcp::server::tests::completion --all-features
```

Expected: both PASS.

- [x] **Step 2: Run relevant all-features checks**

Run:

```bash
cargo check -p lab --all-features
cargo clippy -p lab --all-features -- -D warnings
cargo test -p lab --all-features --lib registry::tests::action
cargo test -p lab --all-features --lib mcp::server::tests::completion
```

Expected: all PASS.

- [x] **Step 3: Confirm required evidence**

Record evidence that:
- Action names are cached at registry registration/build time.
- Completion uses the cached list.
- Cached output equals the former collect/sort/dedup output.
- Empty-prefix cached completion returns all action names in under 1ms.

### Task 5: Write session report

**Files:**
- Create: `docs/sessions/2026-04-25-lab-9vbd6-completion.md`

- [x] **Step 1: Gather required session metadata commands**

Run the exact commands requested by the user and include their outputs or concise factual summaries in the report.

- [x] **Step 2: Write the report**

Create the Markdown report with YAML metadata and the required sections: User Request, Session Overview, Sequence of Events, Key Findings, Technical Decisions, Files Modified, Commands Executed, Errors Encountered if any, Behavior Changes, Verification Evidence, Risks and Rollback, Decisions Not Taken if any, References if any, Open Questions if any, Next Steps.
