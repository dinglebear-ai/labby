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
pub struct DepotDelegationClaims {
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

        let claims = DepotDelegationClaims {
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
        let claims: DepotDelegationClaims = keys.decode_custom_token(
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
        let claims: DepotDelegationClaims = keys.decode_custom_token(
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
        let product_claims: DepotDelegationClaims = keys.decode_custom_token(
            &product_token,
            "https://depot.example",
            "https://team-labby.example",
        );
        let browser_claims: DepotDelegationClaims = keys.decode_custom_token(
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
        let claims: DepotDelegationClaims = keys.decode_custom_token(
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
