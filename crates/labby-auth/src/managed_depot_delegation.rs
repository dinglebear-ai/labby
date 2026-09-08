//! Opt-in wire signer for the managed Depot protocol, separate from
//! [`crate::depot_delegation`]'s browser/product-credential protocol.
//!
//! Signing does not authorize a request or activate managed mode. The caller
//! must supply freshly verified authority, bound service/target identity, exact
//! request bytes and a durable intent. No owner conversion is performed here.
//! In particular, carrying a Project ID does not establish Depot Project ownership.
//!
//! Recovered from Labby ef5c0777; wire contract matches the Depot verifier at
//! db6a2df83460948ac6e5a029b16dd4bfb6c563d5. Transport must send the assertion
//! as `x-labby-delegation` alongside the service credential, never instead of it.

use std::collections::BTreeMap;

use ed25519_dalek::pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const ASSERTION_TYPE: &str = "labby+depot-delegation+jwt";
pub const ASSERTION_ISSUER: &str = "labby";
pub const ASSERTION_AUDIENCE: &str = "depot";
pub const MAX_TTL_SECONDS: u64 = 60;
pub const MAX_VALUES: usize = 64;
/// Depot's verifier rejects assertions larger than this wire bound.
pub const MAX_TOKEN_BYTES: usize = 16_384;
/// Bounded overlapping key rotation: at most this many registered keys, one active.
pub const MAX_SIGNING_KEYS: usize = 8;
/// Longest `resource` path a delegated assertion may bind.
pub const MAX_RESOURCE_BYTES: usize = 2048;

/// Authority epochs bound into an assertion.
///
/// Wire invariant: Depot validates that `epochs` is a map with **exactly
/// seven** keys. Every field here is therefore always serialized (an absent
/// epoch is an explicit `null`, never a skipped key), and no key may be added
/// or removed without a coordinated Depot contract change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAuthorityEpochs {
    pub authority_schema: u64,
    pub organization_policy: u64,
    pub team_membership: Option<u64>,
    pub team_policy: Option<u64>,
    pub project_membership: Option<u64>,
    pub project_policy: Option<u64>,
    pub global_revision: u64,
}

impl ManagedAuthorityEpochs {
    /// Number of keys Depot requires in the serialized `epochs` object.
    pub const WIRE_KEY_COUNT: usize = 7;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedDepotClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    pub jti: String,
    pub deployment_id: String,
    pub account_id: String,
    pub organization_id: String,
    pub team_id: Option<String>,
    pub project_id: Option<String>,
    pub principal_id: String,
    pub method: String,
    pub resource: String,
    pub operation: String,
    pub intent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_length: Option<u64>,
    pub scopes: Vec<String>,
    /// Closed managed-protocol v1 wire vocabulary; not a domain authorization grant.
    pub capabilities: Vec<ManagedCapability>,
    pub epochs: ManagedAuthorityEpochs,
    #[serde(default)]
    pub delegation_chain: Vec<String>,
}

/// Protocol-local v1 capabilities. Future authority wiring must map explicitly
/// from the domain vocabulary; unknown wire values are rejected.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ManagedCapability {
    #[serde(rename = "platform.read")]
    PlatformRead,
    #[serde(rename = "platform.manage")]
    PlatformManage,
    #[serde(rename = "scope.read")]
    ScopeRead,
    #[serde(rename = "scope.use")]
    ScopeUse,
    #[serde(rename = "scope.operate")]
    ScopeOperate,
    #[serde(rename = "scope.create")]
    ScopeCreate,
    #[serde(rename = "scope.manage")]
    ScopeManage,
    #[serde(rename = "scope.delete")]
    ScopeDelete,
    #[serde(rename = "membership.manage")]
    MembershipManage,
    #[serde(rename = "ownership.transfer")]
    OwnershipTransfer,
    #[serde(rename = "policy.explain")]
    PolicyExplain,
    #[serde(rename = "audit.read")]
    AuditRead,
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum ManagedDelegationError {
    #[error("delegated assertion input is invalid")]
    Invalid,
    #[error("delegated assertion signing failed")]
    Signing,
    #[error("delegated assertion signing key material is malformed or unavailable")]
    KeyMaterial,
}

pub struct ManagedDepotSigner {
    active_key_id: String,
    keys: BTreeMap<String, EncodingKey>,
}
impl std::fmt::Debug for ManagedDepotSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedDepotSigner")
            .field("active_key_id", &self.active_key_id)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

impl ManagedDepotSigner {
    /// Register PKCS#8 DER Ed25519 keys. Every document is parsed with
    /// `ed25519_dalek` before use so a malformed key fails construction rather
    /// than the first signature.
    pub fn new(
        active_key_id: String,
        keys: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, ManagedDelegationError> {
        let mut parsed = BTreeMap::new();
        for (id, der) in keys {
            let der = Zeroizing::new(der);
            ed25519_dalek::SigningKey::from_pkcs8_der(&der)
                .map_err(|_| ManagedDelegationError::KeyMaterial)?;
            if parsed.len() >= MAX_SIGNING_KEYS
                || parsed.insert(id, EncodingKey::from_ed_der(&der)).is_some()
            {
                return Err(ManagedDelegationError::Invalid);
            }
        }
        Self::from_encoding_keys(active_key_id, parsed)
    }

    /// Build a single-key signer from a raw 32-byte Ed25519 seed. The seed is
    /// zeroized when this call returns.
    pub fn from_seed(
        key_id: impl Into<String>,
        seed: Zeroizing<[u8; 32]>,
    ) -> Result<Self, ManagedDelegationError> {
        let key_id = key_id.into();
        let der = pkcs8_der_from_seed(&seed)?;
        let mut keys = BTreeMap::new();
        keys.insert(key_id.clone(), EncodingKey::from_ed_der(&der));
        Self::from_encoding_keys(key_id, keys)
    }

    /// Build a single-key signer from a base64url (no padding) seed held in the
    /// named environment variable. The decoded material is zeroized after the
    /// key is derived; the variable's value is never logged.
    pub fn from_seed_env(
        key_id: impl Into<String>,
        env_name: &str,
    ) -> Result<Self, ManagedDelegationError> {
        Self::from_encoded_seed(key_id, read_seed_env(env_name)?)
    }

    /// Build a single-key signer from an already-read base64url (no padding)
    /// seed. The encoded and decoded material are zeroized on return.
    pub fn from_encoded_seed(
        key_id: impl Into<String>,
        encoded: Zeroizing<String>,
    ) -> Result<Self, ManagedDelegationError> {
        Self::from_seed(key_id, decode_seed(&encoded)?)
    }

    /// Register up to [`MAX_SIGNING_KEYS`] seeds from environment variables for
    /// overlapping rotation. `keys` pairs each key ID with its variable name;
    /// `active_key_id` selects the key used for new assertions and must be one
    /// of them.
    pub fn from_seed_envs<'a>(
        active_key_id: impl Into<String>,
        keys: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, ManagedDelegationError> {
        let mut encoded = Vec::new();
        for (key_id, env_name) in keys {
            if encoded.len() >= MAX_SIGNING_KEYS {
                return Err(ManagedDelegationError::Invalid);
            }
            encoded.push((key_id.to_owned(), read_seed_env(env_name)?));
        }
        Self::from_encoded_seeds(active_key_id, encoded)
    }

    /// Rotation variant of [`Self::from_encoded_seed`]: up to
    /// [`MAX_SIGNING_KEYS`] `(key_id, base64url seed)` pairs with one active.
    pub fn from_encoded_seeds(
        active_key_id: impl Into<String>,
        keys: impl IntoIterator<Item = (String, Zeroizing<String>)>,
    ) -> Result<Self, ManagedDelegationError> {
        let mut encoding_keys = BTreeMap::new();
        for (key_id, encoded) in keys {
            if encoding_keys.len() >= MAX_SIGNING_KEYS {
                return Err(ManagedDelegationError::Invalid);
            }
            let der = pkcs8_der_from_seed(&decode_seed(&encoded)?)?;
            if encoding_keys
                .insert(key_id, EncodingKey::from_ed_der(&der))
                .is_some()
            {
                return Err(ManagedDelegationError::Invalid);
            }
        }
        Self::from_encoding_keys(active_key_id.into(), encoding_keys)
    }

    fn from_encoding_keys(
        active_key_id: String,
        keys: BTreeMap<String, EncodingKey>,
    ) -> Result<Self, ManagedDelegationError> {
        if !valid(&active_key_id)
            || keys.is_empty()
            || keys.len() > MAX_SIGNING_KEYS
            || !keys.contains_key(&active_key_id)
            || keys.keys().any(|id| !valid(id))
        {
            return Err(ManagedDelegationError::Invalid);
        }
        Ok(Self {
            active_key_id,
            keys,
        })
    }

    /// Key ID stamped into new assertions.
    #[must_use]
    pub fn active_key_id(&self) -> &str {
        &self.active_key_id
    }

    /// Every registered key ID (active plus overlap keys), sorted.
    pub fn key_ids(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    /// Sign caller-supplied claims after structural checks. This does not check
    /// wall-clock freshness, source-grant expiry, membership, or replay state.
    /// The dispatch layer must revalidate those facts immediately before use.
    pub fn issue(&self, claims: ManagedDepotClaims) -> Result<String, ManagedDelegationError> {
        validate(&claims)?;
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some(ASSERTION_TYPE.into());
        header.kid = Some(self.active_key_id.clone());
        let token = encode(
            &header,
            &claims,
            self.keys
                .get(&self.active_key_id)
                .ok_or(ManagedDelegationError::Invalid)?,
        )
        .map_err(|_| ManagedDelegationError::Signing)?;
        if token.len() > MAX_TOKEN_BYTES {
            return Err(ManagedDelegationError::Invalid);
        }
        Ok(token)
    }
}

fn read_seed_env(env_name: &str) -> Result<Zeroizing<String>, ManagedDelegationError> {
    if env_name.is_empty() || env_name.len() > 128 {
        return Err(ManagedDelegationError::KeyMaterial);
    }
    std::env::var(env_name)
        .map(Zeroizing::new)
        .map_err(|_| ManagedDelegationError::KeyMaterial)
}

fn decode_seed(encoded: &str) -> Result<Zeroizing<[u8; 32]>, ManagedDelegationError> {
    let decoded = Zeroizing::new(
        base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            encoded.trim(),
        )
        .map_err(|_| ManagedDelegationError::KeyMaterial)?,
    );
    let mut seed = Zeroizing::new([0_u8; 32]);
    if decoded.len() != seed.len() {
        return Err(ManagedDelegationError::KeyMaterial);
    }
    seed.copy_from_slice(&decoded);
    Ok(seed)
}

fn pkcs8_der_from_seed(
    seed: &Zeroizing<[u8; 32]>,
) -> Result<Zeroizing<Vec<u8>>, ManagedDelegationError> {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(seed);
    let document = signing_key
        .to_pkcs8_der()
        .map_err(|_| ManagedDelegationError::KeyMaterial)?;
    Ok(Zeroizing::new(document.as_bytes().to_vec()))
}

fn valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'))
}

/// `resource` is the request path without its query string: absolute, bounded,
/// percent-encoding-safe ASCII, and free of `.`/`..` segments, query, fragment,
/// and whitespace.
fn valid_resource(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= MAX_RESOURCE_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'/' | b'-' | b'_' | b'.' | b'~' | b':' | b'@' | b'+' | b'%'
                )
        })
        && value
            .split('/')
            .skip(1)
            .all(|segment| segment != "." && segment != "..")
        && valid_percent_escapes(value)
}

fn valid_percent_escapes(value: &str) -> bool {
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%'
            && !(bytes.next().is_some_and(|digit| digit.is_ascii_hexdigit())
                && bytes.next().is_some_and(|digit| digit.is_ascii_hexdigit()))
        {
            return false;
        }
    }
    true
}

fn validate(c: &ManagedDepotClaims) -> Result<(), ManagedDelegationError> {
    if c.iss != ASSERTION_ISSUER
        || c.aud != ASSERTION_AUDIENCE
        || c.sub != c.principal_id
        || c.exp <= c.iat
        || c.nbf > c.iat
        || c.exp - c.iat > MAX_TTL_SECONDS
        || !matches!(
            c.method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
        )
        || !valid_resource(&c.resource)
        || ![
            &c.jti,
            &c.deployment_id,
            &c.account_id,
            &c.organization_id,
            &c.principal_id,
            &c.operation,
            &c.intent_id,
        ]
        .into_iter()
        .all(|v| valid(v))
        || c.scopes.len() > MAX_VALUES
        || c.scopes.iter().any(|scope| {
            !valid(scope) || scope.len() > 128
                || !(matches!(scope.as_str(), "skills:read" | "skills:write")
                    || scope.strip_prefix("skills:read:").is_some_and(|slug| !slug.is_empty()))
        })
        || c.capabilities.len() > MAX_VALUES
        || c.delegation_chain.len() > 8
        || c.delegation_chain.iter().any(|link| !valid(link))
        // A body binding is all-or-nothing: a digest without its length (or the
        // reverse) would let Depot accept a differently sized body.
        || c.content_digest.is_some() != c.content_length.is_some()
        || c.content_digest.as_ref().is_some_and(|digest| {
            digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || c.team_id.as_deref().is_some_and(|v| !valid(v))
        || c.project_id.as_deref().is_some_and(|v| !valid(v))
    {
        return Err(ManagedDelegationError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePrivateKey;

    #[test]
    fn unknown_capabilities_fail_deserialization() {
        let mut value = serde_json::to_value(claims()).unwrap();
        value["capabilities"] = serde_json::json!(["scope.root"]);
        assert!(serde_json::from_value::<ManagedDepotClaims>(value).is_err());
    }

    #[test]
    fn unknown_claims_and_epochs_are_not_silently_discarded() {
        let mut value = serde_json::to_value(claims()).unwrap();
        value["epochs"]["future_required_epoch"] = serde_json::json!(1);
        assert!(serde_json::from_value::<ManagedDepotClaims>(value).is_err());
        let mut value = serde_json::to_value(claims()).unwrap();
        value["future_required_claim"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ManagedDepotClaims>(value).is_err());
    }

    #[test]
    fn scope_and_total_wire_size_bounds_match_the_verifier() {
        for scopes in [
            vec!["lab:admin".into()],
            vec![format!("skills:read:{}", "a".repeat(117))],
            vec!["skills:read".into(); MAX_VALUES + 1],
        ] {
            let mut invalid = claims();
            invalid.scopes = scopes;
            assert_eq!(
                signer().issue(invalid).unwrap_err(),
                ManagedDelegationError::Invalid
            );
        }
        let mut oversized = claims();
        oversized.scopes = vec![format!("skills:read:{}", "a".repeat(116)); MAX_VALUES];
        oversized.delegation_chain = vec!["a".repeat(256); 8];
        oversized.resource = format!("/{}", "a".repeat(MAX_RESOURCE_BYTES - 1));
        assert_eq!(
            signer().issue(oversized).unwrap_err(),
            ManagedDelegationError::Invalid
        );
    }

    #[test]
    fn signature_is_verified_and_every_claim_round_trips() {
        use base64::Engine as _;
        use ed25519_dalek::Verifier as _;
        let expected = claims();
        let token = signer().issue(expected.clone()).unwrap();
        let parts: Vec<_> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let decode = |part| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .unwrap()
        };
        let header: serde_json::Value = serde_json::from_slice(&decode(parts[0])).unwrap();
        assert_eq!(
            header,
            serde_json::json!({"alg":"EdDSA", "typ":ASSERTION_TYPE,"kid":"current"})
        );
        let restored: ManagedDepotClaims = serde_json::from_slice(&decode(parts[1])).unwrap();
        assert_eq!(restored, expected);
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]).verifying_key();
        let signature = ed25519_dalek::Signature::from_slice(&decode(parts[2])).unwrap();
        let input = format!("{}.{}", parts[0], parts[1]);
        key.verify(input.as_bytes(), &signature).unwrap();
        assert!(key.verify(b"different claims", &signature).is_err());
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/managed-depot-v1.json")).unwrap();
        assert_eq!(fixture["token"], token);
        assert_eq!(fixture["claims"], serde_json::to_value(expected).unwrap());
        assert_eq!(
            fixture["public_key_base64url"],
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.as_bytes())
        );
    }

    #[test]
    fn duplicate_der_keys_are_rejected_instead_of_replaced() {
        let der = ed25519_dalek::SigningKey::from_bytes(&[9; 32])
            .to_pkcs8_der()
            .unwrap();
        assert_eq!(
            ManagedDepotSigner::new(
                "same".into(),
                [
                    ("same".into(), der.as_bytes().to_vec()),
                    ("same".into(), der.as_bytes().to_vec()),
                ]
            )
            .unwrap_err(),
            ManagedDelegationError::Invalid
        );
    }
    fn claims() -> ManagedDepotClaims {
        ManagedDepotClaims {
            iss: ASSERTION_ISSUER.into(),
            sub: "p1".into(),
            aud: ASSERTION_AUDIENCE.into(),
            iat: 10,
            nbf: 10,
            exp: 40,
            jti: "j1".into(),
            deployment_id: "d1".into(),
            account_id: "a1".into(),
            organization_id: "o1".into(),
            team_id: Some("t1".into()),
            project_id: Some("pr1".into()),
            principal_id: "p1".into(),
            method: "GET".into(),
            resource: "/api/artifacts".into(),
            operation: "depot.artifacts.list".into(),
            intent_id: "i1".into(),
            content_digest: None,
            content_length: None,
            scopes: vec!["skills:read".into()],
            capabilities: vec![ManagedCapability::ScopeRead],
            epochs: ManagedAuthorityEpochs {
                authority_schema: 1,
                organization_policy: 1,
                team_membership: Some(1),
                team_policy: Some(1),
                project_membership: Some(1),
                project_policy: Some(1),
                global_revision: 1,
            },
            delegation_chain: vec!["labby".into()],
        }
    }
    fn signer() -> ManagedDepotSigner {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let der = key.to_pkcs8_der().unwrap();
        ManagedDepotSigner::new(
            "current".into(),
            [("current".into(), der.as_bytes().to_vec())],
        )
        .unwrap()
    }
    #[test]
    fn exact_claims_are_signed_and_ttl_is_bounded() {
        let signer = signer();
        let token = signer.issue(claims()).unwrap();
        assert_eq!(
            jsonwebtoken::decode_header(&token).unwrap().kid.as_deref(),
            Some("current")
        );
        let mut invalid = claims();
        invalid.exp = invalid.iat + 61;
        assert_eq!(
            signer.issue(invalid).unwrap_err(),
            ManagedDelegationError::Invalid
        );
    }

    #[test]
    fn epochs_always_serialize_exactly_seven_keys() {
        let mut epochs = claims().epochs;
        epochs.team_membership = None;
        epochs.project_membership = None;
        let value = serde_json::to_value(&epochs).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), ManagedAuthorityEpochs::WIRE_KEY_COUNT);
        assert!(object["team_membership"].is_null());
    }

    #[test]
    fn validation_rejects_each_malformed_claim() {
        let signer = signer();
        let cases: Vec<(&str, Box<dyn Fn(&mut ManagedDepotClaims)>)> = vec![
            (
                "relative resource",
                Box::new(|c| c.resource = "api/x".into()),
            ),
            (
                "query in resource",
                Box::new(|c| c.resource = "/api/x?y=1".into()),
            ),
            (
                "fragment in resource",
                Box::new(|c| c.resource = "/api/x#y".into()),
            ),
            ("dot segment", Box::new(|c| c.resource = "/api/../x".into())),
            (
                "truncated escape",
                Box::new(|c| c.resource = "/api/%2".into()),
            ),
            (
                "invalid escape",
                Box::new(|c| c.resource = "/api/%zz".into()),
            ),
            (
                "space in resource",
                Box::new(|c| c.resource = "/api/a b".into()),
            ),
            (
                "oversized resource",
                Box::new(|c| c.resource = format!("/{}", "a".repeat(MAX_RESOURCE_BYTES))),
            ),
            (
                "digest without length",
                Box::new(|c| c.content_digest = Some(format!("sha256:{}", "a".repeat(64)))),
            ),
            (
                "length without digest",
                Box::new(|c| c.content_length = Some(4)),
            ),
            (
                "uppercase digest",
                Box::new(|c| {
                    c.content_digest = Some(format!("sha256:{}", "A".repeat(64)));
                    c.content_length = Some(4);
                }),
            ),
            ("empty scope", Box::new(|c| c.scopes = vec![String::new()])),
            ("lowercase method", Box::new(|c| c.method = "post".into())),
            ("subject mismatch", Box::new(|c| c.sub = "other".into())),
            ("nbf after iat", Box::new(|c| c.nbf = c.iat + 1)),
            ("wrong audience", Box::new(|c| c.aud = "labby".into())),
            (
                "bad team id",
                Box::new(|c| c.team_id = Some("team id".into())),
            ),
            (
                "long chain",
                Box::new(|c| c.delegation_chain = vec!["l".into(); 9]),
            ),
        ];
        for (label, mutate) in cases {
            let mut invalid = claims();
            mutate(&mut invalid);
            assert_eq!(
                signer.issue(invalid).unwrap_err(),
                ManagedDelegationError::Invalid,
                "{label}"
            );
        }
        let mut bound = claims();
        bound.content_digest = Some(format!("sha256:{}", "a".repeat(64)));
        bound.content_length = Some(4);
        bound.resource = "/api/operations/depot.artifacts.fork%20x".into();
        assert!(signer.issue(bound).is_ok());
    }

    #[test]
    fn seeds_are_parsed_fail_fast_and_rotation_keeps_one_active_key() {
        let seed = Zeroizing::new([7_u8; 32]);
        let signer = ManagedDepotSigner::from_seed("k1", seed).unwrap();
        assert_eq!(signer.active_key_id(), "k1");
        let token = signer.issue(claims()).unwrap();
        assert_eq!(
            jsonwebtoken::decode_header(&token).unwrap().kid.as_deref(),
            Some("k1")
        );

        assert_eq!(
            ManagedDepotSigner::new("k".into(), [("k".into(), vec![1, 2, 3])]).unwrap_err(),
            ManagedDelegationError::KeyMaterial
        );

        let too_many = (0..=MAX_SIGNING_KEYS)
            .map(|index| {
                let key = ed25519_dalek::SigningKey::from_bytes(&[index as u8 + 1; 32]);
                (
                    format!("k{index}"),
                    key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ManagedDepotSigner::new("k0".into(), too_many).unwrap_err(),
            ManagedDelegationError::Invalid
        );

        assert_eq!(
            ManagedDepotSigner::from_encoded_seed("k1", Zeroizing::new("not-base64!!".into()))
                .unwrap_err(),
            ManagedDelegationError::KeyMaterial
        );
        assert_eq!(
            ManagedDepotSigner::from_encoded_seed("k1", Zeroizing::new("AAAA".into())).unwrap_err(),
            ManagedDelegationError::KeyMaterial
        );
        assert_eq!(
            ManagedDepotSigner::from_seed_env("k1", "LABBY_TEST_DEPOT_DELEGATION_UNSET")
                .unwrap_err(),
            ManagedDelegationError::KeyMaterial
        );
        let good = || {
            Zeroizing::new(base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                [3_u8; 32],
            ))
        };
        let rotated = ManagedDepotSigner::from_encoded_seeds(
            "k2",
            [("k1".to_owned(), good()), ("k2".to_owned(), good())],
        )
        .unwrap();
        assert_eq!(rotated.active_key_id(), "k2");
        assert_eq!(rotated.key_ids().collect::<Vec<_>>(), vec!["k1", "k2"]);
        assert_eq!(
            ManagedDepotSigner::from_encoded_seeds("k9", [("k1".to_owned(), good())]).unwrap_err(),
            ManagedDelegationError::Invalid
        );
        assert_eq!(
            ManagedDepotSigner::from_encoded_seeds(
                "k1",
                [("k1".to_owned(), good()), ("k1".to_owned(), good())]
            )
            .unwrap_err(),
            ManagedDelegationError::Invalid
        );
    }
}
