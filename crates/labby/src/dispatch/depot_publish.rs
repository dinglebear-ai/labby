//! Route-owned MCP shim for publishing one skill archive to Team Depot.

use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::Value;

const PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "filename",
        ty: "string",
        required: true,
        description: "Archive filename ending in .zip, .tar.gz, or .tgz",
    },
    ParamSpec {
        name: "archive_base64",
        ty: "string",
        required: true,
        description: "Base64 encoded skill archive bytes",
    },
    ParamSpec {
        name: "namespace",
        ty: "string",
        required: false,
        description: "Optional Team Depot namespace",
    },
];

pub const ACTIONS: &[ActionSpec] = &[ActionSpec {
    name: "depot.publish_skill_archive",
    description: "Publish a skill archive to the authenticated team's Depot",
    destructive: false,
    requires_admin: false,
    returns: "IngestJobReceipt",
    params: PARAMS,
}];

pub async fn dispatch(_action: &str, _params: Value) -> Result<Value, super::error::ToolError> {
    Err(super::error::ToolError::Sdk {
        sdk_kind: "route_scope_denied".into(),
        message: "Depot publishing is available only on a protected team route".into(),
    })
}
