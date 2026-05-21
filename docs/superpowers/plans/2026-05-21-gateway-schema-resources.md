# Gateway Schema Resources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose each upstream MCP server's cached tool catalog as synthetic `lab://gateway/*` MCP resources (plus matching gateway dispatch actions), so agents can fetch a server's full tool schema in one read instead of round-tripping through `tool_search`.

**Architecture:** Three thin layers. The upstream pool already caches schemas per `UpstreamEntry`; new pool methods reshape that cache into two synthetic JSON documents. Two new gateway dispatch actions wrap those methods for HTTP/CLI/MCP-action access. The MCP `read_resource`/`list_resources` handlers gain a `lab://gateway/` branch that calls the same pool methods directly (no business logic on the MCP path). One architecture test pins the URI scheme and JSON shape.

**Tech Stack:** Rust 2024, tokio, rmcp, axum, serde_json, wiremock (unused here — pool tests construct `UpstreamEntry` directly).

**Spec:** `docs/specs/gateway-schema-resources.md`
**Contract:** `docs/contracts/gateway-schema-resources.md`

---

## File Structure

| File | Status | Responsibility |
|------|--------|----------------|
| `crates/lab/src/dispatch/upstream/pool.rs` | Modify | Add three pool methods: `gateway_synthetic_resources`, `gateway_servers_doc`, `gateway_server_schema`, plus a private `health_str` helper. Add unit tests. |
| `crates/lab/src/dispatch/gateway/manager.rs` | Modify | Add `gateway_servers_doc()` and `gateway_server_schema(name)` wrappers that call the pool methods through `runtime.current_pool()`. |
| `crates/lab/src/dispatch/gateway/dispatch.rs` | Modify | Wire two new actions: `gateway.servers` and `gateway.schema`. |
| `crates/lab/src/dispatch/gateway/catalog.rs` | Modify | Register the two new actions in `ACTIONS` with destructive=false. |
| `crates/lab/src/mcp/server.rs` | Modify | Extend `list_resources` with synthetic entries; add `lab://gateway/` branch to `read_resource`. |
| `crates/lab/tests/gateway_schema_resources.rs` | Create | Architecture test pinning URI scheme, JSON document shape, exposure-policy filtering. |
| `docs/surfaces/MCP.md` | Modify | Document the new resource URIs alongside `lab://catalog` and `lab://upstream/...`. |

Each file change is self-contained and committable. The plan is ordered bottom-up: pool methods → dispatch wrapper → MCP surface → arch test → docs.

---

## Task 1: Pool methods — synthetic gateway documents

**Files:**
- Modify: `crates/lab/src/dispatch/upstream/pool.rs` (add methods + tests)

The pool's `catalog: Arc<RwLock<HashMap<String, UpstreamEntry>>>` already
holds every cached tool schema. These methods reshape that data into the
JSON documents defined in the contract — no upstream calls, no new
caching.

- [ ] **Step 1: Write a failing test for `gateway_servers_doc` with one healthy upstream**

Append to the `#[cfg(test)] mod tests` block at the bottom of `crates/lab/src/dispatch/upstream/pool.rs`:

```rust
#[tokio::test]
async fn gateway_servers_doc_lists_one_healthy_upstream() {
    use rmcp::model::Tool;
    use std::borrow::Cow;
    use std::sync::Arc;

    let pool = UpstreamPool::new();
    let mut tools = HashMap::new();
    tools.insert(
        "search".to_string(),
        UpstreamTool {
            tool: Tool {
                name: Cow::Borrowed("search"),
                title: None,
                description: Some(Cow::Borrowed("search the index")),
                input_schema: Arc::new(serde_json::Map::new()),
                output_schema: None,
                annotations: None,
                icons: None,
                meta: None,
            },
            input_schema: Some(serde_json::json!({"type": "object"})),
            upstream_name: Arc::from("alpha"),
        },
    );
    let entry = healthy_in_process_entry(Arc::from("alpha"), tools);
    pool.catalog.write().await.insert("alpha".to_string(), entry);

    let doc = pool.gateway_servers_doc().await;
    let servers = doc.get("servers").and_then(|v| v.as_array()).expect("servers array");
    assert_eq!(servers.len(), 1);
    let s = &servers[0];
    assert_eq!(s["name"], "alpha");
    assert_eq!(s["tool_count"], 1);
    assert_eq!(s["tool_health"], "healthy");
    assert!(s["tool_last_error"].is_null());
    assert_eq!(s["prompt_count"], 0);
    assert_eq!(s["resource_count"], 0);
}
```

- [ ] **Step 2: Run the test, confirm it fails**

```bash
cargo nextest run -p lab --all-features gateway_servers_doc_lists_one_healthy_upstream 2>&1 | tail -20
```

Expected: compile error — `gateway_servers_doc` not found on `UpstreamPool`.

- [ ] **Step 3: Implement `health_str` + `gateway_servers_doc`**

Add to the `impl UpstreamPool` block (locate `pub async fn upstream_status` around line 2209 and add these methods after it). First a free function above the impl:

```rust
fn health_str(health: UpstreamHealth) -> &'static str {
    match health {
        UpstreamHealth::Healthy => "healthy",
        UpstreamHealth::Unhealthy { consecutive_failures }
            if consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD => "open",
        UpstreamHealth::Unhealthy { .. } => "degraded",
    }
}
```

Then inside `impl UpstreamPool`:

```rust
/// Render the synthetic `lab://gateway/servers` document.
///
/// Lists every registered upstream (regardless of health) with the
/// tool count an agent would see in the corresponding schema document.
pub async fn gateway_servers_doc(&self) -> Value {
    let catalog = self.catalog.read().await;
    let mut servers: Vec<Value> = catalog
        .iter()
        .map(|(name, e)| {
            let tool_count = e
                .tools
                .values()
                .filter(|t| e.exposure_policy.matches(&t.tool.name))
                .count();
            serde_json::json!({
                "name": name,
                "tool_count": tool_count,
                "prompt_count": e.prompt_count,
                "resource_count": e.resource_count,
                "tool_health": health_str(e.tool_health),
                "tool_last_error": e.tool_last_error,
            })
        })
        .collect();
    servers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    serde_json::json!({ "servers": servers })
}
```

Note: sorting is for test stability; the contract explicitly says order is not stable, but a deterministic sort makes assertions easier and costs nothing.

- [ ] **Step 4: Run the test, confirm it passes**

```bash
cargo nextest run -p lab --all-features gateway_servers_doc_lists_one_healthy_upstream 2>&1 | tail -10
```

Expected: 1 test passed.

- [ ] **Step 5: Add a failing test for `gateway_server_schema` with exposure filtering**

Append to the same test module:

```rust
#[tokio::test]
async fn gateway_server_schema_respects_exposure_policy() {
    use rmcp::model::Tool;
    use std::borrow::Cow;
    use std::sync::Arc;

    let make_tool = |name: &'static str| UpstreamTool {
        tool: Tool {
            name: Cow::Borrowed(name),
            title: None,
            description: Some(Cow::Borrowed("desc")),
            input_schema: Arc::new(serde_json::Map::new()),
            output_schema: None,
            annotations: None,
            icons: None,
            meta: None,
        },
        input_schema: Some(serde_json::json!({"type": "object"})),
        upstream_name: Arc::from("alpha"),
    };

    let mut tools = HashMap::new();
    tools.insert("github_create".into(), make_tool("github_create"));
    tools.insert("delete_repo".into(),  make_tool("delete_repo"));

    let mut entry = healthy_in_process_entry(Arc::from("alpha"), tools);
    entry.exposure_policy = ToolExposurePolicy::from_patterns(vec!["github_*".into()])
        .expect("policy");

    let pool = UpstreamPool::new();
    pool.catalog.write().await.insert("alpha".to_string(), entry);

    let doc = pool.gateway_server_schema("alpha").await.expect("doc");
    let names: Vec<&str> = doc["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["github_create"]);
    assert_eq!(doc["health"], "healthy");
    assert!(doc["last_error"].is_null());
    assert_eq!(doc["name"], "alpha");
}

#[tokio::test]
async fn gateway_server_schema_unknown_upstream_returns_none() {
    let pool = UpstreamPool::new();
    assert!(pool.gateway_server_schema("nope").await.is_none());
}
```

- [ ] **Step 6: Run, confirm both fail**

```bash
cargo nextest run -p lab --all-features gateway_server_schema 2>&1 | tail -15
```

Expected: compile error — `gateway_server_schema` not found.

- [ ] **Step 7: Implement `gateway_server_schema`**

Add to `impl UpstreamPool`, immediately after `gateway_servers_doc`:

```rust
/// Render the synthetic `lab://gateway/<name>/schema` document.
///
/// Returns `None` when the upstream is not registered. Tools hidden by
/// the upstream's `ToolExposurePolicy` are omitted. `input_schema` and
/// `meta` are passed through verbatim from the cached tool definition.
pub async fn gateway_server_schema(&self, name: &str) -> Option<Value> {
    let catalog = self.catalog.read().await;
    let entry = catalog.get(name)?;
    let mut tools: Vec<Value> = entry
        .tools
        .values()
        .filter(|t| entry.exposure_policy.matches(&t.tool.name))
        .map(|t| {
            serde_json::json!({
                "name": t.tool.name.as_ref(),
                "description": t.tool.description.as_ref().map(|s| s.as_ref()),
                "input_schema": t.input_schema,
                "meta": t.tool.meta,
            })
        })
        .collect();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Some(serde_json::json!({
        "name": name,
        "tools": tools,
        "health": health_str(entry.tool_health),
        "last_error": entry.tool_last_error,
    }))
}
```

- [ ] **Step 8: Run schema tests, confirm pass**

```bash
cargo nextest run -p lab --all-features gateway_server_schema 2>&1 | tail -10
```

Expected: 2 tests passed.

- [ ] **Step 9: Add a failing test for `gateway_synthetic_resources`**

Append to the same test module:

```rust
#[tokio::test]
async fn gateway_synthetic_resources_lists_index_and_per_upstream() {
    let pool = UpstreamPool::new();
    let entry = healthy_in_process_entry(Arc::from("alpha"), HashMap::new());
    pool.catalog.write().await.insert("alpha".to_string(), entry);
    let entry = healthy_in_process_entry(Arc::from("beta"), HashMap::new());
    pool.catalog.write().await.insert("beta".to_string(), entry);

    let resources = pool.gateway_synthetic_resources().await;
    let uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();
    assert!(uris.iter().any(|u| u == "lab://gateway/servers"));
    assert!(uris.iter().any(|u| u == "lab://gateway/alpha/schema"));
    assert!(uris.iter().any(|u| u == "lab://gateway/beta/schema"));
    assert_eq!(uris.len(), 3);
}
```

- [ ] **Step 10: Run, confirm it fails**

```bash
cargo nextest run -p lab --all-features gateway_synthetic_resources 2>&1 | tail -10
```

Expected: compile error.

- [ ] **Step 11: Implement `gateway_synthetic_resources`**

Add to `impl UpstreamPool`, after `gateway_server_schema`. The return type must match `list_upstream_resources` (around line 2220) for clean composition in the MCP server. Inspect that function first to copy the exact `Vec<rmcp::model::Resource>` shape it returns:

```rust
/// Synthetic gateway resources to emit from `list_resources`.
///
/// Returns one entry for `lab://gateway/servers` plus one
/// `lab://gateway/<name>/schema` entry per registered upstream.
pub async fn gateway_synthetic_resources(&self) -> Vec<rmcp::model::Resource> {
    use rmcp::model::RawResource;

    let mut out = vec![
        RawResource::new("lab://gateway/servers", "gateway/servers")
            .with_description("Index of upstream MCP servers connected to the gateway")
            .with_mime_type("application/json")
            .no_annotation(),
    ];
    let catalog = self.catalog.read().await;
    let mut names: Vec<&String> = catalog.keys().collect();
    names.sort();
    for name in names {
        out.push(
            RawResource::new(
                format!("lab://gateway/{name}/schema"),
                format!("gateway/{name}/schema"),
            )
            .with_description(format!("Tool schemas for upstream `{name}`"))
            .with_mime_type("application/json")
            .no_annotation(),
        );
    }
    out
}
```

If `list_upstream_resources` returns a different concrete type (e.g. wraps `Resource` differently), match that shape here — the MCP `list_resources` handler will `extend` both vectors into the same `Vec`.

- [ ] **Step 12: Run all three new tests + the full pool test module**

```bash
cargo nextest run -p lab --all-features gateway_synthetic_resources gateway_servers_doc gateway_server_schema 2>&1 | tail -15
cargo nextest run -p lab --all-features dispatch::upstream::pool 2>&1 | tail -10
```

Expected: 3 new tests pass; full pool module green.

- [ ] **Step 13: Commit**

```bash
git add crates/lab/src/dispatch/upstream/pool.rs
rtk git commit -m "feat(upstream): add synthetic gateway document builders"
```

---

## Task 2: Gateway dispatch — `gateway.servers` and `gateway.schema` actions

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/manager.rs` (add two wrappers)
- Modify: `crates/lab/src/dispatch/gateway/catalog.rs` (register two ActionSpecs)
- Modify: `crates/lab/src/dispatch/gateway/dispatch.rs` (add two match arms)
- Modify: `crates/lab/src/dispatch/gateway/dispatch.rs` (extend the action-presence test around line 795)

- [ ] **Step 1: Write a failing test for the dispatch arms**

Locate the existing test in `crates/lab/src/dispatch/gateway/dispatch.rs` near line 795 (the one that asserts `gateway.discovered_tools` and `gateway.discovered_resources` are present). Append a new test in the same module:

```rust
#[test]
fn gateway_actions_include_servers_and_schema() {
    let names: Vec<&str> = super::catalog::ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"gateway.servers"), "missing gateway.servers; have {names:?}");
    assert!(names.contains(&"gateway.schema"),  "missing gateway.schema; have {names:?}");
}
```

- [ ] **Step 2: Run, confirm it fails**

```bash
cargo nextest run -p lab --all-features gateway_actions_include_servers_and_schema 2>&1 | tail -10
```

Expected: assertion failure listing current action names.

- [ ] **Step 3: Add `ActionSpec` entries to `crates/lab/src/dispatch/gateway/catalog.rs`**

Find the `pub const ACTIONS: &[ActionSpec]` array (starts around line 10). Add two new entries near the existing `gateway.discovered_tools` spec — follow the exact field shape of that neighbor (description, params, destructive=false). Concretely:

```rust
ActionSpec {
    name: "gateway.servers",
    description: "List upstream MCP servers connected to the gateway, with cached tool/prompt/resource counts and tools-capability health.",
    destructive: false,
    params: &[],
},
ActionSpec {
    name: "gateway.schema",
    description: "Return the cached tool schemas (input_schema + meta) for one upstream MCP server, filtered by its exposure policy.",
    destructive: false,
    params: &[ParamSpec {
        name: "name",
        description: "Upstream server name (as listed by gateway.servers).",
        required: true,
    }],
},
```

If `ParamSpec` field names in this file differ (e.g. `kind`, `r#type`), inspect a nearby `ParamSpec` literal and match it exactly — do not invent fields.

- [ ] **Step 4: Add the two manager methods in `crates/lab/src/dispatch/gateway/manager.rs`**

Locate `pub async fn discovered_tools` around line 1966 and add these two methods immediately after it:

```rust
pub async fn gateway_servers_doc(&self) -> Result<Value, ToolError> {
    let Some(pool) = self.runtime.current_pool().await else {
        return Ok(serde_json::json!({ "servers": [] }));
    };
    Ok(pool.gateway_servers_doc().await)
}

pub async fn gateway_server_schema(&self, name: &str) -> Result<Value, ToolError> {
    let Some(pool) = self.runtime.current_pool().await else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("upstream pool not configured"),
        });
    };
    pool.gateway_server_schema(name).await.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("unknown upstream: {name}"),
    })
}
```

If `ToolError` is imported under a different alias in this file, match what's already in scope.

- [ ] **Step 5: Wire the dispatch arms in `crates/lab/src/dispatch/gateway/dispatch.rs`**

Locate the existing `gateway.discovered_tools` match arm around line 576. Add two new arms in the same match (the action match in `dispatch_with_manager`):

```rust
"gateway.servers" => to_json(manager.gateway_servers_doc().await?),
"gateway.schema" => {
    let name = require_str(&params, "name")?;
    to_json(manager.gateway_server_schema(&name).await?)
}
```

Use the same `to_json` / `require_str` helpers the surrounding arms use; do not import new helpers.

Also add the two new strings to whatever upstream/admin gate exists at the top of the function (lines 75–76 show `gateway.discovered_tools` and `gateway.discovered_resources` grouped). If the grouping is admin/non-admin or "requires manager", add the new actions to the same group as `gateway.discovered_tools` — they have the same semantics (read-only, manager-backed).

- [ ] **Step 6: Run the action-presence test, confirm it passes**

```bash
cargo nextest run -p lab --all-features gateway_actions_include_servers_and_schema 2>&1 | tail -10
```

Expected: 1 test passed.

- [ ] **Step 7: Write a dispatch-level test**

In the same `dispatch.rs` test module, add an integration-style test that exercises the dispatch path when `current_pool()` is `None` (default for a freshly constructed manager in tests). The exact constructor follows the pattern used by neighboring tests around line 795 — copy that setup:

```rust
#[tokio::test]
async fn gateway_servers_action_returns_empty_when_no_pool() {
    let manager = test_manager_no_pool().await; // pattern from neighboring tests
    let result = dispatch_with_manager(&manager, "gateway.servers", serde_json::json!({}))
        .await
        .expect("dispatch ok");
    assert_eq!(result["servers"].as_array().map(|a| a.len()), Some(0));
}

#[tokio::test]
async fn gateway_schema_missing_name_returns_missing_param() {
    let manager = test_manager_no_pool().await;
    let err = dispatch_with_manager(&manager, "gateway.schema", serde_json::json!({}))
        .await
        .expect_err("missing name");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(body["kind"], "missing_param");
    assert_eq!(body["param"], "name");
}
```

If the existing tests use a different constructor helper (search for `fn test_manager` or similar in the same `mod tests`), substitute that name. Do not invent a constructor — adapt to whatever the file already has.

- [ ] **Step 8: Run the new dispatch tests**

```bash
cargo nextest run -p lab --all-features gateway_servers_action_returns_empty_when_no_pool gateway_schema_missing_name_returns_missing_param 2>&1 | tail -15
```

Expected: 2 tests passed.

- [ ] **Step 9: Run the full gateway dispatch test module to check nothing regressed**

```bash
cargo nextest run -p lab --all-features dispatch::gateway 2>&1 | tail -10
```

Expected: all green.

- [ ] **Step 10: Commit**

```bash
git add crates/lab/src/dispatch/gateway/catalog.rs crates/lab/src/dispatch/gateway/manager.rs crates/lab/src/dispatch/gateway/dispatch.rs
rtk git commit -m "feat(gateway): add gateway.servers and gateway.schema actions"
```

---

## Task 3: MCP resource surface — `lab://gateway/*`

**Files:**
- Modify: `crates/lab/src/mcp/server.rs` (extend `list_resources` and `read_resource`)

The MCP handlers translate URIs to the pool methods. No business logic
lives here.

- [ ] **Step 1: Extend `list_resources` to include synthetic gateway entries**

In `crates/lab/src/mcp/server.rs`, locate `list_resources` at line 559. Find the block (line 593–599):

```rust
if let Some(pool) = self.current_upstream_pool().await {
    resources.extend(pool.list_upstream_resources().await);
    if let Some(subject) = self.request_subject(&context) {
        let configs = self.oauth_upstream_configs().await;
        resources.extend(pool.subject_scoped_resources(&configs, subject).await);
    }
}
```

Insert a `gateway_synthetic_resources` call as the **first** extension on that branch so the synthetic entries appear before proxied upstream resources:

```rust
if let Some(pool) = self.current_upstream_pool().await {
    resources.extend(pool.gateway_synthetic_resources().await);
    resources.extend(pool.list_upstream_resources().await);
    if let Some(subject) = self.request_subject(&context) {
        let configs = self.oauth_upstream_configs().await;
        resources.extend(pool.subject_scoped_resources(&configs, subject).await);
    }
}
```

- [ ] **Step 2: Add a `lab://gateway/` branch to `read_resource`**

In the same file, locate `read_resource` at line 622. The existing structure dispatches `lab://upstream/...` first (line 639), then falls through to a `lab://catalog` / `lab://<service>/actions` block (line 818).

Insert a new branch **immediately before** the `lab://upstream/` branch (since `gateway` URIs are cheaper to handle and we want them taken first). Match this structure used elsewhere in the function for emitting `dispatch start` + success/failure traces:

```rust
if uri.starts_with("lab://gateway/") {
    tracing::info!(
        surface = "mcp",
        service = "labby",
        action = "read_resource",
        subject,
        resource_uri = uri,
        route = "gateway",
        "dispatch route selected"
    );
    let Some(pool) = self.current_upstream_pool().await else {
        return Err(ErrorData::resource_not_found(
            format!("upstream pool not configured"),
            None,
        ));
    };

    let json = if uri == "lab://gateway/servers" {
        Some(pool.gateway_servers_doc().await)
    } else if let Some(name) = uri
        .strip_prefix("lab://gateway/")
        .and_then(|rest| rest.strip_suffix("/schema"))
        .filter(|name| !name.is_empty() && !name.contains('/'))
    {
        pool.gateway_server_schema(name).await
    } else {
        None
    };

    let elapsed_ms = start.elapsed().as_millis();
    return match json {
        Some(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_default();
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri = uri,
                route = "gateway",
                elapsed_ms,
                "synthetic resource ok"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Success,
            )
            .await;
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(text, uri)],
            })
        }
        None => {
            tracing::warn!(
                surface = "mcp",
                service = "labby",
                action = "read_resource",
                subject,
                resource_uri = uri,
                route = "gateway",
                elapsed_ms,
                kind = "not_found",
                "synthetic resource not found"
            );
            self.emit_dispatch_notification(
                &context,
                "lab",
                "read_resource",
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: LoggingLevel::Warning,
                    kind: "not_found",
                },
            )
            .await;
            Err(ErrorData::resource_not_found(
                format!("unknown resource: {uri}"),
                None,
            ))
        }
    };
}
```

Verify `ResourceContents::text` is the correct constructor in this file by grepping for it (it's used in the `lab://catalog` branch starting around line 832). If it takes different arguments here, match what the catalog branch uses.

- [ ] **Step 3: Build to check it compiles**

```bash
rtk cargo build -p lab --all-features 2>&1 | tail -20
```

Expected: clean build. If `ResourceContents::text` signature mismatches, fix per the existing `lab://catalog` branch.

- [ ] **Step 4: Run the broader MCP test module to catch regressions**

```bash
cargo nextest run -p lab --all-features mcp:: 2>&1 | tail -15
```

Expected: all green. The arch test in Task 4 will exercise the new branch end-to-end.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/mcp/server.rs
rtk git commit -m "feat(mcp): expose lab://gateway/* synthetic resources"
```

---

## Task 4: Architecture test — pin URI scheme and JSON shape

**Files:**
- Create: `crates/lab/tests/gateway_schema_resources.rs`

This test exercises the dispatch path end-to-end (it does not stand up
the MCP transport — that's covered by the unit tests inside
`mcp/server.rs`). It pins the contract: URI scheme, top-level keys,
required tool-entry keys, exposure-policy filtering, unknown-upstream
behavior.

- [ ] **Step 1: Inspect an existing arch-style test to match style**

```bash
rtk read crates/lab/tests/upstream_oauth.rs | head -60
```

Adopt the same imports / harness pattern (e.g. how it builds an
`UpstreamPool` or `Manager` from raw `UpstreamEntry`).

- [ ] **Step 2: Write the failing test file**

Create `crates/lab/tests/gateway_schema_resources.rs` with:

```rust
//! Architecture test: pins the `lab://gateway/*` URI scheme, the JSON
//! shape of the synthetic documents, and exposure-policy filtering.
//!
//! Any change to a top-level key here is a contract change — update
//! `docs/contracts/gateway-schema-resources.md` in the same PR.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::Tool;
use serde_json::json;

use lab::dispatch::upstream::pool::UpstreamPool;
use lab::dispatch::upstream::types::{
    ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};

fn make_tool(name: &'static str, upstream: &str) -> UpstreamTool {
    UpstreamTool {
        tool: Tool {
            name: Cow::Borrowed(name),
            title: None,
            description: Some(Cow::Borrowed("desc")),
            input_schema: Arc::new(serde_json::Map::new()),
            output_schema: None,
            annotations: None,
            icons: None,
            meta: None,
        },
        input_schema: Some(json!({"type": "object", "properties": {}})),
        upstream_name: Arc::from(upstream),
    }
}

fn make_entry(name: &str, tools: Vec<UpstreamTool>, policy: ToolExposurePolicy) -> UpstreamEntry {
    let mut map = HashMap::new();
    for t in tools {
        map.insert(t.tool.name.to_string(), t);
    }
    UpstreamEntry {
        name: Arc::from(name),
        tools: map,
        exposure_policy: policy,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

#[tokio::test]
async fn gateway_servers_doc_shape_is_contract_stable() {
    let pool = UpstreamPool::new();
    pool.insert_entry_for_test(
        "alpha",
        make_entry("alpha", vec![make_tool("search", "alpha")], ToolExposurePolicy::All),
    )
    .await;

    let doc = pool.gateway_servers_doc().await;
    let servers = doc["servers"].as_array().expect("servers array");
    assert_eq!(servers.len(), 1);
    let s = &servers[0];

    // Contract: these keys must be present.
    for key in ["name", "tool_count", "prompt_count", "resource_count", "tool_health", "tool_last_error"] {
        assert!(s.get(key).is_some(), "missing required field: {key}");
    }
    assert_eq!(s["tool_health"].as_str(), Some("healthy"));
    assert!(s["tool_last_error"].is_null());
}

#[tokio::test]
async fn gateway_server_schema_shape_is_contract_stable() {
    let policy = ToolExposurePolicy::from_patterns(vec!["github_*".into()]).expect("policy");
    let tools = vec![make_tool("github_create", "alpha"), make_tool("delete_repo", "alpha")];

    let pool = UpstreamPool::new();
    pool.insert_entry_for_test("alpha", make_entry("alpha", tools, policy)).await;

    let doc = pool.gateway_server_schema("alpha").await.expect("doc");

    for key in ["name", "tools", "health", "last_error"] {
        assert!(doc.get(key).is_some(), "missing top-level field: {key}");
    }
    let tools = doc["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "exposure policy must filter hidden tools");

    let t = &tools[0];
    for key in ["name", "description", "input_schema", "meta"] {
        assert!(t.get(key).is_some(), "missing tool-entry field: {key}");
    }
    assert_eq!(t["name"], "github_create");
}

#[tokio::test]
async fn gateway_server_schema_unknown_upstream_returns_none() {
    let pool = UpstreamPool::new();
    assert!(pool.gateway_server_schema("nope").await.is_none());
}

#[tokio::test]
async fn gateway_synthetic_resources_uri_scheme_is_pinned() {
    let pool = UpstreamPool::new();
    pool.insert_entry_for_test("alpha", make_entry("alpha", vec![], ToolExposurePolicy::All)).await;

    let resources = pool.gateway_synthetic_resources().await;
    let uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();
    assert!(uris.contains(&"lab://gateway/servers".to_string()));
    assert!(uris.contains(&"lab://gateway/alpha/schema".to_string()));
    // Nothing should leak into the lab://upstream/ namespace from here.
    assert!(!uris.iter().any(|u| u.starts_with("lab://upstream/")));
}
```

This test depends on a public test-only insertion helper. Add one in
`pool.rs` if absent:

```rust
#[cfg(any(test, feature = "test-helpers"))]
impl UpstreamPool {
    /// Test-only: insert a fully-formed `UpstreamEntry` into the catalog.
    pub async fn insert_entry_for_test(&self, name: &str, entry: UpstreamEntry) {
        self.catalog.write().await.insert(name.to_string(), entry);
    }
}
```

The `cfg(test)` gate alone is not enough because the helper has to be
visible to a file under `tests/` (which is a separate crate). Use the
`#[cfg(any(test, feature = "test-helpers"))]` form **only** if the crate
already has a `test-helpers` feature; otherwise drop the feature and use
unconditional `pub` access (review with the user — adding a feature flag
is a bigger change). The safest first move: make the helper plain
`pub` and mark it `/// Test-only` in the doc comment.

- [ ] **Step 3: Run the new test file, confirm it fails (compile or assertion)**

```bash
cargo nextest run -p lab --all-features --test gateway_schema_resources 2>&1 | tail -20
```

Expected: either compile errors about `insert_entry_for_test` or assertion failures. Fix the helper visibility (Step 2 last paragraph) until it compiles.

- [ ] **Step 4: Iterate until all four tests pass**

If exposure-policy or doc-shape assertions fail, the regression is in Tasks 1–2, not in this test. Treat assertion failures as legitimate contract violations and fix the producer code.

```bash
cargo nextest run -p lab --all-features --test gateway_schema_resources 2>&1 | tail -10
```

Expected: 4 tests passed.

- [ ] **Step 5: Run the full workspace test suite**

```bash
cargo nextest run --workspace --all-features 2>&1 | tail -20
```

Expected: clean. Investigate any new failures before committing.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/tests/gateway_schema_resources.rs crates/lab/src/dispatch/upstream/pool.rs
rtk git commit -m "test(gateway): pin lab://gateway/* uri scheme and json shape"
```

---

## Task 5: Documentation — `docs/surfaces/MCP.md`

**Files:**
- Modify: `docs/surfaces/MCP.md` (extend the resource list)

- [ ] **Step 1: Inspect the existing resource list**

```bash
rtk read docs/surfaces/MCP.md | sed -n '420,440p'
```

You'll see the current `lab://catalog`, `lab://<service>/actions`, and
`lab://upstream/{name}/{original_uri}` entries.

- [ ] **Step 2: Add the two new resources in the same list**

Insert directly after the `lab://upstream/{name}/{original_uri}` entry:

```markdown
- `lab://gateway/servers` — synthetic index of upstream MCP servers
  connected to the gateway (name, cached tool/prompt/resource counts,
  tools-capability health). See
  [`docs/contracts/gateway-schema-resources.md`](../contracts/gateway-schema-resources.md).
- `lab://gateway/<name>/schema` — synthetic per-upstream tool catalog
  (name, description, input_schema, meta), filtered by the upstream's
  `ToolExposurePolicy`. Lets agents inspect a server's full schema in
  one read without paying a `tool_search` round-trip.
```

If the file has a second resource list (the `MCP.md` references at
line 316 + 428), update both for consistency.

- [ ] **Step 3: Commit**

```bash
git add docs/surfaces/MCP.md
rtk git commit -m "docs(mcp): document lab://gateway/* synthetic resources"
```

---

## Final verification

- [ ] **Step 1: Lint**

```bash
just lint 2>&1 | tail -20
```

Expected: clean. Fix any clippy/fmt issues before declaring done.

- [ ] **Step 2: Full test pass with all features**

```bash
just test 2>&1 | tail -20
```

Expected: clean.

- [ ] **Step 3: Smoke the action surface manually**

```bash
LAB_LOG=labby=info cargo run -p lab --all-features -- gateway servers 2>&1 | tail -10
```

Expected: JSON document with `servers: []` (no upstream pool in a bare CLI run) and no panic. If the CLI shape differs (gateway may only be reachable through `lab serve`), document that in the PR description and skip this step.

---

## Self-review checklist (run after writing, before handoff)

1. **Spec coverage:**
   - Goal "list connected upstreams in one read" → Task 1 (`gateway_servers_doc`) + Task 3 (MCP resource).
   - Goal "fetch full tool catalog for one upstream in one read" → Task 1 (`gateway_server_schema`) + Task 3.
   - Goal "honor `ToolExposurePolicy`" → Task 1 step 5 test + Task 4 arch test.
   - Goal "keep `tools/list` / `tool_search` unchanged" → no changes to those paths; verified implicitly by full test pass in Final verification.
   - Decision "tool_last_error in index" → contract + Task 1 step 3 + Task 4 shape assertion.
   - Decision "_meta passthrough verbatim" → Task 1 step 7 (`"meta": t.tool.meta`) + Task 4 tool-entry shape assertion.
   - Decision "include arch test" → Task 4.
2. **Placeholder scan:** No "TBD", no "add appropriate error handling", every code-changing step shows code, every command shows expected output.
3. **Type consistency:** `gateway_servers_doc`, `gateway_server_schema`, `gateway_synthetic_resources` used identically in pool tests, dispatch wrappers, MCP server, and arch test. Action names `gateway.servers` / `gateway.schema` consistent in catalog, dispatch arm, and tests. JSON top-level keys (`servers`, `tools`, `health`, `last_error`, `tool_health`, `tool_last_error`, `meta`) match across contract, pool implementation, and arch test.

Open during execution: confirm `ResourceContents::text` constructor + the `lab://catalog` branch's exact emission shape match what's written in Task 3 step 2 before committing that task.
