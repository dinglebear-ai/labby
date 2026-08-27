use super::domain::{
    Permission, Principal, PrincipalId, Project, ProjectId, ProjectLoadout, ProjectMembership,
    ProjectRole,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DenyReason {
    OrganizationMismatch,
    ProjectMembershipRequired,
    ProjectMembershipAmbiguous,
    ProjectLoadoutUnavailable,
    ProjectLoadoutAmbiguous,
    PermissionMissing,
}

impl DenyReason {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::OrganizationMismatch => "organization_mismatch",
            Self::ProjectMembershipRequired => "project_membership_required",
            Self::ProjectMembershipAmbiguous => "project_membership_ambiguous",
            Self::ProjectLoadoutUnavailable => "project_loadout_unavailable",
            Self::ProjectLoadoutAmbiguous => "project_loadout_ambiguous",
            Self::PermissionMissing => "permission_missing",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EffectiveProjectWorkspace {
    principal_id: PrincipalId,
    project_id: ProjectId,
    role: ProjectRole,
    loadout_name: String,
}

impl EffectiveProjectWorkspace {
    pub(super) fn principal_id(&self) -> &PrincipalId {
        &self.principal_id
    }

    pub(super) fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    pub(super) const fn role(&self) -> ProjectRole {
        self.role
    }

    pub(super) fn loadout_name(&self) -> &str {
        &self.loadout_name
    }

    pub(super) fn allows(&self, permission: Permission) -> bool {
        self.role.permissions().contains(&permission)
    }
}

pub(super) fn resolve_project(
    principal: &Principal,
    project: &Project,
    memberships: &[ProjectMembership],
    loadouts: &[ProjectLoadout],
) -> Result<EffectiveProjectWorkspace, DenyReason> {
    if principal.organization_id() != project.organization_id() {
        return Err(DenyReason::OrganizationMismatch);
    }

    let mut matching_memberships = memberships.iter().filter(|membership| {
        let same_organization = membership.organization_id() == project.organization_id();
        let same_principal = membership.principal_id() == principal.id();
        let same_project = membership.project_id() == project.id();
        same_organization && same_principal && same_project
    });
    let membership = matching_memberships
        .next()
        .ok_or(DenyReason::ProjectMembershipRequired)?;
    if matching_memberships.next().is_some() {
        return Err(DenyReason::ProjectMembershipAmbiguous);
    }

    let mut matching_loadouts = loadouts.iter().filter(|loadout| {
        let same_organization = loadout.organization_id() == project.organization_id();
        let same_project = loadout.project_id() == project.id();
        same_organization && same_project
    });
    let loadout = matching_loadouts
        .next()
        .ok_or(DenyReason::ProjectLoadoutUnavailable)?;
    if matching_loadouts.next().is_some() {
        return Err(DenyReason::ProjectLoadoutAmbiguous);
    }

    Ok(EffectiveProjectWorkspace {
        principal_id: principal.id().clone(),
        project_id: project.id().clone(),
        role: membership.role(),
        loadout_name: loadout.loadout_name().to_owned(),
    })
}

pub(super) fn check_project_permission(
    principal: &Principal,
    project: &Project,
    memberships: &[ProjectMembership],
    loadouts: &[ProjectLoadout],
    permission: Permission,
) -> Result<(), DenyReason> {
    let workspace = resolve_project(principal, project, memberships, loadouts)?;
    if workspace.allows(permission) {
        Ok(())
    } else {
        Err(DenyReason::PermissionMissing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::domain::{
        Organization, OrganizationId, Permission, Principal, PrincipalId, Project, ProjectId,
        ProjectLoadout, ProjectMembership, ProjectRole,
    };

    struct Fixture {
        principal: Principal,
        project: Project,
        membership: ProjectMembership,
        loadout: ProjectLoadout,
    }

    impl Fixture {
        fn new() -> Self {
            let organization = Organization::new(OrganizationId::new("engineering").unwrap());
            let principal = Principal::new(
                PrincipalId::new("alice").unwrap(),
                organization.id().clone(),
            );
            let project = Project::new(
                ProjectId::new("phoenix").unwrap(),
                organization.id().clone(),
            );
            let membership =
                ProjectMembership::new(&principal, &project, ProjectRole::Member).unwrap();
            let loadout = ProjectLoadout::new(&project, "production").unwrap();
            Self {
                principal,
                project,
                membership,
                loadout,
            }
        }
    }

    #[test]
    fn resolution_defaults_to_a_stable_membership_denial() {
        let fixture = Fixture::new();
        let result = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[],
            &[fixture.loadout],
        );

        assert_eq!(result.unwrap_err().code(), "project_membership_required");
    }

    #[test]
    fn resolution_fails_closed_when_the_project_has_no_loadout() {
        let fixture = Fixture::new();
        let result = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[fixture.membership],
            &[],
        );

        assert_eq!(result.unwrap_err().code(), "project_loadout_unavailable");
    }

    #[test]
    fn resolution_fails_closed_on_ambiguous_memberships_or_loadouts() {
        let fixture = Fixture::new();
        let duplicate_membership = fixture.membership.clone();
        let error = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[fixture.membership.clone(), duplicate_membership],
            &[fixture.loadout.clone()],
        )
        .unwrap_err();
        assert_eq!(error.code(), "project_membership_ambiguous");

        let duplicate_loadout = fixture.loadout.clone();
        let error = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[fixture.membership],
            &[fixture.loadout, duplicate_loadout],
        )
        .unwrap_err();
        assert_eq!(error.code(), "project_loadout_ambiguous");
    }

    #[test]
    fn resolution_returns_only_the_direct_membership_and_named_loadout() {
        let fixture = Fixture::new();
        let workspace = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[fixture.membership],
            &[fixture.loadout],
        )
        .unwrap();

        assert_eq!(workspace.principal_id(), fixture.principal.id());
        assert_eq!(workspace.project_id(), fixture.project.id());
        assert_eq!(workspace.role(), ProjectRole::Member);
        assert_eq!(workspace.loadout_name(), "production");
        assert!(workspace.allows(Permission::AssetUse));
        assert!(!workspace.allows(Permission::ProjectManage));
    }

    #[test]
    fn direct_checks_re_resolve_instead_of_accepting_a_previous_workspace() {
        let fixture = Fixture::new();
        assert!(
            check_project_permission(
                &fixture.principal,
                &fixture.project,
                &[fixture.membership.clone()],
                &[fixture.loadout.clone()],
                Permission::AssetUse,
            )
            .is_ok()
        );

        let error = check_project_permission(
            &fixture.principal,
            &fixture.project,
            &[],
            &[fixture.loadout],
            Permission::AssetUse,
        )
        .unwrap_err();
        assert_eq!(error.code(), "project_membership_required");
    }

    #[test]
    fn insufficient_role_uses_a_stable_non_enumerating_denial() {
        let mut fixture = Fixture::new();
        fixture.membership =
            ProjectMembership::new(&fixture.principal, &fixture.project, ProjectRole::Viewer)
                .unwrap();

        let error = check_project_permission(
            &fixture.principal,
            &fixture.project,
            &[fixture.membership],
            &[fixture.loadout],
            Permission::AssetUse,
        )
        .unwrap_err();
        assert_eq!(error.code(), "permission_missing");
    }

    #[test]
    fn same_ids_from_another_organization_never_supply_membership_or_loadout() {
        let fixture = Fixture::new();
        let other_organization =
            Organization::new(OrganizationId::new("other-organization").unwrap());
        let other_principal = Principal::new(
            fixture.principal.id().clone(),
            other_organization.id().clone(),
        );
        let other_project = Project::new(
            fixture.project.id().clone(),
            other_organization.id().clone(),
        );
        let other_membership =
            ProjectMembership::new(&other_principal, &other_project, ProjectRole::Member).unwrap();
        let other_loadout = ProjectLoadout::new(&other_project, "other-production").unwrap();

        let membership_error = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[other_membership],
            &[fixture.loadout.clone()],
        )
        .unwrap_err();
        assert_eq!(membership_error.code(), "project_membership_required");

        let loadout_error = resolve_project(
            &fixture.principal,
            &fixture.project,
            &[fixture.membership],
            &[other_loadout],
        )
        .unwrap_err();
        assert_eq!(loadout_error.code(), "project_loadout_unavailable");
    }
}
