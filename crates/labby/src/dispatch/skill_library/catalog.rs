use labby_primitives::action::{ActionSpec, ParamSpec};

#[cfg(test)]
pub(crate) const ACTION_NAMES: &[&str] = &[
    "artifacts.search",
    "artifacts.list",
    "artifacts.get",
    "artifacts.read",
    "artifacts.history",
    "artifacts.validate",
    "artifacts.create",
    "artifacts.save",
    "artifacts.activate",
    "artifacts.deactivate",
    "artifacts.archive",
    "artifacts.rollback",
    "artifacts.import",
    "artifacts.import_batch",
    "artifacts.refresh",
    "artifacts.search_remote",
    "artifacts.list_remote",
    "artifacts.get_remote",
    "artifacts.list_candidates",
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
const SEARCH_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "query",
        ty: "string",
        required: true,
        description: "Non-empty metadata query, maximum 256 bytes",
    },
    PAGE[0],
    PAGE[1],
];
const REMOTE_CONNECTION: ParamSpec = ParamSpec {
    name: "connection_id",
    ty: "string",
    required: false,
    description: "Configured authority; optional when exactly one is configured",
};
const REMOTE_SEARCH_PARAMS: &[ParamSpec] = &[
    REMOTE_CONNECTION,
    ParamSpec {
        name: "query",
        ty: "string",
        required: true,
        description: "Case-insensitive remote catalog query",
    },
    PAGE[1],
];
const REMOTE_PAGE_PARAMS: &[ParamSpec] = &[REMOTE_CONNECTION, PAGE[0], PAGE[1]];
const REMOTE_GET_PARAMS: &[ParamSpec] = &[
    REMOTE_CONNECTION,
    ParamSpec {
        name: "id",
        ty: "string",
        required: true,
        description: "Stable remote Artifact id",
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
    description: "Creation scope: private by default, or explicitly shared",
};
const SOURCE: ParamSpec = ParamSpec {
    name: "source",
    ty: "object",
    required: true,
    description: "Server connection id plus exact immutable source, Artifact, and revision selector; never bytes, paths, endpoints, or credentials",
};
const SOURCES: ParamSpec = ParamSpec {
    name: "sources",
    ty: "array",
    required: true,
    description: "One to 100 exact immutable source selectors; acquisition is completed before the first local commit",
};
const IMPORT_PARAMS: &[ParamSpec] = &[SOURCE, VERSION, IDEM];
const IMPORT_BATCH_PARAMS: &[ParamSpec] = &[SOURCES, VERSION, IDEM];
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

pub(crate) const ACTIONS: [ActionSpec; 19] = [
    spec(
        "artifacts.search",
        "Search caller-visible stored Artifacts by identity and descriptive metadata",
        false,
        false,
        "VersionedArtifactPage",
        SEARCH_PARAMS,
    ),
    spec(
        "artifacts.list",
        "List caller-visible stored Skill summaries",
        false,
        false,
        "VersionedSkillLibraryPage",
        PAGE,
    ),
    spec(
        "artifacts.get",
        "Get one stored Skill summary without file bodies",
        false,
        false,
        "VersionedSkillLibrarySummary",
        &[ARTIFACT],
    ),
    spec(
        "artifacts.read",
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
        "artifacts.history",
        "List immutable revision summaries in stable order",
        false,
        false,
        "VersionedRevisionPage",
        HISTORY_PARAMS,
    ),
    spec(
        "artifacts.validate",
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
        "artifacts.create",
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
        "artifacts.save",
        "Save a new immutable revision without activating it",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, REVISION, FILES, VERSION, IDEM],
    ),
    spec(
        "artifacts.activate",
        "Activate an exact stored revision as a new generation",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
    spec(
        "artifacts.deactivate",
        "Deactivate a Skill while retaining immutable revisions",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "artifacts.archive",
        "Archive a Skill while retaining immutable revision storage",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, VERSION, IDEM],
    ),
    spec(
        "artifacts.rollback",
        "Activate a prior immutable revision as a new generation",
        false,
        false,
        "SkillMutationReceipt",
        &[ARTIFACT, REVISION, VERSION, IDEM],
    ),
    spec(
        "artifacts.import",
        "Import an exact revision through a server-configured source without implicit activation",
        false,
        false,
        "SkillMutationReceipt",
        IMPORT_PARAMS,
    ),
    spec(
        "artifacts.import_batch",
        "Import a bounded batch of exact revisions through server-configured sources without implicit activation",
        false,
        false,
        "ArtifactImportBatchReceipt",
        IMPORT_BATCH_PARAMS,
    ),
    spec(
        "artifacts.refresh",
        "Explicitly rebuild and reconcile the committed active generation",
        false,
        false,
        "SkillMutationReceipt",
        &[VERSION, IDEM],
    ),
    spec(
        "artifacts.search_remote",
        "Search the configured remote Artifact catalog",
        false,
        false,
        "RemoteArtifactSearch",
        REMOTE_SEARCH_PARAMS,
    ),
    spec(
        "artifacts.list_remote",
        "List the combined hosted and projected remote Artifact catalog",
        false,
        false,
        "RemoteArtifactPage",
        REMOTE_PAGE_PARAMS,
    ),
    spec(
        "artifacts.get_remote",
        "Get one remote Artifact by stable identifier",
        false,
        false,
        "RemoteArtifact",
        REMOTE_GET_PARAMS,
    ),
    spec(
        "artifacts.list_candidates",
        "List remote discovery candidates awaiting intake",
        false,
        true,
        "ArtifactCandidatePage",
        REMOTE_PAGE_PARAMS,
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
        assert!(ACTIONS.iter().all(|a| !a.destructive));
        assert!(
            ACTIONS
                .iter()
                .filter(|a| a.name != "artifacts.list_candidates")
                .all(|a| !a.requires_admin)
        );
        assert!(
            ACTIONS
                .iter()
                .find(|a| a.name == "artifacts.list_candidates")
                .unwrap()
                .requires_admin
        );
    }
}
