use std::fmt;

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub(super) struct $name(String);

        impl $name {
            pub(super) fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DomainError::EmptyId);
                }
                Ok(Self(value))
            }

            pub(super) fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_id!(PrincipalId);
opaque_id!(OrganizationId);
opaque_id!(ProjectId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DomainError {
    EmptyId,
    EmptyLoadoutName,
    OrganizationMismatch,
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyId => "identifier must not be empty",
            Self::EmptyLoadoutName => "loadout name must not be empty",
            Self::OrganizationMismatch => "access-control records must share an organization",
        })
    }
}

impl std::error::Error for DomainError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Organization {
    id: OrganizationId,
}

impl Organization {
    pub(super) fn new(id: OrganizationId) -> Self {
        Self { id }
    }

    pub(super) fn id(&self) -> &OrganizationId {
        &self.id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Principal {
    id: PrincipalId,
    organization_id: OrganizationId,
}

impl Principal {
    pub(super) fn new(id: PrincipalId, organization_id: OrganizationId) -> Self {
        Self {
            id,
            organization_id,
        }
    }

    pub(super) fn id(&self) -> &PrincipalId {
        &self.id
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Project {
    id: ProjectId,
    organization_id: OrganizationId,
}

impl Project {
    pub(super) fn new(id: ProjectId, organization_id: OrganizationId) -> Self {
        Self {
            id,
            organization_id,
        }
    }

    pub(super) fn id(&self) -> &ProjectId {
        &self.id
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Permission {
    ProjectRead,
    ProjectManage,
    AssetDiscover,
    AssetUse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectRole {
    Owner,
    Admin,
    Member,
    Viewer,
}

impl ProjectRole {
    const ADMIN_PERMISSIONS: [Permission; 4] = [
        Permission::ProjectRead,
        Permission::ProjectManage,
        Permission::AssetDiscover,
        Permission::AssetUse,
    ];
    const MEMBER_PERMISSIONS: [Permission; 3] = [
        Permission::ProjectRead,
        Permission::AssetDiscover,
        Permission::AssetUse,
    ];
    const VIEWER_PERMISSIONS: [Permission; 2] =
        [Permission::ProjectRead, Permission::AssetDiscover];

    pub(super) const fn permissions(self) -> &'static [Permission] {
        match self {
            Self::Owner | Self::Admin => &Self::ADMIN_PERMISSIONS,
            Self::Member => &Self::MEMBER_PERMISSIONS,
            Self::Viewer => &Self::VIEWER_PERMISSIONS,
        }
    }

    pub(super) fn from_persisted(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProjectMembership {
    organization_id: OrganizationId,
    principal_id: PrincipalId,
    project_id: ProjectId,
    role: ProjectRole,
}

impl ProjectMembership {
    pub(super) fn new(
        principal: &Principal,
        project: &Project,
        role: ProjectRole,
    ) -> Result<Self, DomainError> {
        if principal.organization_id() != project.organization_id() {
            return Err(DomainError::OrganizationMismatch);
        }
        Ok(Self {
            organization_id: project.organization_id().clone(),
            principal_id: principal.id().clone(),
            project_id: project.id().clone(),
            role,
        })
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub(super) fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    pub(super) fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub(super) const fn role(&self) -> ProjectRole {
        self.role
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProjectLoadout {
    organization_id: OrganizationId,
    project_id: ProjectId,
    loadout_name: String,
}

impl ProjectLoadout {
    pub(super) fn new(
        project: &Project,
        loadout_name: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let loadout_name = loadout_name.into();
        if loadout_name.trim().is_empty() {
            return Err(DomainError::EmptyLoadoutName);
        }
        Ok(Self {
            organization_id: project.organization_id().clone(),
            project_id: project.id().clone(),
            loadout_name,
        })
    }

    pub(super) fn organization_id(&self) -> &OrganizationId {
        &self.organization_id
    }

    pub(super) fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub(super) fn loadout_name(&self) -> &str {
        &self.loadout_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_ids_reject_empty_values_without_normalizing_valid_values() {
        assert!(PrincipalId::new("").is_err());
        assert!(OrganizationId::new("  ").is_err());
        assert!(ProjectId::new("\n").is_err());

        let id = PrincipalId::new(" Principal-A ").expect("non-empty ID");
        assert_eq!(id.as_str(), " Principal-A ");
    }

    #[test]
    fn project_membership_requires_the_principal_and_project_to_share_an_organization() {
        let org_a = Organization::new(OrganizationId::new("org-a").unwrap());
        let org_b = Organization::new(OrganizationId::new("org-b").unwrap());
        let principal = Principal::new(PrincipalId::new("alice").unwrap(), org_a.id().clone());
        let project = Project::new(ProjectId::new("phoenix").unwrap(), org_b.id().clone());

        let error = ProjectMembership::new(&principal, &project, ProjectRole::Member).unwrap_err();
        assert_eq!(error, DomainError::OrganizationMismatch);
    }

    #[test]
    fn project_membership_retains_its_organization_identity() {
        let organization = Organization::new(OrganizationId::new("org-a").unwrap());
        let principal = Principal::new(
            PrincipalId::new("alice").unwrap(),
            organization.id().clone(),
        );
        let project = Project::new(
            ProjectId::new("phoenix").unwrap(),
            organization.id().clone(),
        );

        let membership = ProjectMembership::new(&principal, &project, ProjectRole::Member).unwrap();
        assert_eq!(membership.organization_id(), organization.id());
    }

    #[test]
    fn project_roles_expand_to_the_milestone_one_permission_set() {
        assert_eq!(
            ProjectRole::Owner.permissions(),
            &[
                Permission::ProjectRead,
                Permission::ProjectManage,
                Permission::AssetDiscover,
                Permission::AssetUse,
            ]
        );
        assert_eq!(
            ProjectRole::Admin.permissions(),
            &[
                Permission::ProjectRead,
                Permission::ProjectManage,
                Permission::AssetDiscover,
                Permission::AssetUse,
            ]
        );
        assert_eq!(
            ProjectRole::Member.permissions(),
            &[
                Permission::ProjectRead,
                Permission::AssetDiscover,
                Permission::AssetUse,
            ]
        );
        assert_eq!(
            ProjectRole::Viewer.permissions(),
            &[Permission::ProjectRead, Permission::AssetDiscover]
        );
    }

    #[test]
    fn a_project_has_at_most_one_non_empty_named_loadout_mapping() {
        let organization_id = OrganizationId::new("engineering").unwrap();
        let project = Project::new(ProjectId::new("phoenix").unwrap(), organization_id.clone());
        assert_eq!(
            ProjectLoadout::new(&project, "  ").unwrap_err(),
            DomainError::EmptyLoadoutName
        );

        let mapping = ProjectLoadout::new(&project, "production").unwrap();
        assert_eq!(mapping.organization_id(), &organization_id);
        assert_eq!(mapping.project_id(), project.id());
        assert_eq!(mapping.loadout_name(), "production");
    }
}
