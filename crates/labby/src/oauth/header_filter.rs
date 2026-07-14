//! Shared hop-by-hop / allowlist header filtering.
//!
//! Used by both the local OAuth relay (`oauth/target.rs`) and the public
//! callback relay (`oauth/public_relay/policy.rs`). The two relays forward
//! third-party OAuth callback traffic and share the same security-sensitive
//! header policy (drop `Authorization`/`Cookie`/`Set-Cookie`, strip
//! hop-by-hop headers, honor a caller-nominated `Connection:` header list).
//! Kept in one place so that policy can't silently drift between the two
//! implementations.

use std::collections::BTreeSet;

use axum::http::{HeaderMap, HeaderName, header};

pub const REQUEST_HEADER_ALLOWLIST: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "origin",
    "referer",
    "user-agent",
];

/// Response headers relayed to the public internet-facing callback surface
/// (`oauth/public_relay/policy.rs`). `location` is deliberately excluded here
/// even though the local relay allows it (see [`LOCAL_RESPONSE_HEADER_ALLOWLIST`]):
/// the public relay's machine registry can be mutated by any admin-scoped
/// caller and its callback path is reachable from the open internet, so
/// blindly relaying an upstream `Location` would let a compromised or
/// misconfigured registry entry turn the relay into an open redirector.
pub const RESPONSE_HEADER_ALLOWLIST: &[&str] = &[
    "cache-control",
    "content-language",
    "content-type",
    "expires",
    "pragma",
];

/// Response headers relayed to the loopback-only local OAuth relay
/// (`oauth/target.rs`). Unlike [`RESPONSE_HEADER_ALLOWLIST`], this includes
/// `location`: the local relay only ever forwards to an operator-configured
/// trusted target (`OauthMachineConfig::target_url` or an explicit CLI
/// target), never to an admin-mutable registry reachable from the internet,
/// so there is no open-redirect trust boundary to protect here. Dropping
/// `Location` would silently break any local target that responds to an
/// OAuth callback with a redirect (e.g. to a "success" page), which is a
/// legitimate and expected pattern for a trusted loopback target.
pub const LOCAL_RESPONSE_HEADER_ALLOWLIST: &[&str] = &[
    "cache-control",
    "content-language",
    "content-type",
    "expires",
    "location",
    "pragma",
];

/// Filter `headers` down to `allowlist`, additionally dropping hop-by-hop
/// headers and any header nominated by a `Connection:` header value.
pub fn filter_headers(headers: &HeaderMap, allowlist: &[&str]) -> HeaderMap {
    let connection_header_names = connection_header_names(headers);
    headers
        .iter()
        .filter(|(name, _)| {
            allowlist.contains(&name.as_str())
                && !is_hop_by_hop_header(name)
                && !connection_header_names.contains(name.as_str())
        })
        .fold(HeaderMap::new(), |mut filtered, (name, value)| {
            filtered.append(name.clone(), value.clone());
            filtered
        })
}

fn connection_header_names(headers: &HeaderMap) -> BTreeSet<String> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_hop_by_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use axum::http::header::{AUTHORIZATION, COOKIE, LOCATION, SET_COOKIE};

    #[test]
    fn request_filter_drops_auth_and_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        headers.insert(COOKIE, HeaderValue::from_static("lab_session=secret"));

        let filtered = filter_headers(&headers, REQUEST_HEADER_ALLOWLIST);

        assert!(filtered.contains_key(header::CONTENT_TYPE));
        assert!(!filtered.contains_key(AUTHORIZATION));
        assert!(!filtered.contains_key(COOKIE));
    }

    #[test]
    fn response_filter_strips_location_and_set_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(
            LOCATION,
            HeaderValue::from_static("https://example.com/leak"),
        );
        headers.insert(SET_COOKIE, HeaderValue::from_static("secret=1"));

        let filtered = filter_headers(&headers, RESPONSE_HEADER_ALLOWLIST);

        assert!(filtered.contains_key(header::CONTENT_TYPE));
        assert!(!filtered.contains_key(LOCATION));
        assert!(!filtered.contains_key(SET_COOKIE));
    }

    #[test]
    fn local_response_filter_preserves_location_but_strips_set_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(
            LOCATION,
            HeaderValue::from_static("https://example.com/success"),
        );
        headers.insert(SET_COOKIE, HeaderValue::from_static("secret=1"));

        let filtered = filter_headers(&headers, LOCAL_RESPONSE_HEADER_ALLOWLIST);

        assert!(filtered.contains_key(header::CONTENT_TYPE));
        assert!(filtered.contains_key(LOCATION));
        assert!(!filtered.contains_key(SET_COOKIE));
    }

    #[test]
    fn connection_nominated_headers_are_dropped() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("origin"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://x.example"),
        );

        let filtered = filter_headers(&headers, REQUEST_HEADER_ALLOWLIST);

        assert!(filtered.contains_key(header::CONTENT_TYPE));
        assert!(!filtered.contains_key(header::ORIGIN));
    }
}
