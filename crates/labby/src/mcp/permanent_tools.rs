//! Permanent product-tool identity/dispatch resolution, and the sole
//! construction site for Labby-owned MCP `Tool` descriptors.
//!
//! Two responsibilities live here deliberately:
//!
//! 1. `PermanentToolRegistry::resolve` maps permanent tool names to dispatch
//!    ids independently of upstream health.
//! 2. The `*_tool` / `*_descriptor` constructors below are the only place a
//!    Labby-owned descriptor is assembled. `handlers_tools::list_tools_impl`
//!    and `peer_contract::visible_tool_descriptors` both consume them, so the
//!    two listing paths cannot drift apart (see
//!    `handlers_tools/tests.rs` descriptor drift tests).
//!
//! Do not construct `Tool` values for Labby-owned tools anywhere else.

use std::sync::{Arc, LazyLock};

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::Value;

#[cfg(feature = "gateway")]
use crate::mcp::call_tool_codemode::{
    CodeModeUpstreamDescription, code_mode_description_with_suffix,
};
#[cfg(feature = "gateway")]
use crate::mcp::catalog::{
    ADD_SERVER_TOOL_NAME, CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME,
};
use crate::mcp::catalog::{CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, SERVER_LOGS_TOOL_NAME};
use crate::mcp::completion::action_schema;
use crate::mcp::handlers_tools::server_logs_tool_meta;
#[cfg(feature = "gateway")]
use crate::mcp::handlers_tools::{
    add_server_tool_meta, add_server_tool_schema, code_mode_app_text_note,
    code_mode_execute_schema, code_mode_tool_meta, code_mode_trace_output_schema,
    code_mode_ui_description, gateway_status_tool_meta, gateway_status_tool_schema,
    mcp_app_tool_description, mcp_app_tool_schema,
};
use crate::registry::RegisteredService;

/// Shared `{action, params, instance}` input schema advertised by every
/// builtin service tool. Kept private so callers must go through
/// [`PermanentToolRegistry::builtin_service_tool`]; the single definition site
/// exists for drift prevention, not performance.
fn builtin_action_schema() -> Arc<serde_json::Map<String, Value>> {
    static BUILTIN_ACTION_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> =
        LazyLock::new(|| Arc::new(action_schema()));
    Arc::clone(&BUILTIN_ACTION_SCHEMA)
}

/// Success-envelope output schema shared by every builtin service tool.
///
/// Mirrors `build_success` (mcp/envelope.rs) — the two must change in the same
/// commit. `data` is intentionally unconstrained: one tool serves many
/// actions, so a tool-level schema cannot describe per-action payloads.
///
/// Error envelopes are deliberately NOT described here — see
/// docs/contracts/mcp-tool-output.md §C3.2. The exemption for
/// `isError` results is ecosystem convention, not explicit spec text.
///
/// `additionalProperties` is `true` by decision (SPEC §5.2): closing the
/// envelope would make any future `build_success` field break all builtins'
/// advertised schemas at once, client-side. If `build_success` ever grows a
/// field, this schema changes in the same commit anyway — the open object just
/// means clients do not break first.
fn dispatch_envelope_output_schema() -> Arc<serde_json::Map<String, Value>> {
    static ENVELOPE_OUTPUT_SCHEMA: LazyLock<Arc<serde_json::Map<String, Value>>> = LazyLock::new(
        || match serde_json::json!({
            "type": "object",
            "properties": {
                "ok": { "const": true },
                "service": { "type": "string",
                    "description": "Service tool that answered the call." },
                "action": { "type": "string",
                    "description": "Resolved dotted action, including the built-in `help` and `schema` actions." },
                "data": { "description": "Action-specific payload; shape varies by action." }
            },
            "required": ["ok", "service", "action", "data"],
            "additionalProperties": true
        }) {
            Value::Object(map) => Arc::new(map),
            _ => unreachable!("dispatch envelope output schema must be an object"),
        },
    );
    Arc::clone(&ENVELOPE_OUTPUT_SCHEMA)
}

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

// The one legitimate `Tool::new` site: this registry IS the sole construction
// point the clippy.toml `disallowed-methods` entry directs everyone to.
#[allow(clippy::disallowed_methods)]
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

    /// Descriptor for one builtin service tool.
    ///
    /// Advertises the success-envelope `outputSchema`: every registry
    /// service's success path flows through `format_dispatch_result`, which
    /// always sets the envelope as `structuredContent` (audit on bead
    /// lab-41e7m.1). The `SERVER_LOGS_TOOL_NAME` check is invariant across
    /// callers and lives here; only `admin_apps_visible` differs, because the
    /// live-request path resolves it from request auth while the stored peer
    /// contract resolves it from a captured `PeerCatalogAudience`.
    #[must_use]
    pub(crate) fn builtin_service_tool(
        &self,
        service: &RegisteredService,
        admin_apps_visible: bool,
    ) -> Tool {
        let tool = Tool::new(service.name, service.description, builtin_action_schema())
            .with_raw_output_schema(dispatch_envelope_output_schema());
        if service.name == SERVER_LOGS_TOOL_NAME && admin_apps_visible {
            tool.with_meta(server_logs_tool_meta(service.name))
        } else {
            tool
        }
    }

    /// Descriptor for the optional Code Mode MCP App twin of `codemode`.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_ui_tool(&self, upstreams: &[CodeModeUpstreamDescription]) -> Tool {
        Tool::new(
            CODE_MODE_UI_TOOL_NAME,
            code_mode_ui_description(upstreams),
            code_mode_execute_schema(),
        )
        .with_annotations(code_mode_full_annotations())
        .with_raw_output_schema(code_mode_trace_output_schema())
        .with_meta(code_mode_tool_meta(CODE_MODE_UI_TOOL_NAME))
    }

    /// Descriptor for the MCP App control tool.
    ///
    /// Deliberately carries no `outputSchema`: its success payload is
    /// `{"kind": "mcp_app_control", …}` (call_tool.rs), not the dispatch
    /// envelope, and advertising a schema the results do not match is a hard
    /// client-side error in strict SDKs.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn mcp_app_tool(&self) -> Tool {
        Tool::new(
            MCP_APP_TOOL_NAME,
            mcp_app_tool_description(),
            mcp_app_tool_schema(),
        )
    }

    /// Descriptor for the Add Server admin app tool.
    ///
    /// Its synthetic actions (`open`/`test`/`create`) all format through
    /// `format_dispatch_result`, so the success envelope schema is accurate.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn add_server_tool(&self) -> Tool {
        Tool::new(
            ADD_SERVER_TOOL_NAME,
            "Open a responsive form to test and add a remote or local MCP server to the Labby gateway catalog.",
            add_server_tool_schema(),
        )
        .with_raw_output_schema(dispatch_envelope_output_schema())
        .with_meta(add_server_tool_meta(ADD_SERVER_TOOL_NAME))
    }

    /// Descriptor for the Gateway Status admin app tool.
    ///
    /// Its synthetic actions (`open`/`refresh`) all format through
    /// `format_dispatch_result`, so the success envelope schema is accurate.
    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn gateway_status_tool(&self) -> Tool {
        Tool::new(
            GATEWAY_STATUS_TOOL_NAME,
            "Display live connection status, capabilities, and warnings for gateway upstream MCP servers.",
            gateway_status_tool_schema(),
        )
        .with_raw_output_schema(dispatch_envelope_output_schema())
        .with_meta(gateway_status_tool_meta(GATEWAY_STATUS_TOOL_NAME))
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
            code_mode_description_with_suffix(upstreams, &code_mode_app_text_note()),
            code_mode_execute_schema(),
        )
        .with_annotations(code_mode_full_annotations())
        .with_raw_output_schema(code_mode_trace_output_schema())
    }

    #[cfg(feature = "gateway")]
    #[must_use]
    pub(crate) fn code_mode_read_descriptor(
        &self,
        upstreams: &[CodeModeUpstreamDescription],
    ) -> Tool {
        Tool::new(
            CODE_MODE_READ_TOOL_NAME,
            code_mode_description_with_suffix(
                upstreams,
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
    use super::{PermanentToolId, PermanentToolRegistry, dispatch_envelope_output_schema};
    #[cfg(feature = "gateway")]
    use crate::mcp::call_tool_codemode::CODE_MODE_DESCRIPTION_MAX_BYTES;
    use crate::mcp::catalog::{
        CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME, SERVER_LOGS_TOOL_NAME,
    };
    use crate::registry::RegisteredService;
    use serde_json::Value;

    fn noop_dispatch(
        _action: String,
        _params: Value,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Value, crate::dispatch::error::ToolError>> + Send>,
    > {
        Box::pin(async { Ok(serde_json::json!({})) })
    }

    fn service(name: &'static str) -> RegisteredService {
        RegisteredService {
            name,
            description: "Test service",
            category: "test",
            kind: crate::registry::RegisteredServiceKind::BootstrapOperator,
            status: "available",
            actions: &[],
            dispatch: noop_dispatch,
        }
    }

    /// AC-15 drift protection: the runtime envelope schema must match the
    /// published JSON Schema artifact, read as plain data (no validator
    /// dependency) — same pattern as
    /// `crates/labby-runtime/tests/agent_error_schema.rs`.
    #[test]
    fn envelope_output_schema_matches_published_schema() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/contracts/schemas/dispatch-envelope.schema.json");
        let published: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!("published schema unreadable at {}: {error}", path.display())
            }))
            .expect("published schema parses");
        let runtime = dispatch_envelope_output_schema();

        assert_eq!(runtime["type"], published["type"]);
        assert_eq!(runtime["required"], published["required"]);
        assert_eq!(
            runtime["additionalProperties"],
            published["additionalProperties"]
        );
        let runtime_props = runtime["properties"].as_object().expect("properties");
        let published_props = published["properties"].as_object().expect("properties");
        assert_eq!(
            runtime_props.keys().collect::<Vec<_>>(),
            published_props.keys().collect::<Vec<_>>(),
            "property sets must match"
        );
        assert_eq!(
            runtime_props["ok"]["const"], published_props["ok"]["const"],
            "`ok` must be const true in both"
        );
        for key in ["service", "action"] {
            assert_eq!(
                runtime_props[key]["type"], published_props[key]["type"],
                "`{key}` core type must match"
            );
        }
    }

    #[test]
    fn builtin_service_tool_advertises_envelope_schema() {
        let registry = PermanentToolRegistry::new();
        let tool = registry.builtin_service_tool(&service("gateway-alpha"), true);
        let schema = tool.output_schema.as_ref().expect("outputSchema");
        assert_eq!(schema["properties"]["ok"]["const"], serde_json::json!(true));
        assert!(tool.meta.is_none(), "only server_logs carries app meta");
    }

    #[test]
    fn server_logs_meta_is_gated_on_admin_visibility() {
        let registry = PermanentToolRegistry::new();
        let visible = registry.builtin_service_tool(&service(SERVER_LOGS_TOOL_NAME), true);
        assert!(visible.meta.is_some(), "admin-visible server_logs has meta");
        let hidden = registry.builtin_service_tool(&service(SERVER_LOGS_TOOL_NAME), false);
        assert!(hidden.meta.is_none(), "non-admin server_logs has no meta");
        // The schema is not audience-dependent.
        assert_eq!(visible.output_schema, hidden.output_schema);
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn admin_app_tools_advertise_envelope_schema_but_mcp_app_does_not() {
        let registry = PermanentToolRegistry::new();
        assert!(registry.add_server_tool().output_schema.is_some());
        assert!(registry.gateway_status_tool().output_schema.is_some());
        // mcp_app returns `{"kind": "mcp_app_control", …}`, not the dispatch
        // envelope — advertising the envelope schema would be a lie strict
        // clients enforce.
        assert!(registry.mcp_app_tool().output_schema.is_none());
        // codemode_ui carries the trace schema, not the envelope schema.
        let ui_schema = registry.code_mode_ui_tool(&[]).output_schema;
        assert!(ui_schema.is_some());
        assert_ne!(ui_schema, registry.add_server_tool().output_schema);
    }

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
    fn codemode_descriptor_is_dynamic_and_final_description_is_bounded() {
        let registry = PermanentToolRegistry::new();
        let descriptor = registry.code_mode_descriptor(&[]);
        let description = descriptor.description.expect("description");
        assert!(description.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES);
        assert!(description.contains("codemode.search"));
        assert!(description.contains("text-only entry point"));
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn codemode_read_descriptor_is_truthfully_annotated_and_bounded() {
        let descriptor = PermanentToolRegistry::new().code_mode_read_descriptor(&[]);
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
