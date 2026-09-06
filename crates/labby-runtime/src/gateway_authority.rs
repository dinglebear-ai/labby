//! Tenant-qualified Gateway runtime identity and redacted credential custody.

use labby_primitives::access::OwnerScope;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GatewayAuthorityKey {
    pub owner: OwnerScope,
    pub project_id: Option<String>,
    pub loadout: String,
    pub authority_epoch: u64,
    pub credential_generation: u64,
}

impl GatewayAuthorityKey {
    #[must_use]
    pub fn partition_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            self.owner.id(),
            self.project_id.as_deref().unwrap_or("-"),
            self.loadout,
            self.authority_epoch,
            self.credential_generation
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamCredentialStatus {
    Active,
    Revoked,
}

/// Metadata only: secret bytes remain in the host credential store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeamCredentialBinding {
    pub binding_id: String,
    pub team_id: String,
    pub upstream_name: String,
    pub custodian_principal_id: String,
    pub generation: u64,
    pub rotated_at_millis: u64,
    pub status: TeamCredentialStatus,
}

impl TeamCredentialBinding {
    #[must_use]
    pub const fn usable(&self, expected_generation: u64) -> bool {
        matches!(self.status, TeamCredentialStatus::Active)
            && self.generation == expected_generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_primitives::access::{OwnerScope, TeamId};

    #[test]
    fn partition_changes_at_authority_boundaries() {
        let base = GatewayAuthorityKey {
            owner: OwnerScope::Team(TeamId::new("a").unwrap()),
            project_id: Some("p".into()),
            loadout: "l".into(),
            authority_epoch: 1,
            credential_generation: 1,
        };
        let mut rotated = base.clone();
        rotated.credential_generation = 2;
        let mut revoked = base.clone();
        revoked.authority_epoch = 2;
        assert_ne!(base.partition_key(), rotated.partition_key());
        assert_ne!(base.partition_key(), revoked.partition_key());
    }

    #[test]
    fn credential_projection_contains_no_secret_material() {
        let binding = TeamCredentialBinding {
            binding_id: "b".into(),
            team_id: "a".into(),
            upstream_name: "u".into(),
            custodian_principal_id: "p".into(),
            generation: 3,
            rotated_at_millis: 4,
            status: TeamCredentialStatus::Active,
        };
        let json = serde_json::to_string(&binding).unwrap();
        assert!(!json.contains("token"));
        assert!(binding.usable(3));
        assert!(!binding.usable(2));
    }
}
