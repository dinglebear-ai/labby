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
/// Bound the standalone DNS resolution (`lookup_host`) — reqwest's
/// `connect_timeout` only covers the subsequent TCP connect, not this lookup, so
/// a slow/blackholed resolver could otherwise stall a dispatch past the budget.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Client used for operation dispatch. Same hardening; the per-request pin is
/// applied in [`execute_operation`]. Kept as a shared handle on the host/runner.
///
/// # Errors
/// [`OpenApiError::ClientBuildFailed`] on catastrophic TLS/root-store init
/// failure — fallible per the workspace `HttpClient::new()` convention.
pub fn build_dispatch_client() -> Result<reqwest::Client, OpenApiError> {
    base_builder()
        .build()
        .map_err(|_| OpenApiError::ClientBuildFailed)
}

/// Resolve `host:port` (bounded by `RESOLVE_TIMEOUT`), validate every resolved
/// IP, and return them so a single one can be pinned. A resolution failure or
/// empty result is [`OpenApiError::ResolveFailed`] (a down/flaky upstream — NOT
/// an SSRF misconfig); any private/blocked address is
/// [`OpenApiError::RequestBlockedPrivateAddr`].
async fn resolve_and_validate(
    host: &str,
    port: u16,
    label: &str,
) -> Result<Vec<SocketAddr>, OpenApiError> {
    // `url::Url::host_str()` returns the BRACKETED form for an IPv6 literal
    // (`[2606:4700::1111]`), but `tokio::net::lookup_host` only accepts the bare
    // form — without stripping, every IPv6-literal base/spec URL would fail with
    // ResolveFailed. `check_host_not_private`/`resolve_to_addrs` still see the
    // original host string.
    let lookup_host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    let addrs: Vec<SocketAddr> = tokio::time::timeout(
        RESOLVE_TIMEOUT,
        tokio::net::lookup_host((lookup_host, port)),
    )
    .await
    .map_err(|_| OpenApiError::ResolveFailed {
        label: label.to_string(),
    })?
    .map_err(|_| OpenApiError::ResolveFailed {
        label: label.to_string(),
    })?
    .collect();
    if addrs.is_empty() {
        return Err(OpenApiError::ResolveFailed {
            label: label.to_string(),
        });
    }
    for addr in &addrs {
        labby_primitives::ssrf::check_ip_not_private(addr.ip(), host).map_err(|_| {
            OpenApiError::RequestBlockedPrivateAddr {
                label: label.to_string(),
            }
        })?;
    }
    Ok(addrs)
}

/// Build a per-request client that pins exactly ONE validated address for the
/// URL's host. Returns the client and the pinned IP for the post-connect
/// re-check. Shrinks the TOCTOU window: reqwest can only connect to the address
/// we validated.
///
/// A fresh client is built per request because `resolve_to_addrs` pins the
/// validated address at *build* time; reqwest exposes no builder-from-client, so
/// there is no shared client to reuse here. (Connection pooling across calls is a
/// documented v1 follow-up — it requires a custom `reqwest::dns::Resolve` on a
/// shared client instead of per-call `resolve_to_addrs`.)
async fn pinned_client_for(
    url: &url::Url,
    label: &str,
) -> Result<(reqwest::Client, IpAddr), OpenApiError> {
    let host = url.host_str().ok_or_else(|| OpenApiError::ResolveFailed {
        label: label.to_string(),
    })?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = resolve_and_validate(host, port, label).await?;
    let pinned = addrs[0];
    let client = base_builder()
        .resolve_to_addrs(host, &[pinned])
        .build()
        .map_err(|_| OpenApiError::ClientBuildFailed)?;
    Ok((client, pinned.ip()))
}

/// Reject a connected peer whose IP does not match the validated pin. The
/// per-request `resolve_to_addrs` pin is the primary DNS-rebinding /
/// redirect-to-internal defense (reqwest can only connect to the one validated
/// address); this post-connect re-check is the backstop. It FAILS CLOSED: an
/// absent peer address (`remote_addr() == None`) is treated as a rejection
/// rather than silently accepted, so the guard cannot degrade to a no-op if the
/// client's connector ever changes.
fn recheck_peer(resp: &reqwest::Response, pinned: IpAddr, label: &str) -> Result<(), OpenApiError> {
    let peer = resp
        .remote_addr()
        .ok_or_else(|| OpenApiError::RequestBlockedPrivateAddr {
            label: label.to_string(),
        })?;
    if peer.ip() != pinned {
        return Err(OpenApiError::RequestBlockedPrivateAddr {
            label: label.to_string(),
        });
    }
    labby_primitives::ssrf::check_ip_not_private(peer.ip(), "openapi peer").map_err(|_| {
        OpenApiError::RequestBlockedPrivateAddr {
            label: label.to_string(),
        }
    })?;
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
        let chunk = resp
            .chunk()
            .await
            .map_err(|_| OpenApiError::UpstreamRequest {
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

/// GET `url` with a hardened, per-request-pinned client, capping the body at
/// `cap` bytes. Used for spec fetch at load time. The client is built fresh here
/// (the SSRF pin is applied at build time — see [`pinned_client_for`]), so there
/// is no shared client to thread in.
pub async fn fetch_url_capped(
    url: &url::Url,
    cap: usize,
    label: &str,
) -> Result<String, OpenApiError> {
    let (pinned_client, pinned_ip) = pinned_client_for(url, label).await?;
    let resp = pinned_client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| map_send_err(e, label))?;
    recheck_peer(&resp, pinned_ip, label)?;
    if !resp.status().is_success() {
        return Err(OpenApiError::SpecParse {
            label: label.to_string(),
        });
    }
    collect_capped(resp, cap, label).await
}

/// Map a `reqwest` send error to a scrubbed `OpenApiError`. NEVER embeds the
/// reqwest error's `Display`. A connect failure here is NOT classified as an SSRF
/// block: the address was already resolved + validated + pinned before the send,
/// so `is_connect()` means a genuine unreachable/refused/TLS-handshake failure
/// (down upstream), which maps to `UpstreamRequest`. The SSRF-block classification
/// (`RequestBlockedPrivateAddr`) comes only from the explicit `resolve_and_validate`
/// pre-check and the `recheck_peer` post-check.
fn map_send_err(e: reqwest::Error, label: &str) -> OpenApiError {
    if e.is_timeout() {
        OpenApiError::UpstreamTimeout {
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
/// Path params are substituted from `params`, each value encoded as an opaque
/// path segment (dots, slashes, `%`, `?`, `#` all escaped) so a `..` value can
/// never traverse above the operator-configured base path; the remaining
/// top-level params become the query string (GET/HEAD/DELETE) or the JSON
/// request body (POST/PUT/PATCH). The credential is injected server-side — the
/// caller's `params` never carry it. Non-2xx and body-decode failures map to
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
        let (c, ip) = pinned_client_for(&url, &op.operation_id).await?;
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
        .map_err(|e| map_send_err(e, &op.operation_id))?;
    if let Some(pinned_ip) = pinned_ip {
        recheck_peer(&resp, pinned_ip, &op.operation_id)?;
    }

    let status = resp.status();
    if !status.is_success() {
        // Do NOT include the body — it may carry the upstream response body.
        return Err(OpenApiError::UpstreamRequest {
            label: op.operation_id.clone(),
        });
    }
    let body = collect_capped(resp, MAX_RESPONSE_BYTES, &op.operation_id).await?;
    if body.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&body).map_err(|_| OpenApiError::UpstreamRequest {
        label: op.operation_id.clone(),
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

/// Percent-encode a single path segment. Escapes EVERYTHING except the RFC 3986
/// unreserved set MINUS `.` — so `.` is `%2E`, which means a `..` value can never
/// survive as a dot-segment that `Url::join` would normalize away. `/`, `?`, `#`,
/// `%` are all escaped too, so a value can never introduce a new segment or
/// query/fragment boundary.
const PATH_SEGMENT_ENCODE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'~');

/// Resolve one required path-param value to a safe, encoded segment string.
/// Rejects missing/non-scalar/null values and the traversal tokens `.`/`..`
/// with a caller-facing `InvalidPathParam` (never leaks the value).
fn path_param_value(
    op: &OperationHandle,
    params: &serde_json::Value,
    name: &str,
) -> Result<String, OpenApiError> {
    let raw = params
        .get(name)
        .ok_or_else(|| OpenApiError::InvalidPathParam {
            label: op.operation_id.clone(),
            param: name.to_string(),
        })?;
    let value = match raw {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        // Null / Object / Array are never a valid scalar path segment.
        _ => {
            return Err(OpenApiError::InvalidPathParam {
                label: op.operation_id.clone(),
                param: name.to_string(),
            });
        }
    };
    // Defense in depth: reject literal traversal tokens outright (belt to the
    // encoder's suspenders, which already escapes `.`).
    if value.is_empty() || value == "." || value == ".." {
        return Err(OpenApiError::InvalidPathParam {
            label: op.operation_id.clone(),
            param: name.to_string(),
        });
    }
    Ok(percent_encoding::utf8_percent_encode(&value, PATH_SEGMENT_ENCODE).to_string())
}

/// Substitute `{name}` path params from a flat params object, encoding each value
/// as an opaque path segment (see [`path_param_value`]). Returns the resolved URL
/// and the set of param keys consumed as path segments (so they are not also sent
/// as query/body). A `..`-valued param can never shorten the URL below the
/// operator-configured base path — verified after the join.
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
            let encoded = path_param_value(op, params, &name)?;
            path.push_str(&encoded);
            consumed.push(name);
        } else {
            path.push(c);
        }
    }
    // Ensure the base path ends with '/' so `join` appends rather than replaces
    // the last segment (mirrors rmcp-openapi's `with_base_url`).
    let mut base = op.base_url.clone();
    let base_path = base.path().to_string();
    let base_prefix = if base_path.ends_with('/') {
        base_path.clone()
    } else {
        let with_slash = format!("{base_path}/");
        base.set_path(&with_slash);
        with_slash
    };
    let joined =
        base.join(path.trim_start_matches('/'))
            .map_err(|_| OpenApiError::UpstreamRequest {
                label: op.operation_id.clone(),
            })?;
    // Belt-and-suspenders: the joined path MUST still start with the operator's
    // base path prefix. Encoding already prevents `..` traversal; this catches
    // any residual normalization that would escape the configured scope.
    if !joined.path().starts_with(base_prefix.trim_end_matches('/')) {
        return Err(OpenApiError::RequestBlockedPrivateAddr {
            label: op.operation_id.clone(),
        });
    }
    Ok((consumed, joined))
}

/// Stringify a scalar JSON value for a query pair. Objects/arrays fall back to
/// their JSON text (query values are far less structurally sensitive than path
/// segments — the `url` crate percent-encodes them fully).
fn json_scalar_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Inject the server-side credential. Header-style only in v1.
fn inject_credential(
    req: reqwest::RequestBuilder,
    op: &OperationHandle,
) -> reqwest::RequestBuilder {
    match &op.credential {
        Some(crate::config::OpenApiCredential::BearerToken(token)) => req.bearer_auth(token),
        Some(crate::config::OpenApiCredential::ApiKey { header, value }) => {
            req.header(header, value)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ipv6_literal_host_resolves_then_is_ssrf_checked() {
        // A bracketed IPv6 literal must reach the IP check (loopback `::1` is
        // rejected as private), NOT fail to parse. Before bracket-stripping this
        // returned `ResolveFailed`; after, it correctly returns
        // `RequestBlockedPrivateAddr` — proving `lookup_host` parsed the literal.
        let err = resolve_and_validate("[::1]", 443, "vendor")
            .await
            .expect_err("loopback must be rejected");
        assert_eq!(
            err.kind(),
            "forbidden",
            "bracketed IPv6 literal must resolve and then be SSRF-rejected, not ResolveFailed: {err:?}"
        );
    }
}
