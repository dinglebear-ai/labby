//! Verification for the private Core-to-Labby trusted-host boundary.
//!
//! This module intentionally verifies only a Core-issued delegated actor
//! assertion.  The Unix listener separately proves the local peer with kernel
//! credentials; neither proof is sufficient by itself.

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::Mutex;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};

use crate::error::AuthError;

#[cfg(feature = "http-axum")]
use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

pub const ASSERTION_TYPE: &str = "unraid+delegated-actor+jwt";
pub const ASSERTION_ISSUER: &str = "unraid-core";
pub const ASSERTION_AUDIENCE: &str = "labby-trusted-host";
pub const MAX_ASSERTION_TTL_SECONDS: usize = 60;
pub const MAX_CLOCK_SKEW_SECONDS: u64 = 5;
pub const MAX_REPLAY_ENTRIES: usize = 100_000;
pub const MAX_REPLAY_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SCOPES: usize = 64;

/// Claims emitted by Core for one already-authorized request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DelegatedActorClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub exp: usize,
    pub nbf: usize,
    pub iat: usize,
    pub jti: String,
    pub actor: String,
    pub client_id: String,
    pub request_id: String,
    pub authority_generation: u64,
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// The original Core authorization decision for the current request.
///
/// This is deliberately separate from [`crate::auth_context::AuthContext`]:
/// Labby's ordinary authorization only needs the delegated subject and scopes,
/// while audit/event consumers also need the Core actor, client, request
/// correlation ID, and authority generation that produced that decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegatedActorContext {
    pub subject: String,
    pub actor: String,
    pub client_id: String,
    pub request_id: String,
    pub authority_generation: u64,
}

/// Opaque assertion retained only for the private Labby-to-Core provider hop.
/// Debug output is deliberately redacted because the compact JWT is a bearer
/// credential for its short lifetime.
#[derive(Clone)]
pub struct DelegatedActorCredential(pub Arc<str>);

impl std::fmt::Debug for DelegatedActorCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DelegatedActorCredential")
            .field(&"[redacted]")
            .finish()
    }
}

impl From<&DelegatedActorClaims> for DelegatedActorContext {
    fn from(claims: &DelegatedActorClaims) -> Self {
        Self {
            subject: claims.sub.clone(),
            actor: claims.actor.clone(),
            client_id: claims.client_id.clone(),
            request_id: claims.request_id.clone(),
            authority_generation: claims.authority_generation,
        }
    }
}

/// An explicit current or overlap verification key. No key material is ever
/// taken from a JWT header or network URL.
#[derive(Clone)]
pub struct TrustedHostKey {
    pub key_id: String,
    pub public_key: DecodingKey,
}

impl std::fmt::Debug for TrustedHostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedHostKey")
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl TrustedHostKey {
    /// Parses one explicitly configured raw Ed25519 public key. The caller
    /// supplies the key ID from trusted startup configuration, never JWT data.
    pub fn from_base64url(key_id: String, encoded: &str) -> Result<Self, TrustedHostError> {
        if key_id.is_empty() || key_id.len() > 128 || key_id.trim() != key_id {
            return Err(TrustedHostError::InvalidHeader);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| TrustedHostError::Invalid)?;
        if bytes.len() != 32 {
            return Err(TrustedHostError::Invalid);
        }
        Ok(Self {
            key_id,
            public_key: DecodingKey::from_ed_der(&bytes),
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrustedHostError {
    #[error("delegated actor assertion is missing or malformed")]
    Malformed,
    #[error("delegated actor assertion has an unsupported header")]
    InvalidHeader,
    #[error("delegated actor assertion key is not active")]
    UnknownKey,
    #[error("delegated actor assertion is invalid")]
    Invalid,
    #[error("delegated actor assertion claims are invalid")]
    InvalidClaims,
    #[error("delegated actor assertion was replayed")]
    Replayed,
    #[error("delegated actor replay cache is saturated")]
    ReplayCacheSaturated,
}

/// Bounded verifier for an integrated Labby instance.
#[derive(Debug)]
pub struct TrustedHostVerifier {
    authority_generation: u64,
    keys: HashMap<String, DecodingKey>,
    replay_cache: Mutex<ReplayCache>,
}

#[derive(Debug, Default)]
struct ReplayCache {
    entries: HashMap<String, usize>,
    bytes: usize,
}

impl TrustedHostVerifier {
    pub fn new(authority_generation: u64, keys: impl IntoIterator<Item = TrustedHostKey>) -> Self {
        Self {
            authority_generation,
            keys: keys
                .into_iter()
                .map(|key| (key.key_id, key.public_key))
                .collect(),
            replay_cache: Mutex::new(ReplayCache::default()),
        }
    }

    /// Verify a compact Core assertion and consume its `jti` exactly once.
    pub fn verify(
        &self,
        token: &str,
        now: usize,
    ) -> Result<DelegatedActorClaims, TrustedHostError> {
        let header = decode_header(token).map_err(|_| TrustedHostError::Malformed)?;
        if header.alg != Algorithm::EdDSA
            || header.typ.as_deref() != Some(ASSERTION_TYPE)
            || header.cty.is_some()
            || header.jku.is_some()
            || header.jwk.is_some()
            || header.x5u.is_some()
            || header.x5c.is_some()
            || header.x5t.is_some()
            || header.x5t_s256.is_some()
            || header.crit.is_some()
            || header.enc.is_some()
            || header.zip.is_some()
            || header.url.is_some()
            || header.nonce.is_some()
            || !header.extras.is_empty()
        {
            return Err(TrustedHostError::InvalidHeader);
        }
        let kid = header.kid.ok_or(TrustedHostError::UnknownKey)?;
        let key = self.keys.get(&kid).ok_or(TrustedHostError::UnknownKey)?;

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[ASSERTION_ISSUER]);
        validation.set_audience(&[ASSERTION_AUDIENCE]);
        validation.leeway = MAX_CLOCK_SKEW_SECONDS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.required_spec_claims = ["exp", "nbf", "iat", "iss", "sub", "aud", "jti"]
            .into_iter()
            .map(str::to_owned)
            .collect();

        let claims = decode::<DelegatedActorClaims>(token, key, &validation)
            .map_err(|_| TrustedHostError::Invalid)?
            .claims;
        validate_claims(&claims, self.authority_generation, now)?;

        let mut cache = self
            .replay_cache
            .lock()
            .map_err(|_| TrustedHostError::ReplayCacheSaturated)?;
        cache.retain_fresh(now);
        if cache.entries.contains_key(&claims.jti) {
            return Err(TrustedHostError::Replayed);
        }
        let entry_bytes = replay_entry_bytes(&claims.jti);
        if cache.entries.len() >= MAX_REPLAY_ENTRIES
            || cache.bytes.saturating_add(entry_bytes) > MAX_REPLAY_BYTES
        {
            return Err(TrustedHostError::ReplayCacheSaturated);
        }
        cache.entries.insert(claims.jti.clone(), claims.exp);
        cache.bytes += entry_bytes;
        Ok(claims)
    }
}

impl ReplayCache {
    fn retain_fresh(&mut self, now: usize) {
        let expired = self
            .entries
            .iter()
            .filter_map(|(jti, expiry)| {
                (expiry.saturating_add(MAX_CLOCK_SKEW_SECONDS as usize) < now)
                    .then_some(jti.clone())
            })
            .collect::<Vec<_>>();

        for jti in expired {
            if self.entries.remove(&jti).is_some() {
                self.bytes = self.bytes.saturating_sub(replay_entry_bytes(&jti));
            }
        }
    }
}

fn replay_entry_bytes(jti: &str) -> usize {
    jti.len().saturating_add(size_of::<usize>())
}

fn validate_claims(
    claims: &DelegatedActorClaims,
    authority_generation: u64,
    now: usize,
) -> Result<(), TrustedHostError> {
    if !valid_identifier(&claims.sub)
        || !valid_identifier(&claims.actor)
        || !valid_identifier(&claims.client_id)
        || !valid_identifier(&claims.request_id)
        || !valid_identifier(&claims.jti)
        || claims.authority_generation != authority_generation
        || claims.iat > now.saturating_add(MAX_CLOCK_SKEW_SECONDS as usize)
        || claims.nbf > claims.iat.saturating_add(MAX_CLOCK_SKEW_SECONDS as usize)
        || claims.exp <= claims.iat
        || claims.exp.saturating_sub(claims.iat) > MAX_ASSERTION_TTL_SECONDS
        || claims.scopes.len() > MAX_SCOPES
        || !claims.scopes.iter().all(|scope| valid_identifier(scope))
    {
        return Err(TrustedHostError::InvalidClaims);
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

impl From<TrustedHostError> for AuthError {
    fn from(_: TrustedHostError) -> Self {
        AuthError::InvalidAccessToken
    }
}

/// Axum boundary for an already peer-authenticated integrated UDS request.
///
/// The Unix listener must install this only after its `SO_PEERCRED` policy has
/// accepted the stream. Every HTTP request still supplies a fresh assertion;
/// connection acceptance never grants a reusable synthetic admin identity.
#[cfg(feature = "http-axum")]
pub async fn require_delegated_actor(mut request: Request, next: Next) -> Response {
    let verifier = request
        .extensions()
        .get::<Arc<TrustedHostVerifier>>()
        .cloned();
    let Some(verifier) = verifier else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let peer_authenticated = request
        .extensions()
        .get::<crate::VerifiedIdentity>()
        .is_some_and(|identity| identity.authenticator() == crate::Authenticator::UnixPeer);
    if !peer_authenticated {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some(token) = token else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs() as usize);
    let Ok(claims) = verifier.verify(token, now) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let identity = match crate::VerifiedIdentity::local_credential_with_issuer(
        crate::Authenticator::UnixPeer,
        ASSERTION_ISSUER,
        format!("core-subject:{}", claims.sub),
    ) {
        Ok(identity) => identity,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let delegated_actor = DelegatedActorContext::from(&claims);
    let delegated_credential = DelegatedActorCredential(Arc::from(token));
    request.extensions_mut().insert(identity);
    request.extensions_mut().insert(delegated_actor);
    request.extensions_mut().insert(delegated_credential);
    request
        .extensions_mut()
        .insert(crate::auth_context::AuthContext {
            sub: claims.sub,
            actor_key: None,
            scopes: claims.scopes,
            issuer: ASSERTION_ISSUER.to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        });
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    #[cfg(feature = "http-axum")]
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        extract::Extension,
        http::{Request, StatusCode},
        middleware,
        response::IntoResponse,
        routing::get,
    };
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use jsonwebtoken::{EncodingKey, Header, encode};
    #[cfg(feature = "http-axum")]
    use tower::ServiceExt;

    use super::*;

    fn verifier(key: &SigningKey) -> TrustedHostVerifier {
        TrustedHostVerifier::new(
            7,
            [TrustedHostKey {
                key_id: "current".to_string(),
                public_key: DecodingKey::from_ed_der(key.verifying_key().as_bytes()),
            }],
        )
    }

    fn token(key: &SigningKey, claims: DelegatedActorClaims) -> String {
        token_with_key_id(key, "current", claims)
    }

    fn token_with_key_id(key: &SigningKey, key_id: &str, claims: DelegatedActorClaims) -> String {
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some(ASSERTION_TYPE.to_string());
        header.kid = Some(key_id.to_string());
        let private_der = key.to_pkcs8_der().unwrap();
        encode(
            &header,
            &claims,
            &EncodingKey::from_ed_der(private_der.as_bytes()),
        )
        .unwrap()
    }

    fn claims() -> DelegatedActorClaims {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;

        DelegatedActorClaims {
            iss: ASSERTION_ISSUER.to_string(),
            sub: "root".to_string(),
            aud: ASSERTION_AUDIENCE.to_string(),
            exp: now + 60,
            nbf: now.saturating_sub(1),
            iat: now,
            jti: "one".to_string(),
            actor: "root".to_string(),
            client_id: "core".to_string(),
            request_id: "request-1".to_string(),
            authority_generation: 7,
            scopes: vec!["lab:read".to_string()],
        }
    }

    #[test]
    fn accepts_current_core_assertion_once() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let verifier = verifier(&key);
        let token = token(&key, claims());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        assert_eq!(verifier.verify(&token, now).unwrap().sub, "root");
        assert_eq!(
            verifier.verify(&token, now),
            Err(TrustedHostError::Replayed)
        );
    }

    #[test]
    fn rejects_wrong_generation_and_excess_ttl() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let verifier = verifier(&key);
        let mut wrong_generation = claims();
        wrong_generation.authority_generation = 8;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        assert_eq!(
            verifier.verify(&token(&key, wrong_generation), now),
            Err(TrustedHostError::InvalidClaims)
        );
        let mut long_lived = claims();
        long_lived.jti = "two".to_string();
        long_lived.exp = long_lived.iat + 61;
        assert_eq!(
            verifier.verify(&token(&key, long_lived), now),
            Err(TrustedHostError::InvalidClaims)
        );

        let mut control_scope = claims();
        control_scope.jti = "scope-control".to_string();
        control_scope.scopes = vec!["lab:read\nadmin".to_string()];
        assert_eq!(
            verifier.verify(&token(&key, control_scope), now),
            Err(TrustedHostError::InvalidClaims)
        );

        let mut too_many_scopes = claims();
        too_many_scopes.jti = "too-many-scopes".to_string();
        too_many_scopes.scopes = vec!["lab:read".to_string(); MAX_SCOPES + 1];
        assert_eq!(
            verifier.verify(&token(&key, too_many_scopes), now),
            Err(TrustedHostError::InvalidClaims)
        );
    }

    #[test]
    fn rejects_expired_and_retired_assertions() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let verifier = verifier(&key);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;

        let mut expired = claims();
        expired.jti = "expired".to_string();
        expired.iat = now.saturating_sub(66);
        expired.nbf = now.saturating_sub(66);
        expired.exp = now.saturating_sub(6);
        assert_eq!(
            verifier.verify(&token(&key, expired), now),
            Err(TrustedHostError::Invalid)
        );

        let mut retired = claims();
        retired.jti = "retired".to_string();
        assert_eq!(
            verifier.verify(&token_with_key_id(&key, "retired", retired), now),
            Err(TrustedHostError::UnknownKey)
        );
    }

    #[test]
    fn rejects_assertion_controlled_key_urls_and_unknown_headers() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let verifier = verifier(&key);
        let private_der = key.to_pkcs8_der().unwrap();

        for name in ["jku", "crit", "extra"] {
            let mut header = Header::new(Algorithm::EdDSA);
            header.typ = Some(ASSERTION_TYPE.to_string());
            header.kid = Some("current".to_string());
            match name {
                "jku" => {
                    header.jku = Some("https://attacker.invalid/jwks".to_string());
                }
                "crit" => {
                    header.crit = Some(vec!["unknown".to_string()]);
                }
                "extra" => {
                    header
                        .extras
                        .insert("unknown".to_string(), "value".to_string());
                }
                _ => unreachable!(),
            }
            let mut claims = claims();
            claims.jti = format!("forbidden-{name}");
            let assertion = encode(
                &header,
                &claims,
                &EncodingKey::from_ed_der(private_der.as_bytes()),
            )
            .unwrap();

            assert_eq!(
                verifier.verify(&assertion, claims.iat),
                Err(TrustedHostError::InvalidHeader)
            );
        }
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test]
    async fn middleware_requires_the_unix_peer_proof_before_a_valid_core_assertion() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let assertion = token(&key, claims());
        let verifier = Arc::new(verifier(&key));
        let app = Router::new()
            .route(
                "/",
                get(|| async { StatusCode::NO_CONTENT.into_response() }),
            )
            .layer(middleware::from_fn(require_delegated_actor))
            .layer(Extension(verifier))
            .layer(Extension(
                crate::VerifiedIdentity::local_credential(
                    crate::Authenticator::UnixPeer,
                    "unix-peer:uid=1000:gid=1000",
                )
                .unwrap(),
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("authorization", format!("Bearer {assertion}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test]
    async fn middleware_replaces_broad_unix_peer_scopes_with_delegated_scopes() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let assertion = token(&key, claims());
        let verifier = Arc::new(verifier(&key));
        let app = Router::new()
            .route(
                "/",
                get(
                    |Extension(context): Extension<crate::auth_context::AuthContext>| async move {
                        Json(context.scopes)
                    },
                ),
            )
            .layer(middleware::from_fn(require_delegated_actor))
            .layer(Extension(verifier))
            .layer(Extension(
                crate::VerifiedIdentity::local_credential(
                    crate::Authenticator::UnixPeer,
                    "unix-peer:uid=1000:gid=1000",
                )
                .unwrap(),
            ));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("authorization", format!("Bearer {assertion}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1_024).await.unwrap();
        assert_eq!(body.as_ref(), br#"["lab:read"]"#);
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test]
    async fn middleware_rejects_a_valid_core_assertion_without_the_unix_peer_proof() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let assertion = token(&key, claims());
        let app = Router::new()
            .route(
                "/",
                get(|| async { StatusCode::NO_CONTENT.into_response() }),
            )
            .layer(middleware::from_fn(require_delegated_actor))
            .layer(Extension(Arc::new(verifier(&key))));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("authorization", format!("Bearer {assertion}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
