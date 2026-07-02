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
