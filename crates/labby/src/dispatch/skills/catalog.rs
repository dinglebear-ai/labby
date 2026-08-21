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

pub const ACTIONS: &[ActionSpec] = &[
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
