use super::RESOURCE_READ_EXAMPLE;

#[test]
fn resource_recovery_example_reassembles_all_blocks_without_replaying_tools() {
    let example = serde_json::to_string(RESOURCE_READ_EXAMPLE).unwrap();
    let script = format!(
        r#"
        const resource = {{contents: [
          {{uri: "lab://upstream/demo/skill", text: "🦀\"\n".repeat(4000)}},
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
          let offset = 0, assembled = "", chunks = 0;
          do {{
            const code = {example}
              .replace("REPLACE_WITH_DISCOVERED_RESOURCE_URI", "lab://upstream/demo/skill")
              .replace("offset = 0", "offset = " + offset);
            const page = await eval("(" + code + ")")();
            if (page.next_offset <= offset || page.chunk.length > 1000)
              throw new Error("invalid progress or bound");
            assembled += page.chunk;
            offset = page.next_offset;
            chunks++;
            if (page.done) break;
            if (chunks > 100) throw new Error("nonterminating pagination");
          }} while (true);
          globalThis.result = JSON.stringify({{
            recovered: JSON.parse(assembled), reads, chunks
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
    assert_eq!(
        value["recovered"]["contents"][0]["text"],
        "🦀\"\n".repeat(4000)
    );
    assert_eq!(value["recovered"]["contents"][1]["blob"], "YWJj");
}
