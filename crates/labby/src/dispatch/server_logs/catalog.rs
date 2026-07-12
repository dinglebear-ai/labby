use labby_primitives::action::{ActionSpec, ParamSpec};

/// Action catalog for the Labby server process log viewer.
pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        params: &[],
        returns: "Catalog",
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
        returns: "Schema",
    },
    ActionSpec {
        name: "server_logs.query",
        description: "Read and filter Labby's rolling JSON server process logs",
        destructive: false,
        requires_admin: true,
        params: &[
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Maximum matching entries to return, newest first after filtering",
            },
            ParamSpec {
                name: "level",
                ty: "string",
                required: false,
                description: "Exact tracing level filter, such as INFO, WARN, or ERROR",
            },
            ParamSpec {
                name: "target",
                ty: "string",
                required: false,
                description: "Substring filter for the tracing target/module",
            },
            ParamSpec {
                name: "service",
                ty: "string",
                required: false,
                description: "Exact or substring filter for the structured service field",
            },
            ParamSpec {
                name: "action",
                ty: "string",
                required: false,
                description: "Exact or substring filter for the structured action field",
            },
            ParamSpec {
                name: "kind",
                ty: "string",
                required: false,
                description: "Exact or substring filter for the structured kind/error field",
            },
            ParamSpec {
                name: "query",
                ty: "string",
                required: false,
                description: "Case-insensitive text search across message, target, and fields",
            },
            ParamSpec {
                name: "file",
                ty: "string",
                required: false,
                description: "Substring filter for a retained log filename",
            },
            ParamSpec {
                name: "max_scan_bytes",
                ty: "integer",
                required: false,
                description: "Bounded byte budget scanned across newest retained log files",
            },
        ],
        returns: "ServerLogsQueryResult",
    },
];
