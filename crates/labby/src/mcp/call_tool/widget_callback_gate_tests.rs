//! Widget-callback destructive gate: app-only upstream tools must not be
//! refused for lack of a confirmation the app has no channel to give.
#![allow(clippy::disallowed_methods)] // fixtures construct upstream Tool values directly

use std::sync::Arc;

use labby_gateway::upstream::types::UpstreamTool;
use rmcp::model::{MetaObject, Tool, ToolAnnotations};

use super::{WidgetCallbackGate, classify_widget_callback_candidates, upstream_tool_is_app_only};

fn tool_with_visibility(
    name: &str,
    destructive: bool,
    visibility: Option<&[&str]>,
) -> UpstreamTool {
    let mut tool = Tool::new(
        name.to_string(),
        "fixture",
        Arc::new(serde_json::Map::new()),
    )
    .with_annotations(
        ToolAnnotations::new()
            .read_only(!destructive)
            .destructive(destructive),
    );
    if let Some(visibility) = visibility {
        tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "visibility": visibility }),
        )])));
    }
    UpstreamTool {
        input_schema: None,
        output_schema: None,
        destructive,
        upstream_name: Arc::from("connexin"),
        tool,
    }
}

fn classify(tool: UpstreamTool) -> Option<WidgetCallbackGate> {
    classify_widget_callback_candidates(
        "upstream_widget_sibling_callback",
        true,
        vec![("connexin".to_string(), tool)],
    )
}

#[test]
fn app_only_visibility_is_detected_precisely() {
    let app_only = tool_with_visibility("t", true, Some(&["app"]));
    assert!(upstream_tool_is_app_only(&app_only.tool));

    let both = tool_with_visibility("t", true, Some(&["model", "app"]));
    assert!(!upstream_tool_is_app_only(&both.tool));

    let app_then_model = tool_with_visibility("t", true, Some(&["app", "model"]));
    assert!(!upstream_tool_is_app_only(&app_then_model.tool));

    let model_only = tool_with_visibility("t", true, Some(&["model"]));
    assert!(!upstream_tool_is_app_only(&model_only.tool));

    let no_meta = tool_with_visibility("t", true, None);
    assert!(!upstream_tool_is_app_only(&no_meta.tool));

    let empty = tool_with_visibility("t", true, Some(&[]));
    assert!(!upstream_tool_is_app_only(&empty.tool));
}

#[test]
fn destructive_app_only_tool_is_allowed_through_the_widget_callback() {
    // The connexin case: `write_connexin_input` is destructive-annotated but
    // only the app can call it, and the app cannot answer an elicitation.
    let gate = classify(tool_with_visibility(
        "write_connexin_input",
        true,
        Some(&["app"]),
    ));
    match gate {
        Some(WidgetCallbackGate::Allowed {
            resolved,
            requires_scope_check,
        }) => {
            assert!(
                requires_scope_check,
                "route's scope requirement is preserved"
            );
            assert_eq!(resolved.tool.tool.name.as_ref(), "write_connexin_input");
            assert!(
                resolved.tool.destructive,
                "the tool's own classification is untouched"
            );
        }
        other => panic!("expected Allowed, got {}", gate_name(other.as_ref())),
    }
}

#[test]
fn destructive_model_visible_tool_still_hits_the_gate() {
    for visibility in [None, Some(&["model"][..]), Some(&["model", "app"][..])] {
        let gate = classify(tool_with_visibility("issue_write", true, visibility));
        assert!(
            matches!(gate, Some(WidgetCallbackGate::Destructive { .. })),
            "visibility {visibility:?} must stay gated, got {}",
            gate_name(gate.as_ref())
        );
    }
}

#[test]
fn non_destructive_tools_are_unaffected() {
    for visibility in [None, Some(&["app"][..]), Some(&["model", "app"][..])] {
        let gate = classify(tool_with_visibility("poll", false, visibility));
        assert!(
            matches!(gate, Some(WidgetCallbackGate::Allowed { .. })),
            "visibility {visibility:?} should be Allowed, got {}",
            gate_name(gate.as_ref())
        );
    }
}

fn gate_name(gate: Option<&WidgetCallbackGate>) -> &'static str {
    match gate {
        None => "None",
        Some(WidgetCallbackGate::Allowed { .. }) => "Allowed",
        Some(WidgetCallbackGate::Destructive { .. }) => "Destructive",
        Some(WidgetCallbackGate::Ambiguous { .. }) => "Ambiguous",
    }
}
