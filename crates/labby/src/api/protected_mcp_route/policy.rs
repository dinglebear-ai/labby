//! JSON-RPC exposure decisions and bounded response/SSE filtering.

use axum::{Json, http::StatusCode, response::IntoResponse};
use futures::StreamExt;

#[derive(Debug)]
pub(in crate::api::router) struct ProtectedRouteExposureDenial {
    capability: &'static str,
    item: String,
}

pub(in crate::api::router) enum ProtectedRouteExposureDecision {
    NotApplicable,
    Allowed,
    Denied(ProtectedRouteExposureDenial),
    Malformed { capability: &'static str },
}

pub(in crate::api::router) struct PreparedProtectedRouteRequest {
    pub(in crate::api::router) forwarded: Option<serde_json::Value>,
    pub(in crate::api::router) errors: Vec<serde_json::Value>,
}

pub(in crate::api::router) fn prepare_protected_route_request(
    config: &crate::config::UpstreamConfig,
    request: serde_json::Value,
) -> PreparedProtectedRouteRequest {
    let is_batch = request.is_array();
    let members = match request {
        serde_json::Value::Array(items) => items,
        other => vec![other],
    };
    let mut forwarded = Vec::new();
    let mut errors = Vec::new();
    for member in members {
        let id = member.get("id").cloned();
        match protected_route_exposure_decision(config, &member) {
            ProtectedRouteExposureDecision::NotApplicable
            | ProtectedRouteExposureDecision::Allowed => forwarded.push(member),
            ProtectedRouteExposureDecision::Denied(denial) => {
                if let Some(id) = id {
                    errors.push(protected_route_json_rpc_error(
                        id,
                        -32601,
                        "route_exposure_denied",
                        denial.capability,
                        format!(
                            "{} `{}` is not exposed by this route",
                            denial.capability, denial.item
                        ),
                    ));
                }
            }
            ProtectedRouteExposureDecision::Malformed { capability } => {
                if let Some(id) = id {
                    errors.push(protected_route_json_rpc_error(
                        id,
                        -32602,
                        "invalid_params",
                        capability,
                        format!("{capability} request has a missing or invalid selector"),
                    ));
                }
            }
        }
    }
    let forwarded = if forwarded.is_empty() {
        None
    } else if is_batch {
        Some(serde_json::Value::Array(forwarded))
    } else {
        forwarded.pop()
    };
    PreparedProtectedRouteRequest { forwarded, errors }
}

pub(in crate::api::router) fn protected_route_json_rpc_error(
    id: serde_json::Value,
    code: i32,
    kind: &str,
    capability: &str,
    message: String,
) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message, "data":{"kind":kind, "capability":capability}}})
}

pub(super) fn protected_route_policy_only_response(
    errors: Vec<serde_json::Value>,
) -> axum::response::Response {
    if errors.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }
    let body = if errors.len() == 1 {
        errors.into_iter().next().expect("one error")
    } else {
        serde_json::Value::Array(errors)
    };
    (StatusCode::OK, Json(body)).into_response()
}

pub(super) fn merge_protected_route_policy_errors(
    body: &mut Vec<u8>,
    errors: &[serde_json::Value],
) {
    if errors.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return;
    };
    let mut combined = match value {
        serde_json::Value::Array(items) => items,
        other => vec![other],
    };
    combined.extend_from_slice(errors);
    if let Ok(encoded) = serde_json::to_vec(&serde_json::Value::Array(combined)) {
        *body = encoded;
    }
}

pub(super) async fn read_bounded_protected_response(
    response: reqwest::Response,
    max: usize,
) -> Result<bytes::Bytes, String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if body.len().saturating_add(chunk.len()) > max {
            return Err(format!("protected MCP response exceeds {max} byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(bytes::Bytes::from(body))
}

pub(in crate::api::router) fn filter_protected_route_sse_stream(
    stream: impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + Unpin + 'static,
    config: crate::config::UpstreamConfig,
    request: serde_json::Value,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send {
    const MAX_EVENT_BYTES: usize = 1024 * 1024;
    futures::stream::unfold(
        (stream.boxed(), Vec::<u8>::new(), false),
        move |(mut stream, mut buffer, done)| {
            let config = config.clone();
            let request = request.clone();
            async move {
                loop {
                    if let Some(end) = find_sse_event_end(&buffer) {
                        if end > MAX_EVENT_BYTES {
                            buffer.clear();
                            return Some((
                                Err(std::io::Error::other(
                                    "protected MCP SSE event exceeds 1 MiB",
                                )),
                                (stream, buffer, true),
                            ));
                        }
                        let event = buffer.drain(..end).collect::<Vec<_>>();
                        return Some((
                            filter_protected_route_sse_event(&config, &request, &event),
                            (stream, buffer, done),
                        ));
                    }
                    if done {
                        if buffer.is_empty() {
                            return None;
                        }
                        let event = std::mem::take(&mut buffer);
                        if event.len() > MAX_EVENT_BYTES {
                            return Some((
                                Err(std::io::Error::other(
                                    "protected MCP SSE event exceeds 1 MiB",
                                )),
                                (stream, buffer, true),
                            ));
                        }
                        return Some((
                            filter_protected_route_sse_event(&config, &request, &event),
                            (stream, buffer, true),
                        ));
                    }
                    match stream.next().await {
                        Some(Ok(chunk)) => {
                            buffer.extend_from_slice(&chunk);
                            if find_sse_event_end(&buffer).is_none()
                                && buffer.len() > MAX_EVENT_BYTES
                            {
                                buffer.clear();
                                return Some((
                                    Err(std::io::Error::other(
                                        "protected MCP SSE event exceeds 1 MiB",
                                    )),
                                    (stream, buffer, true),
                                ));
                            }
                        }
                        Some(Err(error)) => {
                            return Some((
                                Err(std::io::Error::other(error)),
                                (stream, buffer, true),
                            ));
                        }
                        None => {
                            if buffer.is_empty() {
                                return None;
                            }
                            let event = std::mem::take(&mut buffer);
                            if event.len() > MAX_EVENT_BYTES {
                                return Some((
                                    Err(std::io::Error::other(
                                        "protected MCP SSE event exceeds 1 MiB",
                                    )),
                                    (stream, buffer, true),
                                ));
                            }
                            return Some((
                                filter_protected_route_sse_event(&config, &request, &event),
                                (stream, buffer, true),
                            ));
                        }
                    }
                }
            }
        },
    )
}

pub(in crate::api::router) fn find_sse_event_end(buffer: &[u8]) -> Option<usize> {
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| index + 2);
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4);
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(end), None) | (None, Some(end)) => Some(end),
        (None, None) => None,
    }
}

pub(in crate::api::router) fn filter_protected_route_sse_event(
    config: &crate::config::UpstreamConfig,
    request: &serde_json::Value,
    event: &[u8],
) -> Result<bytes::Bytes, std::io::Error> {
    let text = std::str::from_utf8(event).map_err(std::io::Error::other)?;
    let mut output = String::new();
    let mut data_lines = Vec::new();
    for line in text.split_inclusive('\n') {
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().trim_end_matches(['\r', '\n']));
        } else {
            output.push_str(line);
        }
    }
    if !data_lines.is_empty() {
        let data = data_lines.join("\n");
        let filtered = filter_protected_route_list_response(config, request, data.as_bytes())
            .ok_or_else(|| std::io::Error::other("invalid protected MCP SSE JSON payload"))?;
        output = output.trim_end_matches(['\r', '\n']).to_string();
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("data: ");
        output.push_str(std::str::from_utf8(&filtered).map_err(std::io::Error::other)?);
        output.push_str("\n\n");
    }
    Ok(bytes::Bytes::from(output))
}

pub(in crate::api::router) fn protected_route_exposure_decision(
    config: &crate::config::UpstreamConfig,
    request: &serde_json::Value,
) -> ProtectedRouteExposureDecision {
    use labby_gateway::upstream::pool::entries::{
        prompt_exposed, resolve_request_exposure_policy, resolve_request_prompt_exposure_policy,
        resolve_request_resource_exposure_policy, resource_exposed,
    };
    let Some(method) = request.get("method").and_then(serde_json::Value::as_str) else {
        return ProtectedRouteExposureDecision::NotApplicable;
    };
    let capability = match method {
        "tools/call" => "tools",
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => "resources",
        "prompts/get" => "prompts",
        "completion/complete" => "completion",
        _ => return ProtectedRouteExposureDecision::NotApplicable,
    };
    let Some(params) = request.get("params").and_then(serde_json::Value::as_object) else {
        return ProtectedRouteExposureDecision::Malformed { capability };
    };
    let (capability, item, exposed) = match method {
        "tools/call" => {
            let Some(item) = params.get("name").and_then(serde_json::Value::as_str) else {
                return ProtectedRouteExposureDecision::Malformed { capability };
            };
            let policy = resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
            ("tools", item, policy.matches(item))
        }
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => {
            let Some(item) = params.get("uri").and_then(serde_json::Value::as_str) else {
                return ProtectedRouteExposureDecision::Malformed { capability };
            };
            let policy = resolve_request_resource_exposure_policy(
                &config.name,
                config.expose_resources.clone(),
            );
            ("resources", item, resource_exposed(&policy, item))
        }
        "prompts/get" => {
            let Some(item) = params.get("name").and_then(serde_json::Value::as_str) else {
                return ProtectedRouteExposureDecision::Malformed { capability };
            };
            let policy =
                resolve_request_prompt_exposure_policy(&config.name, config.expose_prompts.clone());
            ("prompts", item, prompt_exposed(&policy, &config.name, item))
        }
        "completion/complete" => {
            let Some(reference) = params.get("ref").and_then(serde_json::Value::as_object) else {
                return ProtectedRouteExposureDecision::Malformed { capability };
            };
            let Some(reference_type) = reference.get("type").and_then(serde_json::Value::as_str)
            else {
                return ProtectedRouteExposureDecision::Malformed { capability };
            };
            match reference_type {
                "ref/prompt" => {
                    let Some(item) = reference.get("name").and_then(serde_json::Value::as_str)
                    else {
                        return ProtectedRouteExposureDecision::Malformed { capability };
                    };
                    let policy = resolve_request_prompt_exposure_policy(
                        &config.name,
                        config.expose_prompts.clone(),
                    );
                    ("prompts", item, prompt_exposed(&policy, &config.name, item))
                }
                "ref/resource" => {
                    let Some(item) = reference.get("uri").and_then(serde_json::Value::as_str)
                    else {
                        return ProtectedRouteExposureDecision::Malformed { capability };
                    };
                    let policy = resolve_request_resource_exposure_policy(
                        &config.name,
                        config.expose_resources.clone(),
                    );
                    ("resources", item, resource_exposed(&policy, item))
                }
                _ => return ProtectedRouteExposureDecision::Malformed { capability },
            }
        }
        _ => unreachable!(),
    };
    if exposed {
        ProtectedRouteExposureDecision::Allowed
    } else {
        ProtectedRouteExposureDecision::Denied(ProtectedRouteExposureDenial {
            capability,
            item: item.to_string(),
        })
    }
}

pub(super) fn protected_route_has_list_request(request: &serde_json::Value) -> bool {
    request
        .as_array()
        .is_some_and(|batch| batch.iter().any(protected_route_has_list_request))
        || request
            .get("method")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|method| {
                matches!(method, "tools/list" | "resources/list" | "prompts/list")
            })
}

pub(in crate::api::router) fn filter_protected_route_list_response(
    config: &crate::config::UpstreamConfig,
    request: &serde_json::Value,
    bytes: &[u8],
) -> Option<Vec<u8>> {
    let mut response = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    if let (Some(requests), Some(responses)) = (request.as_array(), response.as_array_mut()) {
        let request_ids = requests
            .iter()
            .filter_map(|request| request.get("id"))
            .collect::<Vec<_>>();
        if responses.iter().any(|response| {
            response
                .get("id")
                .is_none_or(|id| !request_ids.contains(&id))
        }) {
            return None;
        }
        for req in requests {
            if let Some(id) = req.get("id")
                && protected_route_has_list_request(req)
            {
                let matches = responses
                    .iter_mut()
                    .filter(|candidate| candidate.get("id") == Some(id))
                    .collect::<Vec<_>>();
                if matches.len() != 1 {
                    return None;
                }
                if !filter_protected_route_list_result(
                    config,
                    req,
                    matches.into_iter().next().expect("one response"),
                ) {
                    return None;
                }
            }
        }
    } else if let Some(requests) = request.as_array() {
        if let Some(id) = response.get("id")
            && let Some(req) = requests.iter().find(|req| req.get("id") == Some(id))
        {
            if !filter_protected_route_list_result(config, req, &mut response) {
                return None;
            }
        } else {
            return None;
        }
    } else if !filter_protected_route_list_result(config, request, &mut response) {
        return None;
    }
    serde_json::to_vec(&response).ok()
}

pub(super) fn filter_protected_route_list_result(
    config: &crate::config::UpstreamConfig,
    req: &serde_json::Value,
    response: &mut serde_json::Value,
) -> bool {
    let Some(method) = req.get("method").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let Some(result) = response.get_mut("result") else {
        return false;
    };
    let (key, policy): (&str, Box<dyn Fn(&serde_json::Value) -> bool>) = match method {
        "tools/list" => {
            let policy = labby_gateway::upstream::pool::entries::resolve_request_exposure_policy(
                &config.name,
                config.expose_tools.clone(),
            );
            (
                "tools",
                Box::new(move |item| {
                    item.get("name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|name| policy.matches(name))
                }),
            )
        }
        "resources/list" => {
            let policy =
                labby_gateway::upstream::pool::entries::resolve_request_resource_exposure_policy(
                    &config.name,
                    config.expose_resources.clone(),
                );
            (
                "resources",
                Box::new(move |item| {
                    item.get("uri")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|uri| {
                            labby_gateway::upstream::pool::entries::resource_exposed(&policy, uri)
                        })
                }),
            )
        }
        "prompts/list" => {
            let policy =
                labby_gateway::upstream::pool::entries::resolve_request_prompt_exposure_policy(
                    &config.name,
                    config.expose_prompts.clone(),
                );
            let name = config.name.clone();
            (
                "prompts",
                Box::new(move |item| {
                    item.get("name")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|prompt| {
                            labby_gateway::upstream::pool::entries::prompt_exposed(
                                &policy, &name, prompt,
                            )
                        })
                }),
            )
        }
        _ => return false,
    };
    let Some(items) = result
        .get_mut(key)
        .and_then(serde_json::Value::as_array_mut)
    else {
        return false;
    };
    items.retain(policy);
    true
}
