//! Full-stack Code Mode test harness (feature `test-harness`, non-default).
//!
//! This module exposes small public wrappers over the crate-private Code Mode
//! MCP surface so an integration test in `crates/labby/tests/` can drive the
//! ENTIRE real production path — [`LabMcpServer::call_tool_codemode_impl`] (the
//! real settle logic: begin-run → drive → read-durable-status-after-settle),
//! the real [`GatewayManager`] / `CodeModeHost` decide→dispatch→record dance,
//! the production `RunnerPool` (which spawns the real `labby internal
//! code-mode-runner` subprocess, resolved via `LAB_CODE_MODE_RUNNER_EXE`), and
//! a real [`SqliteDecider`] / `CodeModePauseStore` on a temp-file DB — with NO
//! inline host glue.
//!
//! It is gated `#[cfg(feature = "test-harness")]` and is never compiled into a
//! production build. The crate-private constructors it reaches
//! (`test_gateway_manager`, the `LabMcpServer` struct literal,
//! `call_tool_codemode_impl`, the widened `#[cfg(any(test, feature =
//! "test-harness"))]` seams) are unchanged for a normal (feature-off) build.

// Test scaffolding: `panic`/`assert` are expected here (the workspace lints
// `panic = "warn"`, promoted to an error by CI's `-D warnings`). This module is
// only ever compiled under the non-default `test-harness` feature, never in a
// production build, so the production-code panic restriction does not apply.
#![allow(clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use labby_auth::auth_context::AuthContext;
use labby_codemode::CodeModeDecider;
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::service::{Peer, RequestContext, RunningService};
use serde_json::{Map, Value};

use crate::codemode::decider::SqliteDecider;
use crate::dispatch::gateway::config_store::test_gateway_manager;
use crate::dispatch::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
use crate::dispatch::upstream::pool::UpstreamPool;
use crate::dispatch::upstream::types::{
    ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
};
use crate::mcp::catalog::CODE_MODE_TOOL_NAME;
use crate::mcp::logging::logging_level_rank;
use crate::mcp::route_scope::McpRouteScope;
use crate::mcp::server::LabMcpServer;
use crate::registry::ToolRegistry;

/// Names the harness wires into the catalog. The destructive tool is
/// `<upstream>::<tool>`; the read tool is non-destructive.
pub struct HarnessTools {
    /// Upstream namespace (e.g. `"stub"`).
    pub upstream: String,
    /// Destructive tool name within the upstream (e.g. `"delete"`).
    pub destructive_tool: String,
    /// Non-destructive tool name within the upstream (e.g. `"read"`).
    pub read_tool: String,
}

impl HarnessTools {
    /// The `<upstream>::<tool>` id a snippet calls for the destructive tool.
    #[must_use]
    pub fn destructive_id(&self) -> String {
        format!("{}::{}", self.upstream, self.destructive_tool)
    }

    /// The `<upstream>::<tool>` id a snippet calls for the read tool.
    #[must_use]
    pub fn read_id(&self) -> String {
        format!("{}::{}", self.upstream, self.read_tool)
    }
}

fn upstream_config(name: &str) -> crate::config::UpstreamConfig {
    crate::config::UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        // A syntactically valid but unroutable URL: the fixture upstream has no
        // live peer, so an actual dispatch attempt yields `upstream_error` — that
        // is exactly the proof-of-re-dispatch the resume test relies on.
        url: Some("http://127.0.0.1:9/mcp".to_string()),
        bearer_token_env: None,
        command: None,
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
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

fn plain_tool(upstream: &Arc<str>, name: &str, destructive: bool) -> UpstreamTool {
    let tool = Tool::new(
        name.to_string(),
        format!("{name} description"),
        Arc::new(Map::new()),
    );
    UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::clone(upstream),
        destructive,
    }
}

fn upstream_entry(upstream: &str, tools: HashMap<String, UpstreamTool>) -> UpstreamEntry {
    UpstreamEntry {
        name: Arc::from(upstream),
        tools,
        exposure_policy: ToolExposurePolicy::All,
        proxy_resources: false,
        prompt_count: 0,
        resource_count: 0,
        prompt_names: Vec::new(),
        resource_uris: Vec::new(),
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

/// Build the real production [`GatewayManager`] with:
///
/// - Code Mode enabled,
/// - a real [`SqliteDecider`] injected (durable pause/resume over the caller's
///   temp DB),
/// - a real [`UpstreamPool`] whose catalog contains a DESTRUCTIVE tool
///   (`<upstream>::<destructive_tool>`) plus a non-destructive read tool.
///
/// The manager's `RunnerPool` is the production one (built via
/// `RunnerPool::from_env()` at construction); with `LAB_CODE_MODE_RUNNER_EXE`
/// set it spawns the real `labby internal code-mode-runner` subprocess.
async fn build_manager(decider: Arc<SqliteDecider>) -> (Arc<GatewayManager>, HarnessTools) {
    let tools = HarnessTools {
        upstream: "stub".to_string(),
        destructive_tool: "delete".to_string(),
        read_tool: "read".to_string(),
    };
    let upstream_name: Arc<str> = Arc::from(tools.upstream.as_str());

    let pool = Arc::new(UpstreamPool::new());
    let mut catalog = HashMap::new();
    catalog.insert(
        tools.destructive_tool.clone(),
        plain_tool(&upstream_name, &tools.destructive_tool, true),
    );
    catalog.insert(
        tools.read_tool.clone(),
        plain_tool(&upstream_name, &tools.read_tool, false),
    );
    pool.insert_entry_for_test(&tools.upstream, upstream_entry(&tools.upstream, catalog))
        .await;

    let runtime = GatewayRuntimeHandle::default();
    runtime.swap(Some(pool)).await;

    let decider_dyn: Arc<dyn CodeModeDecider> = decider;
    let manager = Arc::new(
        test_gateway_manager(std::path::PathBuf::from("config.toml"), runtime)
            .with_code_mode_decider(decider_dyn),
    );
    manager
        .seed_config_unchecked_for_tests(
            crate::config::LabConfig {
                code_mode: crate::config::CodeModeConfig {
                    enabled: true,
                    ..crate::config::CodeModeConfig::default()
                },
                upstream: vec![upstream_config(&tools.upstream)],
                ..crate::config::LabConfig::default()
            }
            .to_gateway_config(),
        )
        .await;
    (manager, tools)
}

fn server_with_manager(manager: Arc<GatewayManager>) -> LabMcpServer {
    LabMcpServer {
        registry: Arc::new(ToolRegistry::new()),
        gateway_manager: Some(manager),
        node_role: None,
        peers: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: Arc::new(AtomicU8::new(logging_level_rank(
            rmcp::model::LoggingLevel::Emergency,
        ))),
        route_scope: McpRouteScope::Root,
        relay_session_id: 0,
        code_mode_widget_callbacks_enabled_for_test: false,
    }
}

/// A live `LabMcpServer` served over an in-memory duplex transport so it owns a
/// real downstream peer (needed to build a [`RequestContext`]). The
/// [`RunningService`] keeps the peer alive; the harness holds the client end of
/// the duplex so the transport stays open for the test's lifetime.
pub struct CodeModeServerHandle {
    running: RunningService<rmcp::RoleServer, LabMcpServer>,
    _client_transport: tokio::io::DuplexStream,
    /// The names wired into the catalog.
    pub tools: HarnessTools,
}

impl CodeModeServerHandle {
    fn peer(&self) -> Peer<rmcp::RoleServer> {
        self.running.peer().clone()
    }

    fn service(&self) -> &LabMcpServer {
        self.running.service()
    }
}

/// Build a fully-wired Code Mode server around the caller's real
/// [`SqliteDecider`], with a destructive tool in the catalog. Everything on the
/// path is production code: the manager, the upstream pool, the runner pool, and
/// the decider. Drive it with [`drive_codemode`].
pub async fn code_mode_server_with_destructive_tool(
    decider: Arc<SqliteDecider>,
) -> CodeModeServerHandle {
    let (manager, tools) = build_manager(decider).await;
    let server = server_with_manager(manager);
    let (transport, client_transport) = tokio::io::duplex(1024);
    let running = rmcp::service::serve_directly::<rmcp::RoleServer, _, _, std::io::Error, _>(
        server, transport, None,
    );
    CodeModeServerHandle {
        running,
        _client_transport: client_transport,
        tools,
    }
}

/// Build an MCP request context carrying the given scopes + actor.
///
/// Mirrors the crate-private `scoped_context_with_actor` test helper: an
/// [`AuthContext`] is inserted into a request `Parts`, and the `Parts` into the
/// context extensions, exactly as the HTTP→MCP bridge does.
fn scoped_context(
    peer: Peer<rmcp::RoleServer>,
    scopes: &[&str],
    actor: Option<&str>,
) -> RequestContext<rmcp::RoleServer> {
    let mut context = RequestContext::new(rmcp::model::NumberOrString::Number(1), peer);
    let mut parts = axum::http::Request::new(()).into_parts().0;
    parts.extensions.insert(AuthContext {
        sub: "harness".to_string(),
        actor_key: actor.map(Arc::from),
        scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        issuer: "https://lab.example.com".to_string(),
        via_session: true,
        csrf_token: None,
        email: None,
    });
    context.extensions.insert(parts);
    context
}

/// Drive one real `codemode` call through the entire production MCP surface and
/// return the parsed envelope JSON.
///
/// `args` is the full `codemode` params object (must carry `code`; may carry
/// `confirm`, `resume_token`, `upstreams`, `tools`). This calls the real
/// [`LabMcpServer::call_tool_codemode_impl`] — the SAME entry the `call_tool`
/// dispatcher reaches for the `codemode` tool — so the begin-run,
/// decide→dispatch→record, and read-durable-status-after-settle logic all run
/// for real.
pub async fn drive_codemode(
    handle: &CodeModeServerHandle,
    args: Value,
    scopes: &[&str],
    actor: Option<&str>,
) -> Value {
    let args_map: Map<String, Value> = match args {
        Value::Object(map) => map,
        other => panic!("codemode args must be a JSON object, got: {other}"),
    };
    let context = scoped_context(handle.peer(), scopes, actor);
    let request = CallToolRequestParams::new(CODE_MODE_TOOL_NAME).with_arguments(args_map.clone());
    let arguments = request.arguments.unwrap_or_default();
    let result = Box::pin(handle.service().call_tool_codemode_impl(
        CODE_MODE_TOOL_NAME,
        &arguments,
        &context,
    ))
    .await
    .expect("call_tool_codemode_impl must return a CallToolResult");
    let text = result.content[0]
        .as_text()
        .expect("codemode result must carry a text block")
        .text
        .as_str();
    serde_json::from_str(text).expect("codemode envelope must be JSON")
}
