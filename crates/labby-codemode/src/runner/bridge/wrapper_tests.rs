use super::*;

#[test]
fn generated_wrapper_uses_the_shared_composed_snippet_budget() {
    let wrapped = wrap_code_mode("async () => ({ ok: true })", "");
    assert!(wrapped.contains(&format!(
        "globalThis.__labSnippetMaxBytes = {MAX_SNIPPET_RESOLVED_BYTES_PER_RUN};"
    )));
}

#[test]
fn loose_nested_artifact_error_handling_is_valid_in_quickjs() {
    let source = r#"
        const outcomes = [];
        for (const path of ["first", "second"]) {
          try {
            await writeArtifact(path, "payload");
            outcomes.push("published");
          } catch (error) {
            outcomes.push(JSON.parse(error.message).kind);
          }
        }
        return outcomes;
    "#;
    let runtime = javy::Runtime::new(javy::Config::default()).expect("QuickJS runtime");
    let normalized = crate::normalize_user_code(source);
    runtime.context().with(|cx| {
        cx.eval::<(), _>("globalThis.__testArtifactSeq = 0; globalThis.__labEmitArtifactWrite = () => globalThis.__testArtifactSeq++;")
            .expect("fixture emitter");
        let wrapped = wrap_code_mode(&normalized, "");
        cx.eval::<(), _>(wrapped).map_err(|error| {
            format!("{}\n{normalized}", javy::from_js_error(cx.clone(), error))
        }).expect("normalized fixture must parse");
    });
    runtime.resolve_pending_jobs().expect("first request");
    for seq in 0..2 {
        assert!(matches!(
            javy_main_promise_state(&runtime).expect("pending state"),
            JavyMainPromiseState::Pending
        ));
        javy_settle_tool_promise(
            &runtime,
            &CodeModeRunnerInput::ToolError {
                seq,
                error: Box::new(CodeModeCallError::new(
                    "invalid_param",
                    "reserved metadata path",
                )),
            },
        )
        .expect("artifact rejection remains catchable");
    }
    let result = match javy_main_promise_state(&runtime).expect("settled state") {
        JavyMainPromiseState::Resolved(result) => result,
        _ => None,
    };
    assert_eq!(
        result,
        Some(serde_json::json!(["invalid_param", "invalid_param"]))
    );
}

#[test]
fn full_result_helper_exposes_both_success_channels_in_quickjs() {
    let runtime = javy::Runtime::new(javy::Config::default()).expect("QuickJS runtime");
    let proxy = crate::preamble::generate_discovery_js(&[], 0.5, &[]).unwrap();
    runtime.context().with(|cx| {
        cx.eval::<(), _>("globalThis.__labEmitToolCall = (id, params, full) => { globalThis.requestFull = full; return 0; };").unwrap();
        cx.eval::<(), _>(wrap_code_mode(
            "async () => { const r = await codemode.callToolResult('demo::lookup', {}); return { data:r.structuredContent.rows, notice:r.content[1].text }; }",
            &proxy,
        )).unwrap();
    });
    runtime.resolve_pending_jobs().unwrap();
    assert!(
        matches!(
            javy_main_promise_state(&runtime).unwrap(),
            JavyMainPromiseState::Pending
        ),
        "full-result call must reach the bridge"
    );
    let full: bool = runtime
        .context()
        .with(|cx| cx.globals().get("requestFull"))
        .unwrap();
    assert!(full);
    javy_settle_tool_promise(&runtime, &CodeModeRunnerInput::ToolResult {
        seq:0,
        result: serde_json::json!({"structuredContent":{"rows":[1]},"content":[{"type":"text","text":"{\"rows\":[1]}"},{"type":"text","text":"Independent advisory"}]}),
    }).unwrap();
    let result = match javy_main_promise_state(&runtime).unwrap() {
        JavyMainPromiseState::Resolved(result) => result,
        _ => None,
    };
    assert_eq!(
        result,
        Some(serde_json::json!({"data":[1],"notice":"Independent advisory"}))
    );
}
