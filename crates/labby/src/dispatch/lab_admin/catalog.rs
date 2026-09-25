use labby_primitives::action::{ActionSpec, ParamSpec};
use schemars::JsonSchema;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct AuditReportSchema { ok: bool, summary: AuditSummarySchema, services: Vec<AuditServiceSchema> }
#[allow(dead_code)]
#[derive(JsonSchema)]
struct AuditSummarySchema { requested: usize, passed: usize, failed: usize }
#[allow(dead_code)]
#[derive(JsonSchema)]
struct AuditServiceSchema { service: String, registered: bool, status: String, actions: Vec<String>, checks: AuditChecksSchema, passed: bool }
#[allow(dead_code)]
#[derive(JsonSchema)]
struct AuditChecksSchema { registered: bool, available: bool, action_catalog_nonempty: bool }

/// Action catalog for the internal `lab_admin` tool.
///
/// This is the single authoritative source. MCP, CLI, and API re-export
/// or reference it.
pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "Catalog",
        output_schema: None,
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
        returns: "Schema",
        output_schema: None,
    },
    ActionSpec {
        name: "onboarding.audit",
        description: "Audit service onboarding against the current repo contract",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "services",
            ty: "string[]",
            required: true,
            description: "Services to audit",
        }],
        returns: "AuditReport",
        output_schema: Some(labby_primitives::action::schema_for::<AuditReportSchema>),
    },
];
