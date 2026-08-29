use labby_primitives::action::{ActionSpec, ParamSpec};

#[cfg(test)]
pub(crate) const ACTION_NAMES: &[&str] = &[
    "skill_library.list",
    "skill_library.get",
    "skill_library.read",
    "skill_library.history",
    "skill_library.diff",
    "skill_library.validate",
    "skill_library.preview",
    "skill_library.create",
    "skill_library.save",
    "skill_library.activate",
    "skill_library.deactivate",
    "skill_library.archive",
    "skill_library.restore",
    "skill_library.rollback",
    "skill_library.import",
    "skill_library.refresh",
];

const ARTIFACT: ParamSpec = ParamSpec {
    name: "artifact_id",
    ty: "string",
    required: true,
    description: "Canonical Skill Artifact id",
};
const REVISION: ParamSpec = ParamSpec {
    name: "expected_revision_id",
    ty: "string",
    required: true,
    description: "Expected immutable revision precondition",
};
const VERSION: ParamSpec = ParamSpec {
    name: "expected_library_version",
    ty: "integer",
    required: true,
    description: "Expected committed Skill Library version",
};
const IDEM: ParamSpec = ParamSpec {
    name: "idempotency_key",
    ty: "string",
    required: true,
    description: "Bounded request idempotency key (maximum 256 bytes)",
};
const PAGE: &[ParamSpec] = &[
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Opaque bounded pagination cursor",
    },
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size, default 50 and maximum 100",
    },
];
const FILES: ParamSpec = ParamSpec {
    name: "files",
    ty: "array",
    required: true,
    description: "Bounded logical UTF-8 Skill files; bodies are never returned by list/history",
};
const VISIBILITY: ParamSpec = ParamSpec {
    name: "visibility",
    ty: "string",
    required: false,
    description: "Creation scope: private (default for backward compatibility) or shared",
};
const HISTORY_PARAMS: &[ParamSpec] = &[
    ARTIFACT,
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Opaque bounded pagination cursor",
    },
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size, default 50 and maximum 100",
    },
];

pub(crate) const ACTIONS: [ActionSpec; 16] = [
    spec(
        "skill_library.list",
        "List caller-visible stored Skill summaries",
        false,
        false,
        "VersionedSkillLibraryPage",
        PAGE,
    ),
    spec(
        "skill_library.get",
        "Get one stored Skill summary without file bodies",
        false,
        false,
        "VersionedSkillLibrarySummary",
        &[ARTIFACT],
    ),
    spec(
        "skill_library.read",
        "Read one bounded file from an immutable stored revision",
        false,
        false,
        "VersionedRevisionFile",
        &[
            ARTIFACT,
            ParamSpec {
                name: "revision_id",
                ty: "string",
                required: true,
                description: "Immutable revision id",
            },
            ParamSpec {
                name: "path",
                ty: "string",
                required: true,
                description: "Manifest-bound logical file path",
            },
        ],
    ),
    spec(
        "skill_library.history",
        "List immutable revision summaries in stable order",
        false,
        false,
        "VersionedRevisionPage",
        HISTORY_PARAMS,
    ),
    spec(
        "skill_library.diff",
        "Compare two exact immutable Skill revisions without executing content",
        false,
        false,
        "ArtifactRevisionDiff",
        &[
            ARTIFACT,
            ParamSpec {
                name: "from_revision_id",
                ty: "string",
                required: true,
                description: "Base immutable revision id",
            },
            ParamSpec {
                name: "to_revision_id",
                ty: "string",
                required: true,
                description: "Target immutable revision id",
            },
        ],
    ),
    spec(
        "skill_library.validate",
        "Validate logical Skill files without storing or activating",
        false,
        false,
        "SkillValidation",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Skill name",
            },
            FILES,
        ],
    ),
    spec(
        "skill_library.preview",
        "Validate and return an inert bounded text preview without storing or activating",
        false,
        false,
        "SkillPreview",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Skill name",
            },
            FILES,
        ],
    ),
    spec(
        "skill_library.create",
        "Create a stored Skill without activating it; visibility defaults to private",
        false,
        false,
        "SkillMutationReceipt",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Skill name",
            },
            FILES,
            VISIBILITY,
            VERSION,
            IDEM,
        ],
    ),
    spec(
        "skill_library.save",
        "Save a new immutable revision without activating it",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, REVISION, FILES, VERSION, IDEM],
    ),
    spec(
        "skill_library.activate",
        "Activate an exact stored revision as a new generation",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
    spec(
        "skill_library.deactivate",
        "Deactivate a Skill while retaining immutable revisions",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "skill_library.archive",
        "Archive a Skill while retaining immutable revision storage",
        true,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "skill_library.restore",
        "Restore an archived Skill as inactive while retaining immutable revisions",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "skill_library.rollback",
        "Activate a prior immutable revision as a new generation",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
    spec(
        "skill_library.import",
        "Import an exact revision through a server-configured source without implicit activation",
        false,
        false,
        "SkillMutationReceipt",
        &[
            ParamSpec {
                name: "source",
                ty: "object",
                required: true,
                description: "Server connection id plus exact immutable source, Artifact, and revision selector; never bytes, paths, endpoints, or credentials",
            },
            VERSION,
            IDEM,
        ],
    ),
    spec(
        "skill_library.refresh",
        "Explicitly rebuild and reconcile the committed active generation",
        false,
        false,
        "SkillMutationReceipt",
        &[VERSION, IDEM],
    ),
];

pub(crate) const PROMPT_ACTIONS: [ActionSpec; 14] = [
    spec(
        "prompt_library.list",
        "List caller-visible stored Prompt summaries",
        false,
        false,
        "VersionedPromptLibraryPage",
        PAGE,
    ),
    spec(
        "prompt_library.get",
        "Get one stored Prompt summary without bodies",
        false,
        false,
        "VersionedPromptLibrarySummary",
        &[ARTIFACT],
    ),
    spec(
        "prompt_library.read",
        "Read one exact immutable Prompt source",
        false,
        false,
        "VersionedRevisionFile",
        &[
            ARTIFACT,
            ParamSpec {
                name: "revision_id",
                ty: "string",
                required: true,
                description: "Immutable revision id",
            },
            ParamSpec {
                name: "path",
                ty: "string",
                required: true,
                description: "PROMPT.md",
            },
        ],
    ),
    spec(
        "prompt_library.history",
        "List immutable Prompt revisions",
        false,
        false,
        "VersionedRevisionPage",
        HISTORY_PARAMS,
    ),
    spec(
        "prompt_library.diff",
        "Compare two immutable Prompt revisions",
        false,
        false,
        "ArtifactRevisionDiff",
        &[
            ARTIFACT,
            ParamSpec {
                name: "from_revision_id",
                ty: "string",
                required: true,
                description: "Base revision",
            },
            ParamSpec {
                name: "to_revision_id",
                ty: "string",
                required: true,
                description: "Target revision",
            },
        ],
    ),
    spec(
        "prompt_library.validate",
        "Validate Prompt source without persistence",
        false,
        false,
        "PromptValidation",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Prompt name",
            },
            FILES,
        ],
    ),
    spec(
        "prompt_library.preview",
        "Return escaped inert Prompt text without persistence",
        false,
        false,
        "PromptPreview",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Prompt name",
            },
            FILES,
        ],
    ),
    spec(
        "prompt_library.create",
        "Create an inactive Prompt workspace and immutable revision",
        false,
        false,
        "PromptMutationReceipt",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Prompt name",
            },
            FILES,
            VISIBILITY,
            VERSION,
            IDEM,
        ],
    ),
    spec(
        "prompt_library.save",
        "Save a new immutable Prompt revision without activation",
        false,
        false,
        "PromptMutationReceipt",
        &[ARTIFACT, REVISION, FILES, VERSION, IDEM],
    ),
    spec(
        "prompt_library.activate",
        "Activate an exact Prompt revision",
        false,
        false,
        "PromptMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
    spec(
        "prompt_library.deactivate",
        "Deactivate a Prompt while retaining revisions",
        false,
        false,
        "PromptMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "prompt_library.archive",
        "Archive a Prompt while retaining referenced revisions",
        true,
        false,
        "PromptMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "prompt_library.restore",
        "Restore an archived Prompt as inactive",
        false,
        false,
        "PromptMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "prompt_library.rollback",
        "Activate a prior immutable Prompt revision",
        false,
        false,
        "PromptMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
];

pub(crate) const AGENT_ACTIONS: [ActionSpec; 14] = [
    spec(
        "agent_library.list",
        "List caller-visible stored Agent summaries",
        false,
        false,
        "VersionedAgentLibraryPage",
        PAGE,
    ),
    spec(
        "agent_library.get",
        "Get one stored Agent summary without instructions",
        false,
        false,
        "VersionedAgentLibrarySummary",
        &[ARTIFACT],
    ),
    spec(
        "agent_library.read",
        "Read one exact immutable Agent source",
        false,
        false,
        "VersionedRevisionFile",
        &[
            ARTIFACT,
            ParamSpec {
                name: "revision_id",
                ty: "string",
                required: true,
                description: "Immutable revision id",
            },
            ParamSpec {
                name: "path",
                ty: "string",
                required: true,
                description: "AGENT.md",
            },
        ],
    ),
    spec(
        "agent_library.history",
        "List immutable Agent revisions",
        false,
        false,
        "VersionedRevisionPage",
        HISTORY_PARAMS,
    ),
    spec(
        "agent_library.diff",
        "Compare two immutable Agent revisions",
        false,
        false,
        "ArtifactRevisionDiff",
        &[
            ARTIFACT,
            ParamSpec {
                name: "from_revision_id",
                ty: "string",
                required: true,
                description: "Base revision",
            },
            ParamSpec {
                name: "to_revision_id",
                ty: "string",
                required: true,
                description: "Target revision",
            },
        ],
    ),
    spec(
        "agent_library.validate",
        "Validate Agent source and capability references without persistence",
        false,
        false,
        "AgentValidation",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Agent name",
            },
            FILES,
        ],
    ),
    spec(
        "agent_library.preview",
        "Return inert Agent instructions without persistence",
        false,
        false,
        "AgentPreview",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Agent name",
            },
            FILES,
        ],
    ),
    spec(
        "agent_library.create",
        "Create an inactive Agent workspace and immutable revision",
        false,
        false,
        "AgentMutationReceipt",
        &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Agent name",
            },
            FILES,
            VISIBILITY,
            VERSION,
            IDEM,
        ],
    ),
    spec(
        "agent_library.save",
        "Save a new immutable Agent revision without activation",
        false,
        false,
        "AgentMutationReceipt",
        &[ARTIFACT, REVISION, FILES, VERSION, IDEM],
    ),
    spec(
        "agent_library.activate",
        "Explicitly activate an exact Agent revision",
        false,
        false,
        "AgentMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
    spec(
        "agent_library.deactivate",
        "Deactivate an Agent while retaining revisions",
        false,
        false,
        "AgentMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "agent_library.archive",
        "Archive an Agent while retaining referenced revisions",
        true,
        false,
        "AgentMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "agent_library.restore",
        "Restore an archived Agent as inactive",
        false,
        false,
        "AgentMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "agent_library.rollback",
        "Activate a prior immutable Agent revision",
        false,
        false,
        "AgentMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
];

const fn spec(
    name: &'static str,
    description: &'static str,
    destructive: bool,
    requires_admin: bool,
    returns: &'static str,
    params: &'static [ParamSpec],
) -> ActionSpec {
    ActionSpec {
        name,
        description,
        destructive,
        requires_admin,
        returns,
        params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_destructive_policy_are_stable() {
        assert_eq!(
            ACTIONS.iter().map(|a| a.name).collect::<Vec<_>>(),
            ACTION_NAMES
        );
        assert_eq!(
            ACTIONS
                .iter()
                .filter(|a| a.destructive)
                .map(|a| a.name)
                .collect::<Vec<_>>(),
            ["skill_library.archive"]
        );
        assert!(ACTIONS.iter().all(|a| !a.requires_admin));
    }
}
