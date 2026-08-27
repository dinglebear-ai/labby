//! Auth-neutral vocabulary for checked, project-bound product credentials.
//!
//! This module deliberately owns no authentication, persistence, or product
//! policy. It gives authentication and product crates one strict wire parser
//! and typed, non-secret grants without creating an upward dependency.

use std::future::Future;
use std::pin::Pin;

use base64::Engine as _;

pub const PRODUCT_CREDENTIAL_PREFIX: &str = "lby_pc_v1_";
pub const PRODUCT_CREDENTIAL_SECRET_BYTES: usize = 32;
pub const PRODUCT_CREDENTIAL_SECRET_ENCODED_LEN: usize = 43;
pub const PRODUCT_CREDENTIAL_ID_MAX_LEN: usize = 64;
pub const PRODUCT_CREDENTIAL_WIRE_MAX_LEN: usize = PRODUCT_CREDENTIAL_PREFIX.len()
    + PRODUCT_CREDENTIAL_ID_MAX_LEN
    + 1
    + PRODUCT_CREDENTIAL_SECRET_ENCODED_LEN;

/// A strictly parsed product credential.
///
/// This type intentionally does not implement `Debug`, `Display`, `Serialize`,
/// or `Clone`: its secret must not enter diagnostics or be duplicated casually.
pub struct ProductCredential {
    credential_id: String,
    secret: [u8; PRODUCT_CREDENTIAL_SECRET_BYTES],
}

impl ProductCredential {
    pub fn credential_id(&self) -> &str {
        &self.credential_id
    }

    /// Secret bytes for the injected verifier's constant-time digest check.
    /// Callers must not log, persist, or serialize this value.
    pub fn secret(&self) -> &[u8; PRODUCT_CREDENTIAL_SECRET_BYTES] {
        &self.secret
    }

    pub fn parse(token: &str) -> Result<Self, ProductCredentialParseError> {
        if token.len() > PRODUCT_CREDENTIAL_WIRE_MAX_LEN {
            return Err(ProductCredentialParseError::TooLong);
        }
        if !token.is_ascii() {
            return Err(ProductCredentialParseError::Malformed);
        }
        let payload = token
            .strip_prefix(PRODUCT_CREDENTIAL_PREFIX)
            .ok_or(ProductCredentialParseError::WrongPrefix)?;
        let separator = payload
            .len()
            .checked_sub(PRODUCT_CREDENTIAL_SECRET_ENCODED_LEN + 1)
            .ok_or(ProductCredentialParseError::Malformed)?;
        if payload.as_bytes().get(separator) != Some(&b'_') {
            return Err(ProductCredentialParseError::Malformed);
        }
        let credential_id = &payload[..separator];
        let encoded_secret = &payload[separator + 1..];
        if !valid_public_id(credential_id) {
            return Err(ProductCredentialParseError::InvalidCredentialId);
        }
        if encoded_secret.len() != PRODUCT_CREDENTIAL_SECRET_ENCODED_LEN
            || !encoded_secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ProductCredentialParseError::InvalidSecretEncoding);
        }
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded_secret)
            .map_err(|_| ProductCredentialParseError::InvalidSecretEncoding)?;
        let secret = decoded
            .try_into()
            .map_err(|_| ProductCredentialParseError::InvalidSecretLength)?;
        Ok(Self {
            credential_id: credential_id.to_owned(),
            secret,
        })
    }
}

/// Prefix selection result used to prevent malformed product credentials from
/// falling through to static bearer or JWT verification.
pub enum ProductCredentialSelection {
    NotProductCredential,
    Malformed(ProductCredentialParseError),
    Parsed(ProductCredential),
}

pub fn select_product_credential(token: &str) -> ProductCredentialSelection {
    if !token.starts_with(PRODUCT_CREDENTIAL_PREFIX) {
        return ProductCredentialSelection::NotProductCredential;
    }
    match ProductCredential::parse(token) {
        Ok(credential) => ProductCredentialSelection::Parsed(credential),
        Err(error) => ProductCredentialSelection::Malformed(error),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProductCredentialParseError {
    #[error("not a product credential")]
    WrongPrefix,
    #[error("product credential exceeds the maximum length")]
    TooLong,
    #[error("product credential has an invalid canonical form")]
    Malformed,
    #[error("product credential ID is invalid")]
    InvalidCredentialId,
    #[error("product credential secret encoding is invalid")]
    InvalidSecretEncoding,
    #[error("product credential secret length is invalid")]
    InvalidSecretLength,
}

fn valid_public_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= PRODUCT_CREDENTIAL_ID_MAX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// Auth-neutral facts proven by the credential verifier.
#[derive(Clone, Eq, PartialEq)]
pub struct ProductCredentialGrant {
    pub issuer: String,
    pub subject: String,
    pub credential_id: String,
    pub credential_generation: u64,
    pub scopes: Vec<String>,
    pub resource: String,
    pub audience: String,
    pub expires_at: u64,
}

/// Exact product authorization binding resolved after credential verification.
///
/// This intentionally does not implement `Debug`; subject and binding material
/// must pass through redacted observability adapters.
#[derive(Clone, Eq, PartialEq)]
pub struct BoundAccessGrant {
    pub installation_id: String,
    pub issuer: String,
    pub subject: String,
    pub principal_id: String,
    pub organization_id: String,
    pub project_id: String,
    pub loadout_id: String,
    pub loadout_generation: u64,
    pub assignment_generation: u64,
    pub catalog_generation: u64,
    pub route_id: String,
    pub route_generation: u64,
    pub membership_epoch: u64,
    pub organization_policy_epoch: u64,
    pub project_policy_epoch: u64,
    pub credential_id: String,
    pub credential_generation: u64,
    pub scopes: Vec<String>,
    pub resource: String,
    pub audience: String,
    pub expires_at: u64,
    pub requires_admin: bool,
    pub destructive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProductCredentialVerificationError {
    #[error("product credential denied")]
    Denied,
    #[error("product credential verification unavailable")]
    Unavailable,
}

pub type ProductCredentialVerificationFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<ProductCredentialGrant, ProductCredentialVerificationError>>
            + Send
            + 'a,
    >,
>;

/// Injected verifier seam. Implementations live above this dependency leaf.
pub trait ProductCredentialVerifier: Send + Sync {
    fn verify<'a>(
        &'a self,
        credential: &'a ProductCredential,
    ) -> ProductCredentialVerificationFuture<'a>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(id: &str, secret: [u8; 32]) -> String {
        format!(
            "{PRODUCT_CREDENTIAL_PREFIX}{id}_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)
        )
    }

    #[test]
    fn parses_exact_canonical_wire_form() {
        let parsed = ProductCredential::parse(&token("01JTEST-CREDENTIAL", [0xA5; 32])).unwrap();
        assert_eq!(parsed.credential_id(), "01JTEST-CREDENTIAL");
        assert_eq!(parsed.secret(), &[0xA5; 32]);
    }

    #[test]
    fn fixed_width_split_accepts_underscores_inside_base64url_secret() {
        let wire = token("credential-id", [0xFF; 32]);
        assert!(wire.rsplit_once('_').unwrap().1.len() < PRODUCT_CREDENTIAL_SECRET_ENCODED_LEN);
        let parsed = ProductCredential::parse(&wire).unwrap();
        assert_eq!(parsed.credential_id(), "credential-id");
        assert_eq!(parsed.secret(), &[0xFF; 32]);
    }

    #[test]
    fn selector_never_falls_through_a_malformed_product_prefix() {
        assert!(matches!(
            select_product_credential("ordinary.jwt.value"),
            ProductCredentialSelection::NotProductCredential
        ));
        assert!(matches!(
            select_product_credential("lby_pc_v1_bad"),
            ProductCredentialSelection::Malformed(_)
        ));
    }

    #[test]
    fn rejects_noncanonical_ids_lengths_alphabets_and_padding() {
        let valid_secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32]);
        for invalid in [
            format!("{PRODUCT_CREDENTIAL_PREFIX}_{valid_secret}"),
            format!("{PRODUCT_CREDENTIAL_PREFIX}bad_id_{valid_secret}"),
            format!(
                "{PRODUCT_CREDENTIAL_PREFIX}{}{valid_secret}",
                "x".repeat(65)
            ),
            format!("{PRODUCT_CREDENTIAL_PREFIX}id_{valid_secret}="),
            format!("{PRODUCT_CREDENTIAL_PREFIX}id_{}!", &valid_secret[..42]),
        ] {
            assert!(
                ProductCredential::parse(&invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn rejects_wrong_secret_entropy_and_oversized_input() {
        let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([3_u8; 31]);
        assert!(
            ProductCredential::parse(&format!("{PRODUCT_CREDENTIAL_PREFIX}id_{short}")).is_err()
        );
        let oversized = format!("{PRODUCT_CREDENTIAL_PREFIX}id_{}", "A".repeat(10_000));
        assert!(matches!(
            ProductCredential::parse(&oversized),
            Err(ProductCredentialParseError::TooLong)
        ));
    }

    #[test]
    fn admin_and_destructive_axes_remain_independent() {
        let combinations = [(false, false), (false, true), (true, false), (true, true)];
        for (requires_admin, destructive) in combinations {
            let grant = BoundAccessGrant {
                installation_id: "installation".into(),
                issuer: "issuer".into(),
                subject: "subject".into(),
                principal_id: "principal".into(),
                organization_id: "organization".into(),
                project_id: "project".into(),
                loadout_id: "loadout".into(),
                loadout_generation: 1,
                assignment_generation: 1,
                catalog_generation: 1,
                route_id: "route".into(),
                route_generation: 1,
                membership_epoch: 1,
                organization_policy_epoch: 1,
                project_policy_epoch: 1,
                credential_id: "credential".into(),
                credential_generation: 1,
                scopes: vec!["lab:read".into()],
                resource: "https://lab.example/resource".into(),
                audience: "https://lab.example".into(),
                expires_at: 1,
                requires_admin,
                destructive,
            };
            assert_eq!(grant.requires_admin, requires_admin);
            assert_eq!(grant.destructive, destructive);
        }
    }
}
