use std::net::IpAddr;

use labby_primitives::ssrf;
use url::Host;

use crate::dispatch::error::ToolError;
use crate::dispatch::helpers::require_str;

#[derive(Debug)]
pub struct ProxyCheckParams<'a> {
    pub app_url: &'a str,
    pub mcp_url: &'a str,
    pub route: &'a str,
    /// Optional private backend origin for the backend-leak probe.
    /// When present, the probe verifies this origin does not appear in
    /// public error response bodies.
    pub backend_url: Option<&'a str>,
}

pub fn parse_proxy_check(params: &serde_json::Value) -> Result<ProxyCheckParams<'_>, ToolError> {
    let app_url = require_str(params, "app_url")?;
    let mcp_url = require_str(params, "mcp_url")?;
    let route = require_str(params, "route")?;
    if !route.starts_with('/') {
        return Err(ToolError::InvalidParam {
            message: "route must start with /".to_string(),
            param: "route".to_string(),
        });
    }
    if route.len() > 1 && route.ends_with('/') {
        return Err(ToolError::InvalidParam {
            message: "route must not end with /".to_string(),
            param: "route".to_string(),
        });
    }
    if route.contains('?') || route.contains('#') {
        return Err(ToolError::InvalidParam {
            message: "route must be a path without query or fragment".to_string(),
            param: "route".to_string(),
        });
    }
    for (param, value) in [("app_url", app_url), ("mcp_url", mcp_url)] {
        let parsed = url::Url::parse(value).map_err(|error| ToolError::InvalidParam {
            message: format!("{param} must be a valid URL: {error}"),
            param: param.to_string(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ToolError::InvalidParam {
                message: format!("{param} must be an http(s) URL with a host"),
                param: param.to_string(),
            });
        }
        validate_public_proxy_url(param, &parsed)?;
    }
    let backend_url = params
        .get("backend_url")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty());
    if let Some(backend_url) = backend_url {
        let parsed = url::Url::parse(backend_url).map_err(|error| ToolError::InvalidParam {
            message: format!("backend_url must be a valid URL: {error}"),
            param: "backend_url".to_string(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ToolError::InvalidParam {
                message: "backend_url must be an http(s) URL with a host".to_string(),
                param: "backend_url".to_string(),
            });
        }
    }
    Ok(ProxyCheckParams {
        app_url,
        mcp_url,
        route,
        backend_url,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct RelayCheckParams {
    pub probe_targets: bool,
}

pub fn parse_relay_check(params: &serde_json::Value) -> Result<RelayCheckParams, ToolError> {
    let probe_targets = match params.get("probe_targets") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| ToolError::InvalidParam {
            message: "probe_targets must be a boolean".to_string(),
            param: "probe_targets".to_string(),
        })?,
    };
    Ok(RelayCheckParams { probe_targets })
}

/// Static SSRF guard for a URL `doctor.proxy.check` will actually fetch.
///
/// Delegates to the canonical `labby_primitives::ssrf` checks rather than
/// re-deriving the blocked ranges here. The bespoke version this replaced
/// hardcoded `Host::Domain(_) => false`, so any internal *name*
/// (`gateway.lan`, `vault.internal`) passed unexamined; it also missed CGNAT
/// (`100.64.0.0/10`) and the IPv4-mapped-IPv6 form (`::ffff:10.0.0.1`), which
/// Rust's `Ipv6Addr` helpers do not cover.
///
/// Scheme stays `http`-or-`https` on purpose: unlike the archive/registry
/// fetchers that use [`labby_primitives::ssrf::parse_validated_https_url`],
/// probing a plain-HTTP reverse proxy is a legitimate thing to ask this
/// diagnostic to do. The multicast rejection the old code had is dropped
/// because the canonical guard does not carry it; multicast is not a
/// meaningful SSRF target for an HTTP GET.
///
/// This is the static half only — it performs no DNS. The resolved-address
/// half runs in `proxy.rs` immediately before the probes.
fn validate_public_proxy_url(param: &str, parsed: &url::Url) -> Result<(), ToolError> {
    let invalid = |message: String| ToolError::InvalidParam {
        message,
        param: param.to_string(),
    };
    let Some(host) = parsed.host() else {
        return Err(invalid(format!(
            "{param} must be an http(s) URL with a host"
        )));
    };
    let redacted = ssrf::redact_url(parsed.as_str());
    match host {
        Host::Domain(domain) => ssrf::check_host_not_private(domain),
        Host::Ipv4(ip) => ssrf::check_ip_not_private(IpAddr::V4(ip), &redacted),
        Host::Ipv6(ip) => ssrf::check_ip_not_private(IpAddr::V6(ip), &redacted),
    }
    .map_err(|error| {
        invalid(format!(
            "{param} must be a public proxy URL, not a local or private address: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_params(route: &str) -> serde_json::Value {
        serde_json::json!({
            "app_url": "https://lab.example.test",
            "mcp_url": "https://mcp.example.test",
            "route": route,
        })
    }

    #[test]
    fn parse_proxy_check_rejects_ambiguous_route_variants() {
        for route in [
            "telemetry",
            "/telemetry/",
            "/telemetry?debug=true",
            "/telemetry#fragment",
        ] {
            let err = parse_proxy_check(&proxy_params(route)).expect_err("route should fail");
            assert_eq!(err.kind(), "invalid_param");
        }
    }

    #[test]
    fn parse_proxy_check_rejects_private_ipv6_proxy_urls() {
        for value in ["https://[::1]", "https://[fc00::1]", "https://[fe80::1]"] {
            let params = serde_json::json!({
                "app_url": value,
                "mcp_url": "https://mcp.example.test",
                "route": "/telemetry",
            });
            let err = parse_proxy_check(&params).expect_err("private IPv6 should fail");
            assert_eq!(err.kind(), "invalid_param", "{value}");
        }
    }

    /// The bespoke validator hardcoded `Host::Domain(_) => false`, so an
    /// internal *name* was never examined at all — only IP literals were. Each
    /// of these reaches a private network by name.
    #[test]
    fn parse_proxy_check_rejects_private_hostnames() {
        for value in [
            "https://gateway.lan",
            "https://vault.internal",
            "https://printer.local",
            "https://wiki.intranet",
            "https://sso.corp",
            "https://nas.home",
            "http://localhost",
            "https://localhost:8443",
        ] {
            let params = serde_json::json!({
                "app_url": value,
                "mcp_url": "https://mcp.example.test",
                "route": "/telemetry",
            });
            let err = parse_proxy_check(&params).expect_err("private hostname should fail");
            assert_eq!(err.kind(), "invalid_param", "{value}");
        }
    }

    /// Ranges the bespoke validator missed: CGNAT (`100.64.0.0/10`, the
    /// Tailscale range this fleet actually runs on) and the IPv4-mapped-IPv6
    /// form, which `Ipv6Addr::is_loopback`/`is_unique_local` do not cover.
    #[test]
    fn parse_proxy_check_rejects_cgnat_and_ipv4_mapped_ipv6() {
        for value in [
            "https://100.64.0.1",
            "https://100.100.118.47",
            "https://[::ffff:10.0.0.1]",
            "https://[::ffff:127.0.0.1]",
        ] {
            let params = serde_json::json!({
                "app_url": value,
                "mcp_url": "https://mcp.example.test",
                "route": "/telemetry",
            });
            let err = parse_proxy_check(&params).expect_err("blocked range should fail");
            assert_eq!(err.kind(), "invalid_param", "{value}");
        }
    }

    /// `mcp_url` is fetched too, so it must be guarded identically — the guard
    /// is applied in a loop over both, and this pins that.
    #[test]
    fn parse_proxy_check_guards_mcp_url_as_well_as_app_url() {
        let params = serde_json::json!({
            "app_url": "https://lab.example.test",
            "mcp_url": "https://gateway.lan",
            "route": "/telemetry",
        });
        let err = parse_proxy_check(&params).expect_err("private mcp_url should fail");
        assert_eq!(err.kind(), "invalid_param");
    }

    /// `backend_url` is never fetched — `check_backend_leak` requests
    /// `mcp_url` and searches the response body for this string. Restricting
    /// it would defeat the probe, whose entire purpose is detecting a leaked
    /// *private* origin. Pin that it stays permitted.
    #[test]
    fn parse_proxy_check_still_allows_a_private_backend_url_needle() {
        let params = serde_json::json!({
            "app_url": "https://lab.example.test",
            "mcp_url": "https://mcp.example.test",
            "route": "/telemetry",
            "backend_url": "http://10.1.0.1:8080",
        });
        let parsed = parse_proxy_check(&params).expect("private backend_url is the point");
        assert_eq!(parsed.backend_url, Some("http://10.1.0.1:8080"));
    }

    /// Plain HTTP stays legal: probing an http reverse proxy is a legitimate
    /// use of this diagnostic, unlike the archive fetchers that pin https.
    #[test]
    fn parse_proxy_check_still_allows_public_http_urls() {
        let params = serde_json::json!({
            "app_url": "http://lab.example.test",
            "mcp_url": "https://mcp.example.test",
            "route": "/telemetry",
        });
        parse_proxy_check(&params).expect("public http is allowed");
    }
}
