//! Authentication middleware helpers shared by the top-level API router.

use std::sync::Arc;

use subtle::ConstantTimeEq;

/// Adapt the product's typed actor-key derivation to `labby-auth`'s erased
/// callback without coupling the auth crate to product observability types.
pub(super) fn lab_auth_deriver(
    deriver: Arc<crate::observability::activity::ActorKeyDeriver>,
) -> Arc<labby_auth::ActorKeyDeriver> {
    Arc::new(move |subject: &str| {
        deriver
            .derive_subject(subject)
            .map(crate::observability::activity::ActorKey::into_arc)
    })
}

pub(crate) fn tokens_equal(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub(super) fn percent_encode_path(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b'?') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(
                char::from_digit(u32::from(byte >> 4), 16)
                    .expect("four-bit value is valid hex")
                    .to_ascii_uppercase(),
            );
            encoded.push(
                char::from_digit(u32::from(byte & 0xf), 16)
                    .expect("four-bit value is valid hex")
                    .to_ascii_uppercase(),
            );
        }
    }
    encoded
}

pub(crate) fn parse_bearer_token(header_value: &str) -> Option<String> {
    let mut parts = header_value.split_whitespace();
    let scheme = parts.next()?;
    let token = parts.next()?;
    if parts.next().is_some() || !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    Some(token.to_string())
}

pub(super) fn derive_actor_key(
    deriver: Option<&crate::observability::activity::ActorKeyDeriver>,
    subject: &str,
) -> Option<Arc<str>> {
    deriver
        .and_then(|deriver| deriver.derive_subject(subject))
        .map(crate::observability::activity::ActorKey::into_arc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_parser_rejects_ambiguous_headers() {
        assert_eq!(parse_bearer_token("Bearer secret"), Some("secret".into()));
        assert_eq!(parse_bearer_token("bearer secret extra"), None);
        assert_eq!(parse_bearer_token("Basic secret"), None);
    }

    #[test]
    fn percent_encoding_preserves_route_delimiters() {
        assert_eq!(percent_encode_path("/gateway?a=b c"), "/gateway?a%3Db%20c");
    }
}
