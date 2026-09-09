//! Short-lived, exact Labby-to-Depot delegated operation assertions.

use std::collections::BTreeMap;

use ed25519_dalek::pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use labby_primitives::access::{Capability, CapabilitySchemaVersion};
use serde::{Deserialize, Serialize};

pub use product::{
    BrowserDepotAuthorization, DEPOT_DELEGATION_VERSION, DelegatedActor, DepotDelegationScope,
    DepotDelegationSubject, DepotDelegationTarget, MAX_DEPOT_DELEGATION_TTL_SECS,
    ProductDepotDelegationClaims, depot_params_digest,
};
use zeroize::Zeroizing;

pub const ASSERTION_TYPE: &str = "labby+depot-delegation+jwt";
pub const ASSERTION_ISSUER: &str = "labby";
pub const ASSERTION_AUDIENCE: &str = "depot";
pub const MAX_TTL_SECONDS: u64 = 60;
pub const MAX_VALUES: usize = 64;
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
pub struct DelegatedAuthorityEpochs {
    pub authority_schema: u64,
    pub organization_policy: u64,
    pub team_membership: Option<u64>,
    pub team_policy: Option<u64>,
    pub project_membership: Option<u64>,
    pub project_policy: Option<u64>,
    pub global_revision: u64,
}

impl DelegatedAuthorityEpochs {
    /// Number of keys Depot requires in the serialized `epochs` object.
    pub const WIRE_KEY_COUNT: usize = 7;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DepotDelegationClaims {
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
    /// Wire capability names (`Capability::as_wire`). Every entry must be a
    /// known v1 capability; see [`capability_wire_names`].
    pub capabilities: Vec<String>,
    pub epochs: DelegatedAuthorityEpochs,
    #[serde(default)]
    pub delegation_chain: Vec<String>,
}

/// Encode typed capabilities as the claim's wire names.
#[must_use]
pub fn capability_wire_names(capabilities: &[Capability]) -> Vec<String> {
    capabilities
        .iter()
        .map(|capability| capability.as_wire().to_owned())
        .collect()
}

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum DelegationError {
    #[error("delegated assertion input is invalid")]
    Invalid,
    #[error("delegated assertion signing failed")]
    Signing,
    #[error("delegated assertion signing key material is malformed or unavailable")]
    KeyMaterial,
}

pub struct DepotDelegationSigner {
    active_key_id: String,
    keys: BTreeMap<String, EncodingKey>,
}
impl std::fmt::Debug for DepotDelegationSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepotDelegationSigner")
            .field("active_key_id", &self.active_key_id)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

impl DepotDelegationSigner {
    /// Register PKCS#8 DER Ed25519 keys. Every document is parsed with
    /// `ed25519_dalek` before use so a malformed key fails construction rather
    /// than the first signature.
    pub fn new(
        active_key_id: String,
        keys: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, DelegationError> {
        let mut parsed = BTreeMap::new();
        for (id, der) in keys {
            let der = Zeroizing::new(der);
            ed25519_dalek::SigningKey::from_pkcs8_der(&der)
                .map_err(|_| DelegationError::KeyMaterial)?;
            parsed.insert(id, EncodingKey::from_ed_der(&der));
        }
        Self::from_encoding_keys(active_key_id, parsed)
    }

    /// Build a single-key signer from a raw 32-byte Ed25519 seed. The seed is
    /// zeroized when this call returns.
    pub fn from_seed(
        key_id: impl Into<String>,
        seed: Zeroizing<[u8; 32]>,
    ) -> Result<Self, DelegationError> {
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
    ) -> Result<Self, DelegationError> {
        Self::from_encoded_seed(key_id, read_seed_env(env_name)?)
    }

    /// Build a single-key signer from an already-read base64url (no padding)
    /// seed. The encoded and decoded material are zeroized on return.
    pub fn from_encoded_seed(
        key_id: impl Into<String>,
        encoded: Zeroizing<String>,
    ) -> Result<Self, DelegationError> {
        Self::from_seed(key_id, decode_seed(&encoded)?)
    }

    /// Register up to [`MAX_SIGNING_KEYS`] seeds from environment variables for
    /// overlapping rotation. `keys` pairs each key ID with its variable name;
    /// `active_key_id` selects the key used for new assertions and must be one
    /// of them.
    pub fn from_seed_envs<'a>(
        active_key_id: impl Into<String>,
        keys: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, DelegationError> {
        let mut encoded = Vec::new();
        for (key_id, env_name) in keys {
            encoded.push((key_id.to_owned(), read_seed_env(env_name)?));
        }
        Self::from_encoded_seeds(active_key_id, encoded)
    }

    /// Rotation variant of [`Self::from_encoded_seed`]: up to
    /// [`MAX_SIGNING_KEYS`] `(key_id, base64url seed)` pairs with one active.
    pub fn from_encoded_seeds(
        active_key_id: impl Into<String>,
        keys: impl IntoIterator<Item = (String, Zeroizing<String>)>,
    ) -> Result<Self, DelegationError> {
        let mut encoding_keys = BTreeMap::new();
        for (key_id, encoded) in keys {
            if encoding_keys.len() >= MAX_SIGNING_KEYS {
                return Err(DelegationError::Invalid);
            }
            let der = pkcs8_der_from_seed(&decode_seed(&encoded)?)?;
            if encoding_keys
                .insert(key_id, EncodingKey::from_ed_der(&der))
                .is_some()
            {
                return Err(DelegationError::Invalid);
            }
        }
        Self::from_encoding_keys(active_key_id.into(), encoding_keys)
    }

    fn from_encoding_keys(
        active_key_id: String,
        keys: BTreeMap<String, EncodingKey>,
    ) -> Result<Self, DelegationError> {
        if !valid(&active_key_id)
            || keys.is_empty()
            || keys.len() > MAX_SIGNING_KEYS
            || !keys.contains_key(&active_key_id)
            || keys.keys().any(|id| !valid(id))
        {
            return Err(DelegationError::Invalid);
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

    pub fn issue(&self, claims: DepotDelegationClaims) -> Result<String, DelegationError> {
        validate(&claims)?;
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some(ASSERTION_TYPE.into());
        header.kid = Some(self.active_key_id.clone());
        encode(
            &header,
            &claims,
            self.keys
                .get(&self.active_key_id)
                .ok_or(DelegationError::Invalid)?,
        )
        .map_err(|_| DelegationError::Signing)
    }
}

fn read_seed_env(env_name: &str) -> Result<Zeroizing<String>, DelegationError> {
    if env_name.is_empty() || env_name.len() > 128 {
        return Err(DelegationError::KeyMaterial);
    }
    std::env::var(env_name)
        .map(Zeroizing::new)
        .map_err(|_| DelegationError::KeyMaterial)
}

fn decode_seed(encoded: &str) -> Result<Zeroizing<[u8; 32]>, DelegationError> {
    let decoded = Zeroizing::new(
        base64::Engine::decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            encoded.trim(),
        )
        .map_err(|_| DelegationError::KeyMaterial)?,
    );
    let mut seed = Zeroizing::new([0_u8; 32]);
    if decoded.len() != seed.len() {
        return Err(DelegationError::KeyMaterial);
    }
    seed.copy_from_slice(&decoded);
    Ok(seed)
}

fn pkcs8_der_from_seed(seed: &Zeroizing<[u8; 32]>) -> Result<Zeroizing<Vec<u8>>, DelegationError> {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(seed);
    let document = signing_key
        .to_pkcs8_der()
        .map_err(|_| DelegationError::KeyMaterial)?;
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
}

fn validate(c: &DepotDelegationClaims) -> Result<(), DelegationError> {
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
        || c.scopes.iter().any(|scope| !valid(scope))
        || c.capabilities.len() > MAX_VALUES
        || c.capabilities
            .iter()
            .any(|name| Capability::from_wire(CapabilitySchemaVersion::V1, name).is_none())
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
        return Err(DelegationError::Invalid);
    }
    Ok(())
}

mod product {
    //! Short-lived, per-principal assertions for a selected Depot authority.

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use labby_primitives::product_credential::BoundAccessGrant;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    use crate::error::AuthError;
    use crate::jwt::SigningKeys;
    use crate::util::now_unix;

    pub const DEPOT_DELEGATION_VERSION: u8 = 1;
    pub const MAX_DEPOT_DELEGATION_TTL_SECS: u64 = 60;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum DepotDelegationScope {
        Read,
        Write,
    }

    impl DepotDelegationScope {
        const fn as_str(self) -> &'static str {
            match self {
                Self::Read => "skills:read",
                Self::Write => "skills:read skills:write",
            }
        }
    }

    /// Server-owned identity of the selected Depot. None of these values may come
    /// from request headers or an untrusted request body.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct DepotDelegationTarget {
        pub issuer: String,
        pub audience: String,
        pub deployment_id: String,
        pub account_id: String,
        pub tenant_id: String,
        pub team_id: Option<String>,
    }

    /// Browser-session identity revalidated by the API immediately before a Depot
    /// write. This is deliberately distinct from a product credential grant: the
    /// API remains responsible for revalidating the original browser authority,
    /// membership, policy epochs, and selected approval binding on every request.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BrowserDepotAuthorization {
        pub installation_id: String,
        pub principal_id: String,
        pub organization_id: String,
        pub project_id: String,
        pub membership_epoch: u64,
        pub organization_policy_epoch: u64,
        pub project_policy_epoch: u64,
        pub expires_at: u64,
    }

    #[derive(Clone, Copy)]
    pub enum DepotDelegationSubject<'a> {
        ProductCredential(&'a BoundAccessGrant),
        Browser(&'a BrowserDepotAuthorization),
    }

    impl<'a> From<&'a BoundAccessGrant> for DepotDelegationSubject<'a> {
        fn from(grant: &'a BoundAccessGrant) -> Self {
            Self::ProductCredential(grant)
        }
    }

    impl<'a> From<&'a BrowserDepotAuthorization> for DepotDelegationSubject<'a> {
        fn from(authorization: &'a BrowserDepotAuthorization) -> Self {
            Self::Browser(authorization)
        }
    }

    impl<'a> DepotDelegationSubject<'a> {
        fn installation_id(self) -> &'a str {
            match self {
                Self::ProductCredential(grant) => &grant.installation_id,
                Self::Browser(authorization) => &authorization.installation_id,
            }
        }

        pub fn principal_id(self) -> &'a str {
            match self {
                Self::ProductCredential(grant) => &grant.principal_id,
                Self::Browser(authorization) => &authorization.principal_id,
            }
        }

        fn organization_id(self) -> &'a str {
            match self {
                Self::ProductCredential(grant) => &grant.organization_id,
                Self::Browser(authorization) => &authorization.organization_id,
            }
        }

        fn project_id(self) -> &'a str {
            match self {
                Self::ProductCredential(grant) => &grant.project_id,
                Self::Browser(authorization) => &authorization.project_id,
            }
        }

        const fn organization_policy_epoch(self) -> u64 {
            match self {
                Self::ProductCredential(grant) => grant.organization_policy_epoch,
                Self::Browser(authorization) => authorization.organization_policy_epoch,
            }
        }

        const fn project_policy_epoch(self) -> u64 {
            match self {
                Self::ProductCredential(grant) => grant.project_policy_epoch,
                Self::Browser(authorization) => authorization.project_policy_epoch,
            }
        }

        const fn expires_at(self) -> u64 {
            match self {
                Self::ProductCredential(grant) => grant.expires_at,
                Self::Browser(authorization) => authorization.expires_at,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DelegatedActor {
        pub sub: String,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ProductDepotDelegationClaims {
        pub iss: String,
        pub aud: String,
        pub sub: String,
        pub scope: String,
        pub act: DelegatedActor,
        pub iat: usize,
        pub exp: usize,
        pub jti: String,
        pub depot_delegation_version: u8,
        pub depot_deployment_id: String,
        pub depot_account_id: String,
        pub depot_tenant_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub depot_team_id: Option<String>,
        pub depot_membership_epoch: u64,
        pub depot_organization_policy_epoch: u64,
        pub depot_project_policy_epoch: u64,
        pub depot_organization_id: String,
        pub depot_project_id: String,
        pub depot_operation: String,
        pub depot_params_sha256: String,
    }

    impl SigningKeys {
        /// Mint a fresh assertion from a grant that was revalidated for this
        /// request. This deliberately does not accept or preserve an inbound
        /// bearer token.
        pub fn issue_depot_delegation(
            &self,
            target: &DepotDelegationTarget,
            grant: &BoundAccessGrant,
            scope: DepotDelegationScope,
            operation: &str,
            params: &Value,
            ttl_secs: u64,
        ) -> Result<String, AuthError> {
            self.issue_depot_delegation_for_subject(
                target,
                grant.into(),
                scope,
                operation,
                params,
                ttl_secs,
            )
        }

        /// Mint a Depot assertion from browser authority that the API revalidated
        /// for this exact request. This never turns browser authority into, or
        /// pretends it is, a product credential grant.
        pub fn issue_browser_depot_delegation(
            &self,
            target: &DepotDelegationTarget,
            authorization: &BrowserDepotAuthorization,
            scope: DepotDelegationScope,
            operation: &str,
            params: &Value,
            ttl_secs: u64,
        ) -> Result<String, AuthError> {
            self.issue_depot_delegation_for_subject(
                target,
                authorization.into(),
                scope,
                operation,
                params,
                ttl_secs,
            )
        }

        fn issue_depot_delegation_for_subject(
            &self,
            target: &DepotDelegationTarget,
            subject: DepotDelegationSubject<'_>,
            scope: DepotDelegationScope,
            operation: &str,
            params: &Value,
            ttl_secs: u64,
        ) -> Result<String, AuthError> {
            validate_target(target)?;
            if ttl_secs == 0 || ttl_secs > MAX_DEPOT_DELEGATION_TTL_SECS {
                return Err(AuthError::InvalidGrant(
                    "invalid Depot delegation lifetime".into(),
                ));
            }
            for value in [
                subject.installation_id(),
                subject.principal_id(),
                subject.organization_id(),
                subject.project_id(),
            ] {
                validate_identifier(value)?;
            }
            validate_operation(operation)?;

            let now = now_unix();
            let ttl_secs = i64::try_from(ttl_secs)
                .map_err(|_| AuthError::InvalidGrant("invalid Depot delegation lifetime".into()))?;
            let source_expiry = i64::try_from(subject.expires_at())
                .map_err(|_| AuthError::InvalidGrant("source grant expiry is invalid".into()))?;
            let exp = now
                .checked_add(ttl_secs)
                .map(|candidate| candidate.min(source_expiry))
                .filter(|expires| *expires > now)
                .ok_or_else(|| AuthError::InvalidGrant("source grant has expired".into()))?;
            let mut nonce = [0_u8; 24];
            getrandom::fill(&mut nonce)
                .map_err(|_| AuthError::Server("failed to create delegation token ID".into()))?;
            let jti = URL_SAFE_NO_PAD.encode(nonce);
            nonce.fill(0);

            let claims = ProductDepotDelegationClaims {
                iss: target.issuer.clone(),
                aud: target.audience.clone(),
                sub: subject.principal_id().to_owned(),
                scope: scope.as_str().into(),
                act: DelegatedActor {
                    sub: subject.installation_id().to_owned(),
                },
                iat: usize::try_from(now)
                    .map_err(|_| AuthError::Server("system clock is out of range".into()))?,
                exp: usize::try_from(exp)
                    .map_err(|_| AuthError::Server("system clock is out of range".into()))?,
                jti,
                depot_delegation_version: DEPOT_DELEGATION_VERSION,
                depot_deployment_id: target.deployment_id.clone(),
                depot_account_id: target.account_id.clone(),
                depot_tenant_id: target.tenant_id.clone(),
                depot_team_id: target.team_id.clone(),
                // Depot is project-scoped and therefore compares one shared policy
                // generation. Each principal's distinct membership-row epoch stays
                // in the source authorization that Labby revalidates before minting.
                depot_membership_epoch: subject.project_policy_epoch(),
                depot_organization_policy_epoch: subject.organization_policy_epoch(),
                depot_project_policy_epoch: subject.project_policy_epoch(),
                depot_organization_id: subject.organization_id().to_owned(),
                depot_project_id: subject.project_id().to_owned(),
                depot_operation: operation.to_owned(),
                depot_params_sha256: depot_params_digest(params)?,
            };
            self.issue_custom_token(&claims, "Depot delegation")
        }
    }

    /// SHA-256 over deterministic JSON with recursively sorted object keys.
    /// Numbers and strings retain serde_json's compact representation.
    pub fn depot_params_digest(params: &Value) -> Result<String, AuthError> {
        let canonical = canonical_json(params);
        let encoded = serde_json::to_vec(&canonical)
            .map_err(|_| AuthError::InvalidGrant("Depot delegation params are invalid".into()))?;
        let digest = Sha256::digest(encoded);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}")
                .map_err(|_| AuthError::Server("failed to encode Depot request digest".into()))?;
        }
        Ok(hex)
    }

    fn canonical_json(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by_key(|(key, _)| *key);
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), canonical_json(value)))
                        .collect(),
                )
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
            _ => value.clone(),
        }
    }

    fn validate_operation(value: &str) -> Result<(), AuthError> {
        if value.is_empty()
            || value.len() > 256
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(AuthError::InvalidGrant(
                "invalid Depot delegation operation".into(),
            ));
        }
        Ok(())
    }

    fn validate_target(target: &DepotDelegationTarget) -> Result<(), AuthError> {
        validate_authority(&target.issuer)?;
        validate_authority(&target.audience)?;
        for value in [
            target.deployment_id.as_str(),
            target.account_id.as_str(),
            target.tenant_id.as_str(),
        ] {
            validate_identifier(value)?;
        }
        if let Some(team_id) = target.team_id.as_deref() {
            validate_identifier(team_id)?;
        }
        Ok(())
    }

    fn validate_authority(value: &str) -> Result<(), AuthError> {
        if value.is_empty()
            || value.len() > 512
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(AuthError::InvalidGrant(
                "invalid Depot delegation authority".into(),
            ));
        }
        Ok(())
    }

    fn validate_identifier(value: &str) -> Result<(), AuthError> {
        if value.is_empty()
            || value.len() > 256
            || value.trim() != value
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(AuthError::InvalidGrant(
                "invalid Depot delegation identifier".into(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use jsonwebtoken::decode_header;
        use labby_primitives::product_credential::BoundAccessGrant;

        use super::*;

        fn keys() -> SigningKeys {
            let dir = tempfile::tempdir().unwrap();
            SigningKeys::load_or_create(&dir.path().join("signing-key.der")).unwrap()
        }

        fn target() -> DepotDelegationTarget {
            DepotDelegationTarget {
                issuer: "https://team-labby.example".into(),
                audience: "https://depot.example".into(),
                deployment_id: "depot-lime-prod".into(),
                account_id: "account-lime".into(),
                tenant_id: "tenant-lime".into(),
                team_id: Some("team-lime".into()),
            }
        }

        fn grant(expires_at: i64) -> BoundAccessGrant {
            BoundAccessGrant {
                installation_id: "labby-lime-prod".into(),
                issuer: "https://team-labby.example".into(),
                subject: "credential-subject".into(),
                principal_id: "person-123".into(),
                organization_id: "organization-lime".into(),
                project_id: "project-skills".into(),
                loadout_id: "loadout".into(),
                loadout_generation: 1,
                assignment_generation: 2,
                catalog_generation: 3,
                route_id: "route".into(),
                route_generation: 4,
                membership_epoch: 5,
                organization_policy_epoch: 6,
                project_policy_epoch: 7,
                credential_id: "credential".into(),
                credential_generation: 8,
                scopes: vec!["lab:read".into()],
                resource: "https://team-labby.example/mcp".into(),
                audience: "labby".into(),
                expires_at: u64::try_from(expires_at).unwrap(),
                requires_admin: false,
                destructive: false,
            }
        }

        fn browser_authorization(expires_at: i64) -> BrowserDepotAuthorization {
            BrowserDepotAuthorization {
                installation_id: "labby-browser-prod".into(),
                principal_id: "browser-person-456".into(),
                organization_id: "browser-organization".into(),
                project_id: "browser-project".into(),
                membership_epoch: 15,
                organization_policy_epoch: 16,
                project_policy_epoch: 17,
                expires_at: u64::try_from(expires_at).unwrap(),
            }
        }

        #[test]
        fn assertion_preserves_person_actor_and_current_authority_epochs() {
            let keys = keys();
            let token = keys
                .issue_depot_delegation(
                    &target(),
                    &grant(now_unix() + 600),
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"skill.zip"}),
                    30,
                )
                .unwrap();
            let claims: ProductDepotDelegationClaims = keys.decode_custom_token(
                &token,
                "https://depot.example",
                "https://team-labby.example",
            );

            assert_eq!(
                decode_header(&token).unwrap().alg,
                jsonwebtoken::Algorithm::EdDSA
            );
            assert_eq!(claims.sub, "person-123");
            assert_eq!(claims.act.sub, "labby-lime-prod");
            assert_eq!(claims.scope, "skills:read skills:write");
            assert_eq!(claims.depot_account_id, "account-lime");
            assert_eq!(claims.depot_tenant_id, "tenant-lime");
            assert_eq!(claims.depot_organization_id, "organization-lime");
            assert_eq!(claims.depot_project_id, "project-skills");
            assert_eq!(claims.depot_membership_epoch, 7);
            assert_eq!(claims.depot_organization_policy_epoch, 6);
            assert_eq!(claims.depot_project_policy_epoch, 7);
            assert_eq!(claims.depot_operation, "depot.uploads.create");
            assert_eq!(
                claims.depot_params_sha256,
                depot_params_digest(&serde_json::json!({"filename":"skill.zip"})).unwrap()
            );
            assert!(claims.exp - claims.iat <= 30);
            assert_eq!(claims.jti.len(), 32);
        }

        #[test]
        fn assertions_are_fresh_and_never_accept_an_inbound_bearer() {
            let keys = keys();
            let grant = grant(now_unix() + 600);
            let first = keys
                .issue_depot_delegation(
                    &target(),
                    &grant,
                    DepotDelegationScope::Read,
                    "depot.system.status",
                    &serde_json::json!({}),
                    30,
                )
                .unwrap();
            let second = keys
                .issue_depot_delegation(
                    &target(),
                    &grant,
                    DepotDelegationScope::Read,
                    "depot.system.status",
                    &serde_json::json!({}),
                    30,
                )
                .unwrap();
            assert_ne!(first, second);
            assert!(!first.contains("inbound-secret"));
        }

        #[test]
        fn browser_authorization_signs_its_revalidated_identity_without_a_product_grant() {
            let keys = keys();
            let source_expiry = now_unix() + 5;
            let token = keys
                .issue_browser_depot_delegation(
                    &target(),
                    &browser_authorization(source_expiry),
                    DepotDelegationScope::Write,
                    "depot.ingest.start",
                    &serde_json::json!({"kind":"archive"}),
                    30,
                )
                .unwrap();
            let claims: ProductDepotDelegationClaims = keys.decode_custom_token(
                &token,
                "https://depot.example",
                "https://team-labby.example",
            );

            assert_eq!(claims.sub, "browser-person-456");
            assert_eq!(claims.act.sub, "labby-browser-prod");
            assert_eq!(claims.depot_organization_id, "browser-organization");
            assert_eq!(claims.depot_project_id, "browser-project");
            assert_eq!(claims.depot_membership_epoch, 17);
            assert_eq!(claims.depot_organization_policy_epoch, 16);
            assert_eq!(claims.depot_project_policy_epoch, 17);
            assert_eq!(claims.depot_operation, "depot.ingest.start");
            assert!(i64::try_from(claims.exp).unwrap() <= source_expiry);
        }

        #[test]
        fn distinct_membership_rows_share_the_project_policy_epoch_on_the_depot_wire() {
            let keys = keys();
            let product = grant(now_unix() + 600);
            let mut browser = browser_authorization(now_unix() + 600);
            browser.project_policy_epoch = product.project_policy_epoch;

            let product_token = keys
                .issue_depot_delegation(
                    &target(),
                    &product,
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"product.zip"}),
                    30,
                )
                .unwrap();
            let browser_token = keys
                .issue_browser_depot_delegation(
                    &target(),
                    &browser,
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"browser.zip"}),
                    30,
                )
                .unwrap();
            let product_claims: ProductDepotDelegationClaims = keys.decode_custom_token(
                &product_token,
                "https://depot.example",
                "https://team-labby.example",
            );
            let browser_claims: ProductDepotDelegationClaims = keys.decode_custom_token(
                &browser_token,
                "https://depot.example",
                "https://team-labby.example",
            );

            assert_ne!(product.membership_epoch, browser.membership_epoch);
            assert_eq!(product.membership_epoch, 5);
            assert_eq!(browser.membership_epoch, 15);
            assert_eq!(product_claims.depot_membership_epoch, 7);
            assert_eq!(browser_claims.depot_membership_epoch, 7);
        }

        #[test]
        fn request_digest_is_recursive_key_order_independent_and_value_sensitive() {
            let left = serde_json::json!({"z":[{"b":2,"a":1}],"a":"value"});
            let right = serde_json::json!({"a":"value","z":[{"a":1,"b":2}]});
            assert_eq!(
                depot_params_digest(&left).unwrap(),
                depot_params_digest(&right).unwrap()
            );
            assert_ne!(
                depot_params_digest(&left).unwrap(),
                depot_params_digest(&serde_json::json!({"a":"other","z":[{"a":1,"b":2}]})).unwrap()
            );
        }

        #[test]
        fn assertion_lifetime_is_bounded_and_clamped_to_source_grant() {
            let keys = keys();
            assert!(
                keys.issue_depot_delegation(
                    &target(),
                    &grant(now_unix() + 600),
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"skill.zip"}),
                    61
                )
                .is_err()
            );

            let source_expiry = now_unix() + 5;
            let token = keys
                .issue_depot_delegation(
                    &target(),
                    &grant(source_expiry),
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"skill.zip"}),
                    30,
                )
                .unwrap();
            let claims: ProductDepotDelegationClaims = keys.decode_custom_token(
                &token,
                "https://depot.example",
                "https://team-labby.example",
            );
            assert!(i64::try_from(claims.exp).unwrap() <= source_expiry);
        }

        #[test]
        fn malformed_server_authority_is_rejected_before_signing() {
            let keys = keys();
            let mut bad_target = target();
            bad_target.audience = " https://depot.example".into();
            assert!(
                keys.issue_depot_delegation(
                    &bad_target,
                    &grant(now_unix() + 600),
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"skill.zip"}),
                    30,
                )
                .is_err()
            );

            bad_target = target();
            bad_target.tenant_id = "tenant\nspoof".into();
            assert!(
                keys.issue_depot_delegation(
                    &bad_target,
                    &grant(now_unix() + 600),
                    DepotDelegationScope::Write,
                    "depot.uploads.create",
                    &serde_json::json!({"filename":"skill.zip"}),
                    30,
                )
                .is_err()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    fn claims() -> DepotDelegationClaims {
        DepotDelegationClaims {
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
            method: "POST".into(),
            resource: "/api/artifacts".into(),
            operation: "artifact.create".into(),
            intent_id: "i1".into(),
            content_digest: None,
            content_length: None,
            scopes: vec!["skills:write".into()],
            capabilities: capability_wire_names(&[Capability::ScopeCreate]),
            epochs: DelegatedAuthorityEpochs {
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
    fn signer() -> DepotDelegationSigner {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let der = key.to_pkcs8_der().unwrap();
        DepotDelegationSigner::new(
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
        assert_eq!(signer.issue(invalid).unwrap_err(), DelegationError::Invalid);
    }

    #[test]
    fn epochs_always_serialize_exactly_seven_keys() {
        let mut epochs = claims().epochs;
        epochs.team_membership = None;
        epochs.project_membership = None;
        let value = serde_json::to_value(&epochs).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), DelegatedAuthorityEpochs::WIRE_KEY_COUNT);
        assert!(object["team_membership"].is_null());
    }

    #[test]
    fn validation_rejects_each_malformed_claim() {
        let signer = signer();
        let cases: Vec<(&str, Box<dyn Fn(&mut DepotDelegationClaims)>)> = vec![
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
            (
                "unknown capability",
                Box::new(|c| c.capabilities = vec!["scope.root".into()]),
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
                DelegationError::Invalid,
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
        let signer = DepotDelegationSigner::from_seed("k1", seed).unwrap();
        assert_eq!(signer.active_key_id(), "k1");
        let token = signer.issue(claims()).unwrap();
        assert_eq!(
            jsonwebtoken::decode_header(&token).unwrap().kid.as_deref(),
            Some("k1")
        );

        assert_eq!(
            DepotDelegationSigner::new("k".into(), [("k".into(), vec![1, 2, 3])]).unwrap_err(),
            DelegationError::KeyMaterial
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
            DepotDelegationSigner::new("k0".into(), too_many).unwrap_err(),
            DelegationError::Invalid
        );

        assert_eq!(
            DepotDelegationSigner::from_encoded_seed("k1", Zeroizing::new("not-base64!!".into()))
                .unwrap_err(),
            DelegationError::KeyMaterial
        );
        assert_eq!(
            DepotDelegationSigner::from_encoded_seed("k1", Zeroizing::new("AAAA".into()))
                .unwrap_err(),
            DelegationError::KeyMaterial
        );
        assert_eq!(
            DepotDelegationSigner::from_seed_env("k1", "LABBY_TEST_DEPOT_DELEGATION_UNSET")
                .unwrap_err(),
            DelegationError::KeyMaterial
        );
        let good = || {
            Zeroizing::new(base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                [3_u8; 32],
            ))
        };
        let rotated = DepotDelegationSigner::from_encoded_seeds(
            "k2",
            [("k1".to_owned(), good()), ("k2".to_owned(), good())],
        )
        .unwrap();
        assert_eq!(rotated.active_key_id(), "k2");
        assert_eq!(rotated.key_ids().collect::<Vec<_>>(), vec!["k1", "k2"]);
        assert_eq!(
            DepotDelegationSigner::from_encoded_seeds("k9", [("k1".to_owned(), good())])
                .unwrap_err(),
            DelegationError::Invalid
        );
        assert_eq!(
            DepotDelegationSigner::from_encoded_seeds(
                "k1",
                [("k1".to_owned(), good()), ("k1".to_owned(), good())]
            )
            .unwrap_err(),
            DelegationError::Invalid
        );
    }
}
