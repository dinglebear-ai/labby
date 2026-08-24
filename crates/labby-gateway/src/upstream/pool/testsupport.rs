#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
//! Shared `#[cfg(test)]` fixtures and mock servers for the upstream-pool tests.
//!
//! These helpers are consumed by the co-located test modules across `pool/`
//! (discovery, tools, resources, prompts, health, …) and by the pool.rs test
//! module. They are `pub(super)` so every descendant test module can pull them
//! in with `use super::super::testsupport::*;` (or `use super::testsupport::*;`
//! from the pool.rs test module).

#![cfg(any(test, feature = "testkit"))]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomRequest,
    CustomResult, ErrorCode, ErrorData, GetPromptRequestParams, GetPromptResponse, GetPromptResult,
    ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt,
    PromptMessage, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    Role, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{UpstreamRuntimeMetadata, UpstreamTool};
use super::entries::healthy_in_process_entry;
use super::helpers::IN_PROCESS_PEER_BUFFER_BYTES;
use super::{UpstreamConnection, UpstreamPool};

pub(super) fn test_upstream_config() -> UpstreamConfig {
    UpstreamConfig {
        enabled: true,
        name: "test".into(),
        url: None,
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

pub(super) fn named_test_upstream_config(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        command: Some("true".to_string()),
        ..test_upstream_config()
    }
}

pub(super) fn named_disabled_test_upstream_config(name: &str) -> UpstreamConfig {
    UpstreamConfig {
        enabled: false,
        ..named_test_upstream_config(name)
    }
}

pub(super) fn test_tool(name: &str) -> rmcp::model::Tool {
    rmcp::model::Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()))
}

pub(super) fn test_upstream_tool(upstream_name: &Arc<str>, name: &str) -> UpstreamTool {
    let schema = Arc::new(serde_json::Map::new());
    let tool = rmcp::model::Tool::new(name.to_string(), format!("{name} description"), schema);
    UpstreamTool {
        tool,
        input_schema: None,
        output_schema: None,
        upstream_name: Arc::clone(upstream_name),
        destructive: false,
    }
}

pub(super) fn test_upstream_tools(
    upstream_name: &Arc<str>,
    names: &[&str],
) -> HashMap<String, UpstreamTool> {
    names
        .iter()
        .map(|name| (name.to_string(), test_upstream_tool(upstream_name, name)))
        .collect()
}

#[derive(Clone, Default)]
pub(super) struct StaticCatalogServer {
    pub(super) list_prompts_count: Arc<AtomicUsize>,
    pub(super) get_prompt_count: Arc<AtomicUsize>,
    pub(super) fail_list_prompts: Arc<AtomicBool>,
}

impl ServerHandler for StaticCatalogServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new("file:///tmp/upstream-one", "upstream-one"),
            Resource::new(
                "lab://upstream/old-name/file:///tmp/upstream-two",
                "upstream-two",
            ),
        ]))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        self.list_prompts_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_list_prompts.load(Ordering::SeqCst) {
            return Err(ErrorData::internal_error(
                "prompt listing failed for test",
                None,
            ));
        }

        Ok(ListPromptsResult::with_all_items(vec![
            Prompt::new("upstream.prompt.one", Some("first prompt"), None),
            Prompt::new("upstream.prompt.two", Some("second prompt"), None),
        ]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        self.get_prompt_count.fetch_add(1, Ordering::SeqCst);
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!("proxied {}", request.name),
        )])
        .into())
    }
}

pub(super) async fn static_catalog_pool(upstream_name: &str) -> Arc<UpstreamPool> {
    static_catalog_pool_with_server(upstream_name, StaticCatalogServer::default()).await
}

pub(super) async fn static_catalog_pool_with_server(
    upstream_name: &str,
    server: StaticCatalogServer,
) -> Arc<UpstreamPool> {
    catalog_pool_with_server(upstream_name, server).await
}

pub(super) async fn catalog_pool_with_server<S>(upstream_name: &str, server: S) -> Arc<UpstreamPool>
where
    S: ServerHandler,
{
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let server_task = tokio::spawn(async move {
        let running = server
            .serve(server_transport)
            .await
            .expect("catalog server starts");
        running.waiting().await.expect("catalog server runs");
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
        .serve(client_transport)
        .await
        .expect("catalog client starts");
    let peer = client_service.peer().clone();

    let pool = Arc::new(UpstreamPool::new());
    let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
    let previous = pool
        .install_connection_catalog_entry(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        )
        .await
        .expect("connection identity");
    assert!(previous.is_none());
    pool.resource_upstreams
        .write()
        .await
        .push(upstream_name.to_string());

    pool
}

pub(super) struct SlowResponseServer;

impl ServerHandler for SlowResponseServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Vec::new()))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(CallToolResult::success(Vec::new()).into())
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(ReadResourceResult::new(Vec::new()).into())
    }

    async fn get_prompt(
        &self,
        _request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(GetPromptResult::new(Vec::new()).into())
    }
}

pub(super) async fn slow_response_pool(upstream_name: &str) -> Arc<UpstreamPool> {
    let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
    let server_task = tokio::spawn(async move {
        let running = SlowResponseServer
            .serve(server_transport)
            .await
            .expect("slow response server starts");
        running.waiting().await.expect("slow response server runs");
    });
    let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
        .serve(client_transport)
        .await
        .expect("slow response client starts");
    let peer = client_service.peer().clone();

    let pool = Arc::new(UpstreamPool::new().with_request_timeout(Duration::from_millis(25)));
    let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
    let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new());
    entry.prompt_count = 1;
    entry.resource_count = 1;
    entry.prompt_names = vec!["slow.prompt".to_string()];
    entry.resource_uris = vec!["file:///tmp/slow".to_string()];
    pool.catalog
        .write()
        .await
        .insert(upstream_name.to_string(), entry);
    pool.connections.write().await.insert(
        upstream_name.to_string(),
        UpstreamConnection {
            _client_service: client_service.into(),
            _server_task: Some(server_task),
            peer,
            runtime: UpstreamRuntimeMetadata::default(),
            incarnation: None,
        },
    );
    pool.resource_upstreams
        .write()
        .await
        .push(upstream_name.to_string());

    pool
}

impl UpstreamPool {
    /// Register a hermetic skills-capable MCP peer for downstream facade tests.
    ///
    /// The peer returns `list_result` from `skills/list`, `get_entry` from
    /// `skills/get`, and serves the exact text values keyed by native resource
    /// URI. This deliberately exposes data rather than an arbitrary server
    /// implementation as part of the testkit API.
    pub async fn insert_scripted_skills_server_for_tests(
        &self,
        upstream_name: &str,
        list_result: serde_json::Value,
        get_entry: serde_json::Value,
        resources: HashMap<String, String>,
    ) {
        #[derive(Clone)]
        struct ScriptedSkillsServer {
            list_result: serde_json::Value,
            get_entry: serde_json::Value,
            resources: Arc<HashMap<String, String>>,
        }

        impl ServerHandler for ScriptedSkillsServer {
            fn get_info(&self) -> ServerInfo {
                let mut capabilities = ServerCapabilities::builder().enable_tools().build();
                let mut extensions = rmcp::model::ExtensionCapabilities::new();
                extensions.insert(
                    labby_runtime::skills::wire::SKILLS_EXTENSION_KEY.to_string(),
                    serde_json::Map::new(),
                );
                capabilities.extensions = Some(extensions);
                ServerInfo::new(capabilities)
            }

            async fn read_resource(
                &self,
                request: ReadResourceRequestParams,
                _context: RequestContext<RoleServer>,
            ) -> Result<ReadResourceResponse, ErrorData> {
                let text = self.resources.get(&request.uri).cloned().ok_or_else(|| {
                    ErrorData::new(ErrorCode::RESOURCE_NOT_FOUND, "resource not found", None)
                })?;
                Ok(
                    ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(
                        text,
                        request.uri,
                    )])
                    .into(),
                )
            }

            async fn on_custom_request(
                &self,
                request: CustomRequest,
                _context: RequestContext<RoleServer>,
            ) -> Result<CustomResult, ErrorData> {
                match request.method.as_str() {
                    "skills/list" => Ok(CustomResult::new(self.list_result.clone())),
                    "skills/get" => Ok(CustomResult::new(serde_json::json!({
                        "resultType": "complete",
                        "skill": self.get_entry,
                    }))),
                    _ => Err(ErrorData::new(
                        ErrorCode::METHOD_NOT_FOUND,
                        "method not found",
                        None,
                    )),
                }
            }
        }

        let server = ScriptedSkillsServer {
            list_result,
            get_entry,
            resources: Arc::new(resources),
        };
        let fixture = catalog_pool_with_server(upstream_name, server).await;
        let connection = fixture
            .connections
            .write()
            .await
            .remove(upstream_name)
            .expect("scripted skills connection");
        let entry = fixture
            .catalog
            .write()
            .await
            .remove(upstream_name)
            .expect("scripted skills catalog entry");
        self.catalog
            .write()
            .await
            .insert(upstream_name.to_string(), entry);
        self.connections
            .write()
            .await
            .insert(upstream_name.to_string(), connection);
        self.resource_upstreams
            .write()
            .await
            .push(upstream_name.to_string());
    }

    /// Register an in-process upstream whose tool call returns a successful MCP
    /// response carrying `is_error=true`.
    pub async fn insert_tool_error_server_for_tests(
        &self,
        upstream_name: &str,
        message: &'static str,
    ) {
        struct ToolErrorServer {
            message: &'static str,
        }

        impl ServerHandler for ToolErrorServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn call_tool(
                &self,
                _request: CallToolRequestParams,
                _context: RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Ok(CallToolResult::error(vec![ContentBlock::text(self.message)]).into())
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server = ToolErrorServer { message };
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("tool error server starts");
            running.waiting().await.expect("tool error server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("tool error client starts");
        let peer = client_service.peer().clone();

        self.catalog
            .write()
            .await
            .entry(upstream_name.to_string())
            .or_insert_with(|| healthy_in_process_entry(Arc::from(upstream_name), HashMap::new()));
        self.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );
    }

    /// Register an in-process upstream whose tool call returns a JSON-RPC/MCP error.
    pub async fn insert_mcp_error_server_for_tests(&self, upstream_name: &str, error: ErrorData) {
        struct McpErrorServer {
            error: ErrorData,
        }

        impl ServerHandler for McpErrorServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn call_tool(
                &self,
                _request: CallToolRequestParams,
                _context: RequestContext<RoleServer>,
            ) -> Result<CallToolResponse, ErrorData> {
                Err(self.error.clone())
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server = McpErrorServer { error };
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("MCP error server starts");
            running.waiting().await.expect("MCP error server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("MCP error client starts");
        let peer = client_service.peer().clone();

        self.catalog
            .write()
            .await
            .entry(upstream_name.to_string())
            .or_insert_with(|| healthy_in_process_entry(Arc::from(upstream_name), HashMap::new()));
        self.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );
    }

    /// Register an in-process upstream whose advertised tool list is backed by a
    /// shared `Arc<RwLock<Vec<String>>>`, so a test can mutate the live tool set
    /// after connection and exercise live-catalog refresh.
    pub async fn insert_live_tool_server_for_tests(
        &self,
        upstream_name: &str,
        tools: Arc<tokio::sync::RwLock<Vec<String>>>,
    ) {
        struct MutableToolCatalogServer {
            tools: Arc<tokio::sync::RwLock<Vec<String>>>,
        }

        impl ServerHandler for MutableToolCatalogServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }

            async fn list_tools(
                &self,
                _request: Option<PaginatedRequestParams>,
                _context: RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                let tools = self
                    .tools
                    .read()
                    .await
                    .iter()
                    .map(|name| {
                        rmcp::model::Tool::new(
                            name.to_string(),
                            format!("{name} description"),
                            Arc::new(serde_json::Map::new()),
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(ListToolsResult::with_all_items(tools))
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server = MutableToolCatalogServer { tools };
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("mutable tool catalog server starts");
            running
                .waiting()
                .await
                .expect("mutable tool catalog server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("mutable tool catalog client starts");
        let peer = client_service.peer().clone();

        self.catalog
            .write()
            .await
            .entry(upstream_name.to_string())
            .or_insert_with(|| healthy_in_process_entry(Arc::from(upstream_name), HashMap::new()));
        self.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
                incarnation: None,
            },
        );
    }
}

/// Re-home a fixture's pooled connection into the `(upstream, subject)` cache
/// so the OAuth subject-scoped paths hit their fast path without any network.
///
/// Shared because two suites need it: `tools_exposure_tests` (does the
/// `expose_tools` filter apply on this path too?) and
/// `annotation_passthrough_tests` (do upstream annotations survive it?).
/// `tools` is stored exactly as given — seed it *unfiltered*, since the
/// behavior under test always runs after the cache read.
pub(super) async fn move_connection_to_subject_cache_with_tools(
    pool: &UpstreamPool,
    upstream: &str,
    subject: &str,
    tools: Vec<rmcp::model::Tool>,
) {
    // `UpstreamConnection` implements `Drop`, so the whole value has to be moved
    // out of the pool rather than having its fields taken individually.
    let peer = pool
        .connections
        .read()
        .await
        .get(upstream)
        .expect("fixture connection present")
        .peer
        .clone();
    let connection = pool
        .connections
        .write()
        .await
        .remove(upstream)
        .expect("fixture connection present");
    pool.subject_connections.write().await.insert(
        (upstream.to_string(), subject.to_string()),
        super::SubjectScopedConnection {
            _connection: connection,
            peer,
            tools,
            last_used: std::time::Instant::now(),
        },
    );
}
