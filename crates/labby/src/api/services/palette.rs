use axum::{
    Extension, Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use labby_gateway::gateway::palette::{
    LabbyActionLauncherEntry, LauncherCatalogView, LauncherEntryView, PaletteCaller,
    PaletteExecuteRequest, PaletteExecuteResponse,
};
use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/catalog", get(catalog))
        .route("/execute", post(execute))
}

async fn catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<LauncherCatalogView>, ApiError> {
    let manager = state.gateway_manager.clone().ok_or_else(missing_manager)?;
    let caller = palette_caller(auth.as_ref().map(|auth| &auth.0), request_id(&headers))?;
    let mut catalog = manager.palette_catalog(&caller).await?;
    append_labby_actions(&mut catalog, &state, auth.as_ref().map(|auth| &auth.0));
    catalog.entries.sort_by(|a, b| entry_id(a).cmp(entry_id(b)));
    catalog.fingerprint = catalog_fingerprint(&catalog.entries);
    Ok(Json(catalog))
}

async fn execute(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(request): Json<PaletteExecuteRequest>,
) -> Result<Json<PaletteExecuteResponse>, ApiError> {
    if request.id.starts_with("labby:") {
        return execute_labby_action(state, auth.as_ref().map(|auth| &auth.0), request).await;
    }
    let manager = state.gateway_manager.clone().ok_or_else(missing_manager)?;
    let caller = palette_caller(auth.as_ref().map(|auth| &auth.0), request_id(&headers))?;
    Ok(Json(manager.palette_execute(&caller, request).await?))
}

fn palette_caller(
    auth: Option<&AuthContext>,
    request_id: Option<&str>,
) -> Result<PaletteCaller, ToolError> {
    let Some(auth) = auth else {
        return Err(ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "palette routes require authenticated API context".to_string(),
        });
    };
    if auth.scopes.iter().any(|scope| scope == "lab:admin") {
        return Ok(PaletteCaller::admin(Some(&auth.sub), request_id));
    }

    let allowed_upstreams = auth
        .scopes
        .iter()
        .filter_map(|scope| scope.strip_prefix("gateway:"))
        .filter(|name| !name.is_empty() && *name != "*")
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    Ok(PaletteCaller::scoped_read_only(
        Some(&auth.sub),
        request_id,
        allowed_upstreams,
    ))
}

fn request_id(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
}

fn missing_manager() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: "palette routes require an active gateway manager".to_string(),
    }
}

fn append_labby_actions(
    catalog: &mut LauncherCatalogView,
    state: &AppState,
    auth: Option<&AuthContext>,
) {
    for service in state
        .registry
        .services()
        .iter()
        .filter(|service| service.status == "available")
        .filter(|service| state.enabled_services.contains(service.name))
    {
        for action in service.actions {
            if !labby_action_visible(state, service.name, action, auth) {
                continue;
            }
            let input_schema = labby_action_schema(action);
            let schema_fingerprint = input_schema.as_ref().map(stable_json_fingerprint);
            catalog
                .entries
                .push(LauncherEntryView::LabbyAction(LabbyActionLauncherEntry {
                    id: format!("labby:{}::{}", service.name, action.name),
                    label: format!("{} {}", service.name, action.name),
                    description: action.description.to_string(),
                    source: service.name.to_string(),
                    destructive: action.destructive,
                    input_schema,
                    schema_fingerprint,
                    service: service.name.to_string(),
                    action: action.name.to_string(),
                }));
        }
    }
}

async fn execute_labby_action(
    state: AppState,
    auth: Option<&AuthContext>,
    request: PaletteExecuteRequest,
) -> Result<Json<PaletteExecuteResponse>, ApiError> {
    let Some(auth) = auth else {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "auth_failed".to_string(),
            message: "palette routes require authenticated API context".to_string(),
        }));
    };
    let (service_name, action_name) = parse_labby_launcher_id(&request.id)?;
    let service = state
        .registry
        .service(service_name)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{}` was not found", request.id),
        })?;
    let action = service
        .actions
        .iter()
        .find(|action| action.name == action_name)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{}` was not found", request.id),
        })?;
    if !labby_action_visible(&state, service_name, action, Some(auth)) {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{}` was not found", request.id),
        }));
    }
    if let Some(manager) = &state.gateway_manager {
        if !manager
            .surface_enabled_for_service(service_name, "api")
            .await
        {
            return Err(ApiError(ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message: format!("service `{service_name}` is not enabled on the api surface"),
            }));
        }
    }
    if action_requires_admin(action) && !has_admin_scope(auth) {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: format!("action `{service_name}.{action_name}` requires admin scope"),
        }));
    }
    if action.destructive && !request.confirm_destructive {
        return Err(ApiError(ToolError::Sdk {
            sdk_kind: "confirmation_required".to_string(),
            message: format!("action `{service_name}.{action_name}` is destructive"),
        }));
    }
    validate_labby_action_params(action, &request.params)?;
    let result = (service.dispatch)(action_name.to_string(), request.params).await?;
    Ok(Json(PaletteExecuteResponse {
        id: request.id,
        result,
        ui: None,
    }))
}

fn parse_labby_launcher_id(id: &str) -> Result<(&str, &str), ToolError> {
    let rest = id.strip_prefix("labby:").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("launcher entry `{id}` was not found"),
    })?;
    let Some((service, action)) = rest.split_once("::") else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{id}` was not found"),
        });
    };
    if service.is_empty() || action.is_empty() || action.contains("::") {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{id}` was not found"),
        });
    }
    Ok((service, action))
}

fn labby_action_visible(
    state: &AppState,
    service: &str,
    action: &ActionSpec,
    auth: Option<&AuthContext>,
) -> bool {
    if action_requires_admin(action) && !auth.is_some_and(has_admin_scope) {
        return false;
    }
    if service == "setup"
        && setup_plugin_lifecycle_action(action.name)
        && !http_bind_is_loopback(state)
    {
        return false;
    }
    true
}

fn action_requires_admin(action: &ActionSpec) -> bool {
    action.requires_admin
}

fn has_admin_scope(auth: &AuthContext) -> bool {
    auth.scopes.iter().any(|scope| scope == "lab:admin")
}

fn setup_plugin_lifecycle_action(action: &str) -> bool {
    crate::dispatch::setup::PLUGIN_LIFECYCLE_ACTIONS.contains(&action)
}

fn http_bind_is_loopback(state: &AppState) -> bool {
    let host = state.http_bind_host.as_deref().map(String::as_str);
    let host = host.unwrap_or("127.0.0.1");
    let normalized = host.trim().trim_start_matches('[').trim_end_matches(']');
    matches!(normalized, "127.0.0.1" | "::1" | "localhost")
}

fn validate_labby_action_params(action: &ActionSpec, params: &Value) -> Result<(), ToolError> {
    let Some(map) = params.as_object() else {
        return Err(ToolError::Sdk {
            sdk_kind: "invalid_params".to_string(),
            message: "params must be a JSON object".to_string(),
        });
    };
    for param in action.params {
        let value = map.get(param.name);
        if param.required && value.is_none() {
            return Err(ToolError::Sdk {
                sdk_kind: "missing_param".to_string(),
                message: format!("missing required param `{}`", param.name),
            });
        }
        let Some(value) = value else {
            continue;
        };
        if !param_value_matches(param.ty, value) {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_params".to_string(),
                message: format!("param `{}` must be {}", param.name, param.ty),
            });
        }
    }
    Ok(())
}

fn param_value_matches(ty: &str, value: &Value) -> bool {
    match ty {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string[]" => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        "integer[]" => value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| item.as_i64().is_some() || item.as_u64().is_some())
        }),
        ty if ty.contains('|') => value
            .as_str()
            .is_some_and(|text| ty.split('|').any(|allowed| allowed == text)),
        _ => true,
    }
}

fn labby_action_schema(action: &ActionSpec) -> Option<Value> {
    if action.params.is_empty() {
        return None;
    }
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for param in action.params {
        properties.insert(param.name.to_string(), param_schema(param));
        if param.required {
            required.push(Value::String(param.name.to_string()));
        }
    }
    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }
    Some(Value::Object(schema))
}

fn param_schema(param: &ParamSpec) -> Value {
    let mut schema = match param.ty {
        "string" => json!({ "type": "string" }),
        "integer" => json!({ "type": "integer" }),
        "number" => json!({ "type": "number" }),
        "boolean" => json!({ "type": "boolean" }),
        "object" => json!({ "type": "object" }),
        "array" => json!({ "type": "array" }),
        "string[]" => json!({ "type": "array", "items": { "type": "string" } }),
        "integer[]" => json!({ "type": "array", "items": { "type": "integer" } }),
        ty if ty.contains('|') => {
            let values: Vec<Value> = ty
                .split('|')
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .collect();
            json!({ "type": "string", "enum": values })
        }
        _ => json!({ "type": "string" }),
    };
    if let Value::Object(map) = &mut schema {
        map.insert(
            "description".to_string(),
            Value::String(param.description.to_string()),
        );
    }
    schema
}

fn entry_id(entry: &LauncherEntryView) -> &str {
    match entry {
        LauncherEntryView::LabbyAction(entry) => &entry.id,
        LauncherEntryView::McpTool(entry) => &entry.id,
    }
}

fn catalog_fingerprint(entries: &[LauncherEntryView]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry_id(entry).as_bytes());
        hasher.update([0]);
        match entry {
            LauncherEntryView::LabbyAction(entry) => {
                if let Some(fp) = &entry.schema_fingerprint {
                    hasher.update(fp.as_bytes());
                }
            }
            LauncherEntryView::McpTool(entry) => {
                if let Some(fp) = &entry.schema_fingerprint {
                    hasher.update(fp.as_bytes());
                }
            }
        }
        hasher.update([0xff]);
    }
    hex_digest(hasher.finalize().as_slice())
}

fn stable_json_fingerprint(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    hex_digest(hasher.finalize().as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{append_labby_actions, entry_id};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use labby_gateway::gateway::palette::LauncherCatalogView;
    use labby_gateway::upstream::pool::UpstreamPool;
    use labby_gateway::upstream::types::{
        ToolExposurePolicy, UpstreamEntry, UpstreamHealth, UpstreamTool,
    };
    use labby_primitives::action::{ActionSpec, ParamSpec};
    use labby_runtime::gateway_config::{CodeModeConfig, GatewayConfig, UpstreamConfig};
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::api::oauth::AuthContext;
    use crate::api::router::build_router_with_bearer;
    use crate::api::state::AppState;
    use crate::dispatch::error::ToolError;
    use crate::dispatch::gateway::config_store::test_gateway_manager;
    use crate::dispatch::gateway::manager::GatewayRuntimeHandle;
    use crate::registry::{RegisteredService, ToolRegistry, build_default_registry};

    const TEST_ACTION_PARAMS: &[ParamSpec] = &[ParamSpec {
        name: "name",
        ty: "string",
        required: true,
        description: "Name to echo",
    }];

    const TEST_ACTIONS: &[ActionSpec] = &[
        ActionSpec {
            name: "echo.run",
            description: "Echo params",
            destructive: false,
            requires_admin: false,
            params: TEST_ACTION_PARAMS,
            returns: "object",
        },
        ActionSpec {
            name: "admin.run",
            description: "Admin echo",
            destructive: false,
            requires_admin: true,
            params: &[],
            returns: "object",
        },
    ];

    fn echo_dispatch(
        _action: String,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send>> {
        Box::pin(async move { Ok(params) })
    }

    fn test_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(RegisteredService::bootstrap_operator(
            "demo",
            "Demo service",
            "Test",
            TEST_ACTIONS,
            echo_dispatch,
        ));
        registry
    }

    fn test_upstream_config(name: &str) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: name.to_string(),
            url: None,
            bearer_token_env: None,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: Default::default(),
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

    fn healthy_upstream_entry(upstream: &str, tool_name: &str) -> UpstreamEntry {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tool = rmcp::model::Tool::new(
            tool_name.to_string(),
            format!("{tool_name} description"),
            Arc::new(serde_json::Map::new()),
        );
        UpstreamEntry {
            name: Arc::clone(&upstream_name),
            tools: std::collections::HashMap::from([(
                tool_name.to_string(),
                UpstreamTool {
                    tool,
                    input_schema: None,
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
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

    #[tokio::test]
    async fn palette_routes_not_mounted_without_api_auth() {
        let manager = Arc::new(test_gateway_manager(
            std::env::temp_dir().join("palette-no-auth.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let state = AppState::from_registry(build_default_registry()).with_gateway_manager(manager);
        let app = build_router_with_bearer(state, None, None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/palette/catalog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn palette_routes_not_mounted_without_gateway_manager() {
        let state = AppState::from_registry(build_default_registry());
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/palette/catalog")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn palette_catalog_requires_authenticated_request() {
        let manager = Arc::new(test_gateway_manager(
            std::env::temp_dir().join("palette-auth.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let state = AppState::from_registry(build_default_registry()).with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/palette/catalog")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn palette_catalog_returns_catalog_for_static_bearer_admin() {
        let manager = Arc::new(test_gateway_manager(
            std::env::temp_dir().join("palette-ok.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let state = AppState::from_registry(build_default_registry()).with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/palette/catalog")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert!(value.get("fingerprint").is_some());
        assert!(value.get("entries").and_then(Value::as_array).is_some());
    }

    #[tokio::test]
    async fn palette_catalog_includes_labby_registry_actions() {
        let manager = Arc::new(test_gateway_manager(
            std::env::temp_dir().join("palette-labby-catalog.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let state = AppState::from_registry(test_registry()).with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/palette/catalog")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let entries = value["entries"].as_array().unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry["id"] == "labby:demo::echo.run")
            .expect("labby action should be present");
        assert_eq!(entry["kind"], "labbyAction");
        assert_eq!(entry["inputSchema"]["required"][0], "name");
    }

    #[tokio::test]
    async fn palette_catalog_includes_configured_upstream_mcp_tools() {
        let runtime = GatewayRuntimeHandle::default();
        let pool = Arc::new(UpstreamPool::new());
        runtime.swap(Some(Arc::clone(&pool))).await;
        let manager = test_gateway_manager(
            std::env::temp_dir().join("palette-upstream-catalog.toml"),
            runtime,
        );
        manager
            .seed_config_unchecked_for_tests(GatewayConfig {
                code_mode: CodeModeConfig {
                    enabled: true,
                    ..CodeModeConfig::default()
                },
                upstream: vec![test_upstream_config("github")],
                ..GatewayConfig::default()
            })
            .await;
        pool.insert_entry_for_test("github", healthy_upstream_entry("github", "search_repos"))
            .await;

        let state =
            AppState::from_registry(test_registry()).with_gateway_manager(Arc::new(manager));
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/palette/catalog")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry["id"] == "labby:demo::echo.run"),
            "first-party Labby actions should remain in the launcher catalog"
        );
        let upstream = entries
            .iter()
            .find(|entry| entry["id"] == "mcp:github::search_repos")
            .expect("configured upstream MCP tool should be present");
        assert_eq!(upstream["kind"], "mcpTool");
        assert_eq!(upstream["source"], "github");
    }

    #[tokio::test]
    async fn palette_execute_dispatches_labby_registry_action() {
        let manager = Arc::new(test_gateway_manager(
            std::env::temp_dir().join("palette-labby-execute.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let state = AppState::from_registry(test_registry()).with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/palette/execute")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"id":"labby:demo::echo.run","params":{"name":"labby"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], "labby:demo::echo.run");
        assert_eq!(value["result"]["name"], "labby");
    }

    #[tokio::test]
    async fn palette_execute_validates_labby_action_params() {
        let manager = Arc::new(test_gateway_manager(
            std::env::temp_dir().join("palette-labby-validate.toml"),
            GatewayRuntimeHandle::default(),
        ));
        let state = AppState::from_registry(test_registry()).with_gateway_manager(manager);
        let app = build_router_with_bearer(state, Some("test-token".into()), None);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/palette/execute")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"id":"labby:demo::echo.run","params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["kind"], "missing_param");
    }

    #[tokio::test]
    async fn palette_catalog_hides_admin_labby_actions_from_non_admin_callers() {
        let state = AppState::from_registry(test_registry());
        let auth = AuthContext {
            sub: "user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let mut catalog = LauncherCatalogView {
            fingerprint: String::new(),
            entries: Vec::new(),
        };
        append_labby_actions(&mut catalog, &state, Some(&auth));

        assert!(
            catalog
                .entries
                .iter()
                .any(|entry| entry_id(entry) == "labby:demo::echo.run")
        );
        assert!(
            !catalog
                .entries
                .iter()
                .any(|entry| entry_id(entry) == "labby:demo::admin.run")
        );
    }

    #[tokio::test]
    async fn palette_catalog_hides_setup_plugin_lifecycle_actions_on_non_loopback_bind() {
        let mut state = AppState::from_registry(build_default_registry());
        state.http_bind_host = Some(Arc::new("0.0.0.0".to_string()));
        let auth = AuthContext {
            sub: "admin".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string(), "lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let mut catalog = LauncherCatalogView {
            fingerprint: String::new(),
            entries: Vec::new(),
        };
        append_labby_actions(&mut catalog, &state, Some(&auth));

        assert!(
            catalog
                .entries
                .iter()
                .any(|entry| entry_id(entry) == "labby:setup::state")
        );
        assert!(
            !catalog
                .entries
                .iter()
                .any(|entry| entry_id(entry) == "labby:setup::plugin.install")
        );
    }
}
