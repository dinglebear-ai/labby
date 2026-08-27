use crate::upstream::types::UpstreamRuntimeOwner;

pub const SHARED_GATEWAY_OAUTH_SUBJECT: &str = "gateway";

/// Build an [`UpstreamRuntimeOwner`] for an API surface request.
///
/// Encapsulates the `owner`/`origin`/`raw` construction that was previously
/// inlined in `api/services/gateway.rs` (Q-L5 fix). The MCP surface calls the
/// same constructor via `UpstreamRuntimeOwner::for_mcp_request`.
///
/// JSON shape produced (matches the inline shape that was here before):
/// ```json
/// {
///   "surface": "api",
///   "subject": <sub | null>,
///   "request_id": <id | null>,
///   "raw": "api:<sub>:<request_id>"
/// }
/// ```
pub fn make_api_runtime_owner(
    subject: Option<&str>,
    request_id: Option<&str>,
) -> UpstreamRuntimeOwner {
    let raw = Some(match (subject, request_id) {
        (Some(sub), Some(rid)) => format!("api:{sub}:{rid}"),
        (Some(sub), None) => format!("api:{sub}"),
        (None, Some(rid)) => format!("api:anonymous:{rid}"),
        (None, None) => "api:anonymous".to_string(),
    });
    UpstreamRuntimeOwner {
        surface: "api".to_string(),
        subject: subject.map(ToOwned::to_owned),
        request_id: request_id.map(ToOwned::to_owned),
        session_id: None,
        client_name: None,
        raw,
    }
}

/// Build an [`UpstreamRuntimeOwner`] for an MCP surface request.
///
/// Mirrors [`make_api_runtime_owner`] for the MCP transport.  Called from
/// `mcp/context.rs` so both surfaces share identical construction semantics
/// (Q-L5 fix).
pub fn make_mcp_runtime_owner(subject: Option<&str>) -> UpstreamRuntimeOwner {
    let raw = Some(
        subject
            .map(|s| format!("mcp:{s}"))
            .unwrap_or_else(|| "mcp:anonymous".to_string()),
    );
    UpstreamRuntimeOwner {
        surface: "mcp".to_string(),
        subject: subject.map(ToOwned::to_owned),
        request_id: None,
        session_id: None,
        client_name: None,
        raw,
    }
}

/// Whether transport-owned runtime provenance belongs in this action's params.
///
/// Runtime owner/origin are internal transport metadata, not universal gateway
/// parameters. Only actions that actually consume them should receive them.
/// Keeping this list shared prevents strict read-only parameter schemas (such as
/// `gateway.skills.list`) from rejecting otherwise valid API or MCP requests.
pub fn action_accepts_runtime_owner(action: &str) -> bool {
    matches!(
        action,
        "gateway.add"
            | "gateway.update"
            | "gateway.remove"
            | "gateway.reload"
            | "gateway.import_tombstones.restore"
            | "gateway.mcp.enable"
            | "gateway.mcp.disable"
    )
}

#[cfg(test)]
mod tests {
    use super::action_accepts_runtime_owner;

    #[test]
    fn runtime_owner_metadata_is_limited_to_actions_that_consume_it() {
        for action in [
            "gateway.add",
            "gateway.update",
            "gateway.remove",
            "gateway.reload",
            "gateway.import_tombstones.restore",
            "gateway.mcp.enable",
            "gateway.mcp.disable",
        ] {
            assert!(action_accepts_runtime_owner(action), "{action}");
        }

        for action in [
            "gateway.skills.list",
            "gateway.list",
            "gateway.status",
            "gateway.code_mode.get",
            "gateway.oauth.resource_lease.create",
        ] {
            assert!(!action_accepts_runtime_owner(action), "{action}");
        }
    }
}
