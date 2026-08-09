//! Product-level MCP tools whose identity and dispatch exist independently of upstream health.

use rmcp::model::{Tool, ToolAnnotations};

#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::code_mode_description_with_suffix;
use crate::mcp::catalog::{CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME};
#[cfg(feature = "gateway")]
use crate::mcp::handlers_tools::{
    code_mode_app_text_note, code_mode_execute_schema, code_mode_trace_output_schema,
};

/// Typed dispatcher key for a permanent product tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PermanentToolId {
    CodeModeRead,
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
const PERMANENT_TOOLS: [PermanentToolEntry; 2] = [
    PermanentToolEntry {
        id: PermanentToolId::CodeModeRead,
        name: CODE_MODE_READ_TOOL_NAME,
    },
    PermanentToolEntry {
        id: PermanentToolId::CodeMode,
        name: CODE_MODE_TOOL_NAME,
    },
];

#[must_use]
pub(crate) fn code_mode_read_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(true)
}

#[must_use]
pub(crate) fn code_mode_full_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(true)
        .idempotent(false)
        .open_world(true)
}

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
    pub(crate) fn code_mode_descriptor(&self) -> Tool {
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
            code_mode_description_with_suffix(&code_mode_app_text_note()),
            code_mode_execute_schema(),
        )
        .with_annotations(code_mode_full_annotations())
        .with_raw_output_schema(code_mode_trace_output_schema())
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_read_descriptor(&self) -> Tool {
        Tool::new(
            CODE_MODE_READ_TOOL_NAME,
            code_mode_description_with_suffix(
                "Read-only Code Mode execution. Only upstream tools explicitly annotated readOnly=true are discoverable and callable; artifact writes are disabled. Use codemode for write-capable execution.",
            ),
            code_mode_execute_schema(),
        )
        .with_annotations(code_mode_read_annotations())
        .with_raw_output_schema(code_mode_trace_output_schema())
    }
}

#[cfg(test)]
mod tests {
    use super::{PermanentToolId, PermanentToolRegistry};
    #[cfg(feature = "gateway")]
    use crate::mcp::call_tool_codemode::CODE_MODE_DESCRIPTION_MAX_BYTES;
    use crate::mcp::catalog::{CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME};

    #[test]
    fn codemode_identity_is_registered_permanently() {
        let registry = PermanentToolRegistry::new();
        assert_eq!(
            registry.resolve(CODE_MODE_TOOL_NAME),
            Some(PermanentToolId::CodeMode)
        );
        assert_eq!(
            registry.resolve(CODE_MODE_READ_TOOL_NAME),
            Some(PermanentToolId::CodeModeRead)
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn codemode_descriptor_is_stable_and_final_description_is_bounded() {
        let registry = PermanentToolRegistry::new();
        let descriptor = registry.code_mode_descriptor();
        let description = descriptor.description.expect("description");
        assert!(description.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES);
        assert!(description.contains("codemode.search"));
        assert!(description.contains("text-only entry point"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn codemode_read_descriptor_is_truthfully_annotated_and_bounded() {
        let descriptor = PermanentToolRegistry::new().code_mode_read_descriptor();
        let annotations = descriptor.annotations.expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(true));
        assert!(
            descriptor.description.expect("description").len() <= CODE_MODE_DESCRIPTION_MAX_BYTES
        );
    }
}
