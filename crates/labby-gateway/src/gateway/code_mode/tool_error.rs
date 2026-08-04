//! Code Mode adapter over the shared upstream MCP tool-error analyzer.

use labby_codemode::{CodeModeCallError, CodeModeErrorEvidence, CodeModeToolSafetyHints};
use rmcp::model::CallToolResult;

use crate::upstream::tool_error::{
    McpToolErrorEvidence, McpToolSafetyHints, analyze_completed_tool_error,
};
use crate::upstream::types::UpstreamTool;

#[must_use]
pub(super) fn completed_tool_error(
    id: &str,
    result: &CallToolResult,
    safety: CodeModeToolSafetyHints,
) -> CodeModeCallError {
    let analysis = analyze_completed_tool_error(result);
    CodeModeCallError::tool_execution(
        id,
        analysis.kind,
        analysis.original_kind,
        analysis.cause,
        code_mode_evidence(analysis.evidence),
        safety,
        analysis.retry_after_ms,
    )
}

#[must_use]
pub(super) fn upstream_tool_safety(tool: &UpstreamTool) -> CodeModeToolSafetyHints {
    code_mode_safety(McpToolSafetyHints::from_annotations(
        tool.tool.annotations.as_ref(),
    ))
}

fn code_mode_safety(safety: McpToolSafetyHints) -> CodeModeToolSafetyHints {
    CodeModeToolSafetyHints {
        read_only_hint: safety.read_only_hint,
        destructive_hint: safety.destructive_hint,
        idempotent_hint: safety.idempotent_hint,
        open_world_hint: safety.open_world_hint,
    }
}

fn code_mode_evidence(evidence: McpToolErrorEvidence) -> CodeModeErrorEvidence {
    CodeModeErrorEvidence {
        content: evidence.content,
        structured_content: evidence.structured_content,
        parsed_error: evidence.parsed_error,
        omitted_content_blocks: evidence.omitted_content_blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_codemode::{CodeModeErrorOrigin, CodeModeSideEffectRisk};
    use rmcp::model::{CallToolResult, ContentBlock};
    use serde_json::json;

    #[test]
    fn adapter_preserves_shared_analysis() {
        let mut result = CallToolResult::error(vec![
            ContentBlock::text("step one succeeded"),
            ContentBlock::text("ERROR at step 2/3"),
        ]);
        result.structured_content = Some(json!({
            "error": {"kind":"upstream_error","message":"Exit code 7"},
            "partial": true,
        }));
        let error = completed_tool_error(
            "claude-dookie::Bash",
            &result,
            CodeModeToolSafetyHints::default(),
        );
        assert_eq!(error.kind, "tool_error");
        assert_eq!(error.origin, CodeModeErrorOrigin::ToolExecution);
        assert_eq!(error.original_kind.as_deref(), Some("upstream_error"));
        assert_eq!(error.evidence.unwrap().content.len(), 2);
    }

    #[test]
    fn read_only_hint_reduces_side_effect_risk() {
        let result = CallToolResult::error(vec![ContentBlock::text("bad query")]);
        let error = completed_tool_error(
            "search::query",
            &result,
            CodeModeToolSafetyHints {
                read_only_hint: Some(true),
                ..CodeModeToolSafetyHints::default()
            },
        );
        assert_eq!(error.side_effects, CodeModeSideEffectRisk::NoneExpected);
    }
}
