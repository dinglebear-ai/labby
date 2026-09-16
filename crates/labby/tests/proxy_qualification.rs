//! Q5 real-process client -> Labby -> Labby -> fixture proxy qualification.

#![cfg(all(feature = "gateway", feature = "proxy-testkit", feature = "skills"))]
#![allow(clippy::panic)]
#![allow(dead_code, reason = "shared real-process harness has a broader API")]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_primitives_qualification.rs"]
mod primitives;

use std::time::Duration;

use labby_gateway::upstream::http_client::BodyCappedHttpClient;
use labby_runtime::skills::wire::{SkillsGetResult, SkillsListResult};
use primitives::{FixtureMode, PrimitiveClient, PrimitiveFixture};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, ClientRequest, CustomRequest, GetPromptRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::{ClientLifecycleMode, ClientServiceExt, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransportConfig, StreamableHttpClientWorker,
};
use serde_json::json;

const TOKEN: &str = "q5-proxy-qualification-token";
const DEADLINE: Duration = Duration::from_secs(20);

async fn connect(endpoint: String) -> RunningService<RoleClient, PrimitiveClient> {
    let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint);
    config.auth_header = Some(TOKEN.to_owned());
    let worker = StreamableHttpClientWorker::new(
        BodyCappedHttpClient::new(reqwest::Client::new(), 12 * 1024 * 1024),
        config,
    );
    tokio::time::timeout(
        DEADLINE,
        PrimitiveClient::default().serve_with_lifecycle(
            worker,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        ),
    )
    .await
    .expect("Q5 discovery deadline")
    .expect("Q5 MCP connection")
}

fn invalidate_gateway_config(guard: &live_labby::LiveLabbyGuard) {
    let path = guard.root().join("labby-home/config.toml");
    let persisted = std::fs::read_to_string(&path).expect("persisted gateway config");
    let changed = if persisted.contains("upstream_request_timeout_ms = 15000") {
        persisted.replacen(
            "upstream_request_timeout_ms = 15000",
            "upstream_request_timeout_ms = 15001",
            1,
        )
    } else {
        persisted.replacen(
            "upstream_request_timeout_ms = 15001",
            "upstream_request_timeout_ms = 15002",
            1,
        )
    };
    assert_ne!(changed, persisted, "fixture config must be invalidatable");
    std::fs::write(path, changed).expect("invalidate gateway config");
}

#[tokio::test]
async fn q5_multihop_tools_prompts_resources_templates_and_skills_round_trip_once() {
    let fixture = PrimitiveFixture::start("origin", FixtureMode::Normal)
        .await
        .expect("origin fixture");
    let leaf_config = format!(
        "upstream_request_timeout_ms = 15000\n{}[[upstream]]\nname = \"origin\"\nenabled = true\nurl = {}\nproxy_resources = true\nproxy_prompts = true\n",
        primitives::RAW_GATEWAY_TOOL_CONFIG,
        serde_json::to_string(&fixture.url()).expect("fixture URL")
    );
    let leaf = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .config(leaf_config)
        .start()
        .await
        .expect("leaf Labby");
    invalidate_gateway_config(&leaf);
    let leaf_client = connect(format!("{}/mcp", leaf.connection().base_url)).await;
    let reload = leaf_client
        .call_tool(
            CallToolRequestParams::new("gateway").with_arguments(
                json!({"action":"gateway.reload","params":{}})
                    .as_object()
                    .expect("reload object")
                    .clone(),
            ),
        )
        .await
        .expect("leaf gateway reload");
    assert_ne!(
        reload.is_error,
        Some(true),
        "leaf reload failed: {reload:?}"
    );
    let leaf_tools = leaf_client.list_tools(None).await.expect("leaf tools/list");
    assert!(
        leaf_tools
            .tools
            .iter()
            .any(|tool| tool.name == "fixture.echo_v0"),
        "leaf did not discover fixture: {:?}",
        leaf_tools
            .tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>()
    );
    leaf_client.cancel().await.expect("leaf bootstrap cleanup");
    let edge_config = format!(
        "upstream_request_timeout_ms = 15000\n{}[[upstream]]\nname = \"leaf\"\nenabled = true\nurl = {}\nbearer_token_env = \"Q5_LEAF_TOKEN\"\nproxy_resources = true\nproxy_prompts = true\nproxy_skills = true\n",
        primitives::RAW_GATEWAY_TOOL_CONFIG,
        serde_json::to_string(&format!("{}/mcp", leaf.connection().base_url)).expect("leaf URL")
    );
    let edge = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .env("Q5_LEAF_TOKEN", TOKEN)
        .config(edge_config)
        .start()
        .await
        .expect("edge Labby");
    invalidate_gateway_config(&edge);
    let client = connect(format!("{}/mcp", edge.connection().base_url)).await;
    let reload = client
        .call_tool(
            CallToolRequestParams::new("gateway").with_arguments(
                json!({"action":"gateway.reload","params":{}})
                    .as_object()
                    .expect("reload object")
                    .clone(),
            ),
        )
        .await
        .expect("edge gateway reload");
    assert_ne!(
        reload.is_error,
        Some(true),
        "edge reload failed: {reload:?}"
    );

    let tools = client.list_tools(None).await.expect("multihop tools/list");
    let tool = tools
        .tools
        .iter()
        .find(|tool| tool.name == "fixture.echo_v0")
        .unwrap_or_else(|| panic!("nested fixture tool missing: {tools:?}"));
    assert_eq!(
        tool.name, "fixture.echo_v0",
        "tool identity must remain stable"
    );
    let app_uri = tool
        .meta
        .as_ref()
        .and_then(|meta| meta.0["ui"]["resourceUri"].as_str())
        .expect("proxied MCP App binding");
    assert_eq!(app_uri, "ui://fixture/app.html");
    let call = client
        .call_tool(
            CallToolRequestParams::new(tool.name.clone()).with_arguments(
                json!({"subject":"alice","correlation":"q5-correlation-001"})
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
        )
        .await
        .expect("multihop tool call");
    assert_ne!(call.is_error, Some(true), "multihop call failed: {call:?}");
    let text = call.content[0]
        .as_text()
        .expect("text tool result")
        .text
        .as_str();
    assert!(text.contains("q5-correlation-001") && text.contains("alice"));
    assert_eq!(fixture.tool_calls(), 1, "tool must execute exactly once");

    let prompts = client
        .list_prompts(None)
        .await
        .expect("multihop prompts/list");
    let prompt = prompts
        .prompts
        .iter()
        .find(|prompt| prompt.name.ends_with("origin/shared"))
        .unwrap_or_else(|| panic!("nested prompt missing: {prompts:?}"));
    assert!(prompt.name.starts_with("leaf/"));
    let result = client
        .get_prompt(
            GetPromptRequestParams::new(prompt.name.clone()).with_arguments(
                serde_json::Map::from_iter([("subject".to_string(), json!("alice"))]),
            ),
        )
        .await
        .expect("multihop prompts/get");
    assert_eq!(
        result.messages[0]
            .content
            .as_text()
            .expect("text prompt")
            .text,
        "origin:shared:alice"
    );

    let resources = client
        .list_resources(None)
        .await
        .expect("multihop resources/list");
    let resource = resources
        .resources
        .iter()
        .find(|resource| resource.uri.ends_with("fixture://text"))
        .unwrap_or_else(|| panic!("nested resource missing: {resources:?}"));
    assert!(resource.uri.starts_with("lab://upstream/leaf/"));
    let read = client
        .read_resource(ReadResourceRequestParams::new(resource.uri.clone()))
        .await
        .expect("multihop resources/read");
    assert!(matches!(
        read.contents.as_slice(),
        [ResourceContents::TextResourceContents { text, .. }] if text == "origin:exact-text"
    ));
    let app = client
        .read_resource(ReadResourceRequestParams::new(app_uri))
        .await
        .expect("multihop MCP App resource read");
    assert!(matches!(
        app.contents.as_slice(),
        [ResourceContents::TextResourceContents { text, mime_type, .. }]
            if text.contains("origin fixture app")
                && mime_type.as_deref() == Some("text/html;profile=mcp-app")
    ));
    let templates = client
        .list_resource_templates(None)
        .await
        .expect("multihop templates/list");
    assert!(
        templates
            .resource_templates
            .iter()
            .any(|template| template.uri_template.contains("fixture://template/{value}")),
        "nested template missing: {templates:?}"
    );

    let skills: SkillsListResult = client
        .send_request_as(ClientRequest::CustomRequest(CustomRequest::new(
            "skills/list",
            Some(json!({})),
        )))
        .await
        .expect("multihop skills/list");
    let skill = skills
        .skills
        .iter()
        .find(|skill| skill.uri.starts_with("skill://leaf/skill/labby/"))
        .unwrap_or_else(|| panic!("proxied leaf skill missing: {skills:?}"));
    let fetched: SkillsGetResult = client
        .send_request_as(ClientRequest::CustomRequest(CustomRequest::new(
            "skills/get",
            Some(json!({"uri": skill.uri})),
        )))
        .await
        .expect("multihop skills/get");
    assert_eq!(fetched.skill.uri, skill.uri);

    fixture.set_generation(1);
    invalidate_gateway_config(&leaf);
    let refreshed_leaf = connect(format!("{}/mcp", leaf.connection().base_url)).await;
    let reload = refreshed_leaf
        .call_tool(
            CallToolRequestParams::new("gateway").with_arguments(
                json!({"action":"gateway.reload","params":{}})
                    .as_object()
                    .expect("reload object")
                    .clone(),
            ),
        )
        .await
        .expect("leaf catalog refresh");
    assert_ne!(reload.is_error, Some(true));
    refreshed_leaf
        .cancel()
        .await
        .expect("refreshed leaf cleanup");
    invalidate_gateway_config(&edge);
    let reload = client
        .call_tool(
            CallToolRequestParams::new("gateway").with_arguments(
                json!({"action":"gateway.reload","params":{}})
                    .as_object()
                    .expect("reload object")
                    .clone(),
            ),
        )
        .await
        .expect("edge catalog refresh");
    assert_ne!(reload.is_error, Some(true));
    let refreshed_tools = client.list_tools(None).await.expect("refreshed tools");
    assert!(
        refreshed_tools
            .tools
            .iter()
            .any(|tool| tool.name == "fixture.echo_v1")
    );
    assert!(
        refreshed_tools
            .tools
            .iter()
            .all(|tool| tool.name != "fixture.echo_v0")
    );
    let refreshed = client.list_prompts(None).await.expect("refreshed prompts");
    assert!(
        refreshed
            .prompts
            .iter()
            .any(|prompt| { prompt.name.ends_with("origin/dynamic-v1") })
    );
    assert!(
        refreshed
            .prompts
            .iter()
            .all(|prompt| { !prompt.name.ends_with("origin/dynamic-v0") })
    );
    assert_eq!(
        fixture.tool_calls(),
        1,
        "catalog refresh must not replay calls"
    );

    client.cancel().await.expect("client cleanup");
    let edge_cleanup = edge.finish().await;
    let leaf_cleanup = leaf.finish().await;
    fixture.finish().await.expect("fixture cleanup");
    assert!(edge_cleanup.is_clean(), "edge cleanup: {edge_cleanup:?}");
    assert!(leaf_cleanup.is_clean(), "leaf cleanup: {leaf_cleanup:?}");
}

#[tokio::test]
async fn q5_upstream_auth_failure_is_partial_fail_closed_and_redacted() {
    const CANARY: &str = "q5-secret-canary-must-not-appear";
    let leaf = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .start()
        .await
        .expect("auth leaf");
    let config = format!(
        "upstream_request_timeout_ms = 15000\n{}[[upstream]]\nname = \"denied-leaf\"\nenabled = true\nurl = {}\nbearer_token_env = \"Q5_WRONG_TOKEN\"\nproxy_resources = true\nproxy_prompts = true\nproxy_skills = true\n",
        primitives::RAW_GATEWAY_TOOL_CONFIG,
        serde_json::to_string(&format!("{}/mcp", leaf.connection().base_url)).expect("leaf URL")
    );
    let edge = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", TOKEN)
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .env("Q5_WRONG_TOKEN", CANARY)
        .config(config)
        .start()
        .await
        .expect("auth edge");
    invalidate_gateway_config(&edge);
    let client = connect(format!("{}/mcp", edge.connection().base_url)).await;
    let reload = client
        .call_tool(
            CallToolRequestParams::new("gateway").with_arguments(
                json!({"action":"gateway.reload","params":{}})
                    .as_object()
                    .expect("reload object")
                    .clone(),
            ),
        )
        .await
        .expect("failed-upstream reload remains a protocol result");
    assert_ne!(
        reload.is_error,
        Some(true),
        "partial reload failed: {reload:?}"
    );
    let tools = client
        .list_tools(None)
        .await
        .expect("local partial catalog");
    assert!(tools.tools.iter().any(|tool| tool.name == "doctor"));
    let skills: SkillsListResult = client
        .send_request_as(ClientRequest::CustomRequest(CustomRequest::new(
            "skills/list",
            Some(json!({})),
        )))
        .await
        .expect("local skills survive upstream auth failure");
    assert!(
        skills
            .skills
            .iter()
            .all(|skill| !skill.uri.starts_with("skill://denied-leaf/"))
    );
    let logs = ["stdout.log", "stderr.log"]
        .into_iter()
        .filter_map(|name| std::fs::read_to_string(edge.root().join(name)).ok())
        .collect::<String>();
    assert!(
        !logs.contains(CANARY),
        "upstream credential leaked into logs"
    );

    client.cancel().await.expect("auth client cleanup");
    let edge_cleanup = edge.finish().await;
    let leaf_cleanup = leaf.finish().await;
    assert!(
        edge_cleanup.is_clean(),
        "auth edge cleanup: {edge_cleanup:?}"
    );
    assert!(
        leaf_cleanup.is_clean(),
        "auth leaf cleanup: {leaf_cleanup:?}"
    );
}
