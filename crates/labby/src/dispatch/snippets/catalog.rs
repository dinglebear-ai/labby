use labby_primitives::action::{ActionSpec, ParamSpec};
use schemars::JsonSchema;
use std::path::PathBuf;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SnippetListSchema {
    snippets: Vec<labby_codemode::snippet::store::SnippetInfo>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SnippetPromotionResultSchema {
    execution_id: String,
    source: SnippetPromotionSourceSchema,
    snippet: labby_codemode::snippet::store::SnippetInfo,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SnippetPromotionSourceSchema {
    created_at_ms: i64,
    is_admin: bool,
    surface: String,
    route_scope: String,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SnippetValidationSchema {
    valid: bool,
    name: String,
    mode: String,
    source: Option<labby_codemode::snippet::store::SnippetSource>,
    path: Option<PathBuf>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum SnippetTestResultSchema {
    Single(Box<SnippetTestSingleSchema>),
    All(SnippetTestAllSchema),
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SnippetTestSingleSchema {
    name: String,
    passed: bool,
    response: labby_codemode::CodeModeExecutionResponse,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SnippetTestAllSchema {
    passed: bool,
    results: Vec<SnippetTestItemSchema>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum SnippetTestItemSchema {
    Success {
        name: String,
        passed: bool,
        response: labby_codemode::CodeModeExecutionResponse,
    },
    Error {
        name: String,
        passed: bool,
        error: AgentErrorEnvelopeSchema,
    },
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct AgentErrorEnvelopeSchema {
    contract_version: u32,
    kind: String,
    message: String,
    origin: labby_runtime::agent_error::AgentErrorOrigin,
    recovery: labby_runtime::agent_error::AgentRecoveryAdvice,
    side_effects: labby_runtime::agent_error::AgentSideEffectRisk,
    service: Option<String>,
    action: Option<String>,
    tool: Option<String>,
    upstream: Option<String>,
    command: Option<String>,
    prompt: Option<String>,
    resource: Option<String>,
    cause: Option<String>,
}

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        name: "help",
        description: "Show this action catalog",
        destructive: false,
        requires_admin: false,
        returns: "Catalog",
        output_schema: None,
        params: &[],
    },
    ActionSpec {
        name: "schema",
        description: "Return the parameter schema for a named action",
        destructive: false,
        requires_admin: false,
        returns: "Schema",
        output_schema: None,
        params: &[ParamSpec {
            name: "action",
            ty: "string",
            required: true,
            description: "Action name to describe",
        }],
    },
    ActionSpec {
        name: "snippets.list",
        description: "List built-in and user executable Code Mode snippets",
        destructive: false,
        requires_admin: false,
        returns: "SnippetList",
        output_schema: Some(labby_primitives::action::schema_for::<SnippetListSchema>),
        params: &[],
    },
    ActionSpec {
        name: "snippets.get",
        description: "Return one snippet body and metadata",
        destructive: false,
        requires_admin: true,
        returns: "ResolvedSnippet",
        output_schema: Some(
            labby_primitives::action::schema_for::<labby_codemode::snippet::store::ResolvedSnippet>,
        ),
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "Snippet name",
        }],
    },
    ActionSpec {
        name: "snippets.exec",
        description: "Execute a snippet through gateway Code Mode",
        destructive: false,
        requires_admin: true,
        returns: "CodeModeExecutionResponse",
        output_schema: Some(
            labby_primitives::action::schema_for::<labby_codemode::CodeModeExecutionResponse>,
        ),
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Snippet name",
            },
            ParamSpec {
                name: "params",
                ty: "object",
                required: false,
                description: "Input object passed to the snippet async function",
            },
        ],
    },
    ActionSpec {
        name: "snippets.create",
        description: "Create a user snippet under LABBY_HOME/snippets",
        destructive: false,
        requires_admin: true,
        returns: "SnippetInfo",
        output_schema: Some(
            labby_primitives::action::schema_for::<labby_codemode::snippet::store::SnippetInfo>,
        ),
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Snippet name",
            },
            ParamSpec {
                name: "body",
                ty: "string",
                required: true,
                description: "Markdown or JavaScript snippet body",
            },
            ParamSpec {
                name: "description",
                ty: "string",
                required: false,
                description: "Description written to generated frontmatter",
            },
            ParamSpec {
                name: "force",
                ty: "boolean",
                required: false,
                description: "Overwrite an existing user snippet",
            },
        ],
    },
    ActionSpec {
        name: "snippets.promote",
        description: "Promote a successful live Code Mode execution into a user snippet",
        destructive: true,
        requires_admin: true,
        returns: "SnippetPromotionResult",
        output_schema: Some(labby_primitives::action::schema_for::<SnippetPromotionResultSchema>),
        params: &[
            ParamSpec {
                name: "execution_id",
                ty: "string",
                required: true,
                description: "Live gateway Code Mode execution id",
            },
            ParamSpec {
                name: "name",
                ty: "string",
                required: true,
                description: "Target user snippet name",
            },
            ParamSpec {
                name: "description",
                ty: "string",
                required: false,
                description: "Snippet description",
            },
            ParamSpec {
                name: "force",
                ty: "boolean",
                required: false,
                description: "Overwrite an existing user snippet",
            },
            ParamSpec {
                name: "shadow_builtin",
                ty: "boolean",
                required: false,
                description: "Allow creating a user snippet that shadows a built-in snippet",
            },
        ],
    },
    ActionSpec {
        name: "snippets.validate",
        description: "Validate a snippet body or existing snippet without executing it",
        destructive: false,
        requires_admin: true,
        returns: "SnippetValidation",
        output_schema: Some(labby_primitives::action::schema_for::<SnippetValidationSchema>),
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: false,
                description: "Snippet name or filename stem",
            },
            ParamSpec {
                name: "body",
                ty: "string",
                required: false,
                description: "Markdown or JavaScript snippet body to validate",
            },
        ],
    },
    ActionSpec {
        name: "snippets.remove",
        description: "Remove a user snippet",
        destructive: true,
        requires_admin: true,
        returns: "SnippetRemoveResult",
        output_schema: Some(
            labby_primitives::action::schema_for::<
                labby_codemode::snippet::store::SnippetRemoveResult,
            >,
        ),
        params: &[ParamSpec {
            name: "name",
            ty: "string",
            required: true,
            description: "User snippet name",
        }],
    },
    ActionSpec {
        name: "snippets.test",
        description: "Execute one snippet and report pass/fail",
        destructive: false,
        requires_admin: true,
        returns: "SnippetTestResult",
        output_schema: Some(labby_primitives::action::schema_for::<SnippetTestResultSchema>),
        params: &[
            ParamSpec {
                name: "name",
                ty: "string",
                required: false,
                description: "Snippet name",
            },
            ParamSpec {
                name: "params",
                ty: "object",
                required: false,
                description: "Input object passed to the snippet async function",
            },
            ParamSpec {
                name: "all",
                ty: "boolean",
                required: false,
                description: "Run every listed snippet with default params",
            },
        ],
    },
];
