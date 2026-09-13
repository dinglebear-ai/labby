use super::RESOURCE_READ_EXAMPLE;

#[test]
fn resource_recovery_example_reassembles_all_blocks_without_replaying_tools() {
    verify_resource_pages(1000);
    verify_resource_pages(1);
}

fn verify_resource_pages(length: usize) {
    let source = RESOURCE_READ_EXAMPLE.replace("length = 1000", &format!("length = {length}"));
    let example = serde_json::to_string(&source).unwrap();
    let script = format!(
        r#"
        const resource = {{contents: [
          {{uri: "lab://upstream/demo/skill", text: "x" + "🦀".repeat(4000)}},
          {{uri: "lab://upstream/demo/blob", blob: "YWJj", mimeType: "text/plain"}}
        ]}};
        let reads = 0;
        globalThis.codemode = {{readResource: async uri => {{
          if (uri !== "lab://upstream/demo/skill") throw new Error("wrong URI");
          reads++;
          return resource;
        }}}};
        globalThis.callTool = () => {{throw new Error("must not replay tools");}};
        globalThis.result = null;
        (async () => {{
          let offset = 0, assembled = "", chunks = 0, pages = [];
          do {{
            const code = {example}
              .replace("REPLACE_WITH_DISCOVERED_RESOURCE_URI", "lab://upstream/demo/skill")
              .replace("offset = 0", "offset = " + offset);
            const page = await eval("(" + code + ")")();
            if (page.next_offset <= offset || page.chunk.length > Math.max(2, {length}))
              throw new Error("invalid progress or bound");
            pages.push(JSON.stringify(page));
            assembled += page.chunk;
            offset = page.next_offset;
            chunks++;
            if (page.done) break;
            if (chunks > 20000) throw new Error("nonterminating pagination");
          }} while (true);
          globalThis.result = JSON.stringify({{
            recovered: JSON.parse(assembled), reads, chunks, pages
          }});
        }})().catch(error => {{globalThis.result = JSON.stringify({{error: String(error)}});}});
        "#
    );
    let runtime = javy::Runtime::new(javy::Config::default()).unwrap();
    runtime
        .context()
        .with(|cx| cx.eval::<(), _>(script))
        .unwrap();
    runtime.resolve_pending_jobs().unwrap();
    let result: String = runtime
        .context()
        .with(|cx| cx.globals().get("result"))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(value.get("error").is_none(), "{value}");
    assert!(value["chunks"].as_u64().unwrap() > 1);
    assert_eq!(value["reads"], value["chunks"]);
    let mut recovered = String::new();
    for page in value["pages"].as_array().unwrap() {
        // The real runner decodes each JSON response through serde_json before
        // the caller can combine pages. Lone UTF-16 surrogates must not cross it.
        let decoded: serde_json::Value = serde_json::from_str(page.as_str().unwrap()).unwrap();
        recovered.push_str(decoded["chunk"].as_str().unwrap());
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&recovered).unwrap(),
        value["recovered"]
    );
    assert_eq!(
        value["recovered"]["contents"][0]["text"],
        format!("x{}", "🦀".repeat(4000))
    );
    assert_eq!(value["recovered"]["contents"][1]["blob"], "YWJj");
}

#[test]
fn recovery_markers_fit_small_byte_and_token_envelopes() {
    use super::{response_within_budget, truncate_execution_response};
    use crate::{shape::shape_final_result, types::CodeModeExecutionResponse};
    use labby_runtime::CodeModeResultShapePolicy;
    for budget in [1024, 1536, 2048, 4096, 24576] {
        for tokens in [256, 512, 6000] {
            for policy in [
                CodeModeResultShapePolicy::Off,
                CodeModeResultShapePolicy::Truncate,
            ] {
                let shaped = shape_final_result(
                    Some(serde_json::json!("\"🦀".repeat(30000))),
                    policy,
                    budget,
                    tokens,
                    4,
                );
                let response = CodeModeExecutionResponse {
                    execution_id: None,
                    result: shaped.result,
                    result_shaping: Some(shaped.metadata),
                    ui: None,
                    calls: vec![],
                    logs: vec![],
                    artifacts: vec![],
                };
                let truncated = truncate_execution_response(response, budget, tokens, 4);
                assert!(
                    response_within_budget(&truncated, budget, tokens, 4),
                    "budget={budget} tokens={tokens} policy={policy:?}"
                );
                let result = truncated.result.unwrap();
                assert_eq!(result["truncated"], true);
                assert!(
                    result["next_action"]
                        .as_str()
                        .unwrap()
                        .contains("Do not replay mutations")
                );
            }
        }
    }
}
