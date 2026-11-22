use super::*;
use crate::types::{CatalogDescriptor, CodeModeCatalogKind, CodeModeDiscoveryEntry};

fn discovery_entry(namespace: &str, name: &str, description: &str) -> CodeModeDiscoveryEntry {
    let path = format!("{namespace}.{name}");
    CodeModeDiscoveryEntry {
        tools: None,
        kind: CodeModeCatalogKind::Tool,
        id: format!("{namespace}::{name}"),
        path: path.clone(),
        namespace: namespace.to_string(),
        name: name.to_string(),
        helper: format!("codemode.{path}"),
        description: description.to_string(),
        safety: None,
        signature: format!("codemode.{path}(params: unknown): Promise<unknown>"),
        tags: Vec::new(),
        inputs: Vec::new(),
    }
}

#[test]
fn discovery_preamble_preserves_existing_codemode_object() {
    let entries = vec![discovery_entry("arcane", "containers", "List containers")];
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("js");
    assert!(js.contains("globalThis.codemode = globalThis.codemode || {}"));
    assert!(js.contains("codemode.search"));
    assert!(js.contains("codemode.describe"));
    // #787 adds explicit schema lookup status/error fields to describe().
    // Keep guarding the actual discovery payload instead of banning the
    // word "schema" from the generated helper implementation.
    assert!(!js.contains("\"schema\":"));
    assert!(!js.contains("\"output_schema\":"));
    assert!(js.contains("schema_status"));
    assert!(js.contains("schema_error"));
    // No embedded type-declaration lookup table — `.dts` appears only as a
    // property name on the lazily-fetched `__lab_internal::describe_types`
    // response (see `discovery_describe_fetches_tool_types_lazily`), never
    // as pre-injected type content.
    assert!(!js.contains("__codemodeTypes"));
}

#[test]
fn discovery_preamble_advertises_search_describe_read_resource_step_and_batch_semantics() {
    let entries = vec![
        discovery_entry("github", "search_issues", "Search GitHub issues"),
        discovery_entry("github", "list_pull_requests", "List GitHub pull requests"),
    ];
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("js");

    assert!(js.contains("typeof input === \"object\""));
    assert!(js.contains("Math.max(1, Math.min(50"));
    assert!(js.contains("truncated: total > limit"));
    assert!(js.contains("raw === entry.id || raw === entry.path || raw === entry.helper"));
    assert!(js.contains("ambiguous_target"));
    assert!(js.contains("unknown_tool"));
    // codemode.step delegates execution and best-effort journal recording
    // to the runner-side __labCodemodeStep bridge (not the old inline stub).
    assert!(js.contains("globalThis.__labCodemodeStep(name, fn)"));
    // codemode.batch is an isolation helper: mixed success/failure jobs
    // settle independently and return partitioned results.
    assert!(js.contains("codemode.batch = async function(jobs)"));
    assert!(js.contains("codemode.readResource = async function(uri)"));
    assert!(js.contains("__lab_internal::read_resource"));
    assert!(js.contains("codemode.getPrompt = async function(prompt, args)"));
    assert!(js.contains("__lab_internal::get_prompt"));
    assert!(js.contains("codemode.listSkills = async function()"));
    assert!(js.contains("codemode.getSkill = async function(uri)"));
    assert!(js.contains("codemode.readSkill = async function(uri)"));
    assert!(js.contains("__lab_internal::list_skills"));
    assert!(js.contains("__lab_internal::get_skill"));
    assert!(js.contains("__lab_internal::read_skill"));
    assert!(js.contains("Promise.allSettled"));
    assert!(js.contains("return Promise.resolve().then(function()"));
    assert!(js.contains("ok.push({ i: index, value: result.value })"));
    assert!(js.contains("failed.push({ i: index, error: decodeBatchError"));
    assert!(js.contains("must be a function or Promise"));
    assert!(js.contains("all_ok: failed.length === 0"));
}

#[test]
fn discovery_describe_fetches_tool_types_lazily() {
    // Regardless of how large `.dts` is on the source entry, NONE of it is
    // embedded in the generated preamble — the tool-branch of
    // `codemode.describe` fetches it from the host on demand instead.
    let mut entry = CatalogDescriptor::tool("github", "list_tags", "", None, None);
    entry.dts =
        "type UniqueLazySchemaFixture = { owner: string; repo: string; perPage?: number };\n"
            .to_string();
    let js = generate_discovery_js(&[CodeModeDiscoveryEntry::from_catalog(&entry)], 0.5, &[])
        .expect("js");

    assert!(!js.contains("__codemodeTypes"));
    assert!(!js.contains("UniqueLazySchemaFixture"));
    assert!(!js.contains("perPage"));
    assert!(js.contains("codemode.describe = async function(target)"));
    assert!(js.contains(r#"await callTool("__lab_internal::describe_types", { id: entry.id })"#));
    assert!(js.contains("typeResponse.dts"));
    assert!(js.contains("```typescript"));
    // Regression guard: the describe_types round trip must stay inside a
    // try block, and a rejection must be caught and converted into the
    // explicit #787 incomplete-schema contract instead of propagating into
    // a `describe()` rejection. See the end-to-end test in
    // `crates/labby/tests/code_mode_runner.rs` for the behavioral proof.
    assert!(
        js.contains(
            "try {\n      var typeResponse = await callTool(\"__lab_internal::describe_types\""
        ),
        "the describe_types call must be the first statement inside a try block: {js}"
    );
    assert!(
        js.contains("} catch (e) {\n      schemaStatus = \"unavailable\";"),
        "a rejected describe_types call must expose an unavailable schema status: {js}"
    );
    assert!(
        js.contains("schemaError = JSON.parse(typeErrorMessage)"),
        "a rejected describe_types call must preserve the structured lookup error: {js}"
    );
}

#[test]
fn discovery_search_index_stays_lean_when_tools_carry_types() {
    let mut entry = CatalogDescriptor::tool("github", "list_tags", "", None, None);
    entry.schema = Some(serde_json::json!({
        "type": "object",
        "properties": { "owner": { "type": "string" } },
        "required": ["owner"],
    }));
    entry.dts = "type UniqueLazySchemaFixture = { owner: string };\n".to_string();
    let js = generate_discovery_js(&[CodeModeDiscoveryEntry::from_catalog(&entry)], 0.5, &[])
        .expect("js");

    // Neither field is copied into the discovery entry
    // and types are never embedded anywhere now — assert JSON-schema-shaped
    // key syntax (colon-suffixed, distinguishing it from unrelated JS
    // identifiers/string literals like `push("required")`) is absent from
    // the ENTIRE generated preamble, not just a slice of it.
    assert!(!js.contains("\"properties\":"));
    assert!(!js.contains("\"required\":"));
    assert!(!js.contains("UniqueLazySchemaFixture"));
}

#[test]
fn four_thousand_tool_discovery_payload_stays_bounded_with_safety() {
    let entries = (0..4_000)
        .map(|index| {
            let mut entry = discovery_entry(
                "fixture",
                &format!("tool_{index}"),
                "A deterministic catalog fixture description",
            );
            entry.safety = Some(crate::types::CodeModeToolSafety {
                read_only: Some(true),
                destructive: None,
            });
            entry
        })
        .collect::<Vec<_>>();
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("4k discovery JS");

    assert!(
        js.len() < 2_000_000,
        "4k preamble grew to {} bytes",
        js.len()
    );
    assert!(
        js.len() / entries.len() < 500,
        "4k preamble exceeded 500 bytes/tool: {}",
        js.len() / entries.len()
    );
    assert!(!js.contains("\"dts\":"));
    assert!(!js.contains("\"schema\":"));
}

#[test]
fn discovery_search_zero_result_returns_actionable_hint() {
    let entries = vec![discovery_entry(
        "github",
        "list_tags",
        "List repository tags",
    )];
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("js");

    assert!(js.contains("hint: __codemodeNoMatchHint"));
    assert!(js.contains("Broaden the query or try synonyms."));
    assert!(!js.contains("__meta__"));
    assert_eq!(js.matches("hint: __codemodeNoMatchHint").count(), 2);
}

#[test]
fn discovery_serializes_intrinsic_safety_and_omits_unknown_facts() {
    let mut classified = discovery_entry("github", "list_tags", "List tags");
    classified.safety = Some(crate::types::CodeModeToolSafety {
        read_only: Some(true),
        destructive: None,
    });
    let classified = serde_json::to_value(classified).expect("serialize classified entry");
    assert_eq!(classified["safety"]["read_only"], true);
    assert!(classified["safety"].get("destructive").is_none());

    let unknown = serde_json::to_value(discovery_entry(
        "github",
        "mutate_unknown",
        "Unknown safety",
    ))
    .expect("serialize unknown entry");
    assert!(unknown.get("safety").is_none());
}

#[test]
fn generate_discovery_js_includes_semantic_blend() {
    let entries = vec![discovery_entry("arcane", "containers", "List containers")];
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("js generation succeeds");
    assert!(js.contains("__lab_internal::semantic_rank"));
    assert!(js.contains("blendedScore"));
    assert!(js.contains("codemode.search = async function"));
    assert!(js.contains("__lab_internal::artifact_search"));
    assert!(js.contains("searchEntries.push(remoteEntry)"));
}

#[test]
fn query_backed_skill_can_be_described_after_search() {
    let js = generate_discovery_js(&[], 0.5, &[]).expect("js");
    let remote = CodeModeDiscoveryEntry::from_catalog(&CatalogDescriptor::metadata(
        CodeModeCatalogKind::Skill,
        "public_depot",
        "depot:skill:fixture",
        "fixture skill",
        "query-backed result",
        vec!["skill".to_owned()],
    ));
    let remote = serde_json::to_string(&remote).expect("remote entry");
    let script = format!(
        "{js}\n\
         globalThis.callTool = async (id) => id === '__lab_internal::artifact_search'\n\
           ? {{entries: [{remote}]}} : {{ranked: []}};\n\
         globalThis.result = null;\n\
         (async () => {{\n\
           const hit = (await codemode.search('fixture')).results[0];\n\
           const described = await codemode.describe(hit.id);\n\
           globalThis.result = JSON.stringify({{hit: hit.id, described: described.id}});\n\
         }})().catch(error => {{ globalThis.result = JSON.stringify({{error: String(error)}}); }});"
    );
    let runtime = javy::Runtime::new(javy::Config::default()).expect("runtime");
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .expect("script");
    runtime.resolve_pending_jobs().expect("pending jobs");
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .expect("result");
    let value: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert!(value.get("error").is_none(), "{value}");
    assert_eq!(value["hit"], "depot:skill:fixture");
    assert_eq!(value["described"], value["hit"]);
}

#[test]
fn query_backed_partial_search_marks_results_incomplete() {
    let js = generate_discovery_js(&[], 0.5, &[]).expect("js");
    let remote = CodeModeDiscoveryEntry::from_catalog(&CatalogDescriptor::metadata(
        CodeModeCatalogKind::Skill,
        "public_depot",
        "depot:skill:fixture",
        "fixture skill",
        "query-backed result",
        Vec::new(),
    ));
    let remote = serde_json::to_string(&remote).expect("remote entry");
    let script = format!(
        "{js}\n\
         globalThis.callTool = async (id) => id === '__lab_internal::artifact_search'\n\
           ? {{entries: [{remote}], incompleteSources: ['team_depot']}} : {{ranked: []}};\n\
         globalThis.result = null;\n\
         (async () => {{ globalThis.result = JSON.stringify(await codemode.search('fixture')); }})()\n\
           .catch(error => {{ globalThis.result = JSON.stringify({{error: String(error)}}); }});"
    );
    let runtime = javy::Runtime::new(javy::Config::default()).expect("runtime");
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .expect("script");
    runtime.resolve_pending_jobs().expect("pending jobs");
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .expect("result");
    let value: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(value["results"][0]["id"], "depot:skill:fixture");
    assert_eq!(value["incomplete"], true);
    assert_eq!(
        value["incompleteSources"],
        serde_json::json!(["team_depot"])
    );
}

#[test]
fn artifact_helpers_forward_ids_and_pagination_inside_quickjs() {
    let js = generate_discovery_js(&[], 0.5, &[]).expect("discovery JS");
    let script = format!(
        r"{js}
        globalThis.calls = [];
        globalThis.callTool = async (id, params) => {{ calls.push({{id, params}}); return params; }};
        (async () => {{
            await codemode.readArtifact('opaque-id', {{offset:4, length:100, artifact_id:'wrong'}});
            await codemode.artifactInfo('opaque-id');
            await codemode.listArtifacts({{limit:5, cursor:'cursor'}});
            globalThis.result = JSON.stringify(calls);
        }})();"
    );
    let runtime = javy::Runtime::new(javy::Config::default()).expect("runtime");
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .expect("script");
    runtime.resolve_pending_jobs().expect("jobs");
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .expect("result");
    let calls: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(calls[0]["id"], "__lab_internal::read_artifact");
    assert_eq!(calls[0]["params"]["artifact_id"], "opaque-id");
    assert_eq!(calls[0]["params"]["offset"], 4);
    assert_eq!(calls[1]["id"], "__lab_internal::artifact_info");
    assert_eq!(calls[2]["params"]["limit"], 5);
}

#[test]
fn generate_discovery_js_interpolates_configured_blend_weight() {
    let entries = vec![discovery_entry("arcane", "containers", "List containers")];
    let js = generate_discovery_js(&entries, 0.75, &[]).expect("js generation succeeds");
    assert!(js.contains("var BLEND_WEIGHT = 0.75"));
}

#[test]
fn generate_discovery_js_search_never_throws_on_calltool_rejection() {
    // Structural check: the semantic-rank call is wrapped in try/catch
    // with `ranked = []` in the catch body, so a callTool rejection
    // (e.g. network_error surfaced as a JS Error) cannot propagate out
    // of codemode.search() and break the caller's script.
    let entries = vec![discovery_entry("arcane", "containers", "List containers")];
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("js generation succeeds");
    assert!(js.contains("catch (e) {"));
    assert!(js.contains("artifactSearchIncomplete = true"));
    assert!(js.contains("empty.incomplete = true"));
}

#[test]
fn discovery_describe_rejects_namespace_only_targets() {
    let entries = vec![discovery_entry("github", "search_issues", "Search issues")];
    let js = generate_discovery_js(&entries, 0.5, &[]).expect("js");
    assert!(js.contains("ambiguous_target"));
    assert!(js.contains("github.search_issues"));
}
