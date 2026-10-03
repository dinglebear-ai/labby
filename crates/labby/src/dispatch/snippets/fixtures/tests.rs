use super::*;
use serde_json::json;

#[tokio::test(flavor = "current_thread")]
async fn generated_fixture_preserves_optional_input_omissions_for_replay() {
    let home = tempfile::tempdir().unwrap();
    let _home = crate::dispatch::helpers::TestLabHomeGuard::set(home.path().to_path_buf());
    let body = "---\nname: fixture-input-omission\ndescription: Synthetic input replay regression\ninputs:\n  optional:\n    type: string\n  defaulted:\n    type: integer\n    default: 7\n  supplied:\n    type: boolean\ntools:\n  - synthetic::lookup\n---\n```js\nasync input => input\n```\n";
    super::super::store::create_user_snippet(
        home.path(),
        "fixture-input-omission",
        body,
        None,
        false,
    )
    .unwrap();
    let snippet = resolve_snippet(
        home.path(),
        &builtin_snippet_dir(),
        "fixture-input-omission",
    )
    .unwrap();
    for supplied in [
        Value::Null,
        json!({}),
        json!({"supplied":false}),
        json!({"optional":"value", "defaulted":9}),
    ] {
        let request = json!({"name":snippet.name, "params":supplied,
            "schemas":{"synthetic::lookup":{"output_schema":{"type":"boolean"}}}});
        let report = generate(None, request, None).await.unwrap();
        assert_eq!(report["ready"], true);
        let fixture: SnippetFixture = serde_json::from_value(report["fixture"].clone()).unwrap();
        let replay_params = serde_json::to_value(&fixture.params).unwrap();
        assert_eq!(
            replay_params,
            if supplied.is_null() {
                json!({})
            } else {
                supplied.clone()
            }
        );
        assert_eq!(
            merge_snippet_input(&snippet, replay_params).unwrap(),
            merge_snippet_input(&snippet, supplied).unwrap()
        );
    }
    assert!(
        generate(
            None,
            json!({"name":snippet.name,"params":{"optional":null},
        "schemas":{"synthetic::lookup":{"output_schema":{"type":"boolean"}}}}),
            None
        )
        .await
        .is_err()
    );
}

#[test]
fn fixture_output_is_complete_and_never_clobbers_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.json");
    let fixture = json!({"calls":[{"tool":"synthetic::lookup","result":true}]});
    write_fixture_output(&path, &fixture).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&saved).unwrap(), fixture);
    assert_eq!(saved.last(), Some(&b'\n'));
    assert!(write_fixture_output(&path, &json!({"replacement":true})).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), saved);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn failed_fixture_write_leaves_no_partial_destination_or_staging_file() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.json");
    let error = publish_fixture_output(&path, |file| {
        file.write_all(b"{\"partial\":")?;
        anyhow::bail!("injected write failure")
    })
    .unwrap_err();
    assert!(format!("{error:#}").contains("injected write failure"));
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

fn scoped_context() -> SnippetDispatchContext {
    let mut context = SnippetDispatchContext::trusted_local();
    context.execution_scope = labby_codemode::ToolScope::scoped_namespaces(
        vec!["synthetic".into()],
        vec!["synthetic::lookup".into()],
    );
    context.capability_filter_fingerprint = context.execution_scope.fingerprint();
    context
}

fn install_scoped_snippet(home: &std::path::Path, tools: &[&str]) {
    let declarations = tools
        .iter()
        .map(|tool| format!("  - {tool}\n"))
        .collect::<String>();
    let body = format!(
        "---\nname: fixture-admission\ndescription: Synthetic admission regression\ntools:\n{declarations}---\n```js\nasync () => true\n```\n"
    );
    super::super::store::create_user_snippet(home, "fixture-admission", &body, None, false)
        .unwrap();
}

fn assert_admission_error(error: ToolError, expected: &str) {
    match error {
        ToolError::InvalidParam { message, param } => {
            assert_eq!(param, "fixture");
            assert_eq!(message, expected);
        }
        other => panic!("expected fixture admission rejection, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn caller_tool_scope_rejects_fixture_selection_and_supplied_maps_before_discovery() {
    let home = tempfile::tempdir().unwrap();
    let _home = crate::dispatch::helpers::TestLabHomeGuard::set(home.path().to_path_buf());
    install_scoped_snippet(home.path(), &["synthetic::lookup", "synthetic::denied"]);
    let context = scoped_context();
    // With no manager, an admitted metadata request reaches the discovery guard.
    let allowed = json!({"name":"fixture-admission", "tools":["synthetic::lookup"]});
    assert_admission_error(
        generate(None, allowed, Some(context.clone()))
            .await
            .unwrap_err(),
        "schema discovery requires a gateway; alternatively supply saved schemas",
    );
    for params in [
        json!({"name":"fixture-admission", "tools":["synthetic::denied"]}),
        json!({"name":"fixture-admission", "schemas":{"synthetic::denied":{}}}),
        json!({"name":"fixture-admission", "tools":["synthetic::lookup"],
            "schemas":{"synthetic::lookup":{}, "synthetic::denied":{}}}),
        json!({"name":"fixture-admission", "tools":["synthetic::lookup"],
            "results":{"synthetic::denied":true}}),
    ] {
        assert_admission_error(
            generate(None, params, Some(context.clone()))
                .await
                .unwrap_err(),
            "fixture tool is outside the declared or caller tool scope",
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn snippet_declarations_reject_fixture_selection_and_supplied_maps_before_discovery() {
    let home = tempfile::tempdir().unwrap();
    let _home = crate::dispatch::helpers::TestLabHomeGuard::set(home.path().to_path_buf());
    install_scoped_snippet(home.path(), &["synthetic::lookup"]);
    assert_admission_error(
        generate(
            None,
            json!({"name":"fixture-admission", "tools":["synthetic::undeclared"]}),
            None,
        )
        .await
        .unwrap_err(),
        "fixture tools are outside the snippet declaration",
    );
    for params in [
        json!({"name":"fixture-admission", "schemas":{"synthetic::undeclared":{}}}),
        json!({"name":"fixture-admission", "results":{"synthetic::undeclared":true}}),
    ] {
        assert_admission_error(
            generate(None, params, None).await.unwrap_err(),
            "fixture tool is outside the declared or caller tool scope",
        );
    }
}

fn saved_fixture() -> SnippetFixture {
    serde_json::from_value(json!({
        "schemas": {"synthetic::lookup": {
            "input_schema": {"type":"object"},
            "output_schema": {"type":"boolean"}
        }},
        "calls": [{"tool":"synthetic::lookup", "result":true}]
    }))
    .unwrap()
}

fn compare(
    fixture: SnippetFixture,
    contracts: BTreeMap<String, FixtureSchemas>,
) -> Result<Value, ToolError> {
    finish(
        "example",
        vec!["synthetic::lookup".into()],
        Value::Null,
        contracts,
        BTreeMap::new(),
        FixtureVariant::Minimal,
        Some(fixture),
    )
}

#[test]
fn check_reports_unchanged_changed_and_missing_contracts() {
    let fixture = saved_fixture();
    let mut contracts = fixture.schemas.clone();
    assert_eq!(
        compare(fixture.clone(), contracts.clone()).unwrap()["ready"],
        true
    );
    contracts.get_mut("synthetic::lookup").unwrap().input_schema =
        Some(json!({"type":"object", "additionalProperties":false}));
    for current in [contracts, BTreeMap::new()] {
        let report = compare(fixture.clone(), current).unwrap();
        assert_eq!(report["ready"], false);
        assert_eq!(report["changed_tools"], json!(["synthetic::lookup"]));
    }
}

#[test]
fn check_rejects_invalid_current_contracts_even_in_unused_properties() {
    let fixture = saved_fixture();
    for schema in [
        json!({"type":"object", "properties":{"unused":{"type":"string", "format":"email"}}}),
        json!({"type":"object", "properties":{"unused":{"$ref":"#/$defs/missing"}}}),
    ] {
        let mut contracts = fixture.schemas.clone();
        contracts.get_mut("synthetic::lookup").unwrap().input_schema = Some(schema);
        assert!(compare(fixture.clone(), contracts).is_err());
    }
}

#[test]
fn check_rejects_saved_contracts_without_valid_fixtures() {
    assert!(validate_check_fixture(&SnippetFixture::default()).is_err());
    let mut fixture = saved_fixture();
    fixture.calls[0].result = json!("invalid boolean response");
    assert!(validate_check_fixture(&fixture).is_err());
}

#[test]
fn missing_output_is_explicit_and_override_preserves_contracts() {
    let tools = vec!["synthetic::lookup".into()];
    let contracts = BTreeMap::from([(
        "synthetic::lookup".into(),
        FixtureSchemas {
            input_schema: Some(
                json!({"type":"object","required":["id"],"properties":{"id":{"type":"integer"}}}),
            ),
            output_schema: None,
            ..Default::default()
        },
    )]);
    let draft = build_draft(
        "example",
        tools.clone(),
        json!({}),
        contracts.clone(),
        BTreeMap::new(),
        FixtureVariant::Populated,
    )
    .unwrap();
    assert!(!draft.ready);
    assert_eq!(draft.failures.len(), 1);
    let draft = build_draft(
        "example",
        tools,
        json!({}),
        contracts,
        BTreeMap::from([("synthetic::lookup".into(), json!({"ok":true}))]),
        FixtureVariant::Populated,
    )
    .unwrap();
    assert!(draft.ready);
    assert!(
        draft.fixture.schemas["synthetic::lookup"]
            .input_schema
            .is_some()
    );
}
