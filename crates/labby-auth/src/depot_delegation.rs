//! Short-lived, per-principal assertions for a selected Depot authority.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use labby_primitives::product_credential::BoundAccessGrant;
use serde::{Deserialize, Serialize};

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
        ttl_secs: u64,
    ) -> Result<String, AuthError> {
        validate_target(target)?;
        if ttl_secs == 0 || ttl_secs > MAX_DEPOT_DELEGATION_TTL_SECS {
            return Err(AuthError::InvalidGrant(
                "invalid Depot delegation lifetime".into(),
            ));
        }
        for value in [
            grant.installation_id.as_str(),
            grant.principal_id.as_str(),
            grant.organization_id.as_str(),
            grant.project_id.as_str(),
        ] {
            validate_identifier(value)?;
        }

        let now = now_unix();
        let ttl_secs = i64::try_from(ttl_secs)
            .map_err(|_| AuthError::InvalidGrant("invalid Depot delegation lifetime".into()))?;
        let source_expiry = i64::try_from(grant.expires_at)
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
            sub: grant.principal_id.clone(),
            scope: scope.as_str().into(),
            act: DelegatedActor {
                sub: grant.installation_id.clone(),
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
            depot_membership_epoch: grant.membership_epoch,
            depot_organization_policy_epoch: grant.organization_policy_epoch,
            depot_project_policy_epoch: grant.project_policy_epoch,
            depot_organization_id: grant.organization_id.clone(),
            depot_project_id: grant.project_id.clone(),
        };
        self.issue_custom_token(&claims, "Depot delegation")
    }
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

    #[test]
    fn assertion_preserves_person_actor_and_current_authority_epochs() {
        let keys = keys();
        let token = keys
            .issue_depot_delegation(
                &target(),
                &grant(now_unix() + 600),
                DepotDelegationScope::Write,
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
        assert_eq!(claims.depot_membership_epoch, 5);
        assert_eq!(claims.depot_organization_policy_epoch, 6);
        assert_eq!(claims.depot_project_policy_epoch, 7);
        assert!(claims.exp - claims.iat <= 30);
        assert_eq!(claims.jti.len(), 32);
    }

    #[test]
    fn assertions_are_fresh_and_never_accept_an_inbound_bearer() {
        let keys = keys();
        let grant = grant(now_unix() + 600);
        let first = keys
            .issue_depot_delegation(&target(), &grant, DepotDelegationScope::Read, 30)
            .unwrap();
        let second = keys
            .issue_depot_delegation(&target(), &grant, DepotDelegationScope::Read, 30)
            .unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("inbound-secret"));
    }

    #[test]
    fn assertion_lifetime_is_bounded_and_clamped_to_source_grant() {
        let keys = keys();
        assert!(
            keys.issue_depot_delegation(
                &target(),
                &grant(now_unix() + 600),
                DepotDelegationScope::Write,
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
                30,
            )
            .is_err()
        );
    }
}
