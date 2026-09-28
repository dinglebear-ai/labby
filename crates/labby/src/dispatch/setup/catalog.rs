//! Action catalog for the `setup` Bootstrap orchestrator.

use labby_primitives::action::{ActionSpec, ParamSpec};
use schemars::JsonSchema;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[allow(dead_code)]
#[derive(JsonSchema)]
struct BootstrapResultSchema {
    created: bool,
    env_path: String,
    token: Option<String>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct ServiceSchemaMapSchema {
    services: BTreeMap<String, ServiceSchemaEntrySchema>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct ServiceSchemaEntrySchema {
    name: String,
    display_name: String,
    description: String,
    category: String,
    supports_multi_instance: bool,
    default_port: Option<u16>,
    built_in_upstream_api: bool,
    env: Vec<ServiceEnvSchema>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct ServiceEnvSchema {
    name: String,
    description: String,
    example: String,
    secret: bool,
    required: bool,
    ui: Option<ServiceUiSchema>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct ServiceUiSchema {
    kind: String,
    enum_values: Option<Vec<String>>,
    advanced: bool,
    help_url: Option<String>,
    depends_on: Option<String>,
    validation: ServiceUiValidationSchema,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct ServiceUiValidationSchema {
    required: bool,
    min_length: Option<usize>,
    max_length: Option<usize>,
    pattern: Option<String>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct DraftGetResultSchema {
    entries: Vec<super::types::DraftEntry>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct DraftSetResultSchema {
    written: usize,
    skipped: Vec<String>,
    backup_path: Option<PathBuf>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct DraftDiscardResultSchema {
    removed: bool,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsUpdateStateSchema {
    config_path: String,
    changed: bool,
    previous: SettingsUpdatePreviousSchema,
    restart_required: bool,
    restart_note: String,
    services: SettingsUpdateServicesSchema,
    surfaces: SettingsUpdateSurfacesSchema,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsUpdatePreviousSchema {
    services: SettingsUpdatePreviousServicesSchema,
}
#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsUpdatePreviousServicesSchema {
    built_in_upstream_apis_enabled: Option<bool>,
}
#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsUpdateServicesSchema {
    built_in_upstream_apis_enabled: bool,
    built_in_upstream_api_services: Vec<String>,
    bootstrap_services: Vec<String>,
}
#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsUpdateSurfacesSchema {
    mcp: SettingsMcpSurfaceSchema,
    web: SettingsWebSurfaceSchema,
    auth: SettingsAuthSurfaceSchema,
}
#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsMcpSurfaceSchema {
    transport: String,
    host: String,
    port: u16,
    protocol_version: String,
    lifecycle: String,
}
#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsWebSurfaceSchema {
    auth_disabled: bool,
    assets_dir: Option<String>,
}
#[allow(dead_code)]
#[derive(JsonSchema)]
struct SettingsAuthSurfaceSchema {
    mode: String,
    public_url: Option<String>,
}

/// Setup actions that may only run from a trusted local transport. These
/// either mint first-run credentials or initiate an outbound connectivity
/// probe from the host, so an admin bearer alone is not sufficient.
pub const LOCAL_ONLY_ACTIONS: &[&str] = &["bootstrap", "proxy.configure"];

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
        name: "state",
        description: "First-run + draft snapshot for the wizard / settings UI",
        destructive: false,
        requires_admin: true,
        returns: "SetupSnapshot",
        output_schema: Some(labby_primitives::action::schema_for::<super::types::SetupSnapshot>),
        params: &[],
    },
    ActionSpec {
        name: "bootstrap",
        description: "Create ~/.labby/.env with a generated token + loopback defaults when absent (first-run)",
        destructive: true,
        requires_admin: true,
        returns: "BootstrapOutcome",
        output_schema: Some(labby_primitives::action::schema_for::<BootstrapResultSchema>),
        params: &[],
    },
    ActionSpec {
        name: "schema.get",
        description: "UiSchema projection for all (or filtered) services",
        destructive: false,
        requires_admin: false,
        returns: "ServiceSchemaMap",
        output_schema: Some(labby_primitives::action::schema_for::<ServiceSchemaMapSchema>),
        params: &[ParamSpec {
            name: "services",
            ty: "string[]",
            required: false,
            description: "Optional filter; defaults to every service in the registry",
        }],
    },
    ActionSpec {
        name: "draft.get",
        description: "Read .env.draft with secret values masked to '***'",
        destructive: false,
        requires_admin: true,
        returns: "DraftEntry[]",
        output_schema: Some(labby_primitives::action::schema_for::<DraftGetResultSchema>),
        params: &[],
    },
    ActionSpec {
        name: "draft.set",
        description: "Write a key (or section) into .env.draft (validated server-side)",
        destructive: true,
        requires_admin: true,
        returns: "DraftSetOutcome",
        output_schema: Some(labby_primitives::action::schema_for::<DraftSetResultSchema>),
        params: &[
            ParamSpec {
                name: "entries",
                ty: "DraftEntry[]",
                required: true,
                description: "Key/value pairs to write into the draft",
            },
            ParamSpec {
                name: "force",
                ty: "boolean",
                required: false,
                description: "Overwrite conflicting draft keys (default false)",
            },
        ],
    },
    ActionSpec {
        name: "draft.discard",
        description: "Discard .env.draft without modifying .env",
        destructive: true,
        requires_admin: true,
        returns: "DraftDiscardOutcome",
        output_schema: Some(labby_primitives::action::schema_for::<DraftDiscardResultSchema>),
        params: &[],
    },
    ActionSpec {
        name: "draft.commit",
        description: "Run audit and atomically merge .env.draft into .env",
        destructive: true,
        requires_admin: true,
        returns: "CommitOutcome",
        output_schema: Some(labby_primitives::action::schema_for::<super::types::CommitOutcome>),
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Overwrite conflicting .env keys (default false)",
        }],
    },
    ActionSpec {
        name: "settings.state",
        description: "Return section-scoped safe settings values and source metadata",
        destructive: false,
        requires_admin: true,
        returns: "SettingsState",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::settings::SettingsStateResponse>,
        ),
        params: &[ParamSpec {
            name: "section",
            ty: "string",
            required: false,
            description: "Settings section id; defaults to core",
        }],
    },
    ActionSpec {
        name: "settings.schema",
        description: "Return the safe settings schema with risk and write-policy metadata",
        destructive: false,
        requires_admin: false,
        returns: "SettingsSchema",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::settings::SettingsSchemaResponse>,
        ),
        params: &[],
    },
    ActionSpec {
        name: "settings.env_schema",
        description: "Return generated and registry-derived environment variable inventory",
        destructive: false,
        requires_admin: false,
        returns: "EnvSettingSpec[]",
        output_schema: Some(
            labby_primitives::action::schema_for::<Vec<super::settings::EnvSettingSpec>>,
        ),
        params: &[],
    },
    ActionSpec {
        name: "settings.advanced_state",
        description: "Return redacted advanced settings state",
        destructive: false,
        requires_admin: true,
        returns: "SettingsState",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::settings::SettingsStateResponse>,
        ),
        params: &[],
    },
    ActionSpec {
        name: "settings.update",
        description: "Update non-secret operator settings with validation",
        destructive: true,
        requires_admin: true,
        returns: "SettingsState",
        output_schema: Some(labby_primitives::action::schema_for::<SettingsUpdateStateSchema>),
        params: &[ParamSpec {
            name: "services.built_in_upstream_apis_enabled",
            ty: "boolean",
            required: true,
            description: "Enable built-in upstream API service integrations",
        }],
    },
    ActionSpec {
        name: "settings.config.update",
        description: "Admin-only scalar config.toml settings update",
        destructive: true,
        requires_admin: true,
        returns: "SettingsMutationOutcome",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::settings::SettingsMutationOutcome>,
        ),
        params: &[ParamSpec {
            name: "entries",
            ty: "SettingsUpdateEntry[]",
            required: true,
            description: "Schema-approved config scalar updates",
        }],
    },
    ActionSpec {
        name: "settings.env.update",
        description: "Admin-only targeted .env settings update for known low-risk LABBY_* keys",
        destructive: true,
        requires_admin: true,
        returns: "SettingsState",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::settings::SettingsStateResponse>,
        ),
        params: &[ParamSpec {
            name: "entries",
            ty: "SettingsUpdateEntry[]",
            required: true,
            description: "Schema-approved env scalar updates",
        }],
    },
    ActionSpec {
        name: "check",
        description: "Check local Lab setup prerequisites without mutating the filesystem",
        destructive: false,
        requires_admin: false,
        returns: "SetupReport",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::local_setup::SetupReport>,
        ),
        params: &[],
    },
    ActionSpec {
        name: "repair",
        description: "Repair missing local Lab setup prerequisites without contacting external services",
        destructive: true,
        requires_admin: true,
        returns: "SetupReport",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::local_setup::SetupReport>,
        ),
        params: &[],
    },
    ActionSpec {
        name: "proxy.configure",
        description: "Persist local stdio-proxy defaults and securely store a bearer secret when required",
        destructive: true,
        requires_admin: true,
        returns: "ProxySetupOutcome",
        output_schema: Some(
            labby_primitives::action::schema_for::<super::proxy::ProxySetupOutcome>,
        ),
        params: &[
            ParamSpec {
                name: "preferences",
                ty: "ProxyPreferences",
                required: true,
                description: "Validated non-secret proxy preferences to persist in config.toml",
            },
            ParamSpec {
                name: "bearer_token",
                ty: "string",
                required: false,
                description: "Write-only bearer token read from stdin; never returned or logged",
            },
            ParamSpec {
                name: "dry_run",
                ty: "boolean",
                required: false,
                description: "Preview whether config or secret files would change without mutation",
            },
        ],
    },
    ActionSpec {
        name: "finalize",
        description: "Alias for draft.commit; same params, same returns",
        destructive: true,
        requires_admin: true,
        returns: "CommitOutcome",
        output_schema: Some(labby_primitives::action::schema_for::<super::types::CommitOutcome>),
        params: &[ParamSpec {
            name: "force",
            ty: "boolean",
            required: false,
            description: "Overwrite conflicting .env keys (default false)",
        }],
    },
];
