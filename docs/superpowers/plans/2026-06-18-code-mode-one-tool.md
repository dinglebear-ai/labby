# Code Mode One Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `codemode({ code })` the primary model-facing Lab gateway tool while preserving legacy `search` and `execute`, and provide in-sandbox `codemode.search()` / `codemode.describe()` discovery without adding a new runner host-callback protocol.

**Architecture:** Phase 1 is intentionally conservative after engineering review: `codemode` routes through the existing execute path and uses the current Javy/QuickJS runner, broker, route-scope filtering, auth checks, destructive gating, schema validation, truncation, artifacts, traces, and mcp-ui passthrough. In-sandbox discovery is local to the execution: the host injects a reduced, already-sanitized discovery catalog containing only `id`, `path`, `upstream`, `name`, `description`, and `signature`; JavaScript helpers search and describe that reduced data. This avoids repeated host callback round trips, avoids a new pending-promise protocol lane, and avoids injecting full schema/output_schema/dts catalog JSON into normal execute source.

**Tech Stack:** Rust 2024, Tokio, rmcp, serde_json, Javy/QuickJS subprocess runner, existing `GatewayManager`, existing Code Mode broker modules under `crates/lab/src/dispatch/gateway/code_mode/`.

## Global Constraints

- Work in `/home/jmagar/workspace/lab-cloudflare-codemode-parity` on branch `codex/cloudflare-codemode-parity`.
- Do not touch unrelated dirty changes in `/home/jmagar/workspace/lab`.
- `CLAUDE.md` is the source of truth; do not edit `AGENTS.md` or `GEMINI.md` directly.
- Runtime remains Javy/QuickJS via subprocess stdio, not Wasmtime/fuel.
- Preserve current sandbox containment: no ambient network APIs, no host modules, runner `env_clear()`, per-execution cwd isolation, process-group cleanup, and artifact containment.
- Preserve host-side permissioning: `codemode` and `execute` require execute scope (`lab` or `lab:admin`) on authenticated MCP calls; legacy `search` keeps read-scope behavior.
- Preserve route-scope filtering for discovery, proxy generation, and direct `callTool`.
- Definition of destructive: any action that results in the permanent loss of data that cannot be quickly and/or easily regenerated/recreated with minimal effort.
- Existing `search` and `execute` tools stay functional until a later explicit compatibility removal.
- Do not add `HostCall`, `HostCallResult`, `HostCallError`, `pending_host_calls`, or any other new runner callback protocol in this phase.
- Default verification target is all-features: `cargo nextest run --workspace --all-features`.

---

## Engineering Review Applied

The Lavra engineering review produced architecture, simplicity, security, and performance findings. This plan applies all actionable feedback:

- [x] Remove the new host-callback protocol from this implementation slice.
- [x] Keep `codemode` as a thin primary execute alias first, with explicit compatibility boundaries.
- [x] Implement in-sandbox discovery locally over a reduced per-execution catalog instead of repeated host callbacks.
- [x] Avoid N+1 catalog refreshes inside one sandbox run; discovery data is built once when the execute proxy is built.
- [x] Avoid callback DoS; no discovery host callbacks exist in this phase.
- [x] Avoid callback error/pending-promise desynchronization; no new callback settlement variants exist in this phase.
- [x] Keep discovery route-scoped and capability-filtered by using the same tool list source as execute proxy generation.
- [x] Avoid ambiguous `describe("upstream")`; describe accepts exact `id`, exact `upstream.tool`, or exact sanitized helper path only.
- [x] Make docs examples use `codemode.<upstream>.<tool>(...)`, not bare `upstream.tool(...)`.
- [x] Add auth/scope regression tests for the new `codemode` tool.
- [x] Add MCP App/OpenAI Apps metadata tests for `codemode`.
- [x] Add proxy-size/content regression tests: reduced discovery data must omit `schema`, `output_schema`, and `dts`.
- [x] Fix package commands to use `-p labby`, not `-p lab`.

Deferrable items intentionally not implemented in this phase:

- Host-backed `codemode.search()` / `codemode.describe()` callbacks.
- BM25/fuzzy/vector search.
- Persistent cross-process discovery index.
- Removing legacy `search` and `execute`.
- Rich persistent `codemode.step()` tracing.
- Full TypeScript `.d.ts` docs from `codemode.describe`; this phase returns compact docs and signature only.

---

## File Structure

- Modify `crates/lab/src/mcp/catalog.rs`: add `CODE_MODE_TOOL_NAME = "codemode"` and update visibility comments.
- Modify `crates/lab/src/mcp/handlers_tools.rs`: advertise `codemode` before compatibility `search` and `execute`, with execute output schema and Apps metadata.
- Modify `crates/lab/src/mcp/call_tool_codemode.rs`: add primary `CODE_MODE_DESCRIPTION`, keep `CODE_EXECUTE_DESCRIPTION` as compatibility copy or alias, and route `codemode` through execute semantics.
- Modify `crates/lab/src/mcp/handlers_resources.rs`: add `codemode` MCP App and skybridge resources, with versioned URI readback.
- Modify `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`: preserve an existing `globalThis.codemode`, add reduced local discovery helpers, and keep generated upstream namespaces under `codemode.<upstream>.<tool>`.
- Modify `crates/lab/src/dispatch/gateway/code_mode/execute.rs`: build a reduced discovery catalog alongside the runtime proxy, using the same scoped/capability-filtered tool source.
- Modify `crates/lab/src/dispatch/gateway/code_mode/types.rs`: add compact serializable discovery entry types if they do not fit naturally in `preamble.rs`.
- Modify `docs/dev/CODE_MODE.md`: document `codemode` as the primary tool and legacy `search`/`execute` as compatibility.
- Modify `docs/services/UPSTREAM.md`: update Code Mode wording to primary `codemode` plus compatibility tools.
- Extend tests in `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`: local discovery helper behavior, exact describe matching, reduced catalog content, and namespace preservation.
- Extend tests in `crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs`: in-sandbox discovery over reduced catalog and no schema/dts injection.
- Extend tests in `crates/lab/src/mcp/handlers_tools/tests.rs`: list-tools, metadata, auth/scope, and route-scope behavior for `codemode`.

---

### Task 1: Advertise `codemode` as the Primary MCP Tool

**Files:**
- Modify: `crates/lab/src/mcp/catalog.rs`
- Modify: `crates/lab/src/mcp/handlers_tools.rs`
- Modify: `crates/lab/src/mcp/call_tool_codemode.rs`
- Modify: `crates/lab/src/mcp/handlers_resources.rs`
- Test: `crates/lab/src/mcp/handlers_tools/tests.rs`

**Interfaces:**
- Consumes: MCP tool call `codemode({ code, upstreams?, tools?, max_tool_calls? })`
- Produces: same `code_mode_execute_trace` structured output and same permission behavior as current `execute`

- [ ] **Step 1: Add failing list-tools and metadata tests**

Update the Code Mode list-tools snapshot in `crates/lab/src/mcp/handlers_tools/tests.rs` so Code Mode mode contains `codemode`, `execute`, and `search`:

```rust
assert_eq!(
    snapshot.tools,
    ["codemode".to_string(), "execute".to_string(), "search".to_string()]
        .into_iter()
        .collect()
);
assert!(!snapshot.tools.contains("code_search"));
assert!(!snapshot.tools.contains("code_execute"));
assert!(!snapshot.tools.contains("code"));
```

Add a metadata assertion next to the existing search/execute metadata test:

```rust
let codemode = code_mode_tool_meta(CODE_MODE_TOOL_NAME);
let codemode_ui = codemode.0["ui"]["resourceUri"]
    .as_str()
    .expect("codemode resourceUri");
assert!(codemode_ui.starts_with(CODE_MODE_APP_URI));
assert!(codemode_ui.contains("?v="));
let codemode_skybridge = codemode
    .0
    .get("openai/outputTemplate")
    .and_then(serde_json::Value::as_str)
    .expect("codemode openai/outputTemplate");
assert!(codemode_skybridge.starts_with(CODE_MODE_APP_SKYBRIDGE_URI));
assert!(codemode_skybridge.contains("?v="));
```

- [ ] **Step 2: Add failing auth/scope tests**

Add tests proving `codemode` has execute semantics, not legacy search semantics:

```rust
#[tokio::test]
async fn codemode_requires_execute_scope_not_read_scope() {
    let server = code_mode_enabled_server_for_tests().await;
    let context = request_context_with_scopes(&["lab:read"]);
    let result = server
        .call_tool_impl(
            rmcp::model::CallToolRequestParams::new(CODE_MODE_TOOL_NAME)
                .with_arguments(serde_json::json!({
                    "code": "async () => 1"
                }).as_object().unwrap().clone()),
            context,
        )
        .await
        .expect("call result");
    let text = result.content[0].as_text().expect("text").text.as_ref();
    assert!(text.contains("\"kind\":\"forbidden\""), "{text}");
}

#[tokio::test]
async fn codemode_allows_execute_scope() {
    let server = code_mode_enabled_server_for_tests().await;
    let context = request_context_with_scopes(&["lab"]);
    let result = server
        .call_tool_impl(
            rmcp::model::CallToolRequestParams::new(CODE_MODE_TOOL_NAME)
                .with_arguments(serde_json::json!({
                    "code": "async () => 1"
                }).as_object().unwrap().clone()),
            context,
        )
        .await
        .expect("call result");
    let text = result.content[0].as_text().expect("text").text.as_ref();
    assert!(text.contains("code_mode_execute_trace"), "{text}");
}
```

Use existing test helpers in the file for server/context construction. If helper names differ, keep the assertions exactly and adapt only the fixture setup.

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
cargo test -p labby handlers_tools --all-features
```

Expected: FAIL because `codemode` constants, metadata, and routing do not exist yet.

- [ ] **Step 4: Add the canonical tool name**

In `crates/lab/src/mcp/catalog.rs`:

```rust
/// Primary Cloudflare-style Code Mode tool name.
pub(crate) const CODE_MODE_TOOL_NAME: &str = "codemode";
/// Compatibility tool name for catalog JS filtering.
pub(crate) const CODE_MODE_SEARCH_TOOL_NAME: &str = "search";
/// Compatibility tool name for direct Code Mode execution.
pub(crate) const TOOL_EXECUTE_TOOL_NAME: &str = "execute";
```

Update `CodeModeVisibility::RootSynthetic` comments to say it advertises primary `codemode` plus compatibility `search` and `execute`.

- [ ] **Step 5: Add `codemode` resource descriptors**

In `crates/lab/src/mcp/handlers_resources.rs`, import `CODE_MODE_TOOL_NAME` and add:

```rust
pub(crate) const CODE_MODE_APP_URI: &str = "ui://lab/code-mode/codemode";
pub(crate) const CODE_MODE_APP_SKYBRIDGE_URI: &str = "ui://lab/code-mode/codemode.skybridge";
```

Add descriptors before search/execute:

```rust
CodeModeAppResourceDescriptor {
    uri: CODE_MODE_APP_URI,
    name: "code-mode/codemode",
    runtime: CodeModeRuntime::McpApp,
    tool_name: Some(CODE_MODE_TOOL_NAME),
},
CodeModeAppResourceDescriptor {
    uri: CODE_MODE_APP_SKYBRIDGE_URI,
    name: "code-mode/codemode.skybridge",
    runtime: CodeModeRuntime::Skybridge,
    tool_name: Some(CODE_MODE_TOOL_NAME),
},
```

- [ ] **Step 6: Advertise `codemode`**

In `crates/lab/src/mcp/handlers_tools.rs`, import `CODE_MODE_TOOL_NAME`. Create a schema identical to the existing execute schema and register `codemode` before compatibility tools:

```rust
tools.push(
    Tool::new(
        CODE_MODE_TOOL_NAME,
        CODE_MODE_DESCRIPTION,
        Arc::clone(&execute_schema),
    )
    .with_raw_output_schema(Arc::clone(&trace_output_schema))
    .with_meta(code_mode_tool_meta(CODE_MODE_TOOL_NAME)),
);
gateway_tool_count += 1;
```

Then leave `search` and `execute` registrations in place.

- [ ] **Step 7: Route `codemode` through execute semantics**

In the MCP call-tool dispatch branch, route `CODE_MODE_TOOL_NAME` to `call_tool_execute_impl` exactly like `TOOL_EXECUTE_TOOL_NAME`. Do not route it to `call_code_mode_impl`.

In `crates/lab/src/mcp/call_tool_codemode.rs`, define:

```rust
pub(crate) const CODE_MODE_DESCRIPTION: &str = "\
Execute a JavaScript async arrow function in the Code Mode sandbox. This is the primary Lab gateway tool.

Inside the sandbox:
- `await codemode.search(\"short intent phrase\")` searches the reduced in-execution catalog.
- `await codemode.describe(\"upstream.tool\")` returns compact docs for an exact tool target.
- `await codemode.<upstream>.<tool>(params)` calls a discovered upstream method.
- `await callTool(\"upstream::tool\", params)` is the raw escape hatch.

`execute` remains as a compatibility alias. `search` remains as the legacy catalog-filter tool.";

pub(crate) const CODE_EXECUTE_DESCRIPTION: &str = CODE_MODE_DESCRIPTION;
```

Keep the existing budget/error/truncation paragraphs by appending them below this opening rather than deleting useful guidance.

- [ ] **Step 8: Run tests**

Run:

```bash
cargo test -p labby handlers_tools --all-features
cargo test -p labby call_tool_codemode --all-features
```

Expected: PASS.

---

### Task 2: Add Local In-Sandbox Discovery Without Runner Host Callbacks

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/execute.rs`
- Modify: `crates/lab/src/dispatch/gateway/code_mode/types.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs`

**Interfaces:**
- Consumes: scoped `Vec<UpstreamTool>` from the same source used by runtime proxy generation
- Produces: reduced discovery entries for local JS helpers:

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct CodeModeDiscoveryEntry {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) upstream: String,
    pub(crate) name: String,
    pub(crate) helper: String,
    pub(crate) description: String,
    pub(crate) signature: String,
}
```

- [ ] **Step 1: Add failing preamble tests**

In `crates/lab/src/dispatch/gateway/code_mode/preamble.rs`, add tests:

```rust
#[test]
fn discovery_preamble_preserves_existing_codemode_object() {
    let entries = vec![CodeModeDiscoveryEntry {
        id: "arcane::containers".to_string(),
        path: "arcane.containers".to_string(),
        upstream: "arcane".to_string(),
        name: "containers".to_string(),
        helper: "codemode.arcane.containers".to_string(),
        description: "List containers".to_string(),
        signature: "codemode.arcane.containers(params: unknown): Promise<unknown>".to_string(),
    }];
    let js = generate_discovery_js(&entries).expect("js");
    assert!(js.contains("globalThis.codemode = globalThis.codemode || {}"));
    assert!(js.contains("codemode.search"));
    assert!(js.contains("codemode.describe"));
    assert!(!js.contains("schema"));
    assert!(!js.contains("output_schema"));
    assert!(!js.contains("dts"));
}

#[test]
fn discovery_describe_rejects_upstream_only_targets() {
    let entries = vec![CodeModeDiscoveryEntry {
        id: "github::search_issues".to_string(),
        path: "github.search_issues".to_string(),
        upstream: "github".to_string(),
        name: "search_issues".to_string(),
        helper: "codemode.github.search_issues".to_string(),
        description: "Search issues".to_string(),
        signature: "codemode.github.search_issues(params: unknown): Promise<unknown>".to_string(),
    }];
    let js = generate_discovery_js(&entries).expect("js");
    assert!(js.contains("ambiguous_target"));
    assert!(js.contains("github.search_issues"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p labby discovery_preamble --all-features
```

Expected: FAIL because `CodeModeDiscoveryEntry` and `generate_discovery_js` do not exist.

- [ ] **Step 3: Add compact discovery entry type**

In `crates/lab/src/dispatch/gateway/code_mode/types.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct CodeModeDiscoveryEntry {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) upstream: String,
    pub(crate) name: String,
    pub(crate) helper: String,
    pub(crate) description: String,
    pub(crate) signature: String,
}
```

- [ ] **Step 4: Generate local discovery JS**

In `preamble.rs`, import `CodeModeDiscoveryEntry` and add:

```rust
pub(crate) fn generate_discovery_js(entries: &[CodeModeDiscoveryEntry]) -> Result<String, String> {
    let json = serde_json::to_string(entries)
        .map_err(|err| format!("failed to serialize Code Mode discovery catalog: {err}"))?;
    Ok(format!(
        r#"
globalThis.codemode = globalThis.codemode || {{}};
var __codemodeDiscovery = {json};
function __codemodeNormalize(value) {{
  return String(value == null ? "" : value)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}}
function __codemodeTokens(value) {{
  var normalized = __codemodeNormalize(value);
  return normalized ? normalized.split(/\s+/g) : [];
}}
codemode.search = function(input) {{
  var query = typeof input === "object" && input !== null ? String(input.query || "") : String(input || "");
  var limit = typeof input === "object" && input !== null && Number.isFinite(Number(input.limit))
    ? Math.max(1, Math.min(50, Number(input.limit)))
    : 50;
  var tokens = __codemodeTokens(query);
  if (!tokens.length) return Promise.resolve({{ results: [], total: 0, truncated: false }});
  var scored = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    var fields = [
      [__codemodeNormalize(entry.path), 12],
      [__codemodeNormalize(entry.name), 10],
      [__codemodeNormalize(entry.upstream), 8],
      [__codemodeNormalize(entry.description), 5]
    ];
    var covered = 0;
    var score = 0;
    for (var t = 0; t < tokens.length; t++) {{
      var tokenScore = 0;
      for (var f = 0; f < fields.length; f++) {{
        if (fields[f][0].indexOf(tokens[t]) !== -1 && fields[f][1] > tokenScore) tokenScore = fields[f][1];
      }}
      if (tokenScore > 0) {{
        covered++;
        score += tokenScore;
      }}
    }}
    var required = tokens.length <= 2 ? tokens.length : Math.ceil(tokens.length * 0.6);
    if (covered >= required) {{
      scored.push({{
        path: entry.path,
        id: entry.id,
        upstream: entry.upstream,
        name: entry.name,
        description: entry.description,
        signature: entry.signature,
        score: score
      }});
    }}
  }}
  scored.sort(function(a, b) {{
    if (b.score !== a.score) return b.score - a.score;
    return a.path < b.path ? -1 : a.path > b.path ? 1 : 0;
  }});
  var total = scored.length;
  return Promise.resolve({{ results: scored.slice(0, limit), total: total, truncated: total > limit }});
}};
codemode.describe = function(target) {{
  var raw = String(target == null ? "" : target).trim();
  var matches = [];
  for (var i = 0; i < __codemodeDiscovery.length; i++) {{
    var entry = __codemodeDiscovery[i];
    if (raw === entry.id || raw === entry.path || raw === entry.helper) matches.push(entry);
    if (raw === entry.upstream) matches.push({{ __ambiguous: true, path: entry.path }});
  }}
  var ambiguous = matches.filter(function(item) {{ return item.__ambiguous; }});
  if (ambiguous.length) {{
    throw new Error(JSON.stringify({{
      kind: "ambiguous_target",
      message: "codemode.describe requires an exact tool id, upstream.tool path, or helper path",
      valid: ambiguous.map(function(item) {{ return item.path; }}).sort()
    }}));
  }}
  if (!matches.length) {{
    throw new Error(JSON.stringify({{ kind: "unknown_tool", message: "No Code Mode discovery target matched `" + raw + "`" }}));
  }}
  var entry = matches[0];
  return Promise.resolve({{
    path: entry.path,
    id: entry.id,
    markdown: "# " + entry.path + "\n\n" + entry.description + "\n\n- id: `" + entry.id + "`\n- helper: `" + entry.helper + "`\n- signature: `" + entry.signature + "`\n"
  }});
}};
codemode.step = function(name, fn) {{
  if (typeof fn !== "function") throw new Error("codemode.step requires a function");
  return Promise.resolve().then(fn);
}};
"#
    ))
}
```

- [ ] **Step 5: Preserve platform helpers when generating upstream namespaces**

Change the start of `generate_js_proxy` in `preamble.rs` from a fresh object to preservation:

```rust
Ok(format!(
    "// Code Mode proxy - auto-generated\n\
     globalThis.codemode = globalThis.codemode || {{}};\n\
     var codemode = globalThis.codemode;\n\
     {parts}\
     codemode.__meta__ = {{ upstreams: function() {{ return Promise.resolve({upstreams_json}); }} }};\n\
     var __upstreams__ = {upstreams_json};\n"
))
```

Add an assertion to the existing proxy tests that generated proxy JS contains `globalThis.codemode = globalThis.codemode || {}` and does not contain `var codemode = {};`.

- [ ] **Step 6: Build reduced entries in execute proxy generation**

In `execute.rs`, after the same scoped tool list is fetched for runtime proxy generation, build reduced entries:

```rust
let discovery_entries = tools
    .iter()
    .map(|tool| {
        let upstream = tool.upstream_name.to_string();
        let name = tool.tool.name.to_string();
        let upstream_snake = super::preamble::tool_name_to_snake(&upstream);
        let name_snake = super::preamble::tool_name_to_snake(&name);
        let description = tool
            .tool
            .description
            .as_ref()
            .map(|description| {
                crate::dispatch::gateway::projection::sanitize_tool_text(description, 1024)
            })
            .unwrap_or_default();
        let signature = super::ts_signatures::generate_tool_types(
            &upstream,
            &name,
            &description,
            tool.tool.input_schema.as_ref(),
            tool.tool.output_schema.as_ref(),
        )
        .signature;
        CodeModeDiscoveryEntry {
            id: super::types::upstream_tool_id(&upstream, &name),
            path: format!("{upstream_snake}.{name_snake}"),
            upstream,
            name,
            helper: format!("codemode.{upstream_snake}.{name_snake}"),
            description,
            signature,
        }
    })
    .collect::<Vec<_>>();
let discovery_js = super::preamble::generate_discovery_js(&discovery_entries)?;
let namespace_js = super::preamble::generate_js_proxy(&tools, &upstreams)?;
Ok(format!("{discovery_js}\n{namespace_js}"))
```

If ownership of `input_schema` / `output_schema` prevents borrowing here, generate discovery entries before consuming tool values or clone only the schema `Value`s needed for signature generation.

- [ ] **Step 7: Add broker tests for in-sandbox discovery**

Append to `crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs`:

```rust
#[tokio::test]
async fn execute_can_search_and_describe_inside_the_sandbox_without_host_callbacks() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping")).await;
    let registry = crate::registry::builtin_service_registry();
    let broker = super::CodeModeBroker::new(&registry, Some(&manager));

    let response = broker
        .execute(
            r#"async () => {
                const matches = await codemode.search({ query: "ping", limit: 1 });
                const docs = await codemode.describe(matches.results[0].path);
                return { path: matches.results[0].path, docs: docs.path, hasSignature: docs.markdown.includes("signature") };
            }"#,
            4,
            super::CodeModeCaller::TrustedLocal,
            super::CodeModeSurface::Mcp,
            crate::config::CodeModeConfig::default(),
            super::CodeModeCapabilityFilter::default(),
        )
        .await
        .expect("execute ok");

    assert_eq!(response.result.as_ref().unwrap()["path"], serde_json::json!("alpha.ping"));
    assert_eq!(response.result.as_ref().unwrap()["docs"], serde_json::json!("alpha.ping"));
    assert_eq!(response.result.as_ref().unwrap()["hasSignature"], serde_json::json!(true));
    assert!(response.calls.is_empty(), "local discovery must not count as upstream tool calls");
}

#[tokio::test]
async fn execute_rejects_ambiguous_describe_target() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping")).await;
    let registry = crate::registry::builtin_service_registry();
    let broker = super::CodeModeBroker::new(&registry, Some(&manager));

    let err = broker
        .execute(
            r#"async () => codemode.describe("alpha")"#,
            4,
            super::CodeModeCaller::TrustedLocal,
            super::CodeModeSurface::Mcp,
            crate::config::CodeModeConfig::default(),
            super::CodeModeCapabilityFilter::default(),
        )
        .await
        .expect_err("ambiguous describe target must reject");

    assert_eq!(err.kind(), "ambiguous_target");
}
```

- [ ] **Step 8: Add reduced-catalog regression test**

Add a test proving execute proxy source contains reduced discovery data but omits large metadata:

```rust
#[tokio::test]
async fn execute_proxy_embeds_only_reduced_discovery_catalog() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping")).await;
    let registry = crate::registry::builtin_service_registry();
    let broker = super::CodeModeBroker::new(&registry, Some(&manager));

    let proxy = broker
        .build_code_mode_proxy_for_tests(
            &super::CodeModeCaller::TrustedLocal,
            super::CodeModeSurface::Mcp,
            &super::CodeModeCapabilityFilter::default(),
        )
        .await
        .expect("proxy");

    assert!(proxy.contains("__codemodeDiscovery"));
    assert!(proxy.contains("alpha::ping"));
    assert!(proxy.contains("codemode.search"));
    assert!(!proxy.contains("\"schema\""));
    assert!(!proxy.contains("\"output_schema\""));
    assert!(!proxy.contains("\"dts\""));
}
```

Expose `build_code_mode_proxy_for_tests` under `#[cfg(test)]` if needed.

- [ ] **Step 9: Run tests**

Run:

```bash
cargo test -p labby discovery_preamble --all-features
cargo test -p labby execute_can_search_and_describe_inside_the_sandbox_without_host_callbacks --all-features
cargo test -p labby execute_proxy_embeds_only_reduced_discovery_catalog --all-features
```

Expected: PASS.

---

### Task 3: Keep Compatibility Search/Execute Stable

**Files:**
- Modify: `crates/lab/src/mcp/handlers_tools.rs`
- Modify: `crates/lab/src/mcp/call_tool_codemode.rs`
- Modify: `docs/dev/CODE_MODE.md`
- Test: `crates/lab/src/mcp/handlers_tools/tests.rs`
- Test: `crates/lab/src/dispatch/gateway/code_mode/tests_broker.rs`

**Interfaces:**
- Preserves: legacy `search({ code })` with `const tools = [...]` JS filter
- Preserves: legacy `execute({ code })` as compatibility alias
- Adds: `codemode({ code })` as primary wording and first advertised synthetic tool

- [ ] **Step 1: Add compatibility assertions**

Add or keep tests proving:

```rust
assert!(names.contains(&CODE_MODE_TOOL_NAME));
assert!(names.contains(&CODE_MODE_SEARCH_TOOL_NAME));
assert!(names.contains(&TOOL_EXECUTE_TOOL_NAME));
```

Add a call-routing test that both `execute` and `codemode` return `code_mode_execute_trace` for `async () => 1`.

- [ ] **Step 2: Ensure telemetry distinguishes primary and compatibility calls**

In `call_tool_execute_impl`, keep `service = %service` in user-visible error envelopes and add a structured log field:

```rust
code_mode_tool = %service,
```

Do not change `CodeModeHistoryKind::Execute`; history kind remains execute because the runtime behavior is execute. The log field is the migration-uptake signal.

- [ ] **Step 3: Run tests**

Run:

```bash
cargo test -p labby handlers_tools --all-features
cargo test -p labby call_tool_codemode --all-features
```

Expected: PASS.

---

### Task 4: Update Documentation

**Files:**
- Modify: `docs/dev/CODE_MODE.md`
- Modify: `docs/services/UPSTREAM.md`
- Modify: `crates/lab/src/mcp/call_tool_codemode.rs`

**Interfaces:**
- Produces: model-facing guidance that says `codemode({ code })` first
- Preserves: existing references to `search` and `execute` as compatibility surfaces

- [ ] **Step 1: Update `docs/dev/CODE_MODE.md`**

Replace the opening surface section with:

```markdown
Code Mode's primary MCP surface is `codemode({ code })`. The code runs as one async JavaScript function in the sandbox. Discovery, focused compact docs, upstream calls, fan-out, filtering, and final result shaping all happen inside that same execution.

Inside the sandbox:

- `await codemode.search("GitHub pull requests")` searches the reduced in-execution catalog.
- `await codemode.describe("github.list_pull_requests")` returns compact docs for an exact tool target.
- `await codemode.github.list_pull_requests(params)` calls the generated helper.
- `await callTool("github::list_pull_requests", params)` calls the raw bridge.
```

Use this example:

```ts
async () => {
  const matches = await codemode.search({ query: "GitHub pull requests", limit: 1 });
  const docs = await codemode.describe(matches.results[0].path);
  const pulls = await codemode.github.list_pull_requests({ state: "open" });
  return {
    docs: docs.path,
    open: pulls.items.map(pr => ({ number: pr.number, title: pr.title }))
  };
}
```

Add a compatibility subsection:

```markdown
`search` and `execute` remain available for older clients during the migration. New agent instructions should prefer `codemode` because it keeps discovery, tool calls, and intermediate values inside one sandbox execution. Legacy `search` still exposes the full catalog JS filter for compatibility and for callers that need full schema/dts metadata.
```

- [ ] **Step 2: Update `docs/services/UPSTREAM.md`**

Replace stale wording that says Code Mode exposes only synthetic `search` / `execute` helpers with:

```markdown
If gateway-wide `[code_mode].enabled = true`, raw upstream tools are hidden from `list_tools()` and exposed through the primary synthetic `codemode` tool. Compatibility `search` and `execute` tools remain available during the migration window.
```

- [ ] **Step 3: Run stale-text scan**

Run:

```bash
rg -n "two MCP tools|Use before execute|search / execute|search and execute tools|arcane\\.containers|github\\.list_pull_requests" docs crates/lab/src/mcp crates/lab/src/dispatch/gateway/code_mode
```

Expected: no stale primary guidance claiming Code Mode is only `search` + `execute`; no examples use bare upstream globals.

---

### Task 5: Full Verification

**Files:**
- No new source files expected.
- Test: workspace verification.

**Interfaces:**
- Consumes: all prior tasks.
- Produces: proof that the primary one-tool path works and compatibility paths remain intact.

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 2: Run focused Code Mode tests**

Run:

```bash
cargo test -p labby code_mode --all-features
cargo test -p labby call_tool_codemode --all-features
cargo test -p labby handlers_tools --all-features
```

Expected: PASS.

- [ ] **Step 3: Run all-features workspace tests**

Run:

```bash
cargo nextest run --workspace --all-features
```

Expected: PASS.

- [ ] **Step 4: Run clippy**

Run:

```bash
cargo clippy --workspace --all-features -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Manual smoke with local labby if available**

Run:

```bash
cargo run -p labby --all-features -- mcp serve stdio
```

From an MCP client, verify `list_tools` contains `codemode`, `search`, and `execute` in Code Mode mode. Then call:

```json
{
  "code": "async () => { const matches = await codemode.search({ query: 'containers', limit: 1 }); return { total: matches.total, first: matches.results[0]?.path ?? null }; }"
}
```

Expected: returns `code_mode_execute_trace` with a `result.total` number and no upstream `calls`.

---

## Self-Review

**Spec coverage:** The plan covers the primary `codemode` MCP surface, local in-sandbox search/describe, JS chaining, existing execute compatibility, legacy search compatibility, host-side policy, destructive definition, route-scope filtering, Apps metadata, docs, and all-features verification.

**Placeholder scan:** The plan uses exact file paths, function names, command lines, and expected results. Deferrable work is explicitly listed as out of this implementation slice.

**Type consistency:** `CODE_MODE_TOOL_NAME`, `CodeModeDiscoveryEntry`, `generate_discovery_js`, `generate_js_proxy`, `build_code_mode_proxy_for_tests`, and `CODE_MODE_DESCRIPTION` are defined before later tasks consume them.
