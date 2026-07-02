//! The hardened outbound HTTP client. THIS is where the actual HTTP happens —
//! never `rmcp-openapi`'s executor.
//!
//! Every outbound call (spec fetch and operation dispatch) resolves the target
//! host, validates every resolved IP against `labby_primitives::ssrf`, pins ONE
//! validated address via `resolve_to_addrs`, and re-checks the connected peer IP
//! after the response — the workspace-canonical pattern from
//! `labby-apis::acp_registry::installer`. Redirects are OFF and `https_only` is
//! ON so a 3xx can never bounce the request to an internal address.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::error::OpenApiError;
use crate::registry::OperationHandle;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Base hardened builder shared by every outbound client: redirects off,
/// https-only, explicit connect/read timeouts, no proxy.
fn base_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(PER_CALL_TIMEOUT)
}

/// Client used to fetch spec documents at load time. The per-request SSRF pin is
/// applied in [`fetch_url_capped`], so this is a plain hardened base client.
#[must_use]
pub fn build_spec_fetch_client() -> reqwest::Client {
    base_builder().build().expect("build spec fetch client")
}

/// Client used for operation dispatch. Same hardening; the per-request pin is
/// applied in [`execute_operation`]. Kept as a shared handle on the host/runner.
#[must_use]
pub fn build_dispatch_client() -> reqwest::Client {
    base_builder().build().expect("build dispatch client")
}

/// Resolve `host:port`, validate every resolved IP, and return them so a single
/// one can be pinned. Empty resolution and any private/blocked address are hard
/// failures.
async fn resolve_and_validate(host: &str, port: u16) -> Result<Vec<SocketAddr>, OpenApiError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| OpenApiError::RequestBlockedPrivateAddr {
            label: String::new(),
        })?
        .collect();
    if addrs.is_empty() {
        return Err(OpenApiError::RequestBlockedPrivateAddr {
            label: String::new(),
        });
    }
    for addr in &addrs {
        labby_primitives::ssrf::check_ip_not_private(addr.ip(), host).map_err(|_| {
            OpenApiError::RequestBlockedPrivateAddr {
                label: String::new(),
            }
        })?;
    }
    Ok(addrs)
}

/// Build a per-request client that pins exactly ONE validated address for the
/// URL's host. Returns the client and the pinned IP for the post-connect
/// re-check. Shrinks the TOCTOU window: reqwest can only connect to the address
/// we validated.
async fn pinned_client_for(
    template: &reqwest::Client,
    url: &url::Url,
) -> Result<(reqwest::Client, IpAddr), OpenApiError> {
    // `template` carries the hardened config; we clone its policy by rebuilding
    // from `base_builder()` (reqwest has no builder-from-client). The template
    // arg keeps the call sites honest about which logical client is in use.
    let _ = template;
    let host = url
        .host_str()
        .ok_or_else(|| OpenApiError::RequestBlockedPrivateAddr {
            label: String::new(),
        })?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = resolve_and_validate(host, port).await?;
    let pinned = addrs[0];
    let client = base_builder()
        .resolve_to_addrs(host, &[pinned])
        .build()
        .map_err(|_| OpenApiError::UpstreamRequest {
            label: String::new(),
        })?;
    Ok((client, pinned.ip()))
}

/// Reject a connected peer whose IP does not match the validated pin. This is the
/// load-bearing DNS-rebinding / redirect-to-internal defense.
fn recheck_peer(resp: &reqwest::Response, pinned: IpAddr) -> Result<(), OpenApiError> {
    if let Some(peer) = resp.remote_addr() {
        if peer.ip() != pinned {
            return Err(OpenApiError::RequestBlockedPrivateAddr {
                label: String::new(),
            });
        }
        labby_primitives::ssrf::check_ip_not_private(peer.ip(), "openapi peer").map_err(|_| {
            OpenApiError::RequestBlockedPrivateAddr {
                label: String::new(),
            }
        })?;
    }
    Ok(())
}

/// Read a response body into a `String`, aborting past `cap` bytes. Uses
/// `Response::chunk()` so no `bytes::Bytes` type name is needed.
async fn collect_capped(
    mut resp: reqwest::Response,
    cap: usize,
    label: &str,
) -> Result<String, OpenApiError> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = resp.chunk().await.map_err(|_| OpenApiError::UpstreamRequest {
            label: label.to_string(),
        })?;
        match chunk {
            Some(bytes) => {
                if buf.len() + bytes.len() > cap {
                    return Err(OpenApiError::SpecTooLarge {
                        label: label.to_string(),
                    });
                }
                buf.extend_from_slice(&bytes);
            }
            None => break,
        }
    }
    String::from_utf8(buf).map_err(|_| OpenApiError::SpecParse {
        label: label.to_string(),
    })
}

/// GET `url` with the hardened pinned client, capping the body at `cap` bytes.
/// Used for spec fetch at load time.
pub async fn fetch_url_capped(
    client: &reqwest::Client,
    url: &url::Url,
    cap: usize,
    label: &str,
) -> Result<String, OpenApiError> {
    let (pinned_client, pinned_ip) = pinned_client_for(client, url).await?;
    let resp = pinned_client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| map_send_err(e, label))?;
    recheck_peer(&resp, pinned_ip).map_err(|_| OpenApiError::RequestBlockedPrivateAddr {
        label: label.to_string(),
    })?;
    if !resp.status().is_success() {
        return Err(OpenApiError::SpecParse {
            label: label.to_string(),
        });
    }
    collect_capped(resp, cap, label).await
}

/// Map a `reqwest` send error to a scrubbed `OpenApiError`. NEVER embeds the
/// reqwest error's `Display`.
fn map_send_err(e: reqwest::Error, label: &str) -> OpenApiError {
    if e.is_timeout() {
        OpenApiError::UpstreamTimeout {
            label: label.to_string(),
        }
    } else if e.is_connect() {
        OpenApiError::RequestBlockedPrivateAddr {
            label: label.to_string(),
        }
    } else {
        OpenApiError::UpstreamRequest {
            label: label.to_string(),
        }
    }
}

/// Body cap for an operation response (16 MiB — generous for JSON APIs, still
/// bounded against a hostile upstream).
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Execute one operation against the hardened, per-request-pinned client.
///
/// Path params are substituted (PATH_SEGMENT-encoded) from `params`; the
/// remaining top-level params become the query string (GET/HEAD/DELETE) or the
/// JSON request body (POST/PUT/PATCH). The credential is injected server-side —
/// the caller's `params` never carry it. Non-2xx and body-decode failures map to
/// `UpstreamRequest` WITHOUT including the response body.
///
/// # Errors
/// Returns a scrubbed [`OpenApiError`] on SSRF rejection, timeout, transport
/// failure, or a non-success status.
pub async fn execute_operation(
    client: &reqwest::Client,
    op: &OperationHandle,
    params: serde_json::Value,
) -> Result<serde_json::Value, OpenApiError> {
    // Production always enforces the per-request SSRF pin + peer re-check.
    execute_operation_inner(client, op, params, true).await
}

/// Shared implementation. When `enforce_ssrf` is `true` (production) the request
/// is sent through a freshly built, single-address-pinned hardened client and the
/// connected peer IP is re-validated. When `false` (TEST-ONLY — used by the
/// dispatch tests against a loopback wiremock server) the supplied `client` is
/// used directly with no resolve/pin/recheck, so the SSRF guard does not block
/// 127.0.0.1. The guard itself is unit-tested in `tests_ssrf.rs` / `http.rs`.
async fn execute_operation_inner(
    client: &reqwest::Client,
    op: &OperationHandle,
    params: serde_json::Value,
    enforce_ssrf: bool,
) -> Result<serde_json::Value, OpenApiError> {
    let (used_path_params, url) = build_url_with_params(op, &params)?;

    let (send_client, pinned_ip) = if enforce_ssrf {
        let (c, ip) = pinned_client_for(client, &url).await?;
        (c, Some(ip))
    } else {
        (client.clone(), None)
    };

    let url = apply_query(url, op, &params, &used_path_params);
    let mut req = send_client.request(op.method.clone(), url);
    req = inject_credential(req, op);
    req = apply_body(req, op, &params, &used_path_params);

    let resp = req
        .send()
        .await
        .map_err(|e| map_send_err(e, &op_label(op)))?;
    if let Some(pinned_ip) = pinned_ip {
        recheck_peer(&resp, pinned_ip).map_err(|_| OpenApiError::RequestBlockedPrivateAddr {
            label: op_label(op),
        })?;
    }

    let status = resp.status();
    if !status.is_success() {
        // Do NOT include the body — it may carry the upstream response body.
        return Err(OpenApiError::UpstreamRequest {
            label: op_label(op),
        });
    }
    let body = collect_capped(resp, MAX_RESPONSE_BYTES, &op_label(op)).await?;
    if body.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&body).map_err(|_| OpenApiError::UpstreamRequest {
        label: op_label(op),
    })
}

/// TEST-ONLY: dispatch an operation without the SSRF pin so a loopback wiremock
/// server is reachable. Never compiled into a release build.
#[cfg(test)]
pub(crate) async fn execute_operation_no_ssrf(
    client: &reqwest::Client,
    op: &OperationHandle,
    params: serde_json::Value,
) -> Result<serde_json::Value, OpenApiError> {
    execute_operation_inner(client, op, params, false).await
}

/// Operation-scoped label placeholder. The `OperationHandle` does not store its
/// label (the registry key holds it), so error labels use the operationId — which
/// is non-secret and sufficient for attribution.
fn op_label(op: &OperationHandle) -> String {
    op.operation_id.clone()
}

/// Substitute `{name}` path params from a flat params object, PATH_SEGMENT-encoded.
/// Returns the resolved URL and the set of param keys consumed as path segments
/// (so they are not also sent as query/body).
fn build_url_with_params(
    op: &OperationHandle,
    params: &serde_json::Value,
) -> Result<(Vec<String>, url::Url), OpenApiError> {
    let mut consumed = Vec::new();
    let mut path = String::with_capacity(op.path_template.len());
    let mut chars = op.path_template.chars();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for nc in chars.by_ref() {
                if nc == '}' {
                    break;
                }
                name.push(nc);
            }
            let value = params
                .get(&name)
                .map(json_scalar_to_string)
                .ok_or_else(|| OpenApiError::UpstreamRequest {
                    label: op.operation_id.clone(),
                })?;
            let encoded: String =
                url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
            // form-urlencoded uses '+' for space; path segments want %20.
            path.push_str(&encoded.replace('+', "%20"));
            consumed.push(name);
        } else {
            path.push(c);
        }
    }
    // Ensure the base path ends with '/' so `join` appends rather than replaces
    // the last segment (mirrors rmcp-openapi's `with_base_url`).
    let mut base = op.base_url.clone();
    if !base.path().ends_with('/') {
        let with_slash = format!("{}/", base.path());
        base.set_path(&with_slash);
    }
    let joined =
        base.join(path.trim_start_matches('/'))
            .map_err(|_| OpenApiError::UpstreamRequest {
                label: op.operation_id.clone(),
            })?;
    Ok((consumed, joined))
}

fn json_scalar_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Inject the server-side credential. Header-style only in v1.
fn inject_credential(req: reqwest::RequestBuilder, op: &OperationHandle) -> reqwest::RequestBuilder {
    match &op.credential {
        Some(crate::config::OpenApiCredential::BearerToken(token)) => req.bearer_auth(token),
        Some(crate::config::OpenApiCredential::ApiKey { header, value }) => req.header(header, value),
        None => req,
    }
}

/// The non-path params that are not consumed by the path template.
fn remaining_params(
    params: &serde_json::Value,
    used_path_params: &[String],
) -> serde_json::Map<String, serde_json::Value> {
    match params {
        serde_json::Value::Object(map) => map
            .iter()
            .filter(|(k, _)| !used_path_params.contains(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        _ => serde_json::Map::new(),
    }
}

fn is_body_method(method: &reqwest::Method) -> bool {
    matches!(
        *method,
        reqwest::Method::POST | reqwest::Method::PUT | reqwest::Method::PATCH
    )
}

/// For safe methods, fold the remaining params into the URL query string.
/// (Built manually via `url::Url::query_pairs_mut` — no reqwest `query` feature.)
fn apply_query(
    mut url: url::Url,
    op: &OperationHandle,
    params: &serde_json::Value,
    used_path_params: &[String],
) -> url::Url {
    if is_body_method(&op.method) {
        return url;
    }
    let remaining = remaining_params(params, used_path_params);
    if remaining.is_empty() {
        return url;
    }
    {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in &remaining {
            pairs.append_pair(k, &json_scalar_to_string(v));
        }
    }
    url
}

/// For mutating methods, attach the remaining params as a JSON body.
fn apply_body(
    req: reqwest::RequestBuilder,
    op: &OperationHandle,
    params: &serde_json::Value,
    used_path_params: &[String],
) -> reqwest::RequestBuilder {
    if !is_body_method(&op.method) {
        return req;
    }
    let remaining = remaining_params(params, used_path_params);
    if remaining.is_empty() {
        return req;
    }
    req.json(&serde_json::Value::Object(remaining))
}
