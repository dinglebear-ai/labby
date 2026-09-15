//! Q1 real-process MCP resources, prompts, templates, and completion qualification.

#![cfg(all(feature = "gateway", feature = "proxy-testkit"))]
#![allow(clippy::panic)]
#![allow(dead_code, reason = "shared real-process harness has a broader API")]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_primitives_qualification.rs"]
mod primitives;

use std::time::Duration;

use primitives::{FixtureMode, PrimitiveFixture, PrimitiveQualification, REQUEST_TIMEOUT};
use rmcp::model::{
    ArgumentInfo, CallToolRequest, CallToolRequestParams, CallToolResponse, CancelTaskParams,
    ClientRequest, CompleteRequestParams, CustomRequest, CustomResult, ErrorCode,
    GetPromptRequestParams, GetTaskParams, PaginatedRequestParams, ReadResourceRequestParams,
    Reference, ResourceContents, ServerNotification, SubscriptionFilter, TaskStatus,
    UpdateTaskParams,
};
use rmcp::service::{PeerRequestOptions, ServiceError};
use serde_json::json;

fn assert_mcp_code(error: ServiceError, code: ErrorCode) {
    assert!(
        matches!(error, ServiceError::McpError(ref error) if error.code == code),
        "unexpected MCP error: {error:?}"
    );
}

fn assert_mcp_kind(error: ServiceError, kind: &str) {
    assert!(
        matches!(error, ServiceError::McpError(ref error)
            if error.data.as_ref().and_then(|data| data.get("kind")).and_then(serde_json::Value::as_str) == Some(kind)),
        "unexpected MCP error: {error:?}"
    );
}

#[tokio::test]
async fn q1_resources_prompts_templates_completions_and_collisions_round_trip() {
    let alpha = PrimitiveFixture::start("alpha", FixtureMode::Normal)
        .await
        .expect("alpha fixture");
    let beta = PrimitiveFixture::start("beta", FixtureMode::Normal)
        .await
        .expect("beta fixture");
    let runner = PrimitiveQualification::start(&[&alpha, &beta])
        .await
        .expect("real Labby Q1 runner");
    let peer = runner.service().peer();
    let server = peer.peer_info().expect("discover response");
    assert!(
        server.capabilities.resources.is_some()
            && server.capabilities.prompts.is_some()
            && server.capabilities.completions.is_some(),
        "only advertised primitive capabilities may be exercised"
    );

    let resources = tokio::time::timeout(REQUEST_TIMEOUT, peer.list_resources(None))
        .await
        .expect("resources/list deadline")
        .expect("resources/list");
    let resource_uris = resources
        .resources
        .iter()
        .map(|resource| resource.uri.as_str())
        .collect::<Vec<_>>();
    for uri in [
        "lab://upstream/alpha/fixture://text",
        "lab://upstream/alpha/fixture://blob",
        "lab://upstream/alpha/fixture://shared",
        "lab://upstream/beta/fixture://shared",
    ] {
        assert!(
            resource_uris.contains(&uri),
            "missing {uri}: {resource_uris:?}; fixture calls alpha={} beta={}; logs={}",
            alpha.resource_lists(),
            beta.resource_lists(),
            runner.diagnostic_log_tail(),
        );
    }

    let alpha_text = peer
        .read_resource(ReadResourceRequestParams::new(
            "lab://upstream/alpha/fixture://text",
        ))
        .await
        .expect("alpha text");
    assert!(matches!(
        alpha_text.contents.as_slice(),
        [ResourceContents::TextResourceContents { text, uri, mime_type, .. }]
            if text == "alpha:exact-text"
                && uri == "lab://upstream/alpha/fixture://text"
                && mime_type.as_deref() == Some("text/plain")
    ));
    let alpha_blob = peer
        .read_resource(ReadResourceRequestParams::new(
            "lab://upstream/alpha/fixture://blob",
        ))
        .await
        .expect("alpha blob");
    assert!(matches!(
        alpha_blob.contents.as_slice(),
        [ResourceContents::BlobResourceContents { blob, uri, mime_type, .. }]
            if blob == "AAEC/w=="
                && uri == "lab://upstream/alpha/fixture://blob"
                && mime_type.as_deref() == Some("application/octet-stream")
    ));
    for (owner, expected) in [
        ("alpha", "alpha:shared-owner"),
        ("beta", "beta:shared-owner"),
    ] {
        let shared = peer
            .read_resource(ReadResourceRequestParams::new(format!(
                "lab://upstream/{owner}/fixture://shared"
            )))
            .await
            .expect("owner-qualified collision read");
        assert!(matches!(
            shared.contents.as_slice(),
            [ResourceContents::TextResourceContents { text, .. }] if text == expected
        ));
    }

    let templates = peer
        .list_resource_templates(None)
        .await
        .expect("templates/list");
    let template_uris = templates
        .resource_templates
        .iter()
        .map(|template| template.uri_template.as_str())
        .collect::<Vec<_>>();
    assert!(template_uris.contains(&"lab://upstream/alpha/fixture://template/{value}"));
    assert!(template_uris.contains(&"lab://upstream/beta/fixture://template/{value}"));

    let prompts = peer.list_prompts(None).await.expect("prompts/list");
    let prompt_names = prompts
        .prompts
        .iter()
        .map(|prompt| prompt.name.as_str())
        .collect::<Vec<_>>();
    assert!(prompt_names.contains(&"alpha/shared"));
    assert!(prompt_names.contains(&"beta/shared"));
    let prompt = peer
        .get_prompt(GetPromptRequestParams::new("alpha/shared").with_arguments(
            serde_json::Map::from_iter([("subject".to_string(), json!("alice"))]),
        ))
        .await
        .expect("prompts/get");
    assert_eq!(
        prompt.messages[0]
            .content
            .as_text()
            .expect("text prompt")
            .text,
        "alpha:shared:alice"
    );

    let prompt_completion = peer
        .complete(CompleteRequestParams::new(
            Reference::for_prompt("alpha/shared"),
            ArgumentInfo::new("subject", "al"),
        ))
        .await
        .expect("advertised prompt completion");
    assert_eq!(
        prompt_completion.completion.values,
        ["alpha:shared:subject:al"]
    );
    let template_completion = peer
        .complete(CompleteRequestParams::new(
            Reference::for_resource("lab://upstream/beta/fixture://template/{value}"),
            ArgumentInfo::new("value", "be"),
        ))
        .await
        .expect("advertised resource completion");
    assert_eq!(
        template_completion.completion.values,
        ["beta:fixture://template/{value}:value:be"]
    );
    assert_eq!(alpha.prompt_gets(), 1);
    assert_eq!(alpha.completions(), 1);
    assert_eq!(beta.completions(), 1);

    let invalid_prompt = peer
        .get_prompt(GetPromptRequestParams::new("alpha/shared"))
        .await
        .expect_err("missing prompt args must fail");
    assert_mcp_kind(invalid_prompt, "upstream_error");
    let invalid_resource = peer
        .read_resource(ReadResourceRequestParams::new(
            "lab://upstream/alpha/fixture://missing",
        ))
        .await
        .expect_err("unknown URI must fail");
    assert_mcp_kind(invalid_resource, "upstream_error");

    for request in [
        peer.list_resources(Some(
            PaginatedRequestParams::default().with_cursor(Some("invalid".to_string())),
        ))
        .await
        .map(|_| ()),
        peer.list_prompts(Some(
            PaginatedRequestParams::default().with_cursor(Some("invalid".to_string())),
        ))
        .await
        .map(|_| ()),
        peer.list_resource_templates(Some(
            PaginatedRequestParams::default().with_cursor(Some("invalid".to_string())),
        ))
        .await
        .map(|_| ()),
    ] {
        assert_mcp_kind(
            request.expect_err("invalid public cursor"),
            "invalid_cursor",
        );
    }

    let unsupported = peer
        .send_request_as::<CustomResult>(ClientRequest::CustomRequest(CustomRequest::new(
            "sampling/createMessage",
            Some(json!({})),
        )))
        .await
        .expect_err("unsupported server method must be explicit");
    assert_mcp_code(unsupported, ErrorCode::METHOD_NOT_FOUND);

    alpha.set_generation(1);
    let changed_resources = peer.list_resources(None).await.expect("changed resources");
    let changed_prompts = peer.list_prompts(None).await.expect("changed prompts");
    let changed_templates = peer
        .list_resource_templates(None)
        .await
        .expect("changed templates");
    assert!(
        changed_resources
            .resources
            .iter()
            .any(|resource| { resource.uri == "lab://upstream/alpha/fixture://dynamic-v1" })
    );
    assert!(
        changed_resources
            .resources
            .iter()
            .all(|resource| { resource.uri != "lab://upstream/alpha/fixture://dynamic-v0" })
    );
    assert!(
        changed_prompts
            .prompts
            .iter()
            .any(|prompt| prompt.name == "alpha/dynamic-v1")
    );
    assert!(changed_templates.resource_templates.iter().any(|template| {
        template.uri_template == "lab://upstream/alpha/fixture://dynamic-v1/{value}"
    }));

    let cleanup = runner.finish().await;
    let alpha_finish = alpha.finish().await;
    let beta_finish = beta.finish().await;
    assert!(cleanup.is_clean(), "Labby cleanup: {:?}", cleanup.failures);
    alpha_finish.expect("alpha cleanup");
    beta_finish.expect("beta cleanup");
}

#[tokio::test]
async fn q1_advanced_lifecycle_capabilities_match_real_method_behavior() {
    let fixture = PrimitiveFixture::start("lifecycle", FixtureMode::Normal)
        .await
        .expect("lifecycle fixture");
    let runner = PrimitiveQualification::start(&[&fixture])
        .await
        .expect("real lifecycle runner");
    let peer = runner.service().peer();
    let capabilities = &peer.peer_info().expect("discover response").capabilities;
    assert_eq!(
        capabilities
            .tools
            .as_ref()
            .and_then(|value| value.list_changed),
        Some(true)
    );
    assert_eq!(
        capabilities
            .resources
            .as_ref()
            .and_then(|value| value.list_changed),
        Some(true)
    );
    assert_eq!(
        capabilities
            .resources
            .as_ref()
            .and_then(|value| value.subscribe),
        Some(true)
    );
    assert_eq!(
        capabilities
            .prompts
            .as_ref()
            .and_then(|value| value.list_changed),
        Some(true)
    );
    assert!(capabilities.logging.is_none(), "legacy logging is withheld");

    let mut subscription = peer
        .listen(
            SubscriptionFilter::builder()
                .tools_list_changed()
                .resources_list_changed()
                .prompts_list_changed()
                .build(),
        )
        .await
        .expect("modern lifecycle subscription");
    assert_eq!(subscription.acknowledged().tools_list_changed, Some(true));
    assert_eq!(
        subscription.acknowledged().resources_list_changed,
        Some(true)
    );
    assert_eq!(subscription.acknowledged().prompts_list_changed, Some(true));

    let tools_before = peer.list_tools(None).await.expect("tools before mutation");
    let fixture_tool = tools_before
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == "fixture.echo_v0")
        .expect("generation-zero lifecycle tool")
        .name
        .clone();
    peer.call_tool(CallToolRequestParams::new(fixture_tool).with_arguments(
        serde_json::Map::from_iter([
            ("subject".to_string(), json!("list-change")),
            ("correlation".to_string(), json!("q1-lifecycle")),
            ("emit_list_changed".to_string(), json!(true)),
        ]),
    ))
    .await
    .expect("fixture catalog mutation through real Labby process");

    let mut kinds = std::collections::BTreeSet::new();
    while kinds.len() < 3 {
        let notification = tokio::time::timeout(REQUEST_TIMEOUT, subscription.next())
            .await
            .expect("list-change notification deadline")
            .expect("healthy list-change subscription")
            .expect("list-change notification");
        kinds.insert(match notification {
            ServerNotification::ToolListChangedNotification(_) => "tools",
            ServerNotification::ResourceListChangedNotification(_) => "resources",
            ServerNotification::PromptListChangedNotification(_) => "prompts",
            other => panic!("unexpected lifecycle notification: {other:?}"),
        });
    }
    assert_eq!(
        kinds,
        std::collections::BTreeSet::from(["prompts", "resources", "tools"])
    );
    let tools_after = peer.list_tools(None).await.expect("tools after mutation");
    assert!(
        tools_after
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == "fixture.echo_v1"),
        "notification must correspond to a changed published tool catalog"
    );
    subscription
        .cancel()
        .await
        .expect("subscription cancellation");
    peer.call_tool(
        CallToolRequestParams::new("fixture.echo_v1").with_arguments(serde_json::Map::from_iter([
            ("subject".to_string(), json!("post-cancel-list-change")),
            ("correlation".to_string(), json!("q1-lifecycle-cancelled")),
            ("emit_list_changed".to_string(), json!(true)),
        ])),
    )
    .await
    .expect("post-subscription-cancel fixture mutation");
    assert!(
        tokio::time::timeout(REQUEST_TIMEOUT, subscription.next())
            .await
            .expect("cancelled subscription read deadline")
            .expect("cancelled subscription state")
            .is_none(),
        "a cancelled subscription must not deliver later catalog mutations"
    );

    let task_response = peer
        .call_tool_once(CallToolRequestParams::new("fixture.task"))
        .await
        .expect("task-creating proxied tool call");
    let CallToolResponse::Task(created) = task_response else {
        panic!("task-capable upstream response was not preserved: {task_response:?}")
    };
    let gateway_task_id = created.task.task_id;
    assert_ne!(gateway_task_id, "native-q1-task");
    assert!(gateway_task_id.starts_with("labby-task-"));
    assert_eq!(created.task.status, TaskStatus::Working);
    let task = peer
        .get_task(GetTaskParams::new(&gateway_task_id))
        .await
        .expect("gateway task route get");
    assert_eq!(task.task.task.task_id, gateway_task_id);
    assert_eq!(
        task.task.task.status_message.as_deref(),
        Some("native-q1-working")
    );
    peer.update_task(UpdateTaskParams::new(&gateway_task_id, Default::default()))
        .await
        .expect("gateway task route update");
    peer.cancel_task(CancelTaskParams::new(&gateway_task_id))
        .await
        .expect("gateway task route cancel");
    assert_eq!(fixture.task_updates(), 1);
    assert_eq!(fixture.task_cancellations(), 1);
    let cancelled_task = peer
        .get_task(GetTaskParams::new(&gateway_task_id))
        .await
        .expect("gateway task route remains pollable after cancellation");
    assert_eq!(cancelled_task.task.task.status, TaskStatus::Cancelled);
    assert_eq!(
        cancelled_task.task.task.status_message.as_deref(),
        Some("native-q1-cancelled")
    );

    let progress_handle = peer
        .send_request_with_option(
            ClientRequest::CallToolRequest(CallToolRequest::new(CallToolRequestParams::new(
                "fixture.progress",
            ))),
            PeerRequestOptions::no_options(),
        )
        .await
        .expect("start cancellable progress request");
    let downstream_progress_token = progress_handle.progress_token.clone();
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        while runner.client().progress().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("relayed progress deadline");
    let progress = runner.client().progress().await;
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].progress_token, downstream_progress_token);
    assert_eq!(progress[0].progress.to_bits(), 0.25_f64.to_bits());
    assert_eq!(progress[0].message.as_deref(), Some("fixture-quarter"));
    let upstream_progress = fixture.progress_tokens().await;
    assert_eq!(upstream_progress.len(), 1);
    assert_ne!(
        progress[0].progress_token, upstream_progress[0],
        "gateway must translate its upstream token back to the downstream request token"
    );
    progress_handle
        .cancel(Some("q1 literal downstream cancellation".to_string()))
        .await
        .expect("cancel progress request");
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        while fixture.progress_cancellations() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("upstream observes downstream cancellation");
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        runner.client().progress().await.len(),
        1,
        "progress emitted after cancellation must not reach the downstream client"
    );

    let missing_task = peer
        .get_task(GetTaskParams::new("missing-qualification-task"))
        .await
        .expect_err("implemented tasks/get must return a typed missing-task error");
    assert!(
        matches!(missing_task, ServiceError::McpError(ref error) if error.code == ErrorCode::INVALID_PARAMS),
        "unexpected tasks/get error: {missing_task:?}"
    );
    let missing_update = peer
        .update_task(UpdateTaskParams::new(
            "missing-qualification-task",
            Default::default(),
        ))
        .await
        .expect_err("implemented tasks/update must return a typed missing-task error");
    assert_mcp_code(missing_update, ErrorCode::INVALID_PARAMS);
    let missing_cancel = peer
        .cancel_task(CancelTaskParams::new("missing-qualification-task"))
        .await
        .expect_err("implemented tasks/cancel must return a typed missing-task error");
    assert_mcp_code(missing_cancel, ErrorCode::INVALID_PARAMS);

    for method in [
        "roots/list",
        "sampling/createMessage",
        "logging/setLevel",
        "tasks/list",
        "resources/subscribe",
        "resources/unsubscribe",
    ] {
        let error = peer
            .send_request_as::<CustomResult>(ClientRequest::CustomRequest(CustomRequest::new(
                method,
                Some(json!({})),
            )))
            .await
            .unwrap_err();
        assert_mcp_code(error, ErrorCode::METHOD_NOT_FOUND);
    }

    let cleanup = runner.finish().await;
    let fixture_cleanup = fixture.finish().await;
    assert!(cleanup.is_clean(), "Labby cleanup: {:?}", cleanup.failures);
    fixture_cleanup.expect("fixture cleanup");
}

#[tokio::test]
async fn q1_hostile_catalogs_stop_at_cursor_page_item_and_byte_bounds() {
    let cursor = PrimitiveFixture::start("cursor-bomb", FixtureMode::CursorBomb)
        .await
        .expect("cursor fixture");
    let page = PrimitiveFixture::start("page-bomb", FixtureMode::PageBomb)
        .await
        .expect("page fixture");
    let item = PrimitiveFixture::start("item-bomb", FixtureMode::ItemBomb)
        .await
        .expect("item fixture");
    let bytes = PrimitiveFixture::start("byte-bomb", FixtureMode::ByteBomb)
        .await
        .expect("byte fixture");
    let runner = PrimitiveQualification::start(&[&cursor, &page, &item, &bytes])
        .await
        .expect("hostile real Labby runner");
    let peer = runner.service().peer();

    let resources = tokio::time::timeout(REQUEST_TIMEOUT, peer.list_resources(None))
        .await
        .expect("bounded resources deadline")
        .expect("partial resources result");
    assert!(
        resources
            .resources
            .iter()
            .all(|resource| !resource.uri.starts_with("lab://upstream/")),
        "hostile resource leaked through bounded catalog: {:?}",
        resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>()
    );
    let prompts = tokio::time::timeout(REQUEST_TIMEOUT, peer.list_prompts(None))
        .await
        .expect("bounded prompts deadline")
        .expect("partial prompts result");
    assert!(
        prompts
            .prompts
            .iter()
            .all(|prompt| !prompt.name.contains("bomb"))
    );
    let templates = tokio::time::timeout(REQUEST_TIMEOUT, peer.list_resource_templates(None))
        .await
        .expect("bounded template deadline")
        .expect("partial template result");
    assert!(
        templates
            .resource_templates
            .iter()
            .all(|template| !template.name.contains("bomb"))
    );

    // Startup capability probing and the forced full reload exercise three
    // independent resource-list passes. Each cursor/byte breach must stop on
    // its first response. Prompt probing is one single-page capability check
    // plus the exact 64-page bounded reload pass.
    assert_eq!(
        cursor.resource_lists(),
        3,
        "each oversized-cursor pass stops immediately"
    );
    assert_eq!(
        page.prompt_lists(),
        65,
        "single-page probe plus pagination at the exact 64-page cap"
    );
    assert_eq!(item.template_lists(), 1, "oversized page stops immediately");
    assert_eq!(
        bytes.resource_lists(),
        3,
        "each oversized-byte pass stops immediately"
    );

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "Labby cleanup: {:?}", cleanup.failures);
    cursor.finish().await.expect("cursor cleanup");
    page.finish().await.expect("page cleanup");
    item.finish().await.expect("item cleanup");
    bytes.finish().await.expect("byte cleanup");
}

#[tokio::test]
async fn q1_oversized_prompt_body_is_rejected_with_a_typed_bound_error() {
    let fixture = PrimitiveFixture::start("alpha", FixtureMode::Normal)
        .await
        .expect("prompt fixture");
    let runner = PrimitiveQualification::start(&[&fixture])
        .await
        .expect("real Labby Q1 runner");
    let peer = runner.service().peer();
    let result = peer
        .get_prompt(
            GetPromptRequestParams::new("alpha/oversized").with_arguments(
                serde_json::Map::from_iter([("subject".to_string(), json!("alice"))]),
            ),
        )
        .await
        .expect_err("oversized prompt must be rejected");
    assert_mcp_kind(result, "response_too_large");

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "Labby cleanup: {:?}", cleanup.failures);
    fixture.finish().await.expect("fixture cleanup");
}

#[test]
fn q1_deadlines_are_explicit_and_bounded() {
    assert!(REQUEST_TIMEOUT <= Duration::from_secs(15));
}
