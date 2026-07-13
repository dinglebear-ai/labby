use std::collections::BTreeSet;
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName, HeaderValue, header};

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
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%5c") || path.contains('\\') {
        return Err(PublicRelayError::InvalidSuffix(
            "encoded slash or backslash is not allowed".into(),
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

pub fn filter_public_request_headers(headers: &HeaderMap) -> HeaderMap {
    filter_headers(headers, REQUEST_HEADER_ALLOWLIST)
}

pub fn filter_public_response_headers(headers: &HeaderMap) -> HeaderMap {
    filter_headers(headers, RESPONSE_HEADER_ALLOWLIST)
}

const REQUEST_HEADER_ALLOWLIST: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "origin",
    "referer",
    "user-agent",
];

const RESPONSE_HEADER_ALLOWLIST: &[&str] = &[
    "cache-control",
    "content-language",
    "content-type",
    "expires",
    "pragma",
];

fn filter_headers(headers: &HeaderMap, allowlist: &[&str]) -> HeaderMap {
    let connection_header_names = connection_header_names(headers);
    headers
        .iter()
        .filter(|(name, _)| {
            allowlist.contains(&name.as_str())
                && !is_hop_by_hop_header(name)
                && !connection_header_names.contains(name.as_str())
        })
        .fold(HeaderMap::new(), |mut filtered, (name, value)| {
            filtered.append(name.clone(), copy_header_value(value));
            filtered
        })
}

fn copy_header_value(value: &HeaderValue) -> HeaderValue {
    value.clone()
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
            "/callback/dookie/%2fsecret",
            "/callback/dookie/%5csecret",
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
