//! Product-level MCP tools whose identity and dispatch exist independently of upstream health.

use rmcp::model::Tool;

#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::CodeModeUpstreamDescription;
#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::code_mode_description;
use crate::mcp::catalog::CODE_MODE_TOOL_NAME;
#[cfg(feature = "gateway")]
use crate::mcp::handlers_tools::{
    code_mode_app_text_note, code_mode_execute_schema, code_mode_trace_output_schema,
};

/// Typed dispatcher key for a permanent product tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermanentToolId {
    CodeMode,
}

#[derive(Debug, Clone, Copy)]
struct PermanentToolEntry {
    id: PermanentToolId,
    name: &'static str,
}

/// Registry built with every MCP server composition.
///
/// The registry owns permanent identity and dispatch resolution. Request-time
/// visibility and authorization still decide whether a descriptor is listed or
/// a resolved tool may execute.
const PERMANENT_TOOLS: [PermanentToolEntry; 1] = [PermanentToolEntry {
    id: PermanentToolId::CodeMode,
    name: CODE_MODE_TOOL_NAME,
}];

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PermanentToolRegistry;

impl PermanentToolRegistry {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self
    }

    #[must_use]
    pub(crate) fn resolve(&self, name: &str) -> Option<PermanentToolId> {
        PERMANENT_TOOLS
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id)
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_descriptor(&self, upstreams: &[CodeModeUpstreamDescription]) -> Tool {
        debug_assert_eq!(
            PERMANENT_TOOLS
                .iter()
                .find(|entry| entry.name == CODE_MODE_TOOL_NAME)
                .map(|entry| entry.name),
            Some(CODE_MODE_TOOL_NAME),
        );
        // `codemode` is permanently text-only: the MCP App metadata belongs to
        // the optional `codemode_ui` twin so disabling the app surface can never
        // remove the execution entry point. See mcp/CLAUDE.md.
        Tool::new(
            CODE_MODE_TOOL_NAME,
            format!(
                "{}\n\n{}",
                code_mode_description(upstreams),
                code_mode_app_text_note()
            ),
            code_mode_execute_schema(),
        )
        .with_raw_output_schema(code_mode_trace_output_schema())
    }
}

#[cfg(test)]
mod tests {
    use super::{PermanentToolId, PermanentToolRegistry};
    use crate::mcp::catalog::CODE_MODE_TOOL_NAME;

    #[test]
    fn codemode_identity_is_registered_permanently() {
        let registry = PermanentToolRegistry::new();
        assert_eq!(
            registry.resolve(CODE_MODE_TOOL_NAME),
            Some(PermanentToolId::CodeMode)
        );
    }
}
