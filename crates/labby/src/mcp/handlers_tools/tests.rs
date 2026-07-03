//! Tests for tool-list/catalog visibility + upstream-pool resolution.
//! Distributed from `server.rs` (bead `lab-kvji.24.1.6`). Duplicates the
//! small `completion_test_registry` fixture to keep this `tests.rs`
//! self-contained (per the test-distribution plan's minimal-duplication
//! guidance).

use crate::dispatch::error::ToolError;
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::{
    ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};
use crate::mcp::catalog::CODE_MODE_TOOL_NAME;
use crate::mcp::handlers_resources::{CODE_MODE_APP_SKYBRIDGE_URI, CODE_MODE_APP_URI};
use crate::mcp::handlers_tools::{code_mode_tool_meta, code_mode_trace_output_schema};
use crate::mcp::logging::logging_level_rank;
use crate::mcp::server::LabMcpServer;
use crate::registry::{RegisteredService, ToolRegistry};
use labby_apis::core::action::ActionSpec;
use rmcp::model::{CallToolRequestParams, Meta, Tool};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, atomic::AtomicU8};

const TEST_ACTIONS_ONE: &[ActionSpec] = &[
    ActionSpec {
        name: "queue.list",
        description: "List queue",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.search",
        description: "Search movies",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

const TEST_ACTIONS_TWO: &[ActionSpec] = &[
    ActionSpec {
        name: "calendar.list",
        description: "List calendar",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
    ActionSpec {
        name: "movie.lookup",
        description: "Look up movie",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "object",
    },
];

fn noop_dispatch(
    _action: String,
    _params: Value,
) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
    Box::pin(async { Ok(Value::Null) })
}

fn completion_test_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(RegisteredService {
        name: "radarr",
        description: "Movies",
        category: "media",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_ONE,
        dispatch: noop_dispatch,
    });
    registry.register(RegisteredService {
        name: "sonarr",
        description: "Shows",
        category: "media",
        kind: crate::registry::RegisteredServiceKind::BuiltInUpstreamApi,
        status: "available",
        actions: TEST_ACTIONS_TWO,
        dispatch: noop_dispatch,
    });
    registry
}

fn test_server(
    registry: ToolRegistry,
    gateway_manager: Option<Arc<crate::dispatch::gateway::manager::GatewayManager>>,
    route_scope: crate::mcp::route_scope::McpRouteScope,
    logging_level: rmcp::model::LoggingLevel,
) -> LabMcpServer {
    LabMcpServer {
        registry: Arc::new(registry),
        gateway_manager,
        node_role: None,
        peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(logging_level))),
        route_scope,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    }
}

async fn code_mode_manager(
    enabled: bool,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled,
                    ..crate::config::CodeModeConfig::default()
                },
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

async fn code_mode_manager_with_pool(
    enabled: bool,
    upstream: crate::config::UpstreamConfig,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    code_mode_manager_with_pool_and_upstreams(enabled, vec![upstream], pool).await
}

async fn code_mode_manager_with_pool_multi(
    enabled: bool,
    upstreams: Vec<crate::config::UpstreamConfig>,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    code_mode_manager_with_pool_and_upstreams(enabled, upstreams, pool).await
}

async fn code_mode_manager_with_pool_and_upstreams(
    enabled: bool,
    upstreams: Vec<crate::config::UpstreamConfig>,
    pool: Arc<UpstreamPool>,
) -> Arc<crate::dispatch::gateway::manager::GatewayManager> {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    runtime.swap(Some(pool)).await;
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled,
                    ..crate::config::CodeModeConfig::default()
                },
                upstream: upstreams,
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    manager
}

fn fixture_upstream_config(name: &str) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: Some("http://127.0.0.1:9/mcp".to_string()),
        bearer_token_env: None,
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        proxy_resources: true,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

fn fixture_oauth_upstream_config(name: &str) -> crate::config::UpstreamConfig {
    let mut config = fixture_upstream_config(name);
    config.oauth = Some(crate::config::UpstreamOauthConfig {
        mode: crate::config::UpstreamOauthMode::AuthorizationCodePkce,
        registration: crate::config::UpstreamOauthRegistration::Preregistered {
            client_id: "client-id".to_string(),
            client_secret_env: None,
        },
        scopes: None,
        prefer_client_metadata_document: None,
    });
    config
}

fn fixture_upstream_entry(upstream: &str, tools: HashMap<String, UpstreamTool>) -> UpstreamEntry {
    UpstreamEntry {
        name: Arc::from(upstream),
        tools,
        exposure_policy: ToolExposurePolicy::All,
        proxy_resources: true,
        prompt_count: 0,
        resource_count: 1,
        prompt_names: Vec::new(),
        resource_uris: vec![format!("ui://{upstream}/app.html")],
        tool_health: UpstreamHealth::Healthy,
        prompt_health: UpstreamHealth::Healthy,
        resource_health: UpstreamHealth::Healthy,
        tool_unhealthy_since: None,
        prompt_unhealthy_since: None,
        resource_unhealthy_since: None,
        tool_last_error: None,
        prompt_last_error: None,
        resource_last_error: None,
    }
}

fn fixture_upstream_tool(
    upstream: &Arc<str>,
    name: &str,
    ui_resource: Option<&str>,
) -> UpstreamTool {
    let mut tool = Tool::new(
        name.to_string(),
        format!("{name} description"),
        Arc::new(serde_json::Map::new()),
    );
    if let Some(resource_uri) = ui_resource {
        tool.meta = Some(Meta(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": resource_uri }),
        )])));
    }
    UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::clone(upstream),
        destructive: false,
    }
}

fn fixture_destructive_upstream_tool(upstream: &Arc<str>, name: &str) -> UpstreamTool {
    let mut tool = fixture_upstream_tool(upstream, name, None);
    tool.destructive = true;
    tool
}

fn scoped_context(
    peer: rmcp::service::Peer<rmcp::RoleServer>,
    scopes: &[&str],
) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    let mut context =
        rmcp::service::RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
    let mut parts = axum::http::Request::new(()).into_parts().0;
    parts.extensions.insert(crate::api::oauth::AuthContext {
        sub: "reader".to_string(),
        actor_key: None,
        scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    });
    context.extensions.insert(parts);
    context
}

fn request_context_with_peer(
    peer: rmcp::service::Peer<rmcp::RoleServer>,
) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    rmcp::service::RequestContext::new(rmcp::model::NumberOrString::Number(1), peer)
}

async fn call_tool_error_text(server: LabMcpServer, tool_name: &str) -> String {
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new(tool_name.to_string()), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    result.content[0].as_text().expect("text").text.clone()
}

#[test]
fn code_mode_tool_meta_points_to_canonical_ui_resource() {
    let codemode = code_mode_tool_meta(CODE_MODE_TOOL_NAME);

    // The binding URI carries a `?v=<hash>` cache-bust token (so a rebuilt widget
    // forces the host to refetch), but resolves to the canonical base URI.
    let codemode_ui = codemode.0["ui"]["resourceUri"]
        .as_str()
        .expect("codemode resourceUri");
    assert!(codemode_ui.starts_with(CODE_MODE_APP_URI));
    assert!(codemode_ui.contains("?v="));
    // OpenAI Apps hosts (ChatGPT / Codex) bind widgets via `openai/outputTemplate`
    // rather than `_meta.ui`. It points at the skybridge variant (same HTML, the
    // `text/html+skybridge` MIME those hosts expect) so the Claude resource is
    // untouched.
    let codemode_skybridge = codemode
        .0
        .get("openai/outputTemplate")
        .and_then(|value| value.as_str())
        .expect("codemode openai/outputTemplate");
    assert!(
        codemode_skybridge.starts_with(CODE_MODE_APP_SKYBRIDGE_URI),
        "codemode tool must expose the OpenAI Apps output template"
    );
    assert!(codemode_skybridge.contains("?v="));
}

#[test]
fn code_mode_trace_output_schema_advertises_structured_trace_kinds() {
    let schema = code_mode_trace_output_schema();
    assert_eq!(schema["type"].as_str(), Some("object"));

    let variants = schema["oneOf"].as_array().expect("oneOf variants");
    let kinds = variants
        .iter()
        .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["code_mode_execute_trace"]);
}

#[tokio::test]
async fn list_tools_advertises_code_mode_output_schemas() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let codemode = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME)
        .expect("codemode tool");
    assert_eq!(
        codemode.input_schema["properties"]["code"]["minLength"],
        serde_json::json!(1),
        "codemode must advertise non-empty code"
    );
    let schema = codemode.output_schema.as_ref().expect("outputSchema");
    let kinds = schema["oneOf"]
        .as_array()
        .expect("oneOf variants")
        .iter()
        .filter_map(|variant| variant["properties"]["kind"]["const"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["code_mode_execute_trace"]);
}

#[tokio::test]
async fn list_tools_promotes_upstream_mcp_app_tools_when_raw_tools_are_hidden() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(names.contains(&"youtube_search_ui"));
    assert!(!names.contains(&"youtube_probe"));
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
    assert!(!names.contains(&"radarr"));
}

#[tokio::test]
async fn list_tools_does_not_cold_connect_code_mode_catalog() {
    let pool = Arc::new(UpstreamPool::new());
    let manager = code_mode_manager_with_pool(
        true,
        fixture_upstream_config("cold-apps"),
        Arc::clone(&pool),
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    assert!(
        result
            .tools
            .iter()
            .any(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME),
        "root list_tools must keep advertising Code Mode"
    );

    let summary = pool.cached_upstream_summary("cold-apps").await;
    assert!(
        summary.is_none(),
        "root list_tools must not cold-connect or populate a lazy upstream catalog"
    );
    assert!(
        pool.upstream_tool_last_error("cold-apps").await.is_none(),
        "skipping cold discovery should not mark the upstream failed"
    );
}

#[tokio::test]
async fn list_tools_does_not_promote_upstream_mcp_app_tools_when_resources_are_not_proxied() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "github_pr_ui",
        Some("ui://apps/github-pr.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    let mut entry = fixture_upstream_entry(
        "apps",
        HashMap::from([("github_pr_ui".to_string(), ui_tool)]),
    );
    entry.proxy_resources = false;
    pool.insert_entry_for_test("apps", entry).await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.proxy_resources = false;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(!names.contains(&"github_pr_ui"));
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
}

#[tokio::test]
async fn list_tools_skips_upstream_ui_tools_that_collide_with_synthetic_names() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let colliding_tool = fixture_upstream_tool(
        &upstream_name,
        CODE_MODE_TOOL_NAME,
        Some("ui://apps/codemode.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([(CODE_MODE_TOOL_NAME.to_string(), colliding_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let codemode_count = result
        .tools
        .iter()
        .filter(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME)
        .count();

    assert_eq!(
        codemode_count, 1,
        "upstream UI tool must not duplicate the synthetic codemode tool"
    );
}

#[tokio::test]
async fn protected_code_mode_list_tools_hides_raw_siblings_and_disallowed_builtins() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media",
            ["apps"],
            ["radarr"],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(!names.contains(&"radarr"));
    assert!(!names.contains(&"sonarr"));
    assert!(names.contains(&CODE_MODE_TOOL_NAME));
    assert!(names.contains(&"youtube_search_ui"));
    assert!(!names.contains(&"youtube_probe"));
}

#[tokio::test]
async fn codemode_description_lists_route_scoped_enabled_upstreams() {
    let apps = fixture_upstream_config("apps");
    let mut hidden = fixture_upstream_config("hidden");
    hidden.enabled = false;
    let sonarr = fixture_upstream_config("sonarr");
    let pool = Arc::new(UpstreamPool::new());
    let manager = code_mode_manager_with_pool_multi(true, vec![apps, hidden, sonarr], pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media",
            ["apps"],
            ["radarr"],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let codemode = result
        .tools
        .iter()
        .find(|tool| tool.name.as_ref() == CODE_MODE_TOOL_NAME)
        .expect("codemode tool");
    let description = codemode
        .description
        .as_ref()
        .expect("codemode description")
        .as_ref();

    assert!(description.contains("## Available upstream namespaces"));
    assert!(description.contains("- `apps`"));
    assert!(!description.contains("- `hidden`"));
    assert!(!description.contains("- `sonarr`"));
    assert!(description.contains("Never guess helper or method names"));
}

#[tokio::test]
async fn protected_list_tools_filters_disallowed_builtins_when_code_mode_is_off() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media",
            ["apps"],
            ["radarr"],
            false,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = running
        .service()
        .list_tools_impl(None, context)
        .await
        .expect("list tools");
    let names = result
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();

    assert!(names.contains(&"radarr"));
    assert!(!names.contains(&"sonarr"));
    assert!(!names.contains(&CODE_MODE_TOOL_NAME));
}

#[tokio::test]
async fn call_tool_allows_mcp_app_sibling_callbacks_when_raw_tools_are_hidden() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("hidden while code_mode mode is enabled"),
        "MCP App sibling callbacks should reach upstream proxy routing, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "test fixture has no live peer, so allowed callbacks should fail at proxy call, got {text}"
    );
}

#[tokio::test]
async fn call_tool_allows_direct_mcp_app_ui_callbacks_with_read_scope() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_search_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_search_ui"),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "direct MCP App UI tools are render entry points and should not use the sibling execute-scope gate, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "test fixture has no live peer, so allowed UI callbacks should fail at proxy call, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_priority_zero_direct_mcp_app_ui_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_search_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.priority = 0.0;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_search_ui"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "priority-zero upstream must not be callable through the UI callback bypass, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_priority_zero_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.priority = 0.0;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "priority-zero upstream must not be callable through the sibling callback bypass, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_disabled_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let mut upstream = fixture_upstream_config("apps");
    upstream.enabled = false;
    let manager = code_mode_manager_with_pool(true, upstream, pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "disabled upstream must not be callable through the sibling callback bypass, got {text}"
    );
}

#[tokio::test]
async fn call_tool_preserves_selected_mcp_app_sibling_upstream() {
    let unrelated_name: Arc<str> = Arc::from("aaa_plain");
    let unrelated_probe = fixture_upstream_tool(&unrelated_name, "youtube_probe", None);

    let app_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &app_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let app_probe = fixture_upstream_tool(&app_name, "youtube_probe", None);

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "aaa_plain",
        fixture_upstream_entry(
            "aaa_plain",
            HashMap::from([("youtube_probe".to_string(), unrelated_probe)]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), app_probe),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("aaa_plain"),
            fixture_upstream_config("apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("upstream `apps` is not connected"),
        "MCP App sibling callbacks should dispatch to the UI sibling upstream, got {text}"
    );
    assert!(
        !text.contains("upstream `aaa_plain` is not connected"),
        "callback dispatch must not fall through to an unrelated same-name tool, got {text}"
    );
}

#[tokio::test]
async fn call_tool_requires_execute_scope_for_hidden_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "forbidden");
    assert_eq!(
        envelope["error"]["required_scopes"],
        serde_json::json!(["lab", "lab:admin"])
    );
}

/// The legacy `LAB_CODE_MODE_WIDGET_CALLBACKS` bypass surfaces ANY exposed
/// non-destructive upstream tool — including one with no MCP App UI resource that
/// is therefore NOT advertised in `list_tools`. Calling such a hidden tool via
/// the bypass with an authenticated-but-insufficient scope must be rejected, not
/// silently allowed. This pins the `requires_scope_check` flag on the legacy
/// path (it was previously `false`, which let a `lab:read` caller through).
#[tokio::test]
async fn call_tool_requires_execute_scope_for_legacy_widget_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    // A plain tool with no UI sibling: only the legacy "any exposed tool" rule
    // makes it callable via the widget-callback gate.
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_probe".to_string(), plain_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab:read"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "forbidden");
    assert_eq!(
        envelope["error"]["required_scopes"],
        serde_json::json!(["lab", "lab:admin"])
    );
}

#[tokio::test]
async fn codemode_requires_execute_scope_not_read_scope() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab:read"]),
        )
        .await
        .expect("call result");

    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(text.contains("\"kind\":\"forbidden\""), "{text}");
}

#[tokio::test]
async fn codemode_allows_execute_scope_to_reach_runner_path() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab"]),
        )
        .await
        .expect("call result");

    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "lab scope must pass execute auth: {text}"
    );
    if result.is_error.unwrap_or(false) {
        assert!(
            text.contains("\"service\":\"codemode\""),
            "codemode should route through the execute branch with its service name: {text}"
        );
    } else {
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value["kind"].as_str()),
            Some("code_mode_execute_trace")
        );
    }
}

#[tokio::test]
async fn codemode_routes_to_code_mode_path() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = running
        .service()
        .call_tool_impl(
            CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(
                serde_json::json!({ "code": "async () => 1" })
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
            scoped_context(running.peer().clone(), &["lab"]),
        )
        .await
        .expect("call result");

    let text: &str = result.content[0].as_text().expect("text").text.as_ref();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "codemode should pass execute auth: {text}"
    );
    if result.is_error.unwrap_or(false) {
        assert!(
            text.contains("\"service\":\"codemode\""),
            "codemode should preserve the called tool name in error envelopes: {text}"
        );
    } else {
        assert_eq!(
            result
                .structured_content
                .as_ref()
                .and_then(|value| value["kind"].as_str()),
            Some("code_mode_execute_trace"),
            "codemode should return runtime trace structured content"
        );
    }
}

#[tokio::test]
async fn call_tool_allows_execute_scope_for_hidden_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("\"kind\":\"forbidden\""),
        "lab scope should pass the callback execute-scope gate, got {text}"
    );
    assert!(
        text.contains("upstream `apps` is not connected"),
        "allowed callback should reach selected upstream proxy routing, got {text}"
    );
}

#[tokio::test]
async fn call_tool_honors_route_scope_for_mcp_app_sibling_callbacks() {
    let blocked_name: Arc<str> = Arc::from("blocked_apps");
    let ui_tool = fixture_upstream_tool(
        &blocked_name,
        "youtube_search_ui",
        Some("ui://blocked-apps/youtube-search.html"),
    );
    let blocked_probe = fixture_upstream_tool(&blocked_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "blocked_apps",
        fixture_upstream_entry(
            "blocked_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), blocked_probe),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("allowed_apps"),
            fixture_upstream_config("blocked_apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "allowed-only",
            ["allowed_apps"],
            ["gateway"],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );

    let text = call_tool_error_text(server, "youtube_probe").await;
    let envelope: Value = serde_json::from_str(&text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        !text.contains("blocked_apps"),
        "route-scope denial should not reach the blocked upstream, got {text}"
    );
}

#[tokio::test]
async fn call_tool_uses_subject_scoped_route_for_oauth_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("oauth_apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://oauth-apps/youtube-search.html"),
    );
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "oauth_apps",
        fixture_upstream_entry(
            "oauth_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_probe".to_string(), plain_tool),
            ]),
        ),
    )
    .await;
    let manager =
        code_mode_manager_with_pool(true, fixture_oauth_upstream_config("oauth_apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );

    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new("youtube_probe"),
        scoped_context(running.peer().clone(), &["lab"]),
    ))
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("upstream `oauth_apps` call failed"),
        "OAuth callback should use subject-scoped call routing, got {text}"
    );
    assert!(
        !text.contains("upstream `oauth_apps` is not connected"),
        "OAuth callback must not use shared raw-pool routing, got {text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_mcp_app_sibling_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let mut delete_tool = fixture_upstream_tool(&upstream_name, "youtube_delete", None);
    delete_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_delete".to_string(), delete_tool),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("\"kind\":\"confirmation_required\""),
        "{text}"
    );
    assert!(
        text.contains("not callable via the widget callback bypass"),
        "{text}"
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_direct_mcp_app_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_delete_ui",
        Some("ui://apps/youtube-delete.html"),
    );
    ui_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_delete_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );

    let text = call_tool_error_text(server, "youtube_delete_ui").await;
    let envelope: Value = serde_json::from_str(&text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
}

#[tokio::test]
async fn call_tool_blocks_destructive_legacy_widget_callbacks() {
    let upstream_name: Arc<str> = Arc::from("apps");
    let mut delete_tool = fixture_upstream_tool(&upstream_name, "youtube_delete", None);
    delete_tool.destructive = true;
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_delete".to_string(), delete_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_delete"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "confirmation_required");
}

#[tokio::test]
async fn call_tool_allows_legacy_widget_callbacks_for_route_allowed_upstream() {
    let upstream_name: Arc<str> = Arc::from("allowed_apps");
    let plain_tool = fixture_upstream_tool(&upstream_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "allowed_apps",
        fixture_upstream_entry(
            "allowed_apps",
            HashMap::from([("youtube_probe".to_string(), plain_tool)]),
        ),
    )
    .await;
    let manager =
        code_mode_manager_with_pool(true, fixture_upstream_config("allowed_apps"), pool).await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "allowed-only",
            ["allowed_apps"],
            ["gateway"],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("upstream_error"),
        "legacy callback should reach the route-allowed upstream proxy, got {text}"
    );
}

#[tokio::test]
async fn call_tool_honors_route_scope_for_legacy_widget_callbacks() {
    let blocked_name: Arc<str> = Arc::from("blocked_apps");
    let blocked_probe = fixture_upstream_tool(&blocked_name, "youtube_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "blocked_apps",
        fixture_upstream_entry(
            "blocked_apps",
            HashMap::from([("youtube_probe".to_string(), blocked_probe)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("allowed_apps"),
            fixture_upstream_config("blocked_apps"),
        ],
        pool,
    )
    .await;
    let mut server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "allowed-only",
            ["allowed_apps"],
            ["gateway"],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    server.code_mode_widget_callbacks_enabled_for_test = true;
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "not_found");
    assert!(
        !text.contains("blocked_apps"),
        "legacy callback should not reach a route-disallowed upstream, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_ambiguous_mcp_app_sibling_callbacks_when_one_candidate_is_destructive() {
    let safe_name: Arc<str> = Arc::from("safe_apps");
    let safe_ui_tool = fixture_upstream_tool(
        &safe_name,
        "youtube_search_ui",
        Some("ui://safe-apps/youtube-search.html"),
    );
    let safe_probe = fixture_upstream_tool(&safe_name, "youtube_probe", None);

    let destructive_name: Arc<str> = Arc::from("destructive_apps");
    let destructive_ui_tool = fixture_upstream_tool(
        &destructive_name,
        "youtube_search_ui",
        Some("ui://destructive-apps/youtube-search.html"),
    );
    let mut destructive_probe = fixture_upstream_tool(&destructive_name, "youtube_probe", None);
    destructive_probe.destructive = true;

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "safe_apps",
        fixture_upstream_entry(
            "safe_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), safe_ui_tool),
                ("youtube_probe".to_string(), safe_probe),
            ]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "destructive_apps",
        fixture_upstream_entry(
            "destructive_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), destructive_ui_tool),
                ("youtube_probe".to_string(), destructive_probe),
            ]),
        ),
    )
    .await;

    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("safe_apps"),
            fixture_upstream_config("destructive_apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "ambiguous_tool");
    assert_eq!(
        envelope["error"]["valid"],
        serde_json::json!([
            "destructive_apps::youtube_probe",
            "safe_apps::youtube_probe"
        ])
    );
}

#[tokio::test]
async fn call_tool_rejects_ambiguous_non_destructive_mcp_app_sibling_callbacks() {
    let alpha_name: Arc<str> = Arc::from("alpha_apps");
    let alpha_ui_tool = fixture_upstream_tool(
        &alpha_name,
        "youtube_search_ui",
        Some("ui://alpha-apps/youtube-search.html"),
    );
    let alpha_probe = fixture_upstream_tool(&alpha_name, "youtube_probe", None);

    let beta_name: Arc<str> = Arc::from("beta_apps");
    let beta_ui_tool = fixture_upstream_tool(
        &beta_name,
        "youtube_search_ui",
        Some("ui://beta-apps/youtube-search.html"),
    );
    let beta_probe = fixture_upstream_tool(&beta_name, "youtube_probe", None);

    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "alpha_apps",
        fixture_upstream_entry(
            "alpha_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), alpha_ui_tool),
                ("youtube_probe".to_string(), alpha_probe),
            ]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "beta_apps",
        fixture_upstream_entry(
            "beta_apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), beta_ui_tool),
                ("youtube_probe".to_string(), beta_probe),
            ]),
        ),
    )
    .await;

    let manager = code_mode_manager_with_pool_and_upstreams(
        true,
        vec![
            fixture_upstream_config("alpha_apps"),
            fixture_upstream_config("beta_apps"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_probe"), context),
    )
    .await
    .expect("call tool result");

    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("error envelope");
    assert_eq!(envelope["error"]["kind"], "ambiguous_tool");
    assert_eq!(
        envelope["error"]["valid"],
        serde_json::json!(["alpha_apps::youtube_probe", "beta_apps::youtube_probe"])
    );
}

#[tokio::test]
async fn call_tool_blocks_destructive_mcp_app_sibling_callback() {
    // A destructive sibling of a UI tool must be refused with
    // `confirmation_required` — the callback bypass has no confirmation channel.
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let destructive = fixture_destructive_upstream_tool(&upstream_name, "youtube_purge");
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([
                ("youtube_search_ui".to_string(), ui_tool),
                ("youtube_purge".to_string(), destructive),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_purge"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("confirmation_required"),
        "destructive sibling callback must be gated, got {text}"
    );
    assert!(
        !text.contains("upstream_error"),
        "destructive sibling callback must not reach the upstream proxy, got {text}"
    );
}

#[tokio::test]
async fn call_tool_refuses_ambiguous_mcp_app_sibling_callback() {
    // Two UI-bearing upstreams expose the same destructive probe name. The old
    // code collapsed multi-candidate to `tool = None`, which skipped the
    // destructive gate and proxied an arbitrary upstream. The callback must now
    // fail closed with `ambiguous_tool` and never reach the proxy.
    let a: Arc<str> = Arc::from("apps_a");
    let b: Arc<str> = Arc::from("apps_b");
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps_a",
        fixture_upstream_entry(
            "apps_a",
            HashMap::from([
                (
                    "youtube_search_ui".to_string(),
                    fixture_upstream_tool(&a, "youtube_search_ui", Some("ui://apps_a/s.html")),
                ),
                (
                    "youtube_purge".to_string(),
                    fixture_destructive_upstream_tool(&a, "youtube_purge"),
                ),
            ]),
        ),
    )
    .await;
    pool.insert_entry_for_test(
        "apps_b",
        fixture_upstream_entry(
            "apps_b",
            HashMap::from([
                (
                    "calendar_ui".to_string(),
                    fixture_upstream_tool(&b, "calendar_ui", Some("ui://apps_b/c.html")),
                ),
                (
                    "youtube_purge".to_string(),
                    fixture_destructive_upstream_tool(&b, "youtube_purge"),
                ),
            ]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool_multi(
        true,
        vec![
            fixture_upstream_config("apps_a"),
            fixture_upstream_config("apps_b"),
        ],
        pool,
    )
    .await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_purge"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("ambiguous_tool"),
        "multi-upstream sibling callback must fail closed, got {text}"
    );
    assert!(
        !text.contains("upstream_error"),
        "ambiguous destructive callback must not reach the upstream proxy, got {text}"
    );
}

#[tokio::test]
async fn call_tool_rejects_hidden_tool_without_ui_sibling_in_code_mode() {
    // A hidden raw tool whose upstream exposes no MCP App UI tool stays
    // unreachable — Code Mode's confinement guarantee.
    let upstream_name: Arc<str> = Arc::from("plain");
    let plain = fixture_upstream_tool(&upstream_name, "plain_probe", None);
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "plain",
        fixture_upstream_entry("plain", HashMap::from([("plain_probe".to_string(), plain)])),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("plain"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("plain_probe"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        text.contains("hidden while code_mode mode is enabled"),
        "hidden non-UI tool must be refused, got {text}"
    );
}

#[tokio::test]
async fn call_tool_allows_direct_mcp_app_ui_tool_in_code_mode() {
    // The requested tool itself carrying a UI resource is callable over the
    // bypass (the direct-UI route preserved by the refactor).
    let upstream_name: Arc<str> = Arc::from("apps");
    let ui_tool = fixture_upstream_tool(
        &upstream_name,
        "youtube_search_ui",
        Some("ui://apps/youtube-search.html"),
    );
    let pool = Arc::new(UpstreamPool::new());
    pool.insert_entry_for_test(
        "apps",
        fixture_upstream_entry(
            "apps",
            HashMap::from([("youtube_search_ui".to_string(), ui_tool)]),
        ),
    )
    .await;
    let manager = code_mode_manager_with_pool(true, fixture_upstream_config("apps"), pool).await;
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    let result = Box::pin(
        running
            .service()
            .call_tool_impl(CallToolRequestParams::new("youtube_search_ui"), context),
    )
    .await
    .expect("call tool result");
    assert!(result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    assert!(
        !text.contains("hidden while code_mode mode is enabled"),
        "direct MCP App UI tool must be callable, got {text}"
    );
    assert!(
        text.contains("upstream_error"),
        "direct UI callback should reach the proxy (no live peer), got {text}"
    );
}

#[tokio::test]
async fn snapshot_catalog_hides_builtin_tools_when_code_mode_is_enabled() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );

    let snapshot = server.snapshot_catalog().await;

    // Code Mode mode: exactly the primary `codemode` tool. NO legacy aliases.
    assert_eq!(
        snapshot.tools,
        ["codemode".to_string()].into_iter().collect()
    );
    assert!(
        !snapshot.tools.contains("code"),
        "code must not appear in Code Mode mode"
    );
}

#[tokio::test]
async fn snapshot_catalog_shows_no_gateway_tools_when_surface_is_disabled() {
    // When code_mode.enabled=false, none of the gateway Code Mode tool names
    // should appear in the snapshot.
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(false).await),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );

    let snapshot = server.snapshot_catalog().await;

    // Raw mode — none of the gateway meta-tools should appear.
    for meta_tool in ["codemode", "search", "execute", "code"] {
        assert!(
            !snapshot.tools.contains(meta_tool),
            "gateway meta-tool '{meta_tool}' must not appear when neither mode is enabled"
        );
    }
}

#[tokio::test]
async fn protected_scope_denies_direct_code_mode_calls_when_hidden() {
    let server = test_server(
        completion_test_registry(),
        Some(code_mode_manager(true).await),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media",
            ["sonarr"],
            ["radarr"],
            false,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    let context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(1),
        running.peer().clone(),
    );

    for tool_name in [CODE_MODE_TOOL_NAME] {
        let result = Box::pin(
            running
                .service()
                .call_tool_impl(CallToolRequestParams::new(tool_name), context.clone()),
        )
        .await
        .expect("call tool result");
        assert!(result.is_error.unwrap_or(false));
        let text = result.content[0].as_text().expect("text").text.as_str();
        assert!(
            text.contains("route_scope_denied"),
            "{tool_name} should be denied, got {text}"
        );
    }
}

#[tokio::test]
async fn server_reads_current_pool_from_gateway_manager() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime.clone(),
        ),
    );
    let notifier = crate::mcp::peers::PeerNotifier::default();
    let server = LabMcpServer {
        registry: Arc::new(ToolRegistry::new()),
        gateway_manager: Some(Arc::clone(&manager)),
        node_role: None,
        peers: Arc::clone(&notifier.peers),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Info,
        ))),
        route_scope: crate::mcp::route_scope::McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    };

    assert!(server.current_upstream_pool().await.is_none());

    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;

    let current = server.current_upstream_pool().await.expect("pool");
    assert!(Arc::ptr_eq(&current, &pool));
}

#[tokio::test]
async fn snapshot_catalog_hides_mcp_disabled_virtual_services() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: false,
                        webui: false,
                    },
                    mcp_policy: None,
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Info,
    );

    let snapshot = server.snapshot_catalog().await;
    assert!(!snapshot.tools.contains("deploy"));
}

#[tokio::test]
async fn gateway_add_through_mcp_protected_route_suppresses_hidden_enrichment_suggestion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            dir.path().join("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                upstream: vec![{
                    let mut upstream = fixture_upstream_config("rustarr");
                    upstream.enabled = false;
                    upstream
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    pool.insert_entry_for_test(
        "github",
        fixture_upstream_entry(
            "github",
            HashMap::from([(
                "search_repos".to_string(),
                fixture_upstream_tool(&Arc::<str>::from("github"), "search_repos", None),
            )]),
        ),
    )
    .await;
    let mut hidden_spec = fixture_upstream_config("github");
    hidden_spec.enabled = false;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media-route",
            ["rustarr".to_string()],
            ["gateway".to_string()],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let peer_server = test_server(
        ToolRegistry::new(),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        peer_server,
        transport,
        None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let result = Box::pin(server.call_tool_impl(
        CallToolRequestParams::new("gateway").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_string(),
                Value::String("gateway.add".to_string()),
            ),
            (
                "params".to_string(),
                serde_json::json!({ "spec": hidden_spec }),
            ),
        ])),
        context,
    ))
    .await
    .expect("call tool result");

    assert!(!result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("gateway envelope");
    assert_eq!(envelope["ok"], true);
    let view = &envelope["data"];
    assert_eq!(view["config"]["name"], "github");
    assert_eq!(view["enrichment_suggestion"], Value::Null);
    assert!(
        view["enrichment_suggestion_error"]
            .as_str()
            .is_some_and(|message| message.contains("unknown gateway upstream `github`")),
        "hidden upstream suggestion should fail open with a scoped unknown_upstream error: {view}"
    );
}

#[tokio::test]
async fn gateway_pending_import_approve_through_mcp_protected_route_suppresses_hidden_enrichment_suggestion()
 {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            dir.path().join("config.toml"),
            runtime,
        ),
    );
    let mut pending = fixture_upstream_config("paperless");
    pending.enabled = false;
    let mut import_source =
        labby_runtime::gateway_config::ImportSource::new("claude", "/tmp/mcp.json", "now");
    import_source.server_name = Some("paperless".to_string());
    import_source.transport_fingerprint = Some("sha256:test".to_string());
    pending.imported_from = Some(import_source);
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                upstream: vec![{
                    let mut upstream = fixture_upstream_config("rustarr");
                    upstream.enabled = false;
                    upstream
                }],
                upstream_pending: vec![pending],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media-route",
            ["rustarr".to_string()],
            ["gateway".to_string()],
            true,
        ),
        rmcp::model::LoggingLevel::Emergency,
    );
    let peer_server = test_server(
        ToolRegistry::new(),
        None,
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(256 * 1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        peer_server,
        transport,
        None,
    );
    let context = request_context_with_peer(running.peer().clone());

    let result = Box::pin(server.call_tool_impl(
        CallToolRequestParams::new("gateway").with_arguments(serde_json::Map::from_iter([
            (
                "action".to_string(),
                Value::String("gateway.import_pending.approve".to_string()),
            ),
            (
                "params".to_string(),
                serde_json::json!({ "name": "paperless", "confirm": true }),
            ),
        ])),
        context,
    ))
    .await
    .expect("call tool result");

    assert!(!result.is_error.unwrap_or(false));
    let text = result.content[0].as_text().expect("text").text.as_str();
    let envelope: Value = serde_json::from_str(text).expect("pending import envelope");
    assert_eq!(envelope["ok"], true);
    let view = &envelope["data"];
    assert_eq!(view["name"], "paperless");
    assert_eq!(view["enrichment_suggestion"], Value::Null);
    assert!(
        view["enrichment_suggestion_error"]
            .as_str()
            .is_some_and(|message| message.contains("unknown gateway upstream `paperless`")),
        "hidden pending import suggestion should fail open with a scoped unknown_upstream error: {view}"
    );
}

#[tokio::test]
#[ignore = "gateway-pivot: hardcoded plex/radarr fixtures; rework with kept-service fixtures post-pivot"]
async fn service_actions_json_filters_to_allowed_mcp_actions() {
    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        ),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                virtual_servers: vec![crate::config::VirtualServerConfig {
                    id: "deploy".to_string(),
                    service: "deploy".to_string(),
                    enabled: true,
                    surfaces: crate::config::VirtualServerSurfacesConfig {
                        cli: false,
                        api: false,
                        mcp: true,
                        webui: false,
                    },
                    mcp_policy: Some(crate::config::VirtualServerMcpPolicyConfig {
                        allowed_actions: vec!["server.info".to_string()],
                    }),
                }],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;

    let server = test_server(
        crate::registry::build_default_registry(),
        Some(manager),
        crate::mcp::route_scope::McpRouteScope::Root,
        rmcp::model::LoggingLevel::Info,
    );

    let value = server
        .service_actions_json("deploy")
        .await
        .expect("service actions");
    let actions = value.as_array().expect("array");
    assert!(actions.iter().any(|action| action["name"] == "help"));
    assert!(actions.iter().any(|action| action["name"] == "schema"));
    assert!(actions.iter().any(|action| action["name"] == "server.info"));
    assert!(
        !actions
            .iter()
            .any(|action| action["name"] == "session.list")
    );
}

// ===========================================================================
// Code Mode durable pause/resume — MCP-surface authorization gates
//
// These drive `call_tool_impl` over a duplex transport against a `test_server`
// whose `code_mode_manager` has a `SqliteDecider` injected, exercising the
// resume/reject authorization block in `call_tool_codemode.rs` end to end.
//
// The gates under test (V1 live-scope, V3 actor, F2 route-scope, V6 integrity,
// resume-divergence) all run BEFORE `broker.execute`, so they need only a
// pre-seeded durable run — no runner subprocess or live upstream. A true
// runner e2e (approved destructive call actually re-dispatching on resume) is
// NOT expressible here: the runner is spawned via `current_exe()` +
// `internal code-mode-runner`, which resolves to the labby binary only under
// `tests/` (CARGO_BIN_EXE_labby), and `call_tool_codemode_impl` is
// `pub(crate)` and unreachable from that integration harness. The two
// runner-dependent scenarios (#4 C1 and #7 happy path) are covered at the
// closest achievable boundary and their limits are documented at each test.
// ===========================================================================

use crate::codemode::decider::SqliteDecider;
use crate::codemode::sqlite_pauses::{CodeModePauseStore, NewLogEntry, NewRun, RunStatus};
// Trait methods (`begin`, `decide`, `run_status`, `resume_to_running`,
// `set_status`, `record_result`) require the trait in scope; `store()` is inherent.
use labby_codemode::CodeModeDecider as _;

/// `code_hash` the resume handler computes for a given `code` string
/// (`hash_arguments(Value::String(code))`).
fn resume_code_hash(code: &str) -> String {
    crate::mcp::result_format::hash_arguments(&Value::String(code.to_string()))
}

/// The capability-filter fingerprint the handler computes for a Root-scope call
/// whose args carry no `upstreams`/`tools` restriction (the resume shape).
fn root_scope_resume_fingerprint() -> String {
    labby_codemode::ToolScope::new(Vec::new(), Vec::new()).fingerprint()
}

/// The capability-filter fingerprint the handler computes for a protected-route
/// resume whose route allows no upstreams and whose args carry no
/// `upstreams`/`tools` restriction. `route_scoped_capability_filter` produces a
/// `scoped_namespaces(<allowed>, [])` filter, so an empty-upstream protected
/// route yields the empty-scoped fingerprint. Two protected routes that both
/// allow no upstreams share this fingerprint — letting the F2 route-scope test
/// pass the V1 fingerprint gate so ONLY the F2 gate can refuse.
fn empty_protected_scope_resume_fingerprint() -> String {
    labby_codemode::ToolScope::scoped_namespaces(Vec::new(), Vec::new()).fingerprint()
}

/// Build a `code_mode_manager(true)` with a `SqliteDecider` injected over a
/// fresh temp store. Returns the manager plus the `TempDir` (keep it alive for
/// the test's lifetime).
async fn code_mode_manager_with_decider() -> (
    Arc<crate::dispatch::gateway::manager::GatewayManager>,
    SqliteDecider,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = CodeModePauseStore::open(dir.path().join("codemode_pauses.db"))
        .await
        .expect("open store");
    let decider = SqliteDecider::new(store);

    let runtime = crate::dispatch::gateway::manager::GatewayRuntimeHandle::default();
    let manager = Arc::new(
        crate::dispatch::gateway::config_store::test_gateway_manager(
            std::path::PathBuf::from("config.toml"),
            runtime,
        )
        .with_code_mode_decider(Arc::new(decider.clone())),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled: true,
                    ..crate::config::CodeModeConfig::default()
                },
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    (manager, decider, dir)
}

/// Recorded-authz fields for a seeded paused run.
struct SeedPausedRun<'a> {
    execution_id: &'a str,
    code_hash: String,
    actor_key: Option<&'a str>,
    is_admin: bool,
    route_scope: &'a str,
    capability_filter_fingerprint: String,
}

/// Seed a `paused` durable run with a single pending destructive call at seq 0,
/// exactly as the pause-capable path would leave it after journaling +
/// flipping to paused. Returns nothing; assert on the store afterward.
async fn seed_paused_run(store: &CodeModePauseStore, seed: SeedPausedRun<'_>) {
    store
        .begin(NewRun {
            execution_id: seed.execution_id.to_string(),
            code_hash: seed.code_hash,
            actor_key: seed.actor_key.map(ToOwned::to_owned),
            is_admin: seed.is_admin,
            route_scope: seed.route_scope.to_string(),
            capability_filter_fingerprint: seed.capability_filter_fingerprint,
            expires_at_ms: now_ms_test() + 86_400_000,
        })
        .await
        .expect("begin");
    store
        .upsert_log_entry(
            seed.execution_id,
            NewLogEntry {
                seq: 0,
                tool_id: "demo::delete".to_string(),
                raw_args: serde_json::json!({ "id": 1 }),
                requires_approval: true,
                ephemeral: false,
            },
        )
        .await
        .expect("journal pending");
    store
        .set_status(seed.execution_id, RunStatus::Paused, None)
        .await
        .expect("pause");
}

fn now_ms_test() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Build a scoped MCP request context carrying an explicit `actor_key` and
/// scopes (the `scoped_context` helper hardcodes `actor_key: None`).
fn scoped_context_with_actor(
    peer: rmcp::service::Peer<rmcp::RoleServer>,
    scopes: &[&str],
    actor_key: Option<&str>,
) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    let mut context =
        rmcp::service::RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
    let mut parts = axum::http::Request::new(()).into_parts().0;
    parts.extensions.insert(crate::api::oauth::AuthContext {
        sub: "resumer".to_string(),
        actor_key: actor_key.map(Arc::from),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    });
    context.extensions.insert(parts);
    context
}

/// Drive a `codemode` resume/reject through `call_tool_impl` and return the
/// parsed envelope JSON. `confirm=None` omits the flag; `Some(b)` sets it.
async fn drive_codemode_resume(
    context: rmcp::service::RequestContext<rmcp::RoleServer>,
    running: &rmcp::service::RunningService<rmcp::RoleServer, LabMcpServer>,
    code: &str,
    resume_token: &str,
    confirm: Option<bool>,
) -> Value {
    let mut args = serde_json::Map::new();
    args.insert("code".to_string(), Value::String(code.to_string()));
    args.insert(
        "resume_token".to_string(),
        Value::String(resume_token.to_string()),
    );
    if let Some(flag) = confirm {
        args.insert("confirm".to_string(), Value::Bool(flag));
    }
    let result = Box::pin(running.service().call_tool_impl(
        CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(args),
        context,
    ))
    .await
    .expect("call tool result");
    let text = result.content[0].as_text().expect("text").text.as_str();
    serde_json::from_str(text).expect("envelope JSON")
}

/// Spin a duplex-served `test_server` for the codemode manager and hand back the
/// running service + a peer for building contexts.
fn serve_codemode(
    manager: Arc<crate::dispatch::gateway::manager::GatewayManager>,
    route_scope: crate::mcp::route_scope::McpRouteScope,
) -> rmcp::service::RunningService<rmcp::RoleServer, LabMcpServer> {
    let server = test_server(
        completion_test_registry(),
        Some(manager),
        route_scope,
        rmcp::model::LoggingLevel::Emergency,
    );
    let (transport, _client_transport) = tokio::io::duplex(64);
    rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    )
}

// ── V1: live scope narrowed since pause ────────────────────────────────────

#[tokio::test]
async fn resume_refused_when_live_scope_narrowed_since_pause() {
    let (manager, decider, _dir) = code_mode_manager_with_decider().await;
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";
    // Paused as an admin caller (is_admin = true, fingerprint = the Root resume
    // shape so the ONLY thing that changes at resume is the admin scope).
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-v1",
            code_hash: resume_code_hash(code),
            actor_key: Some("actor-a"),
            is_admin: true,
            route_scope: "root",
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
        },
    )
    .await;

    let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
    // Resume with identical code + confirm:true but WITHOUT lab:admin — only
    // `lab`. Live caps recompute `is_admin = false`, mismatching the recorded
    // `is_admin = true` → V1 fails closed.
    let context = scoped_context_with_actor(running.peer().clone(), &["lab"], Some("actor-a"));
    let env = drive_codemode_resume(context, &running, code, "run-v1", Some(true)).await;

    assert_eq!(
        env["error"]["kind"], "forbidden",
        "narrowed scope must be refused, got {env}"
    );
    // The run must NOT have advanced to running — the fail-closed contract.
    assert_eq!(
        decider.run_status("run-v1").await,
        labby_codemode::RunLifecycle::Paused,
        "a refused resume must leave the run paused, not advance it to running"
    );
}

// ── V3: actor identity ─────────────────────────────────────────────────────

#[tokio::test]
async fn resume_refused_for_different_actor() {
    let (manager, decider, _dir) = code_mode_manager_with_decider().await;
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-actor",
            code_hash: resume_code_hash(code),
            actor_key: Some("actor-A"),
            is_admin: true,
            route_scope: "root",
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
        },
    )
    .await;

    let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
    // Actor B tries to resume actor A's run → forbidden.
    let context =
        scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-B"));
    let env = drive_codemode_resume(context, &running, code, "run-actor", Some(true)).await;

    assert_eq!(
        env["error"]["kind"], "forbidden",
        "different actor must be refused, got {env}"
    );
    assert_eq!(
        decider.run_status("run-actor").await,
        labby_codemode::RunLifecycle::Paused,
        "a refused resume must leave the run paused"
    );
}

#[tokio::test]
async fn resume_actor_none_does_not_bridge_to_scoped() {
    // A trusted-local run (actor_key = None) must not be resumable by a scoped
    // actor (Some), and vice-versa — None matches only None.
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";

    // (a) trusted-local (None) run, scoped (Some) resumer → forbidden.
    {
        let (manager, decider, _dir) = code_mode_manager_with_decider().await;
        seed_paused_run(
            decider.store(),
            SeedPausedRun {
                execution_id: "run-none",
                code_hash: resume_code_hash(code),
                actor_key: None,
                is_admin: true,
                route_scope: "root",
                capability_filter_fingerprint: root_scope_resume_fingerprint(),
            },
        )
        .await;
        let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
        let context =
            scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-scoped"));
        let env = drive_codemode_resume(context, &running, code, "run-none", Some(true)).await;
        assert_eq!(
            env["error"]["kind"], "forbidden",
            "a scoped actor must not resume a trusted-local (None) run, got {env}"
        );
        assert_eq!(
            decider.run_status("run-none").await,
            labby_codemode::RunLifecycle::Paused
        );
    }

    // (b) scoped (Some) run, trusted-local (None) resumer → forbidden. A None
    // AuthContext models a local stdio caller; its actor_key is None, so it must
    // not bridge to a run started by a scoped actor.
    {
        let (manager, decider, _dir) = code_mode_manager_with_decider().await;
        seed_paused_run(
            decider.store(),
            SeedPausedRun {
                execution_id: "run-scoped",
                code_hash: resume_code_hash(code),
                actor_key: Some("actor-scoped"),
                is_admin: true,
                route_scope: "root",
                capability_filter_fingerprint: root_scope_resume_fingerprint(),
            },
        )
        .await;
        let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
        // No AuthContext at all → actor_key None (stdio/trusted-local).
        let context = request_context_with_peer(running.peer().clone());
        let env = drive_codemode_resume(context, &running, code, "run-scoped", Some(true)).await;
        assert_eq!(
            env["error"]["kind"], "forbidden",
            "a trusted-local (None) resumer must not resume a scoped run, got {env}"
        );
        assert_eq!(
            decider.run_status("run-scoped").await,
            labby_codemode::RunLifecycle::Paused
        );
    }
}

// ── F2: route-scope identity (resume + reject) ─────────────────────────────

#[tokio::test]
async fn resume_refused_across_route_scope() {
    let (manager, decider, _dir) = code_mode_manager_with_decider().await;
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";
    // Paused under protected route A. Seed the fingerprint route B WILL compute
    // (both routes allow no upstreams → the same empty-scoped fingerprint) and
    // keep is_admin/lab:admin, so the V1 gate PASSES and only the F2 route-scope
    // gate can produce the refusal — pinning F2 specifically.
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-route",
            code_hash: resume_code_hash(code),
            actor_key: Some("actor-a"),
            is_admin: true,
            route_scope: "protected:alpha",
            capability_filter_fingerprint: empty_protected_scope_resume_fingerprint(),
        },
    )
    .await;

    // Resume under a DIFFERENT protected route B (both expose code mode so the
    // call reaches the codemode handler).
    let route_b = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "beta",
        Vec::<String>::new(),
        Vec::<String>::new(),
        true,
    );
    let running = serve_codemode(manager, route_b);
    let context =
        scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-a"));
    let env = drive_codemode_resume(context, &running, code, "run-route", Some(true)).await;

    assert_eq!(
        env["error"]["kind"], "forbidden",
        "resume across a different route scope must be refused, got {env}"
    );
    assert_eq!(
        decider.run_status("run-route").await,
        labby_codemode::RunLifecycle::Paused,
        "a cross-route resume must leave the run paused"
    );
}

#[tokio::test]
async fn reject_refused_across_route_scope() {
    let (manager, decider, _dir) = code_mode_manager_with_decider().await;
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-route-rej",
            code_hash: resume_code_hash(code),
            actor_key: Some("actor-a"),
            is_admin: true,
            route_scope: "protected:alpha",
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
        },
    )
    .await;

    let route_b = crate::mcp::route_scope::McpRouteScope::protected_subset(
        "beta",
        Vec::<String>::new(),
        Vec::<String>::new(),
        true,
    );
    let running = serve_codemode(manager, route_b);
    let context =
        scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-a"));
    // Reject = confirm:false.
    let env = drive_codemode_resume(context, &running, code, "run-route-rej", Some(false)).await;

    assert_eq!(
        env["error"]["kind"], "forbidden",
        "reject across a different route scope must be refused, got {env}"
    );
    assert_eq!(
        decider.run_status("run-route-rej").await,
        labby_codemode::RunLifecycle::Paused,
        "a cross-route reject must leave the run paused"
    );
}

// ── V6 at the MCP surface: run integrity (resume + reject) ─────────────────

#[tokio::test]
async fn resume_refused_when_run_integrity_fails() {
    let (manager, decider, dir) = code_mode_manager_with_decider().await;
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-tamper",
            code_hash: resume_code_hash(code),
            actor_key: Some("actor-a"),
            is_admin: true,
            route_scope: "root",
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
        },
    )
    .await;
    // Tamper the row directly (flip is_admin) → HMAC verify fails.
    let conn = rusqlite::Connection::open(dir.path().join("codemode_pauses.db")).expect("raw open");
    conn.execute(
        "UPDATE codemode_runs SET is_admin = 0 WHERE execution_id = 'run-tamper'",
        [],
    )
    .expect("tamper");
    drop(conn);

    let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
    let context =
        scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-a"));
    let env = drive_codemode_resume(context, &running, code, "run-tamper", Some(true)).await;

    assert_eq!(
        env["error"]["kind"], "internal_error",
        "a tampered run must fail the integrity check before any mutation, got {env}"
    );
    // The tampered row reads back Unknown (fail closed) — never advanced.
    assert_eq!(
        decider.run_status("run-tamper").await,
        labby_codemode::RunLifecycle::Unknown,
        "a tampered run reads Unknown and is never advanced"
    );
}

#[tokio::test]
async fn reject_refused_when_run_integrity_fails() {
    let (manager, decider, dir) = code_mode_manager_with_decider().await;
    let code = "async () => { await callTool('demo::delete', { id: 1 }); }";
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-tamper-rej",
            code_hash: resume_code_hash(code),
            actor_key: Some("actor-a"),
            is_admin: true,
            route_scope: "root",
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
        },
    )
    .await;
    let conn = rusqlite::Connection::open(dir.path().join("codemode_pauses.db")).expect("raw open");
    conn.execute(
        "UPDATE codemode_runs SET is_admin = 0 WHERE execution_id = 'run-tamper-rej'",
        [],
    )
    .expect("tamper");
    drop(conn);

    let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
    let context =
        scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-a"));
    let env = drive_codemode_resume(context, &running, code, "run-tamper-rej", Some(false)).await;

    assert_eq!(
        env["error"]["kind"], "internal_error",
        "a tampered run must fail the integrity check before reject mutates state, got {env}"
    );
}

// ── Resume divergence: resubmitted code differs ────────────────────────────

#[tokio::test]
async fn resume_refused_when_resubmitted_code_differs() {
    let (manager, decider, _dir) = code_mode_manager_with_decider().await;
    let original = "async () => { await callTool('demo::delete', { id: 1 }); }";
    seed_paused_run(
        decider.store(),
        SeedPausedRun {
            execution_id: "run-diverge",
            code_hash: resume_code_hash(original),
            actor_key: Some("actor-a"),
            is_admin: true,
            route_scope: "root",
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
        },
    )
    .await;

    let running = serve_codemode(manager, crate::mcp::route_scope::McpRouteScope::Root);
    let context =
        scoped_context_with_actor(running.peer().clone(), &["lab:admin"], Some("actor-a"));
    // Correct token + confirm:true but DIFFERENT code.
    let different = "async () => { await callTool('demo::delete', { id: 2 }); }";
    let env = drive_codemode_resume(context, &running, different, "run-diverge", Some(true)).await;

    assert_eq!(
        env["error"]["kind"], "resume_divergence",
        "resubmitting different code must diverge, got {env}"
    );
    assert_eq!(
        decider.run_status("run-diverge").await,
        labby_codemode::RunLifecycle::Paused,
        "a divergent resume must leave the run paused"
    );
}

// ── #4 (C1) and #7 (happy path): boundary coverage + documented limits ─────

#[tokio::test]
async fn swallowed_pause_via_all_settled_still_pauses_and_dispatches_nothing() {
    // C1 e2e headline: code that wraps a destructive call in Promise.allSettled
    // then issues a second destructive call must still surface as
    // `confirmation_required`/status:"paused" (the host reads the DURABLE status
    // after settle — NOT the sandbox result), and no upstream destructive
    // dispatch happens.
    //
    // LIMIT: a full runner e2e is NOT expressible in this src/ unit harness — the
    // runner spawns via current_exe()+`internal code-mode-runner`, which is the
    // labby binary only under tests/ (CARGO_BIN_EXE_labby), and
    // call_tool_codemode_impl is pub(crate) so tests/ cannot reach it. The
    // decide-time monotonic gate that MAKES the swallow-proof behavior is proven
    // directly at the decider boundary here: once a run is paused, EVERY
    // subsequent decide() returns Pause and journals nothing, so a second
    // destructive call (the one after allSettled) can drive no dispatch. The
    // handler's post-settle read of this durable `paused` status → pause envelope
    // is separately pinned by the resume tests above (which all assert the run
    // stays Paused).
    let (_manager, decider, _dir) = code_mode_manager_with_decider().await;
    decider
        .begin(labby_codemode::BeginRun {
            execution_id: "run-c1".to_string(),
            code_hash: "hash".to_string(),
            actor_key: Some("actor-a".to_string()),
            is_admin: true,
            route_scope: "root".to_string(),
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
            expires_at_ms: now_ms_test() + 86_400_000,
        })
        .await
        .expect("begin");

    // First destructive call (inside allSettled) pauses the run.
    let first = decider
        .decide(
            "run-c1",
            0,
            "demo::delete",
            &serde_json::json!({ "id": 1 }),
            true,
            false,
        )
        .await;
    assert!(
        matches!(first, labby_codemode::DecideOutcome::Pause),
        "the first destructive call must pause, got {first:?}"
    );
    assert_eq!(
        decider.run_status("run-c1").await,
        labby_codemode::RunLifecycle::Paused
    );

    // Second destructive call (after the swallowed allSettled) — the monotonic
    // gate returns Pause and records NOTHING, so it can drive no dispatch.
    let second = decider
        .decide(
            "run-c1",
            1,
            "demo::delete",
            &serde_json::json!({ "id": 2 }),
            true,
            false,
        )
        .await;
    assert!(
        matches!(second, labby_codemode::DecideOutcome::Pause),
        "a post-pause call must also pause (monotonic gate), got {second:?}"
    );
    // Nothing journaled at seq 1 → no second destructive dispatch is possible.
    assert!(
        decider
            .store()
            .get_log_entry("run-c1", 1)
            .await
            .unwrap()
            .is_none(),
        "a swallowed post-pause destructive call must journal nothing (no dispatch)"
    );
    // The durable status the host would read after settle is still `paused`.
    assert_eq!(
        decider.run_status("run-c1").await,
        labby_codemode::RunLifecycle::Paused,
        "the durable status after settle is paused — the host emits confirmation_required"
    );
}

#[tokio::test]
async fn resume_executes_approved_destructive_call_and_completes() {
    // Happy-path resume: an authorized resume passes every gate, CASes the run
    // Paused→Running, and re-drives. The approved destructive call executes for
    // real; an already-applied non-destructive call short-circuits to its
    // recorded result (Replay, NOT re-dispatch).
    //
    // LIMIT: the actual re-dispatch + completion require the runner subprocess
    // (see the module header), unreachable from call_tool_codemode_impl in this
    // src/ harness. The two load-bearing decider guarantees are pinned directly:
    //  (1) an approved (pending) entry transitions to Execute on the resume pass
    //      (the destructive call re-dispatches), and
    //  (2) an already-applied non-destructive entry Replays its recorded result
    //      instead of re-dispatching (proven via the store: no new dispatch).
    let (_manager, decider, _dir) = code_mode_manager_with_decider().await;
    decider
        .begin(labby_codemode::BeginRun {
            execution_id: "run-happy".to_string(),
            code_hash: "hash".to_string(),
            actor_key: Some("actor-a".to_string()),
            is_admin: true,
            route_scope: "root".to_string(),
            capability_filter_fingerprint: root_scope_resume_fingerprint(),
            expires_at_ms: now_ms_test() + 86_400_000,
        })
        .await
        .expect("begin");

    // seq 0: a non-destructive read runs and records its result on the first pass.
    let read = decider
        .decide(
            "run-happy",
            0,
            "demo::read",
            &serde_json::json!({ "q": 1 }),
            false,
            false,
        )
        .await;
    assert!(matches!(read, labby_codemode::DecideOutcome::Execute));
    decider
        .record_result("run-happy", 0, &serde_json::json!({ "ok": true }))
        .await
        .expect("record");
    // seq 1: a destructive call pauses the run.
    let del = decider
        .decide(
            "run-happy",
            1,
            "demo::delete",
            &serde_json::json!({ "id": 1 }),
            true,
            false,
        )
        .await;
    assert!(matches!(del, labby_codemode::DecideOutcome::Pause));
    assert_eq!(
        decider.run_status("run-happy").await,
        labby_codemode::RunLifecycle::Paused
    );

    // Approve + resume: CAS Paused→Running (what the handler does after all
    // authz gates pass).
    assert!(
        decider.resume_to_running("run-happy").await,
        "an authorized resume CASes the run to running"
    );
    assert_eq!(
        decider.run_status("run-happy").await,
        labby_codemode::RunLifecycle::Running
    );

    // Re-drive pass:
    // (1) the already-applied non-destructive seq 0 short-circuits to Replay of
    //     its recorded result — NOT a re-dispatch.
    let replay = decider
        .decide(
            "run-happy",
            0,
            "demo::read",
            &serde_json::json!({ "q": 1 }),
            false,
            false,
        )
        .await;
    match replay {
        labby_codemode::DecideOutcome::Replay(v) => {
            assert_eq!(v, serde_json::json!({ "ok": true }));
        }
        other => panic!("an applied non-destructive call must Replay, got {other:?}"),
    }
    // (2) the approved (pending) destructive seq 1 now transitions to Execute —
    //     the approved destructive call re-dispatches for real on resume.
    let exec = decider
        .decide(
            "run-happy",
            1,
            "demo::delete",
            &serde_json::json!({ "id": 1 }),
            true,
            false,
        )
        .await;
    assert!(
        matches!(exec, labby_codemode::DecideOutcome::Execute),
        "the approved destructive call must Execute (re-dispatch) on resume, got {exec:?}"
    );
    // After it records its result the run can complete.
    decider
        .record_result("run-happy", 1, &serde_json::json!({ "deleted": 1 }))
        .await
        .expect("record delete");
    assert!(
        decider
            .set_status("run-happy", labby_codemode::RunLifecycle::Completed, None)
            .await
            .expect("complete")
    );
    assert_eq!(
        decider.run_status("run-happy").await,
        labby_codemode::RunLifecycle::Completed
    );
}

// ── #8: local-provider-allowed runs are not pause-capable ──────────────────

#[tokio::test]
async fn run_allowing_local_providers_is_not_pause_capable() {
    // A caller for whom local providers are allowed (unscoped admin / trusted
    // -local) takes the write-free path: `local_providers_allowed(&caller,
    // &scope)` gates `pause_capable`, so no durable run is begun and no
    // resume_token is minted.
    //
    // The load-bearing predicate is `labby_codemode::local_providers_allowed`,
    // which `call_tool_codemode.rs` ANDs into `pause_capable`. Prove it directly:
    // an unscoped admin/trusted-local caller with an unscoped tool filter is
    // local-provider-allowed (⇒ NOT pause-capable), while a scoped caller is not.
    use labby_codemode::{CodeModeCaller, CodeModeCallerCapabilities, ToolScope};

    let trusted_local = CodeModeCaller::TrustedLocal;
    let unscoped = ToolScope::new(Vec::new(), Vec::new());
    assert!(
        labby_codemode::local_providers_allowed(&trusted_local, &unscoped),
        "a trusted-local caller with an unscoped filter is local-provider-allowed \
         ⇒ the handler makes the run NOT pause-capable (no durable run, no resume_token)"
    );

    let unscoped_admin = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_execute: true,
            can_use_snippets: true,
            is_admin: true,
        },
        sub: Some("admin".to_string()),
    };
    assert!(
        labby_codemode::local_providers_allowed(&unscoped_admin, &unscoped),
        "an unscoped admin caller is local-provider-allowed ⇒ NOT pause-capable"
    );

    // A route-scoped run (namespaces present) must NOT allow local providers —
    // this is the run shape that DOES begin a durable, pause-capable run.
    let scoped = ToolScope::scoped_namespaces(vec!["demo".to_string()], Vec::new());
    assert!(
        !labby_codemode::local_providers_allowed(&unscoped_admin, &scoped),
        "a route-scoped run must not allow local providers (it is pause-capable)"
    );
}
