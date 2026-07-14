use std::time::Duration;

use axum::http::HeaderMap;

use crate::oauth::header_filter::{
    REQUEST_HEADER_ALLOWLIST, RESPONSE_HEADER_ALLOWLIST, filter_headers,
};

use super::types::{MachineId, PublicRelayError};

pub const PUBLIC_QUERY_LIMIT_BYTES: usize = 16 * 1024;
pub const PUBLIC_REQUEST_BODY_LIMIT_BYTES: usize = 64 * 1024;
pub const PUBLIC_RESPONSE_LIMIT_BYTES: usize = 128 * 1024;
pub const PUBLIC_GLOBAL_CONCURRENCY: usize = 32;
pub const PUBLIC_PER_MACHINE_CONCURRENCY: usize = 2;
pub const PUBLIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
pub const PUBLIC_READ_TIMEOUT: Duration = Duration::from_secs(2);
pub const PUBLIC_TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

pub fn validate_suffix_path(path: &str) -> Result<String, PublicRelayError> {
    if path.len() > 2048 {
        return Err(PublicRelayError::InvalidSuffix(
            "suffix path too long".into(),
        ));
    }
    if !path.starts_with("/callback/") && path != "/callback" {
        return Err(PublicRelayError::InvalidSuffix(
            "path is not a callback route".into(),
        ));
    }
    // Reject any percent-encoding outright instead of pattern-matching known
    // encoded traversal sequences (`%2e`, `%2f`, `%5c`, ...). A single-layer
    // substring check like that is bypassable with double-encoding —
    // `%252e%252e` never contains the literal substring `%2e`, so it slips
    // past a `%2e`-only check even though a downstream single-decode turns
    // it into `%2e%2e` (and, if decoded again, `..`). OAuth callback suffix
    // segments never legitimately need percent-encoded characters (`code`
    // and `state` live in the query string, not the path), so a blanket
    // rejection closes the whole encoding-bypass class instead of chasing
    // individual sequences.
    if path.contains('%') || path.contains('\\') {
        return Err(PublicRelayError::InvalidSuffix(
            "percent-encoded or backslash path segments are not allowed".into(),
        ));
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(PublicRelayError::InvalidSuffix(
            "dot segments are not allowed".into(),
        ));
    }
    Ok(path.to_string())
}

pub fn suffix_after_machine(
    path: &str,
    machine_id: &MachineId,
) -> Result<String, PublicRelayError> {
    validate_suffix_path(path)?;
    let base = format!("/callback/{}", machine_id.as_str());
    if path == base {
        return Ok(String::new());
    }
    let Some(rest) = path.strip_prefix(&base) else {
        return Err(PublicRelayError::InvalidSuffix(
            "path machine segment does not match target".into(),
        ));
    };
    let Some(rest) = rest.strip_prefix('/') else {
        return Err(PublicRelayError::InvalidSuffix(
            "path is not under machine callback route".into(),
        ));
    };
    Ok(rest.trim_matches('/').to_string())
}

pub fn default_registry_path() -> std::path::PathBuf {
    crate::dispatch::helpers::lab_home()
        .join("oauth-public-relay")
        .join("registry.json")
}

/// Paths reserved for the public OAuth callback relay surface: `/healthz`
/// and `/callback`(`/*`). Case-insensitive.
///
/// Shared by two enforcement points that must never drift apart:
/// - `api/router.rs::protected_mcp_intercept` uses it at request time to
///   bypass the protected-MCP-route intercept for the relay's own routes.
/// - `config.rs::validate_protected_public_path_for_startup` uses it at
///   startup to reject an admin-configured protected route whose
///   `public_path` would collide with (and be silently shadowed by) the
///   relay.
pub fn is_reserved_public_relay_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == "/healthz" || lower == "/callback" || lower.starts_with("/callback/")
}

pub fn filter_public_request_headers(headers: &HeaderMap) -> HeaderMap {
    filter_headers(headers, REQUEST_HEADER_ALLOWLIST)
}

pub fn filter_public_response_headers(headers: &HeaderMap) -> HeaderMap {
    filter_headers(headers, RESPONSE_HEADER_ALLOWLIST)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use axum::http::header;
    use axum::http::header::{AUTHORIZATION, COOKIE, LOCATION, SET_COOKIE};

    #[test]
    fn machine_id_accepts_live_names() {
        for value in [
            "dookie",
            "shart",
            "squirts",
            "steamy",
            "steamy-wsl",
            "tootie",
            "vivobook-wsl",
        ] {
            assert_eq!(MachineId::parse(value).unwrap().as_str(), value);
        }
    }

    #[test]
    fn machine_id_rejects_confusing_values() {
        for value in [
            "", ".", "..", "node/a", "node\\a", " node", "node ", "node?a", "node#a",
        ] {
            assert!(MachineId::parse(value).is_err(), "{value:?} should reject");
        }
    }

    #[test]
    fn suffix_path_rejects_traversal_and_encoded_slash() {
        for value in [
            "/callback2/x",
            "/callback/dookie/../x",
            "/callback/dookie/%2e%2e/secret",
            "/callback/dookie/%2E/secret",
            "/callback/dookie/%2fsecret",
            "/callback/dookie/%5csecret",
            // Double-encoded traversal: `%252e%252e` never contains the
            // literal substring `%2e`, so a naive single-pattern check
            // misses it even though a downstream single-decode turns it
            // into `%2e%2e`.
            "/callback/dookie/%252e%252e/secret",
            "/callback/dookie/%2561secret",
        ] {
            assert!(
                validate_suffix_path(value).is_err(),
                "{value:?} should reject"
            );
        }
    }

    #[test]
    fn public_limits_are_small_and_explicit() {
        assert_eq!(PUBLIC_QUERY_LIMIT_BYTES, 16 * 1024);
        assert_eq!(PUBLIC_REQUEST_BODY_LIMIT_BYTES, 64 * 1024);
        assert_eq!(PUBLIC_RESPONSE_LIMIT_BYTES, 128 * 1024);
        assert_eq!(PUBLIC_GLOBAL_CONCURRENCY, 32);
        assert_eq!(PUBLIC_PER_MACHINE_CONCURRENCY, 2);
    }

    #[test]
    fn public_response_filter_strips_location_and_set_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(
            LOCATION,
            HeaderValue::from_static("https://example.com/leak"),
        );
        headers.insert(SET_COOKIE, HeaderValue::from_static("secret=1"));

        let filtered = filter_public_response_headers(&headers);

        assert!(filtered.contains_key(header::CONTENT_TYPE));
        assert!(!filtered.contains_key(LOCATION));
        assert!(!filtered.contains_key(SET_COOKIE));
    }

    #[test]
    fn public_request_filter_drops_auth_and_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        headers.insert(COOKIE, HeaderValue::from_static("lab_session=secret"));

        let filtered = filter_public_request_headers(&headers);

        assert!(filtered.contains_key(header::CONTENT_TYPE));
        assert!(!filtered.contains_key(AUTHORIZATION));
        assert!(!filtered.contains_key(COOKIE));
    }
}
