//! Unmounted nondestructive regular Tool execution authorization seam.
#![allow(dead_code)]

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::manager::{GatewayManager, PublishedToolCallError};
use labby_gateway::upstream::tool_error::mcp_error_data_kind;
use rmcp::model::{CallToolRequestParams, CallToolResponse};
use thiserror::Error;

use crate::access::{AccessRuntime, Permission};
use crate::mcp::bound_access::{BoundAccessContext, bind_asset_use_access_context};

/// Server-owned inputs for one exact regular non-OAuth Tool execution.
///
/// Deliberately non-`Clone`, non-`Debug`, and non-serializable. The identity
/// and protected-route facts must be trusted server inputs. This unmounted
/// seam does not prove a transport token instance or expiry.
pub(crate) struct ToolExecutionResolutionInput {
    identity: VerifiedIdentity,
    route_name: String,
    resource: String,
    project_id: String,
    request: CallToolRequestParams,
}

impl ToolExecutionResolutionInput {
    pub(crate) fn new(
        identity: VerifiedIdentity,
        route_name: impl Into<String>,
        resource: impl Into<String>,
        project_id: impl Into<String>,
        request: CallToolRequestParams,
    ) -> Self {
        Self {
            identity,
            route_name: route_name.into(),
            resource: resource.into(),
            project_id: project_id.into(),
            request,
        }
    }
}

/// Redacted result classes exposed by the server-owned freshness seam.
#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum ToolExecutionResolutionError {
    #[error("tool execution target is unavailable")]
    Unavailable,
    #[error("tool execution queue is unavailable")]
    QueueUnavailable,
    #[error("upstream tool returned an MCP {kind} error (code {code})")]
    Mcp { kind: &'static str, code: i32 },
    #[error("upstream tool transport failed")]
    Transport,
    #[error("upstream tool protocol failed")]
    Protocol,
    #[error("tool execution timed out")]
    Timeout,
    #[error("tool execution was cancelled")]
    Cancelled,
    #[error("upstream tool input-required rounds were exceeded")]
    InputRequiredRoundsExceeded,
    #[error("tool execution failed")]
    Other,
    #[error("tool response is too large")]
    TooLarge,
}

#[derive(Clone, PartialEq, Eq)]
struct ExactToolTarget {
    upstream: String,
    native_name: String,
    pool_generation: labby_gateway::gateway::manager::PoolPublicationGeneration,
    tool_generation: labby_gateway::upstream::pool::ToolCatalogGeneration,
    destructive: bool,
}

fn resolve_exact_target(context: &BoundAccessContext, wire_name: &str) -> Option<ExactToolTarget> {
    if context.catalog().access().permission != Permission::AssetUse {
        return None;
    }
    if crate::mcp::permanent_tools::is_reserved_non_upstream_tool_name(wire_name)
        || context
            .catalog()
            .catalog()
            .services()
            .services()
            .iter()
            .any(|service| service.name() == wire_name)
    {
        return None;
    }
    let tools = context.catalog().catalog().tools();
    let published = tools.unique_route_for_wire_name(wire_name)?;
    if published.tool.destructive
        || !context.allows_upstream_tool_pair(
            published.upstream_name.as_ref(),
            published.tool_name.as_ref(),
        )
    {
        return None;
    }
    Some(ExactToolTarget {
        upstream: published.upstream_name.to_string(),
        native_name: published.tool_name.to_string(),
        pool_generation: tools.pool_publication_generation(),
        tool_generation: tools.tool_catalog_generation(),
        destructive: published.tool.destructive,
    })
}

/// Authorize and execute one exact nondestructive regular non-OAuth Tool over
/// a bounded Access/manager common interval. This remains unmounted.
pub(crate) async fn execute_exact_project_tool(
    runtime: &AccessRuntime,
    manager: &GatewayManager,
    input: ToolExecutionResolutionInput,
) -> Result<CallToolResponse, ToolExecutionResolutionError> {
    let wire_name = input.request.name.to_string();
    let first = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity.clone(),
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| ToolExecutionResolutionError::Unavailable)?;
    let target = resolve_exact_target(&first, &wire_name)
        .ok_or(ToolExecutionResolutionError::Unavailable)?;
    let mut outbound = input.request;
    outbound.name = target.native_name.clone().into();
    let result = manager
        .execute_published_tool_exact(
            target.pool_generation,
            target.tool_generation,
            &target.upstream,
            &target.native_name,
            outbound,
        )
        .await;
    let second = bind_asset_use_access_context(
        runtime,
        manager,
        input.identity,
        &input.route_name,
        &input.resource,
        &input.project_id,
    )
    .await
    .map_err(|_| ToolExecutionResolutionError::Unavailable)?;
    let second_target = resolve_exact_target(&second, &wire_name)
        .ok_or(ToolExecutionResolutionError::Unavailable)?;
    if !first.same_publication_as(&second) || target != second_target {
        return Err(ToolExecutionResolutionError::Unavailable);
    }
    map_manager_result(result)
}

fn map_manager_result(
    result: Result<CallToolResponse, PublishedToolCallError>,
) -> Result<CallToolResponse, ToolExecutionResolutionError> {
    result.map_err(map_manager_error)
}

fn map_manager_error(error: PublishedToolCallError) -> ToolExecutionResolutionError {
    match error {
        PublishedToolCallError::Unavailable => ToolExecutionResolutionError::Unavailable,
        PublishedToolCallError::QueueUnavailable => ToolExecutionResolutionError::QueueUnavailable,
        PublishedToolCallError::Mcp(data) => ToolExecutionResolutionError::Mcp {
            kind: mcp_error_data_kind(&data),
            code: data.code.0,
        },
        PublishedToolCallError::Transport => ToolExecutionResolutionError::Transport,
        PublishedToolCallError::Protocol => ToolExecutionResolutionError::Protocol,
        PublishedToolCallError::Timeout => ToolExecutionResolutionError::Timeout,
        PublishedToolCallError::Cancelled => ToolExecutionResolutionError::Cancelled,
        PublishedToolCallError::InputRequiredRoundsExceeded => {
            ToolExecutionResolutionError::InputRequiredRoundsExceeded
        }
        PublishedToolCallError::Other => ToolExecutionResolutionError::Other,
        PublishedToolCallError::TooLarge => ToolExecutionResolutionError::TooLarge,
    }
}

#[cfg(all(test, feature = "proxy-testkit"))]
#[allow(clippy::disallowed_methods)] // Test fixture constructs upstream-owned descriptors directly.
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use labby_auth::{Authenticator, VerifiedIdentity};
    use labby_gateway::gateway::config_store::FsGatewayConfigStore;
    use labby_gateway::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use labby_gateway::upstream::pool::UpstreamPool;
    use labby_gateway::upstream::types::UpstreamTool;
    use labby_runtime::gateway_config::{
        GatewayConfig, GatewayLoadoutConfig, ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig,
        ProtectedMcpRouteTarget, UpstreamConfig, VirtualServerConfig, VirtualServerSurfacesConfig,
    };
    use rmcp::model::{
        BooleanSchema, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        CreateTaskResult, ElicitRequest, ElicitRequestParams, ElicitationSchema, InputRequest,
        InputRequests, InputRequiredResult, PrimitiveSchemaDefinition, ServerCapabilities,
        ServerInfo, Task, TaskStatus, Tool,
    };
    use rmcp::model::{ErrorCode, ErrorData};
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use tokio::sync::{Mutex, Notify};

    use super::{
        ToolExecutionResolutionError, ToolExecutionResolutionInput, execute_exact_project_tool,
        map_manager_error, map_manager_result,
    };
    use crate::access::{AccessRuntime, AssignProjectLoadoutInput, BootstrapOwnerInput};
    use crate::mcp::catalog::{
        ADD_SERVER_TOOL_NAME, CODE_MODE_READ_TOOL_NAME, CODE_MODE_TOOL_NAME,
        CODE_MODE_UI_TOOL_NAME, GATEWAY_STATUS_TOOL_NAME, MCP_APP_TOOL_NAME, SETTINGS_TOOL_NAME,
    };
    use labby_gateway::gateway::manager::PublishedToolCallError;

    #[test]
    fn mcp_error_mapping_keeps_only_stable_kind_and_code() {
        let mapped = map_manager_error(PublishedToolCallError::Mcp(ErrorData::new(
            ErrorCode(-32_602),
            "private tenant secret",
            Some(serde_json::json!({"kind": "invalid_params", "secret": "hidden"})),
        )));

        assert_eq!(
            mapped,
            ToolExecutionResolutionError::Mcp {
                kind: "invalid_param",
                code: -32_602,
            }
        );
        let rendered = mapped.to_string();
        assert!(!rendered.contains("private tenant secret"));
        assert!(!rendered.contains("hidden"));

        for (published, expected) in [
            (
                PublishedToolCallError::Unavailable,
                ToolExecutionResolutionError::Unavailable,
            ),
            (
                PublishedToolCallError::QueueUnavailable,
                ToolExecutionResolutionError::QueueUnavailable,
            ),
            (
                PublishedToolCallError::Transport,
                ToolExecutionResolutionError::Transport,
            ),
            (
                PublishedToolCallError::Protocol,
                ToolExecutionResolutionError::Protocol,
            ),
            (
                PublishedToolCallError::Timeout,
                ToolExecutionResolutionError::Timeout,
            ),
            (
                PublishedToolCallError::Cancelled,
                ToolExecutionResolutionError::Cancelled,
            ),
            (
                PublishedToolCallError::InputRequiredRoundsExceeded,
                ToolExecutionResolutionError::InputRequiredRoundsExceeded,
            ),
            (
                PublishedToolCallError::Other,
                ToolExecutionResolutionError::Other,
            ),
            (
                PublishedToolCallError::TooLarge,
                ToolExecutionResolutionError::TooLarge,
            ),
        ] {
            assert_eq!(map_manager_error(published), expected);
        }
    }

    #[test]
    fn wrapper_boundary_preserves_complete_task_and_input_required_responses() {
        let schema = ElicitationSchema::builder()
            .required_property(
                "confirm",
                PrimitiveSchemaDefinition::Boolean(BooleanSchema::default()),
            )
            .build()
            .unwrap();
        let expected = [
            CallToolResponse::Complete(CallToolResult::success(vec![ContentBlock::text(
                "complete",
            )])),
            CallToolResponse::Task(CreateTaskResult::new(Task::new(
                "task-7",
                TaskStatus::Working,
                "2026-08-24T00:00:00Z",
                "2026-08-24T00:00:00Z",
            ))),
            CallToolResponse::InputRequired(InputRequiredResult::from_input_requests(
                InputRequests::from([(
                    "confirmation".into(),
                    InputRequest::Elicitation(ElicitRequest::new(
                        ElicitRequestParams::FormElicitationParams {
                            meta: None,
                            message: "confirm?".into(),
                            requested_schema: schema,
                        },
                    )),
                )]),
            )),
        ];
        for expected in expected {
            let actual = map_manager_result(Ok(expected.clone())).unwrap();
            match (actual, expected) {
                (CallToolResponse::Complete(actual), CallToolResponse::Complete(expected)) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                (CallToolResponse::Task(actual), CallToolResponse::Task(expected)) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                (
                    CallToolResponse::InputRequired(actual),
                    CallToolResponse::InputRequired(expected),
                ) => {
                    assert_eq!(
                        serde_json::to_value(actual).unwrap(),
                        serde_json::to_value(expected).unwrap()
                    );
                }
                _ => panic!("wrapper boundary changed response variant"),
            }
        }
    }

    #[derive(Clone)]
    struct EchoToolServer {
        calls: Arc<AtomicUsize>,
        received_meta: Arc<Mutex<Vec<rmcp::model::RequestMetaObject>>>,
    }

    impl ServerHandler for EchoToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received_meta.lock().await.push(context.meta);
            let value = request
                .arguments
                .as_ref()
                .and_then(|args| args.get("value"))
                .cloned();
            Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "{}:{}",
                request.name,
                value.unwrap_or_default()
            ))])
            .into())
        }
    }

    #[derive(Clone)]
    struct DelayedToolServer {
        calls: Arc<AtomicUsize>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                return Err(ErrorData::internal_error("private delayed failure", None));
            }
            Ok(CallToolResult::success(vec![ContentBlock::text("delayed")]).into())
        }
    }

    fn config(expose_tools: bool) -> GatewayConfig {
        GatewayConfig {
            upstream: ["alpha", "bravo"]
                .into_iter()
                .map(|name| UpstreamConfig {
                    enabled: true,
                    name: name.into(),
                    url: None,
                    transport: None,
                    socket_path: None,
                    headers: Default::default(),
                    bearer_token_env: None,
                    command: Some("node".into()),
                    args: Vec::new(),
                    env: Default::default(),
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
                })
                .collect(),
            loadouts: vec![GatewayLoadoutConfig {
                name: "production".into(),
                upstreams: vec!["alpha".into(), "bravo".into()],
                services: vec!["gateway".into()],
                expose_tools,
                ..GatewayLoadoutConfig::default()
            }],
            protected_mcp_routes: vec![ProtectedMcpRouteConfig {
                name: "project-route".into(),
                enabled: true,
                public_host: "mcp.example.com".into(),
                public_path: "/project".into(),
                upstream: None,
                backend_url: String::new(),
                backend_mcp_path: "/mcp".into(),
                scopes: Vec::new(),
                health_path: None,
                target: Some(ProtectedMcpRouteTarget::GatewaySubset(
                    ProtectedGatewaySubsetTarget {
                        project_id: Some("bootstrap-default".into()),
                        loadout: Some("production".into()),
                        ..ProtectedGatewaySubsetTarget::default()
                    },
                )),
            }],
            virtual_servers: vec![VirtualServerConfig {
                id: "gateway".into(),
                service: "gateway".into(),
                enabled: true,
                surfaces: VirtualServerSurfacesConfig {
                    mcp: true,
                    ..VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..GatewayConfig::default()
        }
    }

    fn upstream_tool(name: &str, destructive: bool) -> UpstreamTool {
        let tool = Tool::new(name.to_string(), "exact", Arc::new(serde_json::Map::new()));
        UpstreamTool {
            input_schema: Some(serde_json::Value::Object((*tool.input_schema).clone())),
            output_schema: None,
            destructive,
            upstream_name: Arc::from("alpha"),
            tool,
        }
    }

    async fn start_delayed_call(
        runtime: Arc<AccessRuntime>,
        manager: Arc<GatewayManager>,
        pool: &Arc<UpstreamPool>,
        identity: VerifiedIdentity,
        calls: Arc<AtomicUsize>,
        fail: bool,
    ) -> (
        tokio::task::JoinHandle<Result<CallToolResponse, ToolExecutionResolutionError>>,
        Arc<Notify>,
        Arc<Notify>,
    ) {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls,
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let task = tokio::spawn(async move {
            execute_exact_project_tool(
                &runtime,
                &manager,
                ToolExecutionResolutionInput::new(
                    identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                    CallToolRequestParams::new("nested/tool"),
                ),
            )
            .await
        });
        (task, started, release)
    }

    #[tokio::test]
    async fn exact_asset_use_tool_rewrites_raw_name_and_rejects_owned_or_destructive_names() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let runtime = Arc::new(AccessRuntime::initialize(directory.path().join("access.db")).await);
        let identity = VerifiedIdentity::local_credential_with_issuer(
            Authenticator::StaticBearer,
            "server-static-issuer",
            "server-credential",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                BootstrapOwnerInput::new(identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .assign_project_loadout(
                AssignProjectLoadoutInput::new(identity.clone(), "bootstrap-default", "production")
                    .unwrap(),
            )
            .await
            .unwrap();
        let usage_store = Arc::new(
            labby_gateway::usage::UsageStore::open(directory.path().join("usage.db"))
                .await
                .unwrap(),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let received_meta = Arc::new(Mutex::new(Vec::new()));
        let pool = Arc::new(UpstreamPool::new().with_usage_store(Some(usage_store)));
        pool.install_tool_server_for_tests(
            "alpha",
            EchoToolServer {
                calls: Arc::clone(&calls),
                received_meta: Arc::clone(&received_meta),
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let gateway_runtime = GatewayRuntimeHandle::default();
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        let path = directory.path().join("tool-execution.toml");
        let manager = Arc::new(
            GatewayManager::with_store(
                path.clone(),
                gateway_runtime.clone(),
                Arc::new(FsGatewayConfigStore::new(path)),
            )
            .with_builtin_service_registry(Arc::new(crate::registry::build_default_registry())),
        );
        manager.try_seed_config(config(true)).await.unwrap();
        let make_input = |name: &str| {
            ToolExecutionResolutionInput::new(
                identity.clone(),
                "project-route",
                "https://mcp.example.com/project",
                "bootstrap-default",
                {
                    let mut request = CallToolRequestParams::new(name.to_string()).with_arguments(
                        serde_json::Map::from_iter([("value".into(), serde_json::json!("kept"))]),
                    );
                    let mut meta = rmcp::model::RequestMetaObject::new();
                    meta.insert("trace-id".into(), serde_json::json!("opaque-meta"));
                    request.meta = Some(meta);
                    request
                },
            )
        };

        let response = execute_exact_project_tool(&runtime, &manager, make_input("nested/tool"))
            .await
            .unwrap();
        let CallToolResponse::Complete(result) = response else {
            panic!("regular fixture must complete")
        };
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized["content"][0]["text"], "nested/tool:\"kept\"");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(received_meta.lock().await[0]["trace-id"], "opaque-meta");
        for _ in 0..100 {
            if pool.usage_row_count_for_tests().await == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(pool.usage_row_count_for_tests().await, 1);

        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("unknown")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let bravo_calls = Arc::new(AtomicUsize::new(0));
        pool.install_tool_server_for_tests(
            "bravo",
            EchoToolServer {
                calls: Arc::clone(&bravo_calls),
                received_meta: Arc::new(Mutex::new(Vec::new())),
            },
        )
        .await;
        let mut duplicate = upstream_tool("nested/tool", false);
        duplicate.upstream_name = Arc::from("bravo");
        pool.insert_tool_routes_for_tests("bravo", vec![duplicate])
            .await;
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(bravo_calls.load(Ordering::SeqCst), 0);

        let mut reverse_order = config(true);
        reverse_order.upstream.reverse();
        reverse_order.loadouts[0].upstreams.reverse();
        manager.try_seed_config(reverse_order).await.unwrap();
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(bravo_calls.load(Ordering::SeqCst), 0);
        pool.insert_tool_routes_for_tests("bravo", Vec::new()).await;

        let excluded_calls = Arc::new(AtomicUsize::new(0));
        pool.install_tool_server_for_tests(
            "charlie",
            EchoToolServer {
                calls: Arc::clone(&excluded_calls),
                received_meta: Arc::new(Mutex::new(Vec::new())),
            },
        )
        .await;
        let mut excluded = upstream_tool("excluded", false);
        excluded.upstream_name = Arc::from("charlie");
        pool.insert_tool_routes_for_tests("charlie", vec![excluded])
            .await;
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("excluded")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(excluded_calls.load(Ordering::SeqCst), 0);

        for (name, destructive) in [
            (CODE_MODE_TOOL_NAME, false),
            (CODE_MODE_READ_TOOL_NAME, false),
            (CODE_MODE_UI_TOOL_NAME, false),
            (MCP_APP_TOOL_NAME, false),
            (ADD_SERVER_TOOL_NAME, false),
            (GATEWAY_STATUS_TOOL_NAME, false),
            (SETTINGS_TOOL_NAME, false),
            ("gateway", false),
            ("danger", true),
        ] {
            pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool(name, destructive)])
                .await;
            let actual = execute_exact_project_tool(&runtime, &manager, make_input(name)).await;
            assert!(
                matches!(&actual, Err(ToolExecutionResolutionError::Unavailable)),
                "{name}: {actual:?}"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{name}");
        }

        let delayed_calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls: Arc::clone(&delayed_calls),
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail: false,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let task_runtime = Arc::clone(&runtime);
        let task_manager = Arc::clone(&manager);
        let task_identity = identity.clone();
        let task = tokio::spawn(async move {
            execute_exact_project_tool(
                &task_runtime,
                &task_manager,
                ToolExecutionResolutionInput::new(
                    task_identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                    CallToolRequestParams::new("nested/tool"),
                ),
            )
            .await
        });
        started.notified().await;
        manager.try_seed_config(config(false)).await.unwrap();
        manager.try_seed_config(config(true)).await.unwrap();
        release.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(delayed_calls.load(Ordering::SeqCst), 1);

        let access_started = Arc::new(Notify::new());
        let access_release = Arc::new(Notify::new());
        pool.install_tool_server_for_tests(
            "alpha",
            DelayedToolServer {
                calls: Arc::clone(&delayed_calls),
                started: Arc::clone(&access_started),
                release: Arc::clone(&access_release),
                fail: false,
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        let task_runtime = Arc::clone(&runtime);
        let task_manager = Arc::clone(&manager);
        let task_identity = identity.clone();
        let access_task = tokio::spawn(async move {
            execute_exact_project_tool(
                &task_runtime,
                &task_manager,
                ToolExecutionResolutionInput::new(
                    task_identity,
                    "project-route",
                    "https://mcp.example.com/project",
                    "bootstrap-default",
                    CallToolRequestParams::new("nested/tool"),
                ),
            )
            .await
        });
        access_started.notified().await;
        let store = runtime.store().await.unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default';
             UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
            )
            .await
            .unwrap();
        store
            .execute_test_statement(
                "UPDATE project_memberships SET role='owner' WHERE project_id='bootstrap-default';
             UPDATE access_metadata SET global_revision=global_revision+1 WHERE singleton=1",
            )
            .await
            .unwrap();
        access_release.notify_one();
        assert!(matches!(
            access_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(delayed_calls.load(Ordering::SeqCst), 2);

        let (tool_task, tool_started, tool_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        tool_started.notified().await;
        let before_tool_generation = pool.published_tool_catalog().await.unwrap().generation();
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("replacement", false)])
            .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        assert_ne!(
            pool.published_tool_catalog().await.unwrap().generation(),
            before_tool_generation
        );
        tool_release.notify_one();
        assert!(matches!(
            tool_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));

        let (safety_task, safety_started, safety_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        safety_started.notified().await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", true)])
            .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        safety_release.notify_one();
        assert!(matches!(
            safety_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));

        let (service_task, service_started, service_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        service_started.notified().await;
        manager.set_builtin_service_registry(Arc::new(crate::registry::build_default_registry()));
        service_release.notify_one();
        assert!(matches!(
            service_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(delayed_calls.load(Ordering::SeqCst), 5);
        for _ in 0..100 {
            if pool.usage_row_count_for_tests().await >= 5 {
                break;
            }
            tokio::task::yield_now().await;
        }
        let usage_before_pool_aba = pool.usage_row_count_for_tests().await;

        let (pool_task, pool_started, pool_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        pool_started.notified().await;
        pool.set_tool_last_error_for_tests("alpha", Some("sentinel".into()))
            .await;
        let replacement = Arc::new(UpstreamPool::new());
        replacement
            .install_tool_server_for_tests(
                "alpha",
                EchoToolServer {
                    calls: Arc::new(AtomicUsize::new(0)),
                    received_meta: Arc::new(Mutex::new(Vec::new())),
                },
            )
            .await;
        replacement
            .insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        gateway_runtime.swap(Some(replacement)).await;
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        pool_release.notify_one();
        assert!(matches!(
            pool_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(
            pool.upstream_tool_last_error("alpha").await.as_deref(),
            Some("sentinel")
        );
        assert!(pool.header_recovery_is_empty_for_tests("alpha"));
        assert_eq!(
            pool.usage_row_count_for_tests().await,
            usage_before_pool_aba
        );

        let (pool_error_task, pool_error_started, pool_error_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            true,
        )
        .await;
        pool_error_started.notified().await;
        pool.set_tool_last_error_for_tests("alpha", Some("error-sentinel".into()))
            .await;
        gateway_runtime
            .swap(Some(Arc::new(UpstreamPool::new())))
            .await;
        gateway_runtime.swap(Some(Arc::clone(&pool))).await;
        pool_error_release.notify_one();
        assert!(matches!(
            pool_error_task.await.unwrap(),
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(
            pool.upstream_tool_last_error("alpha").await.as_deref(),
            Some("error-sentinel")
        );
        assert!(pool.header_recovery_is_empty_for_tests("alpha"));
        assert_eq!(
            pool.usage_row_count_for_tests().await,
            usage_before_pool_aba
        );

        let (cancel_task, cancel_started, cancel_release) = start_delayed_call(
            Arc::clone(&runtime),
            Arc::clone(&manager),
            &pool,
            identity.clone(),
            Arc::clone(&delayed_calls),
            false,
        )
        .await;
        cancel_started.notified().await;
        cancel_task.abort();
        assert!(cancel_task.await.unwrap_err().is_cancelled());
        cancel_release.notify_one();
        pool.install_tool_server_for_tests(
            "alpha",
            EchoToolServer {
                calls: Arc::clone(&calls),
                received_meta: Arc::clone(&received_meta),
            },
        )
        .await;
        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        assert!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool"))
                .await
                .is_ok()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        pool.insert_tool_routes_for_tests("alpha", vec![upstream_tool("nested/tool", false)])
            .await;
        manager.try_seed_config(config(false)).await.unwrap();
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        manager.try_seed_config(config(true)).await.unwrap();
        runtime
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE project_memberships SET role='viewer' WHERE project_id='bootstrap-default'",
            )
            .await
            .unwrap();
        assert!(matches!(
            execute_exact_project_tool(&runtime, &manager, make_input("nested/tool")).await,
            Err(ToolExecutionResolutionError::Unavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
