//! End-to-end fixture tests using the compiled product and its real QuickJS runner.
//! Every invocation gets an empty home and environment; no gateway is configured.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn invoke(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .args(["--json", "snippet"])
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("NO_COLOR", "1")
        .env("TMPDIR", home)
        .stdin(Stdio::null())
        .current_dir(home);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command.output().expect("run compiled labby")
}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .map_err(|error| {
            format!(
                "invalid JSON report: {error}; stdout={}; stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .expect("valid JSON report")
}

fn install(home: &Path, name: &str, code: &str, fixture: Option<Value>) {
    let dir = home.join(".labby/snippets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.js")), code).unwrap();
    if let Some(fixture) = fixture {
        std::fs::write(
            dir.join(format!("{name}.test.json")),
            serde_json::to_vec(&fixture).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn fast_triage_uses_real_runner_with_ten_synthetic_calls_and_no_gateway() {
    let home = tempfile::tempdir().unwrap();
    let output = invoke(home.path(), &["test", "unraid-linear-pr-triage"]);
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["passed"], true);
    assert_eq!(value["mode"], "mock");
    assert_eq!(value["metrics"]["tool_calls"], 10);
    assert_eq!(value["result"]["summary"]["issueCount"], 26);
    assert!(value["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
    for issue in value["result"]["issues"].as_array().unwrap() {
        assert!(issue.get("historicalPRs").is_none());
        assert!(issue.get("handoff").is_none());
    }
}

#[test]
fn deep_triage_uses_explicit_fixture_and_compact_handoffs() {
    let home = tempfile::tempdir().unwrap();
    let fixture = root().join("docs/snippets/unraid-linear-pr-triage.deep.test.json");
    let output = invoke(
        home.path(),
        &[
            "test",
            "unraid-linear-pr-triage",
            "--fixture",
            fixture.to_str().unwrap(),
            "--param",
            "deep=true",
        ],
    );
    let value = report(&output);
    assert!(output.status.success(), "{value}");
    assert_eq!(value["passed"], true);
    assert_eq!(value["mode"], "mock");
    assert_eq!(value["result"]["summary"]["handoffCalls"], 26);
    assert!(value["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
}

#[test]
fn missing_fixture_never_falls_back_to_live_execution() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "no-fixture",
        "async () => ({ ok: true })",
        None,
    );
    let output = invoke(home.path(), &["test", "no-fixture"]);
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("fixture"), "{text}");
    assert!(text.contains("live"), "{text}");
}

#[test]
fn swallowed_unexpected_mock_calls_still_fail_the_cli_exit_status() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "unexpected",
        "async () => { try { await callTool('synthetic::missing', {}); } catch (_) {} return { ok: true }; }",
        Some(json!({"calls": []})),
    );
    let output = invoke(home.path(), &["test", "unexpected"]);
    let value = report(&output);
    assert!(!output.status.success());
    assert_eq!(value["passed"], false);
    assert!(
        value["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("unexpected"))
    );
}

#[test]
fn global_bridge_escape_is_denied_and_reported_without_writing_artifacts() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "escape",
        "async () => { try { await globalThis.callTool('synthetic::escape', {}); } catch (_) {} return { ok: true }; }",
        Some(json!({"calls": []})),
    );
    let output = invoke(home.path(), &["test", "escape"]);
    let value = report(&output);
    assert!(!output.status.success());
    assert_eq!(value["passed"], false);
    assert!(!home.path().join(".labby/code-mode-artifacts").exists());
}

#[test]
fn generated_saved_schema_fixture_runs_offline_and_preserves_existing_files() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "schema-example",
        "async () => await callTool('synthetic::lookup', {id: 1})",
        None,
    );
    let schemas = home.path().join("schemas.json");
    std::fs::write(&schemas, serde_json::to_vec(&json!({"synthetic::lookup":{
        "input_schema":{"type":"object","required":["id"],"properties":{"id":{"type":"integer"}}},
        "output_schema":{"type":"object","required":["items"],"properties":{"items":{"type":"array","items":{"type":"string"}}}}
    }})).unwrap()).unwrap();
    let fixture = home.path().join(".labby/snippets/schema-example.test.json");
    let args = [
        "fixture",
        "schema-example",
        "--schemas",
        schemas.to_str().unwrap(),
        "--output",
        fixture.to_str().unwrap(),
    ];
    let generated = invoke(home.path(), &args);
    assert!(generated.status.success(), "{}", report(&generated));
    assert_eq!(
        report(&generated)["fixture"]["calls"][0]["result"],
        json!({"items":["synthetic"]})
    );
    let original = std::fs::read(&fixture).unwrap();
    let output = invoke(home.path(), &["test", "schema-example"]);
    assert!(output.status.success(), "{}", report(&output));
    assert!(!invoke(home.path(), &args).status.success());
    assert_eq!(std::fs::read(&fixture).unwrap(), original);
}

#[test]
fn wrong_actual_arguments_fail_even_when_snippet_returns_success() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "wrong-arguments",
        "async () => { await callTool('synthetic::lookup', {id: 'private-value'}); return {ok:true}; }",
        Some(
            json!({"calls":[{"tool":"synthetic::lookup","result":true}],"schemas":{"synthetic::lookup":{
            "input_schema":{"type":"object","required":["id"],"properties":{"id":{"type":"integer"}}},"output_schema":{"type":"boolean"}}}}),
        ),
    );
    let output = invoke(home.path(), &["test", "wrong-arguments"]);
    let value = report(&output);
    assert!(!output.status.success());
    assert_eq!(value["passed"], false);
    assert!(
        value["failures"][0]
            .as_str()
            .unwrap()
            .contains("input schema")
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-value"));
}

#[test]
fn missing_output_schema_produces_incomplete_draft_and_no_fixture_file() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "missing-output",
        "async () => await callTool('synthetic::lookup', {})",
        None,
    );
    let schemas = home.path().join("schemas.json");
    std::fs::write(
        &schemas,
        r#"{"synthetic::lookup":{"input_schema":{"type":"object"}}}"#,
    )
    .unwrap();
    let fixture = home.path().join("missing.test.json");
    let output = invoke(
        home.path(),
        &[
            "fixture",
            "missing-output",
            "--schemas",
            schemas.to_str().unwrap(),
            "--output",
            fixture.to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert_eq!(report(&output)["ready"], false);
    assert!(!fixture.exists());
}

#[test]
fn generated_contract_fingerprints_detect_changed_and_missing_schemas_offline() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "drift",
        "async () => await callTool('synthetic::lookup', {query:'synthetic'})",
        None,
    );
    let schemas = home.path().join("schemas.json");
    let mut contract = json!({"synthetic::lookup":{
        "input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}},
        "output_schema":{"type":"object","required":["items"],"properties":{"items":{"type":"array","items":{"type":"string"}}}}
    }});
    std::fs::write(&schemas, serde_json::to_vec(&contract).unwrap()).unwrap();
    let fixture = home.path().join("drift.test.json");
    let generated = invoke(
        home.path(),
        &[
            "fixture",
            "drift",
            "--schemas",
            schemas.to_str().unwrap(),
            "--output",
            fixture.to_str().unwrap(),
        ],
    );
    assert!(generated.status.success(), "{}", report(&generated));
    let saved = std::fs::read(&fixture).unwrap();
    assert!(
        report(&generated)["fixture"]["schemas"]["synthetic::lookup"]["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let args = [
        "fixture",
        "drift",
        "--schemas",
        schemas.to_str().unwrap(),
        "--check",
        fixture.to_str().unwrap(),
    ];
    assert!(invoke(home.path(), &args).status.success());
    contract["synthetic::lookup"]["input_schema"]["properties"]["query"]["minLength"] = json!(5);
    std::fs::write(&schemas, serde_json::to_vec(&contract).unwrap()).unwrap();
    let drift = invoke(home.path(), &args);
    assert!(!drift.status.success());
    assert_eq!(
        report(&drift)["changed_tools"],
        json!(["synthetic::lookup"])
    );
    std::fs::write(&schemas, b"{}").unwrap();
    let missing = invoke(home.path(), &args);
    assert!(!missing.status.success());
    assert_eq!(
        report(&missing)["changed_tools"],
        json!(["synthetic::lookup"])
    );
    assert_eq!(std::fs::read(&fixture).unwrap(), saved);
}

#[test]
fn generated_minimal_fixture_covers_empty_results_and_repeated_calls() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "empty-repeat",
        "async () => { const a = await callTool('synthetic::lookup', {query:'synthetic'}); const b = await callTool('synthetic::lookup', {query:'synthetic'}); return {empty:a.items.length===0 && b.items.length===0}; }",
        None,
    );
    let schemas = home.path().join("schemas.json");
    std::fs::write(&schemas, serde_json::to_vec(&json!({"synthetic::lookup":{
        "input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}},
        "output_schema":{"type":"object","required":["items"],"properties":{"items":{"type":"array","items":{"type":"string"}}}}
    }})).unwrap()).unwrap();
    let generated = invoke(
        home.path(),
        &[
            "fixture",
            "empty-repeat",
            "--schemas",
            schemas.to_str().unwrap(),
            "--variant",
            "minimal",
        ],
    );
    assert!(generated.status.success(), "{}", report(&generated));
    let mut fixture = report(&generated)["fixture"].clone();
    assert_eq!(fixture["calls"][0]["result"], json!({"items":[]}));
    fixture["calls"][0]["times"] = json!(2);
    fixture["budgets"]["tool_calls"] = json!(2);
    fixture["expect"] = json!({"/empty":true});
    let path = home.path().join("empty.test.json");
    std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let good = invoke(
        home.path(),
        &["test", "empty-repeat", "--fixture", path.to_str().unwrap()],
    );
    assert!(good.status.success(), "{}", report(&good));
    fixture["calls"][0]["times"] = json!(1);
    std::fs::write(&path, serde_json::to_vec(&fixture).unwrap()).unwrap();
    assert!(
        !invoke(
            home.path(),
            &["test", "empty-repeat", "--fixture", path.to_str().unwrap()]
        )
        .status
        .success()
    );
}

#[test]
fn synthetic_error_response_still_checks_actual_arguments_and_consumption() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "handled-error",
        "async () => { try { await callTool('synthetic::lookup', {query:'synthetic'}); } catch (error) { return {handled:true}; } return {handled:false}; }",
        Some(json!({
            "schemas":{"synthetic::lookup":{"input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}},"output_schema":{"type":"object"}}},
            "calls":[{"tool":"synthetic::lookup","error":{"kind":"timeout","message":"synthetic timeout"}}],
            "expect":{"/handled":true}
        })),
    );
    let output = invoke(home.path(), &["test", "handled-error"]);
    assert!(output.status.success(), "{}", report(&output));
    assert_eq!(report(&output)["calls"][0]["ok"], false);
    let source = home.path().join(".labby/snippets/handled-error.js");
    std::fs::write(source,"async () => { try { await callTool('synthetic::lookup', {'query':1}); } catch (error) { return {handled:true}; } }").unwrap();
    let bad = invoke(home.path(), &["test", "handled-error"]);
    assert!(!bad.status.success());
    assert!(
        report(&bad)["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("input schema"))
    );
}

#[test]
fn argument_contracts_survive_mutated_javascript_intrinsics() {
    for (name, mutation, restore) in [
        (
            "stringify",
            "const original=JSON.stringify; JSON.stringify=(value,...rest)=>value===args ? '{\"query\":\"valid\"}' : original(value,...rest);",
            "JSON.stringify=original;",
        ),
        (
            "parse",
            "const original=JSON.parse; JSON.parse=(value,...rest)=>value==='{'+'\"query\":1}' ? {query:'valid'} : original(value,...rest);",
            "JSON.parse=original;",
        ),
        (
            "ownership",
            "const original=Object.prototype.hasOwnProperty; Object.prototype.hasOwnProperty=()=>false;",
            "Object.prototype.hasOwnProperty=original;",
        ),
        (
            "push",
            "const original=Array.prototype.push; Array.prototype.push=()=>0;",
            "Array.prototype.push=original;",
        ),
        (
            "index-setter",
            "Object.defineProperty(Array.prototype,'0',{configurable:true,set(value){Object.defineProperty(this,'0',{configurable:true,enumerable:true,writable:true,value:value.params?{...value,params:{query:'valid'}}:value})}});",
            "delete Array.prototype['0'];",
        ),
        (
            "iteration",
            "const original=Array.prototype.map; Array.prototype.map=()=>[];",
            "Array.prototype.map=original;",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let code = format!(
            "async () => {{ const args={{query:1}}; {mutation} try {{ await callTool('synthetic::lookup',args); }} finally {{ {restore} }} return {{ok:true}}; }}"
        );
        install(
            home.path(),
            name,
            &code,
            Some(json!({
                "schemas":{"synthetic::lookup":{"input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}}}},
                "calls":[{"tool":"synthetic::lookup","result":{"ok":true}}]
            })),
        );
        let output = invoke(home.path(), &["test", name]);
        let value = report(&output);
        assert!(!output.status.success(), "{name}: {value}");
        assert!(
            value["failures"]
                .as_array()
                .unwrap()
                .iter()
                .any(|failure| failure.as_str().unwrap().contains("input schema")),
            "{name}: {value}"
        );
    }
}

#[test]
fn inherited_to_json_cannot_replace_mock_responses_or_private_contract_capture() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "inherited-json",
        "async () => { Object.prototype.toJSON=()=>({query:'valid',ok:false}); try { const response=await callTool('synthetic::lookup',{query:'valid'}); return {ok:response.ok}; } finally { delete Object.prototype.toJSON; } }",
        Some(json!({
            "schemas":{"synthetic::lookup":{"input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}}}},
            "calls":[{"tool":"synthetic::lookup","result":{"ok":true}}],
            "expect":{"/ok":true}
        })),
    );
    let output = invoke(home.path(), &["test", "inherited-json"]);
    assert!(output.status.success(), "{}", report(&output));
}

#[test]
fn own_to_json_uses_the_same_argument_semantics_as_the_json_wire() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "wire-json",
        "async () => { await callTool('synthetic::lookup',{query:1,toJSON:()=>({query:'valid'})}); return {ok:true}; }",
        Some(json!({
            "schemas":{"synthetic::lookup":{"input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}}}}},
            "calls":[{"tool":"synthetic::lookup","result":true}],
            "expect":{"/ok":true}
        })),
    );
    let output = invoke(home.path(), &["test", "wire-json"]);
    assert!(output.status.success(), "{}", report(&output));
}

#[test]
fn string_prototype_mutation_cannot_disable_argument_capture_byte_budget() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "string-budget",
        "async () => { const args={query:'💩'.repeat(140000)}; const iterator=String.prototype[Symbol.iterator]; const point=String.prototype.codePointAt; const code=String.prototype.charCodeAt; String.prototype[Symbol.iterator]=function*(){}; String.prototype.codePointAt=()=>0; String.prototype.charCodeAt=()=>0; try { await callTool('synthetic::lookup',args); } catch (_) {} finally { String.prototype[Symbol.iterator]=iterator; String.prototype.codePointAt=point; String.prototype.charCodeAt=code; } return {ok:true}; }",
        Some(json!({
            "schemas":{"synthetic::lookup":{"input_schema":{"type":"object","properties":{"query":{"type":"string"}}}}},
            "calls":[{"tool":"synthetic::lookup","result":true}]
        })),
    );
    let output = invoke(home.path(), &["test", "string-budget"]);
    let value = report(&output);
    assert!(!output.status.success(), "{value}");
    assert!(
        value["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|failure| failure.as_str().unwrap().contains("unexpected")),
        "{value}"
    );
}

#[test]
fn contract_check_does_not_require_repeating_saved_required_snippet_inputs() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".labby/snippets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("required-check.md"),
        "---\nname: required-check\ndescription: Required input contract check\ninputs:\n  path:\n    type: string\n    required: true\n---\n```js\nasync (input) => await callTool('synthetic::lookup',{path:input.path})\n```\n",
    )
    .unwrap();
    let schemas = home.path().join("schemas.json");
    std::fs::write(
        &schemas,
        serde_json::to_vec(&json!({"synthetic::lookup":{
            "input_schema":{"type":"object","properties":{"path":{"type":"string"}}},
            "output_schema":{"type":"boolean"}
        }}))
        .unwrap(),
    )
    .unwrap();
    let fixture = home.path().join("required.test.json");
    let generated = invoke(
        home.path(),
        &[
            "fixture",
            "required-check",
            "--schemas",
            schemas.to_str().unwrap(),
            "--param",
            "path=/synthetic",
            "--output",
            fixture.to_str().unwrap(),
        ],
    );
    assert!(generated.status.success(), "{}", report(&generated));
    let saved = std::fs::read(&fixture).unwrap();
    let checked = invoke(
        home.path(),
        &[
            "fixture",
            "required-check",
            "--schemas",
            schemas.to_str().unwrap(),
            "--check",
            fixture.to_str().unwrap(),
        ],
    );
    assert!(checked.status.success(), "{}", report(&checked));
    assert_eq!(std::fs::read(&fixture).unwrap(), saved);
}

#[test]
fn own_mock_response_serializers_preserve_live_json_mutability() {
    for mutation in [
        "response.toJSON=()=>({'count':7});",
        "Object.defineProperty(response,'toJSON',{value:()=>({'count':7})});",
    ] {
        let home = tempfile::tempdir().unwrap();
        let code = format!(
            "async () => {{ const response=await callTool('synthetic::lookup',{{}}); {mutation} return response; }}"
        );
        install(
            home.path(),
            "result-json",
            &code,
            Some(json!({
                "calls":[{"tool":"synthetic::lookup","result":{"count":0}}],
                "expect":{"/count":7}
            })),
        );
        let output = invoke(home.path(), &["test", "result-json"]);
        assert!(output.status.success(), "{}", report(&output));
    }
}

#[test]
fn missing_contract_metadata_cannot_pass_after_result_encoder_tampering() {
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        "missing-capture",
        "async () => { const keys=Object.keys; Object.keys=value=>keys(value).filter(key=>key!=='contract_calls'); await callTool('synthetic::lookup',{'query':1}); return {ok:true}; }",
        Some(json!({"calls":[{"tool":"synthetic::lookup","result":true}],
            "schemas":{"synthetic::lookup":{"input_schema":{"type":"object","properties":{"query":{"type":"string"}}}}}})),
    );
    let output = invoke(home.path(), &["test", "missing-capture"]);
    assert!(!output.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("contract_calls"), "{text}");
}
