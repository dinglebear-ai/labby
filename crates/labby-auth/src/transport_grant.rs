//! Context-bound envelopes for transporting native credentials without exposing
//! a reusable product bearer to browser JavaScript. Native access is revalidated
//! after opening the envelope; encryption never replaces authorization.

use crate::{at_rest::TokenEncryptionKey, error::AuthError};
use labby_primitives::product_credential::ProductCredential;
use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct TransportGrantBinding {
    pub installation: String,
    pub principal: String,
    pub peer: String,
    pub origin: String,
    pub resource: String,
    pub upstream: String,
    pub generation: String,
    pub expires_at: u64,
}

const PREFIX: &str = "lby_tg_v1_";
const MAX_GRANT: usize = 512;

fn denied() -> AuthError {
    AuthError::InvalidGrant("transport grant denied".into())
}

fn context(binding: &TransportGrantBinding, now: u64) -> Result<Vec<u8>, AuthError> {
    let origin = url::Url::parse(&binding.origin).map_err(|_| denied())?;
    let resource = url::Url::parse(&binding.resource).map_err(|_| denied())?;
    let peer = binding.peer.strip_prefix("nodekey:").ok_or_else(denied)?;
    if binding.expires_at <= now
        || binding.expires_at.saturating_sub(now) > 900
        || [
            &binding.installation,
            &binding.principal,
            &binding.upstream,
            &binding.generation,
        ]
        .iter()
        .any(|id| id.is_empty() || id.len() > 160 || !id.bytes().all(|b| b.is_ascii_graphic()))
        || peer.len() != 64
        || peer.bytes().all(|b| b == b'0')
        || !peer
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || binding.origin.len() > 2048
        || origin.scheme() != "https"
        || origin.origin().ascii_serialization() != binding.origin
        || resource.scheme() != "https"
        || binding.resource.len() > 2048
        || resource.host_str().is_none()
        || !resource.username().is_empty()
        || resource.password().is_some()
        || resource.query().is_some()
        || resource.fragment().is_some()
    {
        return Err(denied());
    }
    serde_json::to_vec(&("labby.tailcat.transport-grant/v1", binding)).map_err(|_| denied())
}

/// Seal a native credential using the existing auth key and exact approved context.
/// The returned bearer is accepted only by the dedicated transport endpoint.
pub fn seal(
    key: &TokenEncryptionKey,
    credential: &ProductCredential,
    binding: &TransportGrantBinding,
    now: u64,
) -> Result<String, AuthError> {
    use base64::Engine as _;
    let context = context(binding, now)?;
    let wire = format!(
        "{}{}_{}",
        labby_primitives::product_credential::PRODUCT_CREDENTIAL_PREFIX,
        credential.credential_id(),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(credential.secret())
    );
    let encrypted = crate::at_rest::encrypt_provider_token_with_context(key, &wire, &context)
        .map_err(|_| AuthError::Storage("transport grant unavailable".into()))?;
    let encoded = encrypted.strip_prefix("enc2:").ok_or_else(denied)?;
    Ok(format!("{PREFIX}{encoded}"))
}

/// Open an envelope only for its approved context, then revalidate the credential
/// through the native access adapter. This function does not authorize operations.
pub fn open(
    key: &TokenEncryptionKey,
    grant: &str,
    binding: &TransportGrantBinding,
    now: u64,
) -> Result<ProductCredential, AuthError> {
    if grant.len() > MAX_GRANT {
        return Err(denied());
    }
    let encoded = grant.strip_prefix(PREFIX).ok_or_else(denied)?;
    let context = context(binding, now)?;
    // Always reconstruct the context-encrypted format; legacy/plaintext fallback
    // in the storage helper is deliberately unreachable from this boundary.
    let wire = crate::at_rest::decrypt_provider_token_with_context(
        key,
        &format!("enc2:{encoded}"),
        &context,
    )
    .map_err(|_| denied())?;
    ProductCredential::parse(&wire).map_err(|_| denied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    fn binding() -> TransportGrantBinding {
        TransportGrantBinding {
            installation: "machine".into(),
            principal: "principal".into(),
            peer: format!("nodekey:{}", "1".repeat(64)),
            origin: "https://depot.example".into(),
            resource: "https://labby.example/sandbox".into(),
            upstream: "msb".into(),
            generation: "generation".into(),
            expires_at: 1900,
        }
    }
    fn key() -> TokenEncryptionKey {
        TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap()
    }
    fn credential() -> ProductCredential {
        ProductCredential::parse(&format!(
            "lby_pc_v1_child_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
        ))
        .unwrap()
    }
    #[test]
    fn sealed_grant_is_not_a_product_bearer_and_only_opens_for_its_binding() {
        let binding = binding();
        let credential = credential();
        let grant = seal(&key(), &credential, &binding, 1000).unwrap();
        assert!(ProductCredential::parse(&grant).is_err());
        assert!(!grant.contains(credential.credential_id()));
        let opened = open(&key(), &grant, &binding, 1001).unwrap();
        assert_eq!(opened.credential_id(), credential.credential_id());
        assert_eq!(opened.secret(), credential.secret());
        for case in 0..8 {
            let mut changed = binding.clone();
            match case {
                0 => changed.installation.push('x'),
                1 => changed.principal.push('x'),
                2 => changed.peer = format!("nodekey:{}", "2".repeat(64)),
                3 => changed.origin = "https://other.example".into(),
                4 => changed.resource.push('x'),
                5 => changed.upstream.push('x'),
                6 => changed.generation.push('x'),
                _ => changed.expires_at -= 1,
            }
            assert!(open(&key(), &grant, &changed, 1001).is_err());
        }
        assert!(open(&key(), &grant, &binding, 1900).is_err());
        assert!(
            open(
                &TokenEncryptionKey::from_encoded(&"02".repeat(32)).unwrap(),
                &grant,
                &binding,
                1001
            )
            .is_err()
        );
    }
    #[test]
    fn plaintext_legacy_ciphertexts_and_excessive_lifetimes_are_rejected() {
        for grant in [
            "plaintext",
            "enc:plaintext",
            "enc2:plaintext",
            "lby_tg_v1_invalid",
        ] {
            assert!(open(&key(), grant, &binding(), 1000).is_err());
        }
        assert!(seal(&key(), &credential(), &binding(), 999).is_err());
        assert!(seal(&key(), &credential(), &binding(), 1900).is_err());
    }
}
