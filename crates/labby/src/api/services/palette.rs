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
    append_labby_actions(&mut catalog, &state);
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

fn append_labby_actions(catalog: &mut LauncherCatalogView, state: &AppState) {
    for service in state
        .registry
        .services()
        .iter()
        .filter(|service| service.status == "available")
        .filter(|service| state.enabled_services.contains(service.name))
    {
        for action in service.actions {
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
    if action.requires_admin && !auth.scopes.iter().any(|scope| scope == "lab:admin") {
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
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use labby_primitives::action::{ActionSpec, ParamSpec};
    use serde_json::Value;
    use tower::ServiceExt;

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

    const TEST_ACTIONS: &[ActionSpec] = &[ActionSpec {
        name: "echo.run",
        description: "Echo params",
        destructive: false,
        requires_admin: false,
        params: TEST_ACTION_PARAMS,
        returns: "object",
    }];

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
}
