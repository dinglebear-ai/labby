//! Bounded protected-route HTTP forwarding and response adaptation.

use super::policy::{
    filter_protected_route_list_response, filter_protected_route_sse_stream,
    merge_protected_route_policy_errors, prepare_protected_route_request,
    protected_route_has_list_request, protected_route_policy_only_response,
    read_bounded_protected_response,
};
use crate::api::{error::ApiError, state::AppState};
use crate::config::ProtectedMcpRouteEffectiveTarget;
use crate::dispatch::{
    error::ToolError, gateway::SHARED_GATEWAY_OAUTH_SUBJECT,
    upstream::auth::configured_bearer_token,
};
use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request, StatusCode, header},
    response::IntoResponse,
};
use futures::StreamExt;
use std::time::Instant;

pub(super) async fn proxy_protected_mcp_route(
    state: &AppState,
    request: Request<Body>,
    route: crate::config::ProtectedMcpRouteConfig,
) -> axum::response::Response {
    let started = Instant::now();
    let suffix = request
        .uri()
        .path()
        .strip_prefix(&route.public_path)
        .unwrap_or("");

    let (mut upstream, upstream_auth_token, upstream_target, exposure_config) =
        match protected_route_upstream_target(state, &route).await {
            Ok(target) => target,
            Err(response) => return response,
        };

    let mut backend_path = upstream.path().trim_end_matches('/').to_string();
    if backend_path.is_empty() {
        backend_path.push('/');
    }
    if !suffix.is_empty() {
        if !backend_path.ends_with('/') {
            backend_path.push('/');
        }
        backend_path.push_str(suffix.trim_start_matches('/'));
    }
    upstream.set_path(&backend_path);
    upstream.set_query(request.uri().query());

    let method = request.method().clone();
    let original_path = request.uri().path().to_string();
    let headers = request.headers().clone();
    let body = match axum::body::to_bytes(request.into_body(), 50 * 1024 * 1024).await {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                method = %method,
                path = %original_path,
                error = %error,
                "protected MCP route proxy failed: request body read error"
            );
            return ApiError::new(ToolError::Sdk {
                sdk_kind: "bad_request".into(),
                message: format!("failed to read MCP request body: {error}"),
            })
            .into_response();
        }
    };
    let mut body_json = serde_json::from_slice::<serde_json::Value>(&body).ok();
    if exposure_config.is_some()
        && method == Method::POST
        && !body.is_empty()
        && body_json.is_none()
    {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            kind = "bad_request",
            "protected MCP route rejected malformed JSON before policy evaluation"
        );
        return ApiError::new(ToolError::Sdk {
            sdk_kind: "bad_request".into(),
            message: "protected MCP route request body must be valid JSON".into(),
        })
        .into_response();
    }
    let body_method = body_json.as_ref().and_then(|value| {
        let method = value.get("method")?.as_str()?;
        HeaderValue::from_str(method).ok()
    });
    let mut policy_errors = Vec::new();
    if let (Some(config), Some(request_json)) = (exposure_config.as_ref(), body_json.take()) {
        let prepared = prepare_protected_route_request(config, request_json);
        policy_errors = prepared.errors;
        let Some(forwarded) = prepared.forwarded else {
            return protected_route_policy_only_response(policy_errors);
        };
        body_json = Some(forwarded);
    }
    let outbound_body = body_json
        .as_ref()
        .and_then(|value| serde_json::to_vec(value).ok())
        .unwrap_or_else(|| body.to_vec());
    tracing::info!(
        route = %route.name,
        resource = %route.public_resource(),
        method = %method,
        path = %original_path,
        upstream = %upstream_target,
        upstream_auth = upstream_auth_token.is_some(),
        "protected MCP route proxy start"
    );
    let mut builder = state
        .protected_mcp_http_client
        .request(method.clone(), upstream);
    if let Some(token) = upstream_auth_token {
        builder = builder.bearer_auth(token);
    }
    for header_name in [
        header::ACCEPT,
        header::CONTENT_TYPE,
        HeaderName::from_static("mcp-protocol-version"),
        HeaderName::from_static("mcp-session-id"),
        HeaderName::from_static("last-event-id"),
    ] {
        if let Some(value) = headers.get(&header_name) {
            builder = builder.header(&header_name, value);
        }
    }
    if let Some(method) = body_method {
        builder = builder.header(HeaderName::from_static("mcp-method"), method);
    }
    let upstream_response = match builder.body(outbound_body).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                method = %method,
                path = %original_path,
                upstream = %upstream_target,
                elapsed_ms = started.elapsed().as_millis(),
                error = %error,
                "protected MCP route proxy failed: backend request failed"
            );
            return ApiError::new(ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: format!("protected MCP backend request failed: {error}"),
            })
            .into_response();
        }
    };
    let status = StatusCode::from_u16(upstream_response.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::info!(
        route = %route.name,
        resource = %route.public_resource(),
        method = %method,
        path = %original_path,
        upstream = %upstream_target,
        status = status.as_u16(),
        elapsed_ms = started.elapsed().as_millis(),
        "protected MCP route proxy finish"
    );
    let mut response = axum::response::Response::builder().status(status);
    for header_name in [
        header::CONTENT_TYPE,
        header::CACHE_CONTROL,
        HeaderName::from_static("mcp-session-id"),
        HeaderName::from_static("mcp-protocol-version"),
    ] {
        if let Some(value) = upstream_response.headers().get(&header_name) {
            response = response.header(&header_name, value);
        }
    }
    let is_sse = upstream_response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
    let response_body = if let (Some(config), Some(request_json)) =
        (exposure_config.as_ref(), body_json.as_ref())
        && protected_route_has_list_request(request_json)
    {
        if is_sse {
            let filtered = filter_protected_route_sse_stream(
                upstream_response.bytes_stream(),
                config.clone(),
                request_json.clone(),
            );
            let errors = futures::stream::iter(
                policy_errors
                    .iter()
                    .filter_map(|error| {
                        serde_json::to_string(error).ok().map(|json| {
                            Ok(bytes::Bytes::from(format!(
                                "event: message\ndata: {json}\n\n"
                            )))
                        })
                    })
                    .collect::<Vec<Result<bytes::Bytes, std::io::Error>>>(),
            );
            Body::from_stream(errors.chain(filtered))
        } else {
            match read_bounded_protected_response(upstream_response, 1024 * 1024).await {
                Ok(bytes) => {
                    match filter_protected_route_list_response(config, request_json, &bytes) {
                        Some(mut filtered) => {
                            merge_protected_route_policy_errors(&mut filtered, &policy_errors);
                            Body::from(filtered)
                        }
                        None => {
                            tracing::warn!(
                                route = %route.name,
                                upstream = %config.name,
                                kind = "bad_gateway",
                                "protected MCP route rejected malformed list response before exposure filtering"
                            );
                            return ApiError::new(ToolError::Sdk {
                                sdk_kind: "bad_gateway".into(),
                                message: "protected MCP backend returned an invalid list response"
                                    .into(),
                            })
                            .into_response();
                        }
                    }
                }
                Err(error) => {
                    return ApiError::new(ToolError::Sdk {
                        sdk_kind: "bad_gateway".into(),
                        message: format!("failed to read protected MCP response: {error}"),
                    })
                    .into_response();
                }
            }
        }
    } else if !policy_errors.is_empty() && !is_sse {
        match read_bounded_protected_response(upstream_response, 1024 * 1024).await {
            Ok(bytes) => {
                let mut body = if bytes.is_empty() {
                    response = response.status(StatusCode::OK);
                    serde_json::to_vec(&if policy_errors.len() == 1 {
                        policy_errors[0].clone()
                    } else {
                        serde_json::Value::Array(policy_errors.clone())
                    })
                    .unwrap_or_default()
                } else {
                    bytes.to_vec()
                };
                if !bytes.is_empty() {
                    let before = body.clone();
                    merge_protected_route_policy_errors(&mut body, &policy_errors);
                    if body == before {
                        return ApiError::new(ToolError::Sdk { sdk_kind: "bad_gateway".into(), message: "protected MCP backend returned malformed JSON while policy errors were pending".into() }).into_response();
                    }
                }
                Body::from(body)
            }
            Err(error) => {
                return ApiError::new(ToolError::Sdk {
                    sdk_kind: "bad_gateway".into(),
                    message: error.to_string(),
                })
                .into_response();
            }
        }
    } else {
        Body::from_stream(upstream_response.bytes_stream())
    };
    response.body(response_body).unwrap_or_else(|error| {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            elapsed_ms = started.elapsed().as_millis(),
            error = %error,
            "protected MCP route proxy failed: response build failed"
        );
        ApiError::new(ToolError::Sdk {
            sdk_kind: "bad_gateway".into(),
            message: format!("failed to build protected MCP response: {error}"),
        })
        .into_response()
    })
}

pub(super) async fn protected_route_upstream_target(
    state: &AppState,
    route: &crate::config::ProtectedMcpRouteConfig,
) -> Result<
    (
        reqwest::Url,
        Option<String>,
        String,
        Option<crate::config::UpstreamConfig>,
    ),
    axum::response::Response,
> {
    let upstream_name = match route.effective_target() {
        ProtectedMcpRouteEffectiveTarget::BackendUrl { url } => {
            let url = reqwest::Url::parse(&url).map_err(|error| {
                tracing::warn!(
                    route = %route.name,
                    resource = %route.public_resource(),
                    error = %error,
                    "protected MCP route proxy failed: invalid backend_url"
                );
                ApiError::new(ToolError::Sdk {
                    sdk_kind: "bad_gateway".into(),
                    message: format!("protected MCP route backend_url is invalid: {error}"),
                })
                .into_response()
            })?;
            return Ok((url, None, "backend_url".to_string(), None));
        }
        ProtectedMcpRouteEffectiveTarget::Upstream { name } => name,
        ProtectedMcpRouteEffectiveTarget::GatewaySubset(_) => {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                "protected MCP gateway subset reached legacy proxy path"
            );
            return Err(ApiError::new(ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: "gateway_subset routes must be served by the scoped MCP service".into(),
            })
            .into_response());
        }
    };

    let Some(manager) = state.gateway_manager.as_ref() else {
        tracing::error!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            "protected MCP route proxy failed: gateway manager missing"
        );
        return Err(ApiError::new(ToolError::Sdk {
            sdk_kind: "bad_gateway".into(),
            message: "gateway manager is not available for upstream protected route".into(),
        })
        .into_response());
    };
    let Some(upstream_config) = manager.upstream_config(&upstream_name).await else {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            "protected MCP route proxy failed: configured upstream not found"
        );
        return Err(ApiError::new(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("upstream `{upstream_name}` not found for protected MCP route"),
        })
        .into_response());
    };
    if !upstream_config.enabled
        || upstream_config.priority.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    {
        tracing::warn!(route = %route.name, upstream = %upstream_name, enabled = upstream_config.enabled, priority = upstream_config.priority, kind = "not_found", "protected MCP route target is not routable");
        return Err(ApiError::new(ToolError::Sdk {
            sdk_kind: "not_found".into(),
            message: format!("upstream `{upstream_name}` is not routable for protected MCP route"),
        })
        .into_response());
    }
    let Some(raw_url) = upstream_config.url.as_deref() else {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            "protected MCP route proxy failed: upstream has no HTTP URL"
        );
        return Err(ApiError::new(ToolError::Sdk {
            sdk_kind: "bad_gateway".into(),
            message: format!("upstream `{upstream_name}` does not have an HTTP MCP URL"),
        })
        .into_response());
    };
    let url = reqwest::Url::parse(raw_url).map_err(|error| {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            upstream = %upstream_name,
            error = %error,
            "protected MCP route proxy failed: invalid upstream URL"
        );
        StatusCode::BAD_GATEWAY.into_response()
    })?;

    let token = if upstream_config.oauth.is_some() {
        let Some(oauth_manager) = manager.upstream_oauth_manager(&upstream_name) else {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                upstream = %upstream_name,
                subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
                "protected MCP route proxy failed: upstream oauth manager missing"
            );
            return Err(ApiError::new(ToolError::Sdk {
                sdk_kind: "oauth_needs_reauth".into(),
                message: format!("upstream `{upstream_name}` is not connected with OAuth"),
            })
            .into_response());
        };
        let auth_client = oauth_manager
            .build_auth_client(SHARED_GATEWAY_OAUTH_SUBJECT)
            .await
            .map_err(|error| {
                tracing::warn!(
                    route = %route.name,
                    resource = %route.public_resource(),
                    upstream = %upstream_name,
                    subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
                    kind = error.kind(),
                    error = %error,
                    "protected MCP route proxy failed: upstream oauth auth client unavailable"
                );
                ApiError::new(ToolError::Sdk {
                    sdk_kind: error.kind().to_string(),
                    message: format!(
                        "upstream `{upstream_name}` OAuth authorization required: {error}"
                    ),
                })
                .into_response()
            })?;
        Some(auth_client.get_access_token().await.map_err(|error| {
            tracing::warn!(
                route = %route.name,
                resource = %route.public_resource(),
                upstream = %upstream_name,
                subject = %SHARED_GATEWAY_OAUTH_SUBJECT,
                error = %error,
                "protected MCP route proxy failed: upstream oauth token unavailable"
            );
            ApiError::new(ToolError::Sdk {
                sdk_kind: "oauth_needs_reauth".into(),
                message: format!("upstream `{upstream_name}` OAuth token unavailable: {error}"),
            })
            .into_response()
        })?)
    } else {
        upstream_config
            .bearer_token_env
            .as_deref()
            .and_then(configured_bearer_token)
    };

    Ok((
        url,
        token,
        format!("upstream:{upstream_name}"),
        Some(upstream_config),
    ))
}
