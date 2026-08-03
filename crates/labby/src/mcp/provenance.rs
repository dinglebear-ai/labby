//! Result provenance metadata for the MCP response boundary.

use rmcp::model::{
    CallToolResponse, CallToolResult, CompleteResult, CreateTaskResult, GetPromptResponse,
    GetPromptResult, GetTaskResult, Implementation, InputRequiredResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, MetaObject,
    ReadResourceResponse, ReadResourceResult,
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
        ReadResourceResponse::Complete(ReadResourceResult { meta, .. }) => stamp_meta(meta),
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
    use rmcp::model::{CallToolResult, ContentBlock, MetaObject};
    use serde_json::json;

    use super::*;

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
