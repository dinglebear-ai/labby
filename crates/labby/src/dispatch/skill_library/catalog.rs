use labby_primitives::action::{ActionSpec, ParamSpec};
use std::sync::LazyLock;

#[cfg(test)]
pub(crate) const LOCAL_ACTION_NAMES: &[&str] = &[
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
    "artifacts.transfer_options",
    "artifacts.pin",
    "artifacts.follow_managed",
    "artifacts.follow_update",
    "artifacts.fork_personal",
    "artifacts.refresh",
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
    description: "One to 100 exact immutable source selectors; each acquisition is committed before the next begins",
};
const ASSIGNMENT_OPTIONAL: ParamSpec = ParamSpec {
    name: "assignment_id",
    ty: "string",
    required: false,
    description: "Exact source Assignment/share whose distribution ceiling should be evaluated",
};
const ASSIGNMENT_REQUIRED: ParamSpec = ParamSpec {
    name: "assignment_id",
    ty: "string",
    required: true,
    description: "Exact active source Assignment/share authorizing managed byte materialization",
};
const UPDATE_POLICY: ParamSpec = ParamSpec {
    name: "update_policy",
    ty: "string",
    required: true,
    description: "Follow policy: notify, auto_approved, or pinned",
};
const IMPORT_PARAMS: &[ParamSpec] = &[SOURCE, VERSION, IDEM];
const IMPORT_BATCH_PARAMS: &[ParamSpec] = &[SOURCES, VERSION, IDEM];
const TRANSFER_OPTIONS_PARAMS: &[ParamSpec] = &[SOURCE, ASSIGNMENT_OPTIONAL];
const PIN_PARAMS: &[ParamSpec] = &[SOURCE, ASSIGNMENT_REQUIRED, IDEM];
const FOLLOW_PARAMS: &[ParamSpec] = &[SOURCE, ASSIGNMENT_REQUIRED, IDEM, UPDATE_POLICY];
const FORK_NAME: ParamSpec = ParamSpec {
    name: "name",
    ty: "string",
    required: true,
    description: "Name for the new caller-owned Personal Artifact",
};
const FORK_TITLE: ParamSpec = ParamSpec {
    name: "title",
    ty: "string",
    required: false,
    description: "Optional title for the new Personal Artifact",
};
const FOLLOW_UPDATE_PARAMS: &[ParamSpec] = &[SOURCE, IDEM];
const FORK_PERSONAL_PARAMS: &[ParamSpec] =
    &[SOURCE, ASSIGNMENT_REQUIRED, FORK_NAME, FORK_TITLE, IDEM];
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

pub(crate) const LOCAL_ACTIONS: [ActionSpec; 20] = [
    spec(
        "artifacts.search",
        "Search caller-visible stored Artifacts by indexed identity, description, tags, and provenance metadata",
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
        "artifacts.transfer_options",
        "Evaluate current remote-use and distribution options for one exact source revision without mutating local state",
        false,
        false,
        "ArtifactTransferOptions",
        TRANSFER_OPTIONS_PARAMS,
    ),
    spec(
        "artifacts.pin",
        "Materialize one exact revision as a managed local mirror without following future revisions",
        false,
        false,
        "ManagedArtifactReceipt",
        PIN_PARAMS,
    ),
    spec(
        "artifacts.follow_managed",
        "Materialize one exact revision as a managed local mirror and create an explicit update subscription",
        false,
        false,
        "ManagedArtifactReceipt",
        FOLLOW_PARAMS,
    ),
    spec(
        "artifacts.follow_update",
        "Apply one exact currently-authorized source revision to an existing followed managed mirror",
        false,
        false,
        "ManagedArtifactReceipt",
        FOLLOW_UPDATE_PARAMS,
    ),
    spec(
        "artifacts.fork_personal",
        "Fork one exact permitted source revision into a new caller-owned Personal Artifact",
        false,
        false,
        "PersonalArtifactForkReceipt",
        FORK_PERSONAL_PARAMS,
    ),
    spec(
        "artifacts.refresh",
        "Explicitly rebuild and reconcile the committed active generation",
        false,
        false,
        "SkillMutationReceipt",
        &[VERSION, IDEM],
    ),
];

/// Skill Library callback catalog. Remote discovery specs are borrowed from the canonical remote
/// catalog rather than redeclared here; the broader `artifacts` service composes every remote spec.
pub(crate) static ACTIONS: LazyLock<Vec<ActionSpec>> = LazyLock::new(|| {
    LOCAL_ACTIONS
        .into_iter()
        .chain(crate::dispatch::artifact_control::CALLBACK_REMOTE_ACTIONS)
        .collect()
});

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
            LOCAL_ACTION_NAMES
                .iter()
                .copied()
                .chain(
                    crate::dispatch::artifact_control::CALLBACK_REMOTE_ACTIONS
                        .iter()
                        .map(|action| action.name),
                )
                .collect::<Vec<_>>()
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
