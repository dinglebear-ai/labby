use labby_primitives::action::{ActionSpec, ParamSpec};

const ORIGIN_PARAM: ParamSpec = ParamSpec {
    name: "origin",
    ty: "string",
    required: false,
    description: "Optional visible origin label to restrict results",
};

const URI_PARAM: ParamSpec = ParamSpec {
    name: "uri",
    ty: "string",
    required: true,
    description: "Published skill or skill-file URI",
};

pub(crate) const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        returns: "Catalog",
        params: &[],
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        returns: "Schema",
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
    },
    ActionSpec {
        name: "skills.list",
        description: "List compact metadata for caller-visible Agent Skills",
        destructive: false,
        requires_admin: false,
        returns: "SkillListResponse",
        params: &[
            ORIGIN_PARAM,
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Maximum results, default 100 and maximum 500",
            },
        ],
    },
    ActionSpec {
        name: "skills.search",
        description: "Search caller-visible Agent Skills by metadata without loading bodies",
        destructive: false,
        requires_admin: false,
        returns: "SkillSearchResponse",
        params: &[
            ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Non-empty metadata search query",
            },
            ORIGIN_PARAM,
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Maximum matches, default 20 and maximum 100",
            },
        ],
    },
    ActionSpec {
        name: "skills.get",
        description: "Resolve one caller-visible skill entry by published URI",
        destructive: false,
        requires_admin: false,
        returns: "SkillGetResponse",
        params: &[URI_PARAM],
    },
    ActionSpec {
        name: "skills.read",
        description: "Read one manifest-bound verified text file from a caller-visible skill",
        destructive: false,
        requires_admin: false,
        returns: "VisibleSkillFile",
        params: &[URI_PARAM],
    },
];

/// The HTTP control plane is Artifact-centered; native Skill protocol reads do
/// not share its action namespace.
pub(crate) fn api_actions() -> &'static [ActionSpec] {
    &crate::dispatch::artifacts::ACTIONS
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn api_catalog_contains_artifact_management_without_collisions() {
        let names = api_actions()
            .iter()
            .map(|action| action.name)
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 30);
        assert_eq!(
            names.iter().copied().collect::<BTreeSet<_>>().len(),
            names.len()
        );
        assert!(names.contains(&"artifacts.search"));
        assert!(names.contains(&"artifacts.activate"));
        assert!(!names.iter().any(|name| name.contains("list_changed")));
        for action in api_actions().iter().filter(|action| {
            matches!(
                action.name,
                "artifacts.create"
                    | "artifacts.save"
                    | "artifacts.activate"
                    | "artifacts.deactivate"
                    | "artifacts.archive"
                    | "artifacts.rollback"
                    | "artifacts.import"
                    | "artifacts.refresh"
            )
        }) {
            let names = action
                .params
                .iter()
                .map(|param| param.name)
                .collect::<BTreeSet<_>>();
            assert!(
                names.contains("expected_library_version"),
                "{} lacks CAS",
                action.name
            );
            assert!(
                names.contains("idempotency_key"),
                "{} lacks idempotency",
                action.name
            );
        }
    }

    #[test]
    fn shared_mcp_and_cli_catalog_excludes_management_and_callbacks() {
        assert_eq!(ACTIONS.len(), 6);
        assert!(
            ACTIONS
                .iter()
                .all(|action| !action.name.starts_with("artifacts."))
        );
        assert!(
            ACTIONS
                .iter()
                .all(|action| !action.name.contains("callback"))
        );
    }
}
