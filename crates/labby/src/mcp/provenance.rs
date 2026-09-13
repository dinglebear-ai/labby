//! Result provenance metadata for the MCP response boundary.

use rmcp::model::{
    CallToolResponse, CallToolResult, CompleteResult, CreateTaskResult, GetPromptResponse,
    GetPromptResult, GetTaskResult, Implementation, InputRequiredResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, MetaObject,
    ReadResourceResponse,
};

const SERVER_INFO_META_KEY: &str = "io.modelcontextprotocol/serverInfo";
const UPSTREAM_SERVER_INFO_META_KEY: &str = "ai.dinglebear.labby/upstreamServerInfo";

fn labby_server_info() -> Implementation {
    Implementation::new("labby", env!("CARGO_PKG_VERSION"))
}

fn stamp_meta(meta: &mut Option<MetaObject>) {
    let meta = meta.get_or_insert_default();
    let labby = serde_json::to_value(labby_server_info())
        .expect("Labby implementation metadata always serializes");
    if let Some(existing) = meta.0.get(SERVER_INFO_META_KEY).cloned()
        && existing != labby
    {
        meta.0
            .entry(UPSTREAM_SERVER_INFO_META_KEY.to_string())
            .or_insert(existing);
    }
    meta.0.insert(SERVER_INFO_META_KEY.to_string(), labby);
}

pub(crate) fn stamp_call_tool_response(mut response: CallToolResponse) -> CallToolResponse {
    match &mut response {
        CallToolResponse::Complete(CallToolResult { meta, .. }) => stamp_meta(meta),
        CallToolResponse::InputRequired(InputRequiredResult { meta, .. }) => stamp_meta(meta),
        CallToolResponse::Task(CreateTaskResult { meta, .. }) => stamp_meta(meta),
        _ => {}
    }
    response
}

pub(crate) fn stamp_get_prompt_response(mut response: GetPromptResponse) -> GetPromptResponse {
    match &mut response {
        GetPromptResponse::Complete(GetPromptResult { meta, .. }) => stamp_meta(meta),
        GetPromptResponse::InputRequired(InputRequiredResult { meta, .. }) => stamp_meta(meta),
        _ => {}
    }
    response
}

pub(crate) fn stamp_read_resource_response(
    mut response: ReadResourceResponse,
) -> ReadResourceResponse {
    match &mut response {
        ReadResourceResponse::Complete(result) => {
            // Legacy upstreams omit this field. Labby owns the downstream
            // envelope; RMCP strips it again for legacy downstream peers.
            result
                .result_type
                .get_or_insert(rmcp::model::ResultType::COMPLETE);
            stamp_meta(&mut result.meta);
        }
        ReadResourceResponse::InputRequired(InputRequiredResult { meta, .. }) => stamp_meta(meta),
        _ => {}
    }
    response
}

pub(crate) fn stamp_complete_result(mut result: CompleteResult) -> CompleteResult {
    stamp_meta(&mut result.meta);
    result
}

pub(crate) fn stamp_list_prompts_result(mut result: ListPromptsResult) -> ListPromptsResult {
    stamp_meta(&mut result.meta);
    result
}

pub(crate) fn stamp_list_resources_result(mut result: ListResourcesResult) -> ListResourcesResult {
    stamp_meta(&mut result.meta);
    result
}

pub(crate) fn stamp_list_resource_templates_result(
    mut result: ListResourceTemplatesResult,
) -> ListResourceTemplatesResult {
    stamp_meta(&mut result.meta);
    result
}

pub(crate) fn stamp_list_tools_result(mut result: ListToolsResult) -> ListToolsResult {
    stamp_meta(&mut result.meta);
    result
}

pub(crate) fn stamp_get_task_result(mut result: GetTaskResult) -> GetTaskResult {
    stamp_meta(&mut result.meta);
    result
}

#[cfg(test)]
mod tests {
    use rmcp::model::{CallToolResult, ContentBlock, MetaObject, ReadResourceResult};
    use serde_json::json;

    use super::*;

    #[test]
    fn legacy_resource_response_gets_required_downstream_discriminator() {
        let upstream = json!({"contents": [{
            "uri": "lab://upstream/qa-vm-service/qa-vm-service://skill",
            "mimeType": "text/markdown",
            "text": "# QA VM skill"
        }]});
        let result: ReadResourceResult = serde_json::from_value(upstream.clone()).unwrap();
        let response = stamp_read_resource_response(result.into());
        let mut result: rmcp::model::ServerResult = response.into();
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(wire["resultType"], "complete");
        assert_eq!(wire["contents"], upstream["contents"]);
        assert_eq!(wire["_meta"][SERVER_INFO_META_KEY]["name"], "labby");

        result.strip_result_type_for_legacy_peer();
        let legacy_wire = serde_json::to_value(result).unwrap();
        assert!(legacy_wire.get("resultType").is_none());
        assert_eq!(legacy_wire["contents"], upstream["contents"]);
    }

    #[test]
    fn stamps_labby_and_preserves_upstream_identity_and_custom_metadata() {
        let mut meta = MetaObject::default();
        meta.0.insert(
            SERVER_INFO_META_KEY.to_string(),
            json!({"name": "upstream", "version": "2.0.0"}),
        );
        meta.0.insert("vendor.trace".to_string(), json!("trace-7"));
        let response = CallToolResponse::Complete(
            CallToolResult::success(vec![ContentBlock::text("ok")]).with_meta(Some(meta)),
        );

        let CallToolResponse::Complete(result) = stamp_call_tool_response(response) else {
            panic!("expected complete response");
        };
        let meta = result.meta.expect("provenance metadata");
        assert_eq!(meta.0[SERVER_INFO_META_KEY]["name"], json!("labby"));
        assert_eq!(
            meta.0[UPSTREAM_SERVER_INFO_META_KEY],
            json!({"name": "upstream", "version": "2.0.0"})
        );
        assert_eq!(meta.0["vendor.trace"], json!("trace-7"));
    }
}
