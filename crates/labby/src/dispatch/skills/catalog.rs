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

const fn api_actions() -> [ActionSpec; 64] {
    let mut result = [ACTIONS[0]; 64];
    let mut index = 0;
    while index < ACTIONS.len() {
        result[index] = ACTIONS[index];
        index += 1;
    }
    let mut management = 0;
    while management < crate::dispatch::skill_library::catalog::ACTIONS.len() {
        result[index] = crate::dispatch::skill_library::catalog::ACTIONS[management];
        index += 1;
        management += 1;
    }
    let mut prompts = 0;
    while prompts < crate::dispatch::skill_library::catalog::PROMPT_ACTIONS.len() {
        result[index] = crate::dispatch::skill_library::catalog::PROMPT_ACTIONS[prompts];
        index += 1;
        prompts += 1;
    }
    let mut agents = 0;
    while agents < crate::dispatch::skill_library::catalog::AGENT_ACTIONS.len() {
        result[index] = crate::dispatch::skill_library::catalog::AGENT_ACTIONS[agents];
        index += 1;
        agents += 1;
    }
    let mut hooks = 0;
    while hooks < crate::dispatch::skill_library::catalog::HOOK_ACTIONS.len() {
        result[index] = crate::dispatch::skill_library::catalog::HOOK_ACTIONS[hooks];
        index += 1;
        hooks += 1;
    }
    result
}

const ALL_MANAGEMENT_ACTIONS: [ActionSpec; 64] = api_actions();
/// Authenticated HTTP MCP/App contract: compatibility reads plus the bounded
/// Skill Library management vocabulary. Stdio and private in-process callers
/// intentionally retain [`ACTIONS`].
pub(crate) const MCP_ACTIONS: &[ActionSpec] = &ALL_MANAGEMENT_ACTIONS;
pub(crate) const API_ACTIONS: &[ActionSpec] = MCP_ACTIONS;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn api_catalog_contains_compatibility_and_management_without_collisions() {
        let names = API_ACTIONS
            .iter()
            .map(|action| action.name)
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 64);
        assert_eq!(
            names.iter().copied().collect::<BTreeSet<_>>().len(),
            names.len()
        );
        assert!(names.contains(&"skills.list"));
        assert!(names.contains(&"skill_library.activate"));
        assert!(names.contains(&"prompt_library.activate"));
        assert!(names.contains(&"agent_library.activate"));
        assert!(names.contains(&"hook_library.activate"));
        assert!(!names.iter().any(|name| name.contains("list_changed")));
        for action in API_ACTIONS.iter().filter(|action| {
            matches!(
                action.name,
                "skill_library.create"
                    | "skill_library.save"
                    | "skill_library.activate"
                    | "skill_library.deactivate"
                    | "skill_library.archive"
                    | "skill_library.rollback"
                    | "skill_library.import"
                    | "skill_library.refresh"
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
                .all(|action| !action.name.starts_with("skill_library."))
        );
        assert!(
            ACTIONS
                .iter()
                .all(|action| !action.name.contains("callback"))
        );
    }
}
