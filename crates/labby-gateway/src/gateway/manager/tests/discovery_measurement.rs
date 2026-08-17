//! Structural activation measurement for the native Web tool-browser path.
//!
//! This is deliberately test-only: it compares the least-expensive safe JS
//! alternative with a direct borrowed projection over the same eager render.

use labby_codemode::{CodeModeToolSafety, ToolDescriptor};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct FixtureSpec {
    version: u8,
    tool_count: usize,
    namespace: String,
    name_prefix: String,
    description: String,
    query: String,
    limit: usize,
}

#[derive(Debug)]
struct Measurement {
    render_acquisitions: usize,
    runner_starts: usize,
    dts_generations: usize,
    full_catalog_serialization_bytes: usize,
    serialized_response_bytes: usize,
    returned_dtos: usize,
    tool_calls: usize,
}

fn fixture() -> (FixtureSpec, Vec<ToolDescriptor>) {
    let spec: FixtureSpec = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../labby-codemode/tests/fixtures/discovery-v1.json"
    )))
    .expect("valid discovery fixture");
    assert_eq!(spec.version, 1);
    let tools = (0..spec.tool_count)
        .map(|index| {
            ToolDescriptor::tool_with_safety(
                &spec.namespace,
                &format!("{}{index}", spec.name_prefix),
                &spec.description,
                None,
                None,
                Some(CodeModeToolSafety {
                    read_only: Some(true),
                    destructive: None,
                }),
            )
        })
        .collect();
    (spec, tools)
}

#[test]
fn discovery_native_activation_measurement() {
    let (spec, tools) = fixture();
    let full_catalog = serde_json::to_vec(&tools).expect("serialize full render");

    // The proposed native path borrows the already-authorized render and owns
    // only its bounded final DTOs. This test-only projection intentionally does
    // not add a production search API before the activation decision.
    let tokens = spec.query.split_ascii_whitespace().collect::<Vec<_>>();
    let projected = tools
        .iter()
        .filter(|entry| {
            let haystack = format!(
                "{} {} {} {}",
                entry.namespace, entry.name, entry.description, entry.signature
            )
            .to_ascii_lowercase();
            tokens.iter().all(|token| haystack.contains(token))
        })
        .take(spec.limit)
        .map(|entry| {
            json!({
                "id": entry.id,
                "namespace": entry.namespace,
                "name": entry.name,
                "description": entry.description,
                "signature": entry.signature,
                "safety": entry.safety,
            })
        })
        .collect::<Vec<_>>();
    let returned_dtos = projected.len();
    let response =
        serde_json::to_vec(&json!({ "results": projected })).expect("serialize bounded projection");
    let native = Measurement {
        render_acquisitions: 1,
        runner_starts: 0,
        dts_generations: tools.len(),
        // Current ToolsRender eagerly owns catalog_json, so a cold native
        // acquisition does not avoid this existing render cost.
        full_catalog_serialization_bytes: full_catalog.len(),
        serialized_response_bytes: response.len(),
        returned_dtos,
        tool_calls: 0,
    };
    // A Web adapter that starts Code Mode solely to run codemode.search must
    // acquire the render, inject its full catalog into a runner, and start at
    // least one sandbox process/runtime. Its final response can be equally
    // bounded, but it cannot avoid that setup work.
    let javascript = Measurement {
        render_acquisitions: 1,
        runner_starts: 1,
        dts_generations: tools.len(),
        full_catalog_serialization_bytes: full_catalog.len(),
        serialized_response_bytes: response.len(),
        returned_dtos,
        tool_calls: 0,
    };

    eprintln!("native={native:?} javascript={javascript:?}");

    assert_eq!(native.tool_calls, 0);
    assert_eq!(native.runner_starts, 0);
    assert_eq!(native.render_acquisitions, 1);
    assert!(native.serialized_response_bytes <= 256 * 1024);
    assert!(native.returned_dtos <= 50);
    assert_eq!(javascript.tool_calls, 0);
    assert_eq!(javascript.runner_starts, 1);
    assert_eq!(javascript.render_acquisitions, 1);
    assert_eq!(javascript.dts_generations, native.dts_generations);
    assert_eq!(
        javascript.full_catalog_serialization_bytes,
        native.full_catalog_serialization_bytes
    );
    assert!(javascript.full_catalog_serialization_bytes > 3_000_000);
    assert_eq!(
        javascript.serialized_response_bytes,
        native.serialized_response_bytes
    );
    assert_eq!(javascript.returned_dtos, native.returned_dtos);
}
