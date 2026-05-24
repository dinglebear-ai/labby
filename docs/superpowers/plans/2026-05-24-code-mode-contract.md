# Code Mode Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the first Code Mode contract slice for the Lab gateway: canonical Code Mode tool IDs plus `code_search` and `code_schema` discovery tools that are distinct from `scout`/`invoke`.

**Architecture:** Keep Code Mode as a gateway-owned MCP meta-tool surface, parallel to but separate from `scout`/`invoke`. Add a small `dispatch/gateway/code_mode.rs` module for canonical IDs and schema summaries, then wire `code_search` and `code_schema` in `mcp/server.rs` using existing search and schema sources.

**Tech Stack:** Rust 2024, `rmcp`, `serde_json`, existing `GatewayManager`, existing `ToolRegistry`, `ActionSpec` / upstream `input_schema`, Cargo unit tests.

**Implementation status:** Completed in branch `feat/code-mode-contract`.

- Added canonical Code Mode id parsing and response structs.
- Added `code_search` and `code_schema` MCP meta-tools behind gateway tool-search mode.
- Added action-level built-in Code Mode search, upstream schema resolution, docs, and tests.
- Verified with focused unit tests, `cargo fmt --all -- --check`, and `cargo check --manifest-path crates/lab/Cargo.toml --all-features`.

---

## File Structure

- Create: `crates/lab/src/dispatch/gateway/code_mode.rs`
  - Owns canonical Code Mode IDs, parsing, generated-schema summary envelopes, and small conversion helpers.
- Modify: `crates/lab/src/dispatch/gateway.rs`
  - Exposes the new `code_mode` module.
- Modify: `crates/lab/src/mcp/catalog.rs`
  - Adds canonical MCP meta-tool names: `code_search` and `code_schema`.
- Modify: `crates/lab/src/mcp/server.rs`
  - Lists the two Code Mode tools when gateway tool-search mode is enabled.
  - Handles `code_search` by adapting existing `scout` search results into Code Mode candidates.
  - Handles `code_schema` by resolving a canonical ID to a Lab built-in action schema or an upstream tool schema.
- Modify: `docs/services/GATEWAY.md`
  - Documents the difference between `scout`/`invoke` and Code Mode discovery/schema flow.
- Modify: `docs/dev/ERRORS.md`
  - Documents `invalid_code_mode_id` if introduced by the parser.

## Contract Shape

The first implementation slice must expose two MCP tools:

```json
code_search({
  "query": "github issues",
  "top_k": 10,
  "detail": "brief"
})
```

```json
code_schema({
  "id": "lab::gateway.gateway.schema"
})
```

`code_search` returns JSON text containing an array:

```json
[
  {
    "id": "lab::gateway.gateway.schema",
    "name": "gateway",
    "upstream": "lab",
    "description": "Gateway management and discovery. Actions: gateway.schema, ...",
    "score": 42.0,
    "schema_available": true
  }
]
```

`code_schema` returns JSON text:

```json
{
  "id": "lab::gateway.gateway.schema",
  "kind": "lab_action",
  "name": "gateway.schema",
  "upstream": "lab",
  "schema": {
    "action": "gateway.schema",
    "description": "Return the discovered schema for one upstream gateway server",
    "destructive": false,
    "returns": "Value",
    "params": []
  },
  "schema_format": "lab_action_spec"
}
```

For upstream tools:

```json
{
  "id": "upstream::github::search_issues",
  "kind": "upstream_tool",
  "name": "search_issues",
  "upstream": "github",
  "schema": { "type": "object", "properties": {} },
  "schema_format": "json_schema"
}
```

## Canonical ID Rules

- Lab built-in action ID: `lab::<service>.<action>`
  - Example: `lab::gateway.gateway.schema`
  - Parser split: prefix `lab::`, then service before first `.`, action after first `.`.
- Upstream tool ID: `upstream::<upstream-name>::<tool-name>`
  - Example: `upstream::github::search_issues`
  - Parser split: prefix `upstream::`, then upstream before next `::`, tool after next `::`.
- Invalid or incomplete IDs return `invalid_code_mode_id`.

## Task 1: Add Canonical ID Parser and Tests

**Files:**
- Create: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/dispatch/gateway.rs`

- [ ] **Step 1: Write failing parser tests**

Add this test module to the new file:

```rust
#[cfg(test)]
mod tests {
    use super::{CodeModeToolId, CodeModeToolRef};

    #[test]
    fn parses_lab_action_id() {
        let parsed = CodeModeToolId::parse("lab::gateway.gateway.schema").unwrap();
        assert_eq!(
            parsed,
            CodeModeToolId {
                raw: "lab::gateway.gateway.schema".to_string(),
                reference: CodeModeToolRef::LabAction {
                    service: "gateway".to_string(),
                    action: "gateway.schema".to_string(),
                },
            }
        );
    }

    #[test]
    fn parses_upstream_tool_id() {
        let parsed = CodeModeToolId::parse("upstream::github::search_issues").unwrap();
        assert_eq!(
            parsed,
            CodeModeToolId {
                raw: "upstream::github::search_issues".to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: "github".to_string(),
                    tool: "search_issues".to_string(),
                },
            }
        );
    }

    #[test]
    fn rejects_invalid_ids() {
        for id in ["", "gateway.gateway.schema", "lab::gateway", "upstream::github", "upstream::::tool"] {
            assert!(CodeModeToolId::parse(id).is_err(), "{id} should be invalid");
        }
    }
}
```

- [ ] **Step 2: Run the parser tests to verify they fail**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
```

Expected: FAIL because `code_mode` module and types do not exist.

- [ ] **Step 3: Implement the parser**

Create `crates/lab/src/dispatch/gateway/code_mode.rs` with:

```rust
use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeToolId {
    pub raw: String,
    pub reference: CodeModeToolRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeToolRef {
    LabAction { service: String, action: String },
    UpstreamTool { upstream: String, tool: String },
}

impl CodeModeToolId {
    pub fn parse(raw: &str) -> Result<Self, ToolError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid_code_mode_id("Code Mode tool id must not be empty"));
        }

        if let Some(rest) = raw.strip_prefix("lab::") {
            let (service, action) = rest
                .split_once('.')
                .ok_or_else(|| invalid_code_mode_id("lab Code Mode ids must use lab::<service>.<action>"))?;
            if service.trim().is_empty() || action.trim().is_empty() {
                return Err(invalid_code_mode_id("lab Code Mode ids must include service and action"));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::LabAction {
                    service: service.trim().to_string(),
                    action: action.trim().to_string(),
                },
            });
        }

        if let Some(rest) = raw.strip_prefix("upstream::") {
            let (upstream, tool) = rest
                .split_once("::")
                .ok_or_else(|| invalid_code_mode_id("upstream Code Mode ids must use upstream::<upstream>::<tool>"))?;
            if upstream.trim().is_empty() || tool.trim().is_empty() {
                return Err(invalid_code_mode_id("upstream Code Mode ids must include upstream and tool"));
            }
            return Ok(Self {
                raw: raw.to_string(),
                reference: CodeModeToolRef::UpstreamTool {
                    upstream: upstream.trim().to_string(),
                    tool: tool.trim().to_string(),
                },
            });
        }

        Err(invalid_code_mode_id("Code Mode ids must start with lab:: or upstream::"))
    }
}

pub fn lab_action_id(service: &str, action: &str) -> String {
    format!("lab::{service}.{action}")
}

pub fn upstream_tool_id(upstream: &str, tool: &str) -> String {
    format!("upstream::{upstream}::{tool}")
}

pub fn invalid_code_mode_id(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_code_mode_id".to_string(),
        message: message.into(),
    }
}
```

Expose it in `crates/lab/src/dispatch/gateway.rs`:

```rust
pub mod code_mode;
```

- [ ] **Step 4: Run the parser tests to verify they pass**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway.rs crates/lab/src/dispatch/gateway/code_mode.rs
git commit -m "feat(code-mode): add canonical tool ids"
```

## Task 2: Add Code Mode Search Contract

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/mcp/catalog.rs`
- Modify: `crates/lab/src/mcp/server.rs`

- [ ] **Step 1: Write failing unit tests for Code Mode search results**

Append to `code_mode.rs` tests:

```rust
use serde_json::json;

#[test]
fn builds_search_candidate_for_lab_action() {
    let candidate = super::CodeModeSearchCandidate::lab_action(
        "gateway",
        "gateway.schema",
        "Return gateway schema",
        10.0,
    );
    assert_eq!(candidate.id, "lab::gateway.gateway.schema");
    assert_eq!(candidate.upstream, "lab");
    assert_eq!(candidate.name, "gateway.schema");
    assert!(candidate.schema_available);
}

#[test]
fn builds_search_candidate_for_upstream_tool() {
    let candidate = super::CodeModeSearchCandidate::upstream_tool(
        "github",
        "search_issues",
        "Search issues",
        8.5,
        Some(json!({"type": "object"})),
    );
    assert_eq!(candidate.id, "upstream::github::search_issues");
    assert_eq!(candidate.upstream, "github");
    assert_eq!(candidate.name, "search_issues");
    assert!(candidate.schema_available);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
```

Expected: FAIL because `CodeModeSearchCandidate` does not exist.

- [ ] **Step 3: Implement search candidate type**

Add to `code_mode.rs`:

```rust
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSearchCandidate {
    pub id: String,
    pub name: String,
    pub upstream: String,
    pub description: String,
    pub score: f32,
    pub schema_available: bool,
}

impl CodeModeSearchCandidate {
    pub fn lab_action(service: &str, action: &str, description: &str, score: f32) -> Self {
        Self {
            id: lab_action_id(service, action),
            name: action.to_string(),
            upstream: "lab".to_string(),
            description: description.to_string(),
            score,
            schema_available: true,
        }
    }

    pub fn upstream_tool(
        upstream: &str,
        tool: &str,
        description: &str,
        score: f32,
        schema: Option<Value>,
    ) -> Self {
        Self {
            id: upstream_tool_id(upstream, tool),
            name: tool.to_string(),
            upstream: upstream.to_string(),
            description: description.to_string(),
            score,
            schema_available: schema.is_some(),
        }
    }
}
```

- [ ] **Step 4: Add MCP catalog names**

In `crates/lab/src/mcp/catalog.rs`, add:

```rust
pub const CODE_SEARCH_TOOL_NAME: &str = "code_search";
pub const CODE_SCHEMA_TOOL_NAME: &str = "code_schema";
```

Do not remove or rename `TOOL_SEARCH_TOOL_NAME` or `TOOL_EXECUTE_TOOL_NAME`.

- [ ] **Step 5: Wire `code_search` into `list_tools` and `call_tool`**

In `server.rs`, import the names:

```rust
CODE_SCHEMA_TOOL_NAME, CODE_SEARCH_TOOL_NAME,
```

Add two schemas near the existing `scout`/`invoke` tool definitions:

```rust
let code_search_schema = match serde_json::json!({
    "type": "object",
    "properties": {
        "query": { "type": "string", "maxLength": 500 },
        "top_k": { "type": "integer", "minimum": 1, "maximum": 50 },
        "detail": {
            "type": "string",
            "enum": ["brief", "detailed", "full"],
            "default": "brief"
        }
    },
    "required": ["query"]
}) {
    Value::Object(map) => Arc::new(map),
    _ => unreachable!("code_search schema must be an object"),
};
tools.push(Tool::new(
    CODE_SEARCH_TOOL_NAME,
    "Search Lab Code Mode candidates. Returns canonical ids for use with code_schema. This is schema-first discovery, not execution.",
    code_search_schema,
));
gateway_tool_count += 1;
```

Add a `call_tool` branch before normal service dispatch:

```rust
if service == CODE_SEARCH_TOOL_NAME {
    let started = Instant::now();
    let subject = self.request_subject_log_tag(&context);
    let auth = auth_context_from_extensions(&context.extensions);
    if !tool_search_scope_allowed(auth) {
        let env = build_error_extra(
            &service,
            "call_tool",
            "forbidden",
            "code_search requires one of scopes: lab:read, lab, lab:admin",
            &serde_json::json!({ "required_scopes": ["lab:read", "lab", "lab:admin"] }),
        );
        return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
    }
    let query = args.get("query").and_then(Value::as_str).unwrap_or_default().to_string();
    let top_k = args.get("top_k").and_then(Value::as_u64).map_or(10, |value| value as usize);
    let score_floor_fraction = self
        .gateway_manager
        .as_ref()
        .map(|manager| async move { manager.tool_search_config().await.score_floor_fraction });
    let score_floor_fraction = match score_floor_fraction {
        Some(future) => future.await,
        None => 0.0,
    };
    let builtin = self
        .search_builtin_tools(&query, top_k, false, score_floor_fraction)
        .await
        .into_iter()
        .map(|result| crate::dispatch::gateway::code_mode::CodeModeSearchCandidate::lab_action(
            &result.name,
            "",
            &result.description,
            result.score,
        ))
        .collect::<Vec<_>>();
    let mut candidates = builtin;
    if let Some(manager) = &self.gateway_manager
        && let Ok(upstream_results) = manager.search_tools(&query, top_k, true).await
    {
        candidates.extend(upstream_results.into_iter().map(|result| {
            crate::dispatch::gateway::code_mode::CodeModeSearchCandidate::upstream_tool(
                &result.upstream,
                &result.name,
                &result.description,
                result.score,
                result.input_schema,
            )
        }));
    }
    candidates.truncate(top_k.max(1).min(50));
    tracing::info!(
        surface = "mcp",
        service = "code_mode",
        action = "code_search",
        subject,
        result_count = candidates.len(),
        elapsed_ms = started.elapsed().as_millis(),
        "code mode search ok"
    );
    return Ok(CallToolResult::success(vec![Content::text(
        serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".to_string()),
    )]));
}
```

After adding this, fix the built-in mapping so it uses real action names. Replace the temporary `lab_action(&result.name, "", ...)` mapping by adding a helper in `server.rs`:

```rust
async fn search_builtin_code_mode_candidates(
    &self,
    query: &str,
    top_k: usize,
    score_floor_fraction: f32,
) -> Vec<crate::dispatch::gateway::code_mode::CodeModeSearchCandidate> {
    let results = self
        .search_builtin_tools(query, top_k, false, score_floor_fraction)
        .await;
    let mut candidates = Vec::new();
    for result in results {
        let Some(service) = self.registry.services().iter().find(|service| service.name == result.name) else {
            continue;
        };
        for action in self.searchable_builtin_actions(service).await {
            candidates.push(crate::dispatch::gateway::code_mode::CodeModeSearchCandidate::lab_action(
                service.name,
                action.name,
                action.description,
                result.score,
            ));
        }
    }
    candidates.truncate(top_k.max(1).min(50));
    candidates
}
```

Use that helper in the `code_search` branch.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
cargo test -p labby --lib tool_search_indexes_builtin_lab_services --all-features
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs crates/lab/src/mcp/catalog.rs crates/lab/src/mcp/server.rs
git commit -m "feat(code-mode): add schema-first search surface"
```

## Task 3: Add Code Mode Schema Contract

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode.rs`
- Modify: `crates/lab/src/mcp/server.rs`
- Modify: `docs/dev/ERRORS.md`

- [ ] **Step 1: Write failing schema envelope tests**

Append to `code_mode.rs` tests:

```rust
#[test]
fn builds_lab_schema_response() {
    let response = super::CodeModeSchemaResponse::lab_action(
        "lab::gateway.gateway.schema",
        "gateway.schema",
        json!({"action": "gateway.schema"}),
    );
    assert_eq!(response.kind, "lab_action");
    assert_eq!(response.schema_format, "lab_action_spec");
}

#[test]
fn builds_upstream_schema_response() {
    let response = super::CodeModeSchemaResponse::upstream_tool(
        "upstream::github::search_issues",
        "github",
        "search_issues",
        json!({"type": "object"}),
    );
    assert_eq!(response.kind, "upstream_tool");
    assert_eq!(response.schema_format, "json_schema");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
```

Expected: FAIL because `CodeModeSchemaResponse` does not exist.

- [ ] **Step 3: Implement schema response type**

Add to `code_mode.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSchemaResponse {
    pub id: String,
    pub kind: &'static str,
    pub name: String,
    pub upstream: String,
    pub schema: Value,
    pub schema_format: &'static str,
}

impl CodeModeSchemaResponse {
    pub fn lab_action(id: &str, action: &str, schema: Value) -> Self {
        Self {
            id: id.to_string(),
            kind: "lab_action",
            name: action.to_string(),
            upstream: "lab".to_string(),
            schema,
            schema_format: "lab_action_spec",
        }
    }

    pub fn upstream_tool(id: &str, upstream: &str, tool: &str, schema: Value) -> Self {
        Self {
            id: id.to_string(),
            kind: "upstream_tool",
            name: tool.to_string(),
            upstream: upstream.to_string(),
            schema,
            schema_format: "json_schema",
        }
    }
}
```

- [ ] **Step 4: Add `code_schema` MCP tool listing**

In `list_tools`, add:

```rust
let code_schema_schema = match serde_json::json!({
    "type": "object",
    "properties": {
        "id": {
            "type": "string",
            "description": "Canonical Code Mode id returned by code_search, e.g. lab::gateway.gateway.schema or upstream::github::search_issues."
        }
    },
    "required": ["id"]
}) {
    Value::Object(map) => Arc::new(map),
    _ => unreachable!("code_schema schema must be an object"),
};
tools.push(Tool::new(
    CODE_SCHEMA_TOOL_NAME,
    "Fetch the schema envelope for one canonical Code Mode id returned by code_search. Use this before generating code.",
    code_schema_schema,
));
gateway_tool_count += 1;
```

- [ ] **Step 5: Add `code_schema` handler**

In `call_tool`, before normal service dispatch:

```rust
if service == CODE_SCHEMA_TOOL_NAME {
    let id = args.get("id").and_then(Value::as_str).unwrap_or_default();
    let parsed = match crate::dispatch::gateway::code_mode::CodeModeToolId::parse(id) {
        Ok(parsed) => parsed,
        Err(err) => {
            let env = build_error(&service, "call_tool", err.kind(), &err.to_string());
            return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
        }
    };
    match parsed.reference {
        crate::dispatch::gateway::code_mode::CodeModeToolRef::LabAction { service: ref target_service, action: ref target_action } => {
            let Some(entry) = self.registry.services().iter().find(|entry| entry.name == target_service) else {
                let env = build_error(&service, "call_tool", "unknown_tool", &format!("unknown Lab service `{target_service}`"));
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            };
            let schema = match crate::dispatch::helpers::action_schema(entry.actions, target_action) {
                Ok(schema) => schema,
                Err(err) => {
                    let env = build_error(&service, "call_tool", err.kind(), &err.to_string());
                    return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
                }
            };
            let response = crate::dispatch::gateway::code_mode::CodeModeSchemaResponse::lab_action(
                &parsed.raw,
                target_action,
                schema,
            );
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string()),
            )]));
        }
        crate::dispatch::gateway::code_mode::CodeModeToolRef::UpstreamTool { ref upstream, ref tool } => {
            let Some(pool) = self.current_upstream_pool().await else {
                let env = build_error(&service, "call_tool", "upstream_error", "upstream pool is not available");
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            };
            let matches = pool
                .healthy_tools_for_upstream(upstream)
                .await
                .into_iter()
                .filter(|candidate| candidate.tool.name.as_ref() == tool)
                .collect::<Vec<_>>();
            let Some(candidate) = matches.into_iter().next() else {
                let env = build_error(&service, "call_tool", "unknown_tool", &format!("unknown upstream tool `{}`", parsed.raw));
                return Ok(CallToolResult::error(vec![Content::text(env.to_string())]));
            };
            let schema = candidate
                .input_schema
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
            let response = crate::dispatch::gateway::code_mode::CodeModeSchemaResponse::upstream_tool(
                &parsed.raw,
                upstream,
                tool,
                schema,
            );
            return Ok(CallToolResult::success(vec![Content::text(
                serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string()),
            )]));
        }
    }
}
```

- [ ] **Step 6: Document error kind**

In `docs/dev/ERRORS.md`, under dispatcher-level kinds add:

```markdown
- `invalid_code_mode_id` — Code Mode schema lookup received an id that does not match `lab::<service>.<action>` or `upstream::<upstream>::<tool>`. HTTP 400.
```

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
cargo test -p labby --lib invoke_ambiguous_tool_error_envelope_guides_retry --all-features
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/gateway/code_mode.rs crates/lab/src/mcp/server.rs docs/dev/ERRORS.md
git commit -m "feat(code-mode): add schema lookup surface"
```

## Task 4: Document Gateway Code Mode Discovery

**Files:**
- Modify: `docs/services/GATEWAY.md`

- [ ] **Step 1: Add documentation section**

Add this section near the existing gateway tool-search section:

```markdown
### Code Mode Discovery

Code Mode is a schema-first companion to `scout` and `invoke`. `scout` finds tools and `invoke` executes one selected tool call. Code Mode adds a safer programmatic path for agents that need to generate code from schemas:

1. `code_search` returns canonical ids for candidate Lab actions and upstream tools.
2. `code_schema` returns the schema envelope for one canonical id.
3. Future Code Mode execution must use those ids and re-check the same gateway auth, exposure, and destructive-action policies as normal invocation.

Canonical ids are stable strings:

- Lab built-in action: `lab::<service>.<action>`, for example `lab::gateway.gateway.schema`.
- Upstream tool: `upstream::<upstream-name>::<tool-name>`, for example `upstream::github::search_issues`.

Code Mode does not replace `scout` or `invoke`; it is a separate opt-in contract for schema-first generated-code workflows.
```

- [ ] **Step 2: Verify docs mention all new tool names**

Run:

```bash
rg -n "code_search|code_schema|Code Mode" docs/services/GATEWAY.md
```

Expected: Shows all three terms in the new section.

- [ ] **Step 3: Commit**

```bash
git add docs/services/GATEWAY.md
git commit -m "docs: explain gateway code mode discovery"
```

## Task 5: Final Verification

**Files:**
- No new files. Verify all changed files.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --check
```

Expected: PASS.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test -p labby --lib dispatch::gateway::code_mode::tests --all-features
cargo test -p labby --lib tool_search_indexes_builtin_lab_services --all-features
cargo test -p labby --lib invoke_ambiguous_tool_error_envelope_guides_retry --all-features
```

Expected: PASS.

- [ ] **Step 3: Run cargo check**

Run:

```bash
cargo check
```

Expected: PASS. Existing warnings may appear; do not expand scope unless a new warning is introduced by Code Mode files.

- [ ] **Step 4: Validate bead remains ready for next wave**

Run:

```bash
bd swarm validate lab-le0w0
```

Expected: PASS. `lab-le0w0.1` may remain open until PR merge, but the DAG should still be valid.

- [ ] **Step 5: Commit final verification note if needed**

If verification only changes no files, do not commit. If docs or generated artifacts changed, commit:

```bash
git add <changed-files>
git commit -m "chore(code-mode): refresh verification artifacts"
```

## Self-Review

- Spec coverage: The plan implements only `lab-le0w0.1`, the first ready Code Mode slice. It intentionally does not implement generated TS bindings, sandbox execution, broker policy, or rollout config; those remain in `lab-le0w0.2` through `lab-le0w0.5`.
- Placeholder scan: No task uses TBD/TODO/fill-in instructions. Every code step contains exact code or exact command.
- Type consistency: `CodeModeToolId`, `CodeModeToolRef`, `CodeModeSearchCandidate`, and `CodeModeSchemaResponse` names are consistent across tasks.
