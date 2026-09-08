//! Browser adapter for the host-owned, single-project Depot read boundary.
use super::*;
use crate::access::{AuthorizeProjectInput, Permission, ProjectPermissionSnapshot};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest as _, Sha256};

pub(super) struct ReadAccess(Option<ProjectPermissionSnapshot>);

impl ReadAccess {
    pub(super) async fn begin(
        state: &AppState,
        authority: &BrowserAuthority,
        auth: Option<&AuthContext>,
        identity: Option<&VerifiedIdentity>,
    ) -> Result<Self, (StatusCode, Json<Value>)> {
        require_read(authority).await?;
        let Some(project) = state.config.depot.read_project_id.as_ref() else {
            return Ok(Self(None));
        };
        let auth = auth.filter(|auth| auth.via_session).ok_or_else(forbidden)?;
        let identity = identity.ok_or_else(forbidden)?;
        if project.trim().is_empty()
            || matches!(identity.principal_link(), PrincipalLink::External { subject, .. } if subject != &auth.sub)
        {
            return Err(forbidden());
        }
        let store = state
            .access_runtime
            .store()
            .await
            .map_err(|_| forbidden())?;
        let access = store
            .authorize_project(AuthorizeProjectInput::new(
                identity.clone(),
                project.clone(),
                Permission::AssetDiscover,
            ))
            .await
            .map_err(|_| forbidden())?;
        Ok(Self(Some(access)))
    }

    pub(super) async fn finish<T>(
        &self,
        state: &AppState,
        authority: &BrowserAuthority,
        auth: Option<&AuthContext>,
        identity: Option<&VerifiedIdentity>,
        result: Result<T, (StatusCode, Json<Value>)>,
    ) -> Result<T, (StatusCode, Json<Value>)> {
        let current = Self::begin(state, authority, auth, identity).await?;
        if self.0 != current.0 {
            return Err(forbidden());
        }
        result
    }

    /// Only the server constructs this digest; it never comes from request JSON.
    pub(super) fn epoch(&self) -> Option<String> {
        self.0.as_ref().map(|access| {
            let facts = json!({
                "principal": access.principal_id,
                "organization": access.organization_id,
                "project": access.project_id,
                "role": format!("{:?}", access.role),
                "loadout": access.loadout_name,
                "revision": access.global_revision,
                "membership": access.membership_epoch,
                "organizationPolicy": access.organization_policy_epoch,
                "projectPolicy": access.project_policy_epoch,
                "assignment": access.assignment_generation,
            });
            URL_SAFE_NO_PAD.encode(Sha256::digest(facts.to_string().as_bytes()))
        })
    }
}

#[cfg(test)]
#[path = "depot_read_access_test.rs"]
mod tests;
