use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use crate::dispatch::error::ToolError;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsBackend {
    Env,
    ConfigToml,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsControl {
    Text,
    Url,
    Bool,
    Number,
    Enum,
    StringList,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsRisk {
    Low,
    Restart,
    SecuritySensitive,
    Dangerous,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsWritePolicy {
    Editable,
    ReadOnly,
    DangerousFlowRequired,
    SecretWriteOnlyFuture,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsApplyMode {
    Immediate,
    Restart,
    Partial,
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SettingsOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SettingsFieldSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub section: &'static str,
    pub backend: SettingsBackend,
    pub control: SettingsControl,
    pub risk: SettingsRisk,
    pub write_policy: SettingsWritePolicy,
    pub apply_mode: SettingsApplyMode,
    pub secret: bool,
    pub required: bool,
    pub env_override: Option<&'static str>,
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub options: Vec<SettingsOption>,
    pub example: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct SettingsSectionSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub advanced: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SettingsSchemaResponse {
    pub schema_version: u32,
    pub sections: Vec<SettingsSectionSpec>,
    pub fields: Vec<SettingsFieldSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SettingsSourceKind {
    Env,
    ConfigToml,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SettingsValueSource {
    pub source: SettingsSourceKind,
    pub overridden_by_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SettingsStateResponse {
    pub schema_version: u32,
    pub config_path: String,
    pub env_path: String,
    pub section: String,
    pub values: BTreeMap<String, Value>,
    pub sources: BTreeMap<String, SettingsValueSource>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SettingsUpdateEntry {
    pub key: String,
    pub value: Value,
    #[serde(default)]
    pub previous: Value,
    #[serde(default)]
    pub unset: bool,
    #[serde(skip)]
    pub previous_present: bool,
}

impl<'de> Deserialize<'de> for SettingsUpdateEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let mut object = Map::<String, Value>::deserialize(deserializer)?;
        let key = object
            .remove("key")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::missing_field("key"))?;
        let value = object.remove("value").unwrap_or(Value::Null);
        let previous_present = object.contains_key("previous");
        let previous = object.remove("previous").unwrap_or(Value::Null);
        let unset = object
            .remove("unset")
            .map(|value| {
                value
                    .as_bool()
                    .ok_or_else(|| D::Error::custom("unset must be boolean"))
            })
            .transpose()?
            .unwrap_or(false);
        Ok(Self {
            key,
            value,
            previous,
            unset,
            previous_present,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SettingsMutationOutcome {
    pub state: SettingsStateResponse,
    pub backup_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintenance_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct EnvSettingSpec {
    pub service: String,
    pub key: String,
    pub required: bool,
    pub secret: bool,
    pub description: String,
    pub example: String,
    pub editable: bool,
}

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
const READONLY_MAX_DEPTH: usize = 6;
const READONLY_MAX_OBJECT_KEYS: usize = 80;
const READONLY_MAX_ARRAY_ITEMS: usize = 50;
const READONLY_MAX_STRING_BYTES: usize = 4096;

fn editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    backend: SettingsBackend,
    control: SettingsControl,
    apply_mode: SettingsApplyMode,
    env_override: Option<&'static str>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    SettingsFieldSpec {
        key,
        label,
        description,
        section,
        backend,
        control,
        risk: if apply_mode == SettingsApplyMode::Restart {
            SettingsRisk::Restart
        } else {
            SettingsRisk::Low
        },
        write_policy: SettingsWritePolicy::Editable,
        apply_mode,
        secret: false,
        required: false,
        env_override,
        min: None,
        max: None,
        options: Vec::new(),
        example,
    }
}

fn secret_env_editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::Env,
        SettingsControl::Text,
        SettingsApplyMode::Restart,
        None,
        example,
    );
    field.risk = SettingsRisk::SecuritySensitive;
    field.secret = true;
    field
}

fn env_number_editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    min: i64,
    max: i64,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::Env,
        SettingsControl::Number,
        SettingsApplyMode::Restart,
        None,
        example,
    );
    field.min = Some(min);
    field.max = Some(max);
    field
}

fn readonly(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    risk: SettingsRisk,
    write_policy: SettingsWritePolicy,
) -> SettingsFieldSpec {
    SettingsFieldSpec {
        key,
        label,
        description,
        section,
        backend: SettingsBackend::ConfigToml,
        control: SettingsControl::ReadOnly,
        risk,
        write_policy,
        apply_mode: SettingsApplyMode::ReadOnly,
        secret: matches!(write_policy, SettingsWritePolicy::SecretWriteOnlyFuture),
        required: false,
        env_override: None,
        min: None,
        max: None,
        options: Vec::new(),
        example: None,
    }
}

fn enum_editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    options: Vec<SettingsOption>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::ConfigToml,
        SettingsControl::Enum,
        apply_mode,
        None,
        example,
    );
    field.options = options;
    field
}

fn enum_editable_with_env(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    options: Vec<SettingsOption>,
    env_override: Option<&'static str>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = enum_editable(
        section,
        key,
        label,
        description,
        apply_mode,
        options,
        example,
    );
    field.env_override = env_override;
    field
}

fn number_editable(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    min: i64,
    max: i64,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::ConfigToml,
        SettingsControl::Number,
        apply_mode,
        None,
        example,
    );
    field.min = Some(min);
    field.max = Some(max);
    field
}

fn number_editable_with_env(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    apply_mode: SettingsApplyMode,
    min: i64,
    max: i64,
    env_override: Option<&'static str>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = number_editable(
        section,
        key,
        label,
        description,
        apply_mode,
        min,
        max,
        example,
    );
    field.env_override = env_override;
    field
}

pub fn schema_response() -> SettingsSchemaResponse {
    SettingsSchemaResponse {
        schema_version: SETTINGS_SCHEMA_VERSION,
        sections: vec![
            SettingsSectionSpec {
                id: "core",
                label: "Core",
                description: "Where Labby listens, stores workspace files, writes logs, and formats CLI output.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "authentication",
                label: "Authentication",
                description: "Browser sign-in administrators. Only the operator may change them.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "agents",
                label: "Agent provider",
                description: "Connect the OpenAI-compatible provider that executes Labby's built-in Agents.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "surfaces",
                label: "Surfaces",
                description: "How browsers and MCP clients reach this gateway, including public URLs and allowed origins.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "features",
                label: "Features",
                description: "Choose which gateway capabilities run and whether changes take effect now or after restart.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "services",
                label: "Services",
                description: "Connection details and credentials for external services Labby can use.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "notifications",
                label: "Notifications",
                description: "Which operational events Labby records, how long it keeps them, and where it sends alerts.",
                advanced: false,
            },
            SettingsSectionSpec {
                id: "advanced",
                label: "Advanced",
                description: "Inspect effective complex configuration and settings that require a dedicated workflow.",
                advanced: true,
            },
        ],
        fields: settings_fields(),
    }
}

/// The administrator list. Each listed email's browser session receives the
/// configured admin scopes; writing it is reserved for the operator.
fn admin_emails_field() -> SettingsFieldSpec {
    let mut field = editable(
        "authentication",
        ADMIN_EMAILS_KEY,
        "Administrators",
        "Emails whose browser sign-in receives full admin access. One per line. \
         Takes effect after restart; at least one is required.",
        SettingsBackend::Env,
        SettingsControl::StringList,
        SettingsApplyMode::Restart,
        None,
        Some("owner@example.com"),
    );
    field.risk = SettingsRisk::SecuritySensitive;
    field.required = true;
    field
}

const ADMIN_EMAILS_KEY: &str = "LABBY_AUTH_ADMIN_EMAIL";

pub fn settings_fields() -> Vec<SettingsFieldSpec> {
    let mut fields = vec![
        admin_emails_field(),
        enum_env(
            "agents",
            "LABBY_AGENT_PROVIDER_PROTOCOL",
            "Agent provider protocol",
            "Choose OpenAI for standard /models and /chat/completions APIs. Choose Phoenix only for a provider supporting Phoenix's /sessions extension. Existing installations without this setting retain Phoenix behavior. New Agent runs use changes immediately.",
            vec![
                SettingsOption {
                    value: "openai",
                    label: "OpenAI-compatible API",
                },
                SettingsOption {
                    value: "phoenix",
                    label: "Phoenix session extension",
                },
            ],
            Some("openai"),
        ),
        editable(
            "agents",
            "LABBY_PHOENIX_OPENAI_BASE_URL",
            "Agent provider URL",
            "Base URL of the OpenAI-compatible API that Labby Agents call from the gateway server. The address must be reachable from that server, not only from this browser. New Agent runs use the saved connection immediately. Existing runs keep their original connection; restart Labby to update other provider consumers.",
            SettingsBackend::Env,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("https://provider.example.com/v1"),
        ),
        secret_env_editable(
            "agents",
            "LABBY_PHOENIX_OPENAI_API_KEY",
            "Agent provider API key",
            "Credential sent by the Labby server to the Agent provider. Leave blank only if the provider explicitly supports unauthenticated access. The saved value is never shown again. New Agent runs use the saved connection immediately. Existing runs keep their original connection; restart Labby to update other provider consumers.",
            None,
        ),
        editable(
            "core",
            "LABBY_MCP_HTTP_HOST",
            "Bind host",
            "Network address on the Labby server that accepts HTTP and MCP connections. Use 127.0.0.1 for this computer only; use a specific interface address for remote access. Restart Labby to apply it.",
            SettingsBackend::Env,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("127.0.0.1"),
        ),
        env_number_editable(
            "core",
            "LABBY_MCP_HTTP_PORT",
            "Gateway port",
            "TCP port where this Labby server accepts HTTP and MCP connections. The port must be free on the server host. Restart Labby to apply it.",
            1,
            65_535,
            Some("8765"),
        ),
        editable(
            "core",
            "LABBY_LOG",
            "Log filter",
            "Which components write diagnostic logs and at what level, for example labby=info,labby_apis=warn. Restart Labby to apply it.",
            SettingsBackend::Env,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("labby=info,labby_apis=warn"),
        ),
        enum_env(
            "core",
            "LABBY_LOG_FORMAT",
            "Log format",
            "Choose readable text logs or one JSON object per log line for log collectors. Restart Labby to apply it.",
            vec![
                SettingsOption {
                    value: "text",
                    label: "Text",
                },
                SettingsOption {
                    value: "json",
                    label: "JSON",
                },
            ],
            Some("json"),
        ),
        editable(
            "notifications",
            "LABBY_NOTIFICATIONS_ENABLED",
            "Enable notifications",
            "Store Labby operational events for display in Notifications and check Team Depot ingestion runs for failures. Restart Labby to apply this choice.",
            SettingsBackend::Env,
            SettingsControl::Bool,
            SettingsApplyMode::Restart,
            None,
            Some("true"),
        ),
        editable(
            "notifications",
            "APPRISE_URL",
            "Apprise API URL",
            "Address of the Apprise service that receives Labby alerts. Labby sends notifications to its /notify endpoint or /notify/{KEY} when a configuration key is set.",
            SettingsBackend::Env,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("http://apprise:8000"),
        ),
        secret_env_editable(
            "notifications",
            "APPRISE_TOKEN",
            "Apprise configuration key",
            "Optional Apprise configuration key appended to /notify when Labby sends alerts. The saved key is never returned by the settings API.",
            Some("labby"),
        ),
        env_number_editable(
            "notifications",
            "LABBY_NOTIFICATION_RETENTION",
            "Notification retention",
            "Maximum number of recent operational notifications Labby keeps in its local store; older records are removed as new ones arrive.",
            10,
            2_000,
            Some("200"),
        ),
        env_number_editable(
            "notifications",
            "LABBY_DEPOT_MONITOR_INTERVAL_SECONDS",
            "Depot failure check interval",
            "Number of seconds between checks for newly failed Team Depot ingestion runs. Smaller values detect failures sooner and send more requests.",
            10,
            3_600,
            Some("30"),
        ),
        editable(
            "surfaces",
            "LABBY_PUBLIC_URL",
            "Web app address in service environment",
            "Address users open for the Labby web app. OAuth redirects and issuer identity also use this URL. Set the externally reachable HTTPS address when using a reverse proxy.",
            SettingsBackend::Env,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("https://lab.example.com"),
        ),
        editable(
            "surfaces",
            "LABBY_MCP_GATEWAY_URL",
            "MCP client address in service environment",
            "Address external MCP clients use to reach Labby. This may differ from the web app address; configure the URL exposed by your proxy.",
            SettingsBackend::Env,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            None,
            Some("https://mcp.example.com"),
        ),
        editable(
            "core",
            "log.filter",
            "Log filter default",
            "Default diagnostic log levels for components when LABBY_LOG is not set in the service environment.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            Some("LABBY_LOG"),
            Some("labby=info,labby_apis=warn"),
        ),
        enum_editable_with_env(
            "core",
            "log.format",
            "Log format default",
            "Default log format when LABBY_LOG_FORMAT is not set in the service environment.",
            SettingsApplyMode::Restart,
            vec![
                SettingsOption {
                    value: "text",
                    label: "Text",
                },
                SettingsOption {
                    value: "json",
                    label: "JSON",
                },
            ],
            Some("LABBY_LOG_FORMAT"),
            Some("text"),
        ),
        enum_editable(
            "core",
            "output.format",
            "CLI output format",
            "How Labby CLI commands display results unless a command explicitly requests JSON output.",
            SettingsApplyMode::Restart,
            vec![
                SettingsOption {
                    value: "human",
                    label: "Readable text",
                },
                SettingsOption {
                    value: "json",
                    label: "JSON",
                },
            ],
            Some("human"),
        ),
        editable(
            "core",
            "workspace.root",
            "Workspace root",
            "Directory on the Labby server that its filesystem browser can open. This is a server path, not a path on your browser's computer.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("~/.labby/workspace"),
        ),
        enum_editable_with_env(
            "surfaces",
            "mcp.transport",
            "MCP transport",
            "How Labby exposes MCP by default: HTTP for network clients or stdio for a client that launches Labby as a subprocess. LABBY_MCP_TRANSPORT overrides this choice.",
            SettingsApplyMode::Restart,
            vec![
                SettingsOption {
                    value: "http",
                    label: "HTTP",
                },
                SettingsOption {
                    value: "stdio",
                    label: "stdio",
                },
            ],
            Some("LABBY_MCP_TRANSPORT"),
            Some("http"),
        ),
        editable(
            "surfaces",
            "mcp.host",
            "MCP HTTP host",
            "Default network address the Labby HTTP server binds to. LABBY_MCP_HTTP_HOST takes precedence when set by the service.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            Some("LABBY_MCP_HTTP_HOST"),
            Some("127.0.0.1"),
        ),
        number_editable_with_env(
            "surfaces",
            "mcp.port",
            "MCP HTTP port",
            "Default TCP port for the Labby HTTP and MCP server. LABBY_MCP_HTTP_PORT takes precedence when set by the service.",
            SettingsApplyMode::Restart,
            1,
            65535,
            Some("LABBY_MCP_HTTP_PORT"),
            Some("8765"),
        ),
        editable(
            "surfaces",
            "mcp.allowed_hosts",
            "Allowed hosts",
            "Host names Labby accepts in incoming HTTP requests in addition to its built-in local names. Enter one host per line, without a URL scheme. LABBY_MCP_ALLOWED_HOSTS overrides this list.",
            SettingsBackend::ConfigToml,
            SettingsControl::StringList,
            SettingsApplyMode::Restart,
            Some("LABBY_MCP_ALLOWED_HOSTS"),
            Some("lab.example.com"),
        ),
        editable(
            "surfaces",
            "api.cors_origins",
            "CORS origins",
            "Browser origins permitted to call Labby across origins. Enter each full scheme, host, and port on a separate line. Local loopback origins are already allowed; LABBY_CORS_ORIGINS overrides this list.",
            SettingsBackend::ConfigToml,
            SettingsControl::StringList,
            SettingsApplyMode::Restart,
            Some("LABBY_CORS_ORIGINS"),
            Some("https://lab.example.com"),
        ),
        editable(
            "surfaces",
            "web.assets_dir",
            "Web assets directory",
            "Directory on the Labby server containing the built web app files that labby serve publishes.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            None,
            Some("apps/web/out"),
        ),
        editable(
            "surfaces",
            "public_urls.app",
            "Public app URL",
            "Default browser address for the Labby app and its OAuth issuer. LABBY_PUBLIC_URL takes precedence when set by the service.",
            SettingsBackend::ConfigToml,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            Some("LABBY_PUBLIC_URL"),
            Some("https://lab.example.com"),
        ),
        editable(
            "surfaces",
            "public_urls.mcp_gateway",
            "Public MCP gateway URL",
            "Default address external MCP clients use to reach Labby. LABBY_MCP_GATEWAY_URL takes precedence when set by the service.",
            SettingsBackend::ConfigToml,
            SettingsControl::Url,
            SettingsApplyMode::Restart,
            Some("LABBY_MCP_GATEWAY_URL"),
            Some("https://mcp.example.com"),
        ),
        editable(
            "features",
            "services.built_in_upstream_apis_enabled",
            "Built-in upstream API services",
            "Expose Labby's bundled integrations with external service APIs. Labby's own bootstrap and setup tools stay available when this is off.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Immediate,
            None,
            Some("true"),
        ),
        editable(
            "features",
            "code_mode.trace_params",
            "Trace Code Mode params",
            "Include bounded, redacted tool arguments in Code Mode traces for debugging. Sensitive values remain masked, but more request detail is retained.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Partial,
            None,
            Some("false"),
        ),
        editable(
            "features",
            "gateway.auto_reconnect",
            "Automatically recover disconnected MCPs",
            "Periodically test disconnected upstream MCP servers and reconnect them when they respond again.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Immediate,
            None,
            Some("false"),
        ),
        editable(
            "services",
            "services.tailscale.tailnet",
            "Tailscale tailnet",
            "Tailscale tailnet name used by Tailscale service integrations. TAILSCALE_TAILNET takes precedence when set by the service.",
            SettingsBackend::ConfigToml,
            SettingsControl::Text,
            SettingsApplyMode::Restart,
            Some("TAILSCALE_TAILNET"),
            Some("-"),
        ),
        editable(
            "advanced",
            "setup.install_android_sdk",
            "Install android-sdk on provision",
            "Install Android SDK during provisioning for the claude-in-mobile MCP integration. Off by default; LABBY_ENABLE_ANDROID_SDK=1 overrides this choice.",
            SettingsBackend::ConfigToml,
            SettingsControl::Bool,
            SettingsApplyMode::Immediate,
            Some("LABBY_ENABLE_ANDROID_SDK"),
            Some("false"),
        ),
        number_editable(
            "advanced",
            "upstream_request_timeout_ms",
            "Upstream request timeout",
            "Maximum number of milliseconds Labby waits for one upstream MCP response before returning a timeout to the caller.",
            SettingsApplyMode::Restart,
            1,
            300_000,
            Some("30000"),
        ),
        number_editable(
            "advanced",
            "upstream_relay_timeout_ms",
            "Upstream relay (elicitation) timeout",
            "Maximum milliseconds for a relayed upstream MCP call while a person answers an elicitation. Applies only to the enabled relay path.",
            SettingsApplyMode::Restart,
            1,
            1_800_000,
            Some("300000"),
        ),
        number_editable(
            "advanced",
            "local_logs.retention_days",
            "Log retention days",
            "Number of days Labby keeps local diagnostic logs before removing old records.",
            SettingsApplyMode::Partial,
            1,
            3650,
            Some("30"),
        ),
        number_editable(
            "advanced",
            "local_logs.max_bytes",
            "Max log bytes",
            "Maximum total logical bytes of local diagnostic logs Labby retains before pruning older records.",
            SettingsApplyMode::Partial,
            1,
            1_099_511_627_776,
            Some("1073741824"),
        ),
        number_editable(
            "advanced",
            "local_logs.queue_capacity",
            "Log queue capacity",
            "Maximum log records waiting to be written to the local log store. Restart Labby after changing it.",
            SettingsApplyMode::Restart,
            1,
            1_000_000,
            Some("4096"),
        ),
        number_editable(
            "advanced",
            "local_logs.subscriber_capacity",
            "Subscriber capacity",
            "Maximum recent log records held for live log subscribers. Restart Labby after changing it.",
            SettingsApplyMode::Restart,
            1,
            1_000_000,
            Some("1024"),
        ),
    ];
    fields.extend([
        number_editable(
            "advanced",
            "code_mode.timeout_ms",
            "Code Mode timeout",
            "Maximum wall-clock milliseconds for one Code Mode JavaScript execution before Labby stops it.",
            SettingsApplyMode::Partial,
            1,
            // Derived from the shared validation ceiling so the editor can
            // never reject a value config.toml accepts.
            i64::try_from(labby_runtime::gateway_config::MAX_CODE_MODE_TIMEOUT_MS)
                .unwrap_or(i64::MAX),
            Some("30000"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_source_bytes",
            "Code Mode max source bytes",
            "Largest JavaScript source body, in UTF-8 bytes, accepted for one Code Mode execution.",
            SettingsApplyMode::Partial,
            1024,
            1_048_576,
            Some("1048576"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_response_bytes",
            "Code Mode max response bytes",
            "Largest serialized response, in bytes, that one Code Mode execution may return.",
            SettingsApplyMode::Partial,
            1024,
            1_048_576,
            Some("1048576"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_response_tokens",
            "Code Mode max response tokens",
            "Approximate maximum response tokens returned from one Code Mode execution, estimated from its serialized text.",
            SettingsApplyMode::Partial,
            256,
            256_000,
            Some("64000"),
        ),
        number_editable(
            "advanced",
            "code_mode.token_estimate_divisor",
            "Token estimate divisor",
            "Number of response bytes counted as one estimated token. Lower values estimate more tokens and reach the response limit sooner.",
            SettingsApplyMode::Partial,
            1,
            64,
            Some("4"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_log_entries",
            "Code Mode max log entries",
            "Maximum number of console log entries Labby captures from one Code Mode execution.",
            SettingsApplyMode::Partial,
            1,
            100_000,
            Some("1000"),
        ),
        number_editable(
            "advanced",
            "code_mode.max_log_bytes",
            "Code Mode max log bytes",
            "Maximum total bytes of console logs Labby captures from one Code Mode execution.",
            SettingsApplyMode::Partial,
            1,
            104_857_600,
            Some("1048576"),
        ),
    ]);
    for field in &mut fields {
        if matches!(
            field.key,
            "LABBY_PHOENIX_OPENAI_BASE_URL"
                | "LABBY_PHOENIX_OPENAI_API_KEY"
                | "LABBY_AGENT_PROVIDER_PROTOCOL"
        ) {
            field.apply_mode = SettingsApplyMode::Partial;
        }
    }
    fields.extend(readonly_fields());
    fields
}

fn enum_env(
    section: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    options: Vec<SettingsOption>,
    example: Option<&'static str>,
) -> SettingsFieldSpec {
    let mut field = editable(
        section,
        key,
        label,
        description,
        SettingsBackend::Env,
        SettingsControl::Enum,
        SettingsApplyMode::Restart,
        None,
        example,
    );
    field.options = options;
    field
}

fn readonly_fields() -> Vec<SettingsFieldSpec> {
    let mut fields = vec![
        readonly(
            "surfaces",
            "web.disable_auth",
            "Disable web auth",
            "Whether the web app accepts unauthenticated requests. Changing this can expose the operator UI, so it is shown here for inspection and requires a separate security-reviewed flow.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "surfaces",
            "auth",
            "Auth config",
            "The gateway's inbound sign-in and bearer-token configuration. Secret values are hidden; use the authentication workflow to change them.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::SecretWriteOnlyFuture,
        ),
        readonly(
            "features",
            "admin.enabled",
            "Admin tool enabled",
            "Whether the privileged lab_admin MCP tool is exposed to eligible callers. It cannot be enabled from a general settings form.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "code_mode.enabled",
            "Code Mode enabled",
            "Whether Labby exposes its Code Mode execution tool. Changing this alters the MCP tools clients see and requires a dedicated exposure workflow.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
    ];
    // Gateway-owned settings only exist in builds with the `gateway` feature —
    // same philosophy as the nodes gating above: don't advertise config for a
    // capability this build cannot run.
    #[cfg(feature = "gateway")]
    fields.extend([
        readonly(
            "features",
            "gateway_import_mode",
            "Gateway import mode",
            "Controls whether Labby reads external client MCP configurations as possible upstream imports. Imports can expose new capabilities, so changing this requires review.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "gateway.extra_stdio_commands",
            "Extra stdio commands",
            "Extra executable commands Labby may launch for stdio MCP upstreams. Editing the allowlist changes what can run on the server and requires a dedicated workflow.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
        readonly(
            "features",
            "gateway.disable_spawn_guard",
            "Disable spawn guard",
            "Whether Labby skips its stdio command safety checks. This weakens command validation and requires typed confirmation with a rollback path.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::DangerousFlowRequired,
        ),
    ]);
    fields.extend([
        readonly(
            "advanced",
            "oauth.machines",
            "OAuth relay machines",
            "Machines permitted to relay OAuth callbacks to this gateway. Their identities and bindings are shown read-only.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "deploy",
            "Deploy preferences",
            "Default deployment choices and host-specific overrides used when Labby provisions services.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "upstream",
            "Gateway upstreams",
            "Configured upstream MCP connections that Labby may proxy to authorized clients. Manage each connection in Gateway.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "upstream_pending",
            "Pending upstream imports",
            "MCP upstreams discovered from external configuration that are waiting for operator review before activation.",
            SettingsRisk::SecuritySensitive,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "upstream_import_tombstones",
            "Import tombstones",
            "Records of removed imported MCP connections that prevent automatic re-import of the same source.",
            SettingsRisk::Restart,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "protected_mcp_routes",
            "Protected MCP routes",
            "Public MCP paths and hosts that require OAuth access checks. Use the protected routes editor to change them.",
            SettingsRisk::Dangerous,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "virtual_servers",
            "Virtual servers",
            "MCP server definitions that expose Labby service capabilities without a separate upstream process.",
            SettingsRisk::Restart,
            SettingsWritePolicy::ReadOnly,
        ),
        readonly(
            "advanced",
            "quarantined_virtual_servers",
            "Quarantined virtual servers",
            "Virtual MCP servers disabled because their backing Labby service is no longer registered.",
            SettingsRisk::Restart,
            SettingsWritePolicy::ReadOnly,
        ),
    ]);
    fields
}

pub fn state_response(
    cfg: &crate::config::LabConfig,
    config_path: String,
    env_path: String,
    section: &str,
) -> Result<SettingsStateResponse, ToolError> {
    let explicit_config_paths = explicit_config_paths(&config_path)?;
    let env_path_ref = std::path::Path::new(&env_path);
    let mut values = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for field in settings_fields()
        .into_iter()
        .filter(|field| field.section == section)
    {
        let (value, source) = value_for_field(cfg, &field, &explicit_config_paths, env_path_ref)?;
        values.insert(field.key.to_string(), value);
        sources.insert(field.key.to_string(), source);
    }
    Ok(SettingsStateResponse {
        schema_version: SETTINGS_SCHEMA_VERSION,
        config_path,
        env_path,
        section: section.to_string(),
        values,
        sources,
    })
}

fn value_for_field(
    cfg: &crate::config::LabConfig,
    field: &SettingsFieldSpec,
    explicit_config_paths: &BTreeSet<String>,
    env_path: &std::path::Path,
) -> Result<(Value, SettingsValueSource), ToolError> {
    if field.backend == SettingsBackend::Env {
        // A variable the service manager (or shell) set before `.env` loaded
        // wins for the whole process: report that effective value and flag
        // it, because an edit to `.env` could never take effect.
        if let Some(process_value) = env_process_override(field) {
            return Ok((
                process_value,
                SettingsValueSource {
                    source: SettingsSourceKind::Env,
                    overridden_by_env: Some(field.key.to_string()),
                },
            ));
        }
        let value = env_current_value(env_path, field)?.unwrap_or(Value::Null);
        return Ok((
            value.clone(),
            SettingsValueSource {
                source: if value.is_null() {
                    SettingsSourceKind::Default
                } else {
                    SettingsSourceKind::Env
                },
                overridden_by_env: None,
            },
        ));
    }
    let override_source = env_override_source(env_path, field)?;
    let mut value = override_source.as_ref().map_or_else(
        || crate::config::config_json_value_for_path(cfg, field.key),
        |(_, value)| env_override_value(field, value),
    );
    if field.control == SettingsControl::ReadOnly {
        value = redact_value(value);
        value = cap_readonly_value(value, 0);
    }
    let source = if let Some((name, _)) = override_source.clone() {
        SettingsValueSource {
            source: SettingsSourceKind::Env,
            overridden_by_env: Some(name.to_string()),
        }
    } else if !explicit_config_paths.contains(field.key) {
        SettingsValueSource {
            source: SettingsSourceKind::Default,
            overridden_by_env: None,
        }
    } else {
        SettingsValueSource {
            source: SettingsSourceKind::ConfigToml,
            overridden_by_env: None,
        }
    };
    Ok((value, source))
}

fn explicit_config_paths(config_path: &str) -> Result<BTreeSet<String>, ToolError> {
    let raw = match std::fs::read_to_string(config_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeSet::new());
        }
        Err(error) => {
            return Err(ToolError::Sdk {
                sdk_kind: "config_read_error".into(),
                message: format!("failed to read settings config {config_path}: {error}"),
            });
        }
    };
    let document = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "config_parse_error".into(),
            message: format!("failed to parse settings config {config_path}: {error}"),
        })?;
    let mut paths = BTreeSet::new();
    collect_toml_paths(document.as_item(), "", &mut paths);
    Ok(paths)
}

fn collect_toml_paths(item: &toml_edit::Item, prefix: &str, paths: &mut BTreeSet<String>) {
    if let Some(table) = item.as_table() {
        for (key, value) in table {
            let next = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            paths.insert(next.clone());
            collect_toml_paths(value, &next, paths);
        }
    } else if let Some(inline) = item.as_value().and_then(toml_edit::Value::as_inline_table) {
        for (key, value) in inline {
            let next = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };
            paths.insert(next.clone());
            collect_toml_value_paths(value, &next, paths);
        }
    }
}

fn collect_toml_value_paths(value: &toml_edit::Value, prefix: &str, paths: &mut BTreeSet<String>) {
    if let Some(inline) = value.as_inline_table() {
        for (key, child) in inline {
            let next = format!("{prefix}.{key}");
            paths.insert(next.clone());
            collect_toml_value_paths(child, &next, paths);
        }
    }
}

fn env_process_value(field: &SettingsFieldSpec) -> Value {
    match crate::dispatch::helpers::env_non_empty(field.key) {
        Some(value) if field.secret => secret_marker(&value),
        Some(value) if field.control == SettingsControl::Number => value
            .parse::<i64>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        Some(value) if field.control == SettingsControl::Bool => value
            .parse::<bool>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        Some(value) if field.control == SettingsControl::StringList => env_list_value(&value),
        Some(value) => json!(value),
        None => Value::Null,
    }
}

/// The effective value of an env-backed field when the process environment,
/// not `.env`, supplies it. `None` when `.env` is authoritative for the key.
fn env_process_override(field: &SettingsFieldSpec) -> Option<Value> {
    if !crate::dispatch::helpers::env_set_outside_dotenv(field.key) {
        return None;
    }
    let value = env_process_value(field);
    (!value.is_null()).then_some(value)
}

fn env_current_value(
    path: &std::path::Path,
    field: &SettingsFieldSpec,
) -> Result<Option<Value>, ToolError> {
    let file_value = env_file_value(path, field)?;
    Ok(file_value.or_else(|| {
        let value = env_process_value(field);
        (!value.is_null()).then_some(value)
    }))
}

fn env_override_source(
    path: &std::path::Path,
    field: &SettingsFieldSpec,
) -> Result<Option<(&'static str, String)>, ToolError> {
    let Some(name) = field.env_override else {
        return Ok(None);
    };
    Ok(env_file_value_by_name(path, name)?
        .or_else(|| crate::dispatch::helpers::env_non_empty(name))
        .map(|value| (name, value)))
}

fn env_override_value(field: &SettingsFieldSpec, value: &str) -> Value {
    match field.control {
        SettingsControl::Number => value
            .parse::<i64>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        SettingsControl::StringList => Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(|entry| json!(entry))
                .collect(),
        ),
        SettingsControl::Bool => value
            .parse::<bool>()
            .map_or_else(|_| json!(value), |parsed| json!(parsed)),
        _ => json!(value),
    }
}

pub fn redact_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let redacted = map
                .into_iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let looks_secret = lower.contains("secret")
                        || lower.contains("token")
                        || lower.contains("password")
                        || lower.contains("api_key")
                        || lower.contains("client_secret");
                    if looks_secret {
                        (key, json!({ "has_value": !value.is_null() }))
                    } else {
                        (key, redact_value(value))
                    }
                })
                .collect();
            Value::Object(redacted)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        other => other,
    }
}

fn cap_readonly_value(value: Value, depth: usize) -> Value {
    if depth >= READONLY_MAX_DEPTH {
        return json!({
            "truncated": true,
            "reason": "max_depth",
        });
    }

    match value {
        Value::String(text) if text.len() > READONLY_MAX_STRING_BYTES => {
            let preview = truncate_utf8_preview(text, READONLY_MAX_STRING_BYTES);
            json!({
                "truncated": true,
                "kind": "string",
                "bytes": preview.len(),
                "preview": preview,
            })
        }
        Value::Array(values) => {
            let original_len = values.len();
            let preview: Vec<Value> = values
                .into_iter()
                .take(READONLY_MAX_ARRAY_ITEMS)
                .map(|value| cap_readonly_value(value, depth + 1))
                .collect();
            if original_len > READONLY_MAX_ARRAY_ITEMS {
                json!({
                    "truncated": true,
                    "kind": "array",
                    "total_items": original_len,
                    "preview": preview,
                })
            } else {
                Value::Array(preview)
            }
        }
        Value::Object(map) => {
            let original_len = map.len();
            let mut preview = Map::new();
            for (key, value) in map.into_iter().take(READONLY_MAX_OBJECT_KEYS) {
                preview.insert(key, cap_readonly_value(value, depth + 1));
            }
            if original_len > READONLY_MAX_OBJECT_KEYS {
                json!({
                    "truncated": true,
                    "kind": "object",
                    "total_keys": original_len,
                    "preview": Value::Object(preview),
                })
            } else {
                Value::Object(preview)
            }
        }
        other => other,
    }
}

fn truncate_utf8_preview(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let end = text
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    text.truncate(end);
    text
}

pub fn config_patches_from_entries(
    entries: &[SettingsUpdateEntry],
) -> Result<Vec<crate::config::ConfigScalarPatch>, ToolError> {
    let fields = settings_fields_by_key();
    let mut patches = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            return Err(ToolError::InvalidParam {
                message: format!("unknown setting `{}`", entry.key),
                param: entry.key.clone(),
            });
        };
        if field.backend != SettingsBackend::ConfigToml
            || field.write_policy != SettingsWritePolicy::Editable
        {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is not editable through settings.config.update",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        if field.secret {
            return Err(ToolError::InvalidParam {
                message: "secret config writes are not supported by this settings slice".into(),
                param: entry.key.clone(),
            });
        }
        if let Some((name, _)) = env_override_source(&super::client::env_path(), field)? {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is overridden by env var `{name}` and cannot be edited here",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        require_previous(entry)?;
        patches.push(config_patch_for_field(field, entry)?);
    }
    Ok(patches)
}

pub fn expected_config_scalars(
    entries: &[SettingsUpdateEntry],
) -> Result<Vec<crate::config::ExpectedConfigScalar>, ToolError> {
    let fields = settings_fields_by_key();
    let mut expected = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            continue;
        };
        require_previous(entry)?;
        expected.push(crate::config::ExpectedConfigScalar::new(
            field.key,
            entry.previous.clone(),
        ));
    }
    Ok(expected)
}

fn config_patch_for_field(
    field: &SettingsFieldSpec,
    entry: &SettingsUpdateEntry,
) -> Result<crate::config::ConfigScalarPatch, ToolError> {
    use crate::config::{ConfigScalarPatch, ConfigScalarValue};
    if entry.unset {
        return Ok(ConfigScalarPatch::new(
            field.key,
            ConfigScalarValue::UnsetOptional,
        ));
    }
    let value = match field.control {
        SettingsControl::Bool => ConfigScalarValue::Bool(
            entry
                .value
                .as_bool()
                .ok_or_else(|| invalid_field(field, "must be boolean"))?,
        ),
        SettingsControl::Number => {
            let raw = entry
                .value
                .as_i64()
                .ok_or_else(|| invalid_field(field, "must be an integer"))?;
            validate_number_field(field, raw)?;
            ConfigScalarValue::I64(raw)
        }
        SettingsControl::Text | SettingsControl::Url | SettingsControl::Enum => {
            let raw = entry
                .value
                .as_str()
                .ok_or_else(|| invalid_field(field, "must be a string"))?
                .trim()
                .to_string();
            validate_string_field(field, &raw)?;
            ConfigScalarValue::String(raw)
        }
        SettingsControl::StringList => {
            let values = entry
                .value
                .as_array()
                .ok_or_else(|| invalid_field(field, "must be an array"))?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .collect::<Option<Vec<String>>>()
                .ok_or_else(|| invalid_field(field, "must be an array of strings"))?;
            validate_string_list_field(field, &values)?;
            ConfigScalarValue::StringList(values)
        }
        SettingsControl::ReadOnly => return Err(invalid_field(field, "is read-only")),
    };
    Ok(ConfigScalarPatch::new(field.key, value))
}

fn invalid_field(field: &SettingsFieldSpec, message: &'static str) -> ToolError {
    ToolError::InvalidParam {
        message: format!("{} {message}", field.key),
        param: field.key.to_string(),
    }
}

fn validate_number_field(field: &SettingsFieldSpec, value: i64) -> Result<(), ToolError> {
    if field.min.is_some_and(|min| value < min) {
        return Err(invalid_field(field, "below minimum"));
    }
    if field.max.is_some_and(|max| value > max) {
        return Err(invalid_field(field, "above maximum"));
    }
    Ok(())
}

fn validate_string_field(field: &SettingsFieldSpec, value: &str) -> Result<(), ToolError> {
    if matches!(field.key, "LABBY_MCP_HTTP_HOST" | "mcp.host")
        && value.parse::<std::net::IpAddr>().is_err()
        && url::Host::parse(value).is_err()
    {
        return Err(invalid_field(
            field,
            "must be an IP address or DNS host name without a port or URL scheme",
        ));
    }
    if matches!(field.key, "LABBY_LOG" | "log.filter")
        && tracing_subscriber::EnvFilter::try_new(value).is_err()
    {
        return Err(invalid_field(
            field,
            "must be a valid log filter, such as info or labby=debug,labby_apis=warn",
        ));
    }
    if field.control == SettingsControl::Url && !value.is_empty() {
        let parsed = url::Url::parse(value)
            .map_err(|_| invalid_field(field, "must be a valid HTTP or HTTPS URL"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.has_host()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid_field(
                field,
                "must be an HTTP or HTTPS URL without credentials, query, or fragment",
            ));
        }
    }
    if field.control == SettingsControl::Enum
        && !field.options.iter().any(|option| option.value == value)
    {
        return Err(invalid_field(field, "must be one of the allowed values"));
    }
    Ok(())
}

fn validate_string_list_field(
    field: &SettingsFieldSpec,
    values: &[String],
) -> Result<(), ToolError> {
    for value in values {
        if field.key == "api.cors_origins" {
            let parsed = url::Url::parse(value)
                .map_err(|_| invalid_field(field, "entries must be HTTP or HTTPS origins"))?;
            if !matches!(parsed.scheme(), "http" | "https")
                || !parsed.has_host()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err(invalid_field(
                    field,
                    "entries must be HTTP or HTTPS origins without a path, credentials, query, or fragment",
                ));
            }
        } else if field.key == "mcp.allowed_hosts" {
            if value == "*"
                || value.chars().any(char::is_whitespace)
                || value
                    .chars()
                    .any(|ch| matches!(ch, '/' | '\\' | '@' | '?' | '#'))
                || (value.parse::<std::net::IpAddr>().is_err()
                    && url::Url::parse(&format!("http://{value}/"))
                        .ok()
                        .is_none_or(|parsed| {
                            !parsed.has_host()
                                || !parsed.username().is_empty()
                                || parsed.password().is_some()
                        }))
            {
                return Err(invalid_field(
                    field,
                    "entries must be host names or IP addresses, optionally with a port; wildcard and URL values are not allowed",
                ));
            }
        }
    }
    Ok(())
}

pub fn env_entries_from_updates(
    entries: &[SettingsUpdateEntry],
) -> Result<Vec<crate::dispatch::setup::DraftEntry>, ToolError> {
    let fields = settings_fields_by_key();
    let mut out = Vec::new();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            return Err(ToolError::InvalidParam {
                message: format!("unknown setting `{}`", entry.key),
                param: entry.key.clone(),
            });
        };
        if field.backend != SettingsBackend::Env
            || field.write_policy != SettingsWritePolicy::Editable
        {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is not editable through settings.env.update",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        if env_process_override(field).is_some() {
            return Err(ToolError::InvalidParam {
                message: format!(
                    "setting `{}` is set in the server's process environment, which takes \
                     precedence over .env; change it where the process is started",
                    entry.key
                ),
                param: entry.key.clone(),
            });
        }
        require_previous(entry)?;
        let value = match field.control {
            SettingsControl::Number => {
                let raw = entry
                    .value
                    .as_i64()
                    .ok_or_else(|| invalid_field(field, "must be an integer"))?;
                validate_number_field(field, raw)?;
                raw.to_string()
            }
            SettingsControl::Bool => entry
                .value
                .as_bool()
                .ok_or_else(|| invalid_field(field, "must be a boolean"))?
                .to_string(),
            SettingsControl::Enum | SettingsControl::Text | SettingsControl::Url => {
                let raw = entry
                    .value
                    .as_str()
                    .ok_or_else(|| invalid_field(field, "must be a string"))?
                    .trim()
                    .to_string();
                validate_string_field(field, &raw)?;
                raw
            }
            SettingsControl::StringList => env_string_list_value(field, &entry.value)?,
            _ => return Err(invalid_field(field, "has unsupported env control")),
        };
        out.push(crate::dispatch::setup::DraftEntry {
            key: entry.key.clone(),
            value,
        });
    }
    Ok(out)
}

/// Serialize a list setting to its comma-separated `.env` form. The
/// administrator list is validated with the same parser the server uses at
/// startup, so a value saved here cannot make the next start fail closed.
fn env_string_list_value(field: &SettingsFieldSpec, value: &Value) -> Result<String, ToolError> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid_field(field, "must be a list of strings"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::trim)
                .ok_or_else(|| invalid_field(field, "must be a list of strings"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if items.iter().any(|item| item.contains(',')) {
        return Err(invalid_field(field, "entries must not contain commas"));
    }
    let joined = items
        .into_iter()
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    if field.key == ADMIN_EMAILS_KEY {
        return validate_admin_email_list(&joined).map_err(|message| invalid_field(field, message));
    }
    Ok(joined)
}

/// Validate a raw `LABBY_AUTH_ADMIN_EMAIL` value with the parser the server
/// uses at startup and return its canonical comma-separated form. Every setup
/// write path (`settings.env.update`, `draft.set`, `draft.commit`) goes
/// through this one check, so no path can stage a value that makes the next
/// start fail closed.
pub(super) fn validate_admin_email_list(raw: &str) -> Result<String, &'static str> {
    let emails = labby_auth::config::parse_admin_emails(raw);
    if emails.is_empty() {
        return Err("must list at least one administrator; an empty list locks every account out");
    }
    if !emails
        .iter()
        .all(|email| labby_auth::config::is_plausible_email(email))
    {
        return Err("entries must each be a single email address");
    }
    Ok(emails.join(","))
}

/// Value-level validation for an environment entry headed for `.env`,
/// independent of which setup action carries it.
pub(super) fn validate_env_entry_value(key: &str, value: &str) -> Result<(), ToolError> {
    if key == ADMIN_EMAILS_KEY {
        validate_admin_email_list(value).map_err(|message| ToolError::InvalidParam {
            message: format!("{key} {message}"),
            param: key.to_string(),
        })?;
    }
    Ok(())
}

fn secret_marker(raw: &str) -> Value {
    static SALT: OnceLock<String> = OnceLock::new();
    let salt = SALT.get_or_init(|| uuid::Uuid::new_v4().to_string());
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update([0]);
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    json!({
        "configured": true,
        "fingerprint": format!("opaque:{}", hex::encode(digest)),
    })
}

fn env_list_value(raw: &str) -> Value {
    json!(
        raw.split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
    )
}

pub fn validate_env_previous(
    entries: &[SettingsUpdateEntry],
    env_path: &std::path::Path,
) -> Result<(), ToolError> {
    let fields = settings_fields_by_key();
    for entry in entries {
        let Some(field) = fields.get(entry.key.as_str()) else {
            continue;
        };
        require_previous(entry)?;
        let current = env_current_value(env_path, field)?.unwrap_or(Value::Null);
        if current != entry.previous {
            return Err(ToolError::InvalidParam {
                message: format!("setting `{}` changed since it was loaded", entry.key),
                param: entry.key.clone(),
            });
        }
    }
    Ok(())
}

fn require_previous(entry: &SettingsUpdateEntry) -> Result<(), ToolError> {
    if entry.previous_present {
        return Ok(());
    }
    Err(ToolError::InvalidParam {
        message: format!(
            "setting `{}` requires previous for stale-write protection",
            entry.key
        ),
        param: entry.key.clone(),
    })
}

fn settings_fields_by_key() -> BTreeMap<&'static str, SettingsFieldSpec> {
    settings_fields()
        .into_iter()
        .map(|field| (field.key, field))
        .collect()
}

fn env_file_value(
    path: &std::path::Path,
    field: &SettingsFieldSpec,
) -> Result<Option<Value>, ToolError> {
    let Some(raw) = env_file_value_by_name(path, field.key)? else {
        return Ok(None);
    };
    if field.secret {
        return Ok(Some(secret_marker(&raw)));
    }
    if field.control == SettingsControl::Number {
        return Ok(raw
            .parse::<i64>()
            .map_or_else(|_| Some(json!(raw)), |parsed| Some(json!(parsed))));
    }
    if field.control == SettingsControl::Bool {
        return Ok(raw
            .parse::<bool>()
            .map_or_else(|_| Some(json!(raw)), |parsed| Some(json!(parsed))));
    }
    if field.control == SettingsControl::StringList {
        return Ok(Some(env_list_value(&raw)));
    }
    Ok(Some(json!(raw)))
}

fn env_file_value_by_name(path: &std::path::Path, name: &str) -> Result<Option<String>, ToolError> {
    let iter = match dotenvy::from_path_iter(path) {
        Ok(iter) => iter,
        Err(error) if error.not_found() => return Ok(None),
        Err(error) => {
            return Err(ToolError::Sdk {
                sdk_kind: "config_read_error".into(),
                message: format!(
                    "failed to read settings environment {}: {error}",
                    path.display()
                ),
            });
        }
    };
    let entries = iter
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "config_parse_error".into(),
            message: format!(
                "failed to parse settings environment {}: {error}",
                path.display()
            ),
        })?;
    Ok(entries
        .into_iter()
        .find_map(|(key, value)| (key == name).then_some(value)))
}

pub fn env_schema() -> Result<Vec<EnvSettingSpec>, ToolError> {
    build_env_schema_from_json(include_str!(
        "../../../../../docs/generated/env-reference.json"
    ))
}

fn build_env_schema_from_json(raw: &str) -> Result<Vec<EnvSettingSpec>, ToolError> {
    let mut by_key: BTreeMap<String, EnvSettingSpec> = BTreeMap::new();
    let generated: Value = serde_json::from_str(raw).map_err(|error| ToolError::Sdk {
        sdk_kind: "config_schema_error".into(),
        message: format!("generated environment catalog is invalid: {error}"),
    })?;
    let entries = generated.as_array().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "config_schema_error".into(),
        message: "generated environment catalog must be a JSON array".into(),
    })?;
    for entry in entries {
        let key = entry
            .get("env_var")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: "generated environment catalog entry is missing env_var".into(),
            })?;
        if by_key.contains_key(key) {
            return Err(ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!("generated environment catalog contains duplicate `{key}`"),
            });
        }
        let required_field = |name: &str| {
            entry.get(name).ok_or_else(|| ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!("generated environment catalog entry `{key}` is missing {name}"),
            })
        };
        let service = required_field("service")?
            .as_str()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!("generated environment catalog entry `{key}` has invalid service"),
            })?;
        let required = required_field("required")?
            .as_bool()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!(
                    "generated environment catalog entry `{key}` has invalid required"
                ),
            })?;
        let secret = required_field("secret")?
            .as_bool()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!("generated environment catalog entry `{key}` has invalid secret"),
            })?;
        let description =
            required_field("description")?
                .as_str()
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "config_schema_error".into(),
                    message: format!(
                        "generated environment catalog entry `{key}` has invalid description"
                    ),
                })?;
        let example = required_field("example")?
            .as_str()
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!("generated environment catalog entry `{key}` has invalid example"),
            })?;
        by_key.insert(
            key.to_string(),
            EnvSettingSpec {
                service: service.to_string(),
                key: key.to_string(),
                required,
                secret,
                description: description.to_string(),
                example: example.to_string(),
                editable: is_editable_core_env(key),
            },
        );
    }
    for required in [
        "LABBY_MCP_HTTP_HOST",
        "LABBY_MCP_HTTP_PORT",
        "LABBY_LOG",
        "LABBY_LOG_FORMAT",
        "LABBY_PUBLIC_URL",
        "LABBY_MCP_GATEWAY_URL",
        "LABBY_MCP_HTTP_TOKEN",
    ] {
        if !by_key.contains_key(required) {
            return Err(ToolError::Sdk {
                sdk_kind: "config_schema_error".into(),
                message: format!(
                    "generated environment catalog is incomplete: missing `{required}`"
                ),
            });
        }
    }
    for entry in super::client::cached_registry().services() {
        if let Some(meta) = crate::registry::service_meta(entry.name) {
            for (required, vars) in [(true, meta.required_env), (false, meta.optional_env)] {
                for var in vars {
                    by_key
                        .entry(var.name.to_string())
                        .and_modify(|existing| {
                            existing.secret |= var.secret;
                            existing.required |= required;
                            existing.editable = is_editable_core_env(var.name);
                        })
                        .or_insert_with(|| EnvSettingSpec {
                            service: entry.name.to_string(),
                            key: var.name.to_string(),
                            required,
                            secret: var.secret,
                            description: var.description.to_string(),
                            example: var.example.to_string(),
                            editable: is_editable_core_env(var.name),
                        });
                }
            }
        }
    }
    // ACP is retired from the gateway host; do not advertise stale env keys
    // even if generated env-reference.json still contains them.
    by_key.retain(|key, _| !(key.starts_with("LABBY_ACP_") || key.starts_with("ACP_")));
    Ok(by_key.into_values().collect())
}

fn is_editable_core_env(key: &str) -> bool {
    matches!(
        key,
        "LABBY_MCP_HTTP_HOST"
            | "LABBY_MCP_HTTP_PORT"
            | "LABBY_LOG"
            | "LABBY_LOG_FORMAT"
            | "LABBY_PUBLIC_URL"
            | "LABBY_MCP_GATEWAY_URL"
            | "LABBY_PHOENIX_OPENAI_BASE_URL"
            | "LABBY_PHOENIX_OPENAI_API_KEY"
            | "LABBY_AGENT_PROVIDER_PROTOCOL"
    )
}

/// Resolve the shared provider for a new Agent operation without mutating the
/// process environment. Service-manager overrides retain their precedence.
#[cfg(test)]
pub(crate) fn agent_provider_values(
    store: &crate::access::AccessStore,
) -> Result<(Option<String>, Option<String>), ToolError> {
    let saved = agent_provider_saved_values(store)?;
    Ok((
        saved
            .get(crate::dispatch::phoenix_openai::BASE_URL_ENV)
            .cloned(),
        saved
            .get(crate::dispatch::phoenix_openai::API_KEY_ENV)
            .cloned(),
    ))
}

pub(crate) fn agent_provider_configuration(
    store: &crate::access::AccessStore,
) -> Result<(Option<String>, Option<String>, Option<String>), ToolError> {
    let saved = agent_provider_saved_values(store)?;
    Ok((
        saved
            .get(crate::dispatch::phoenix_openai::BASE_URL_ENV)
            .cloned(),
        saved
            .get(crate::dispatch::phoenix_openai::API_KEY_ENV)
            .cloned(),
        saved
            .get(crate::dispatch::phoenix_openai::PROTOCOL_ENV)
            .cloned(),
    ))
}

fn agent_provider_saved_values(
    store: &crate::access::AccessStore,
) -> Result<BTreeMap<String, String>, ToolError> {
    let path = store.storage_dir().join(".env");
    let bytes = match super::secure_file::read_private(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            tracing::warn!(
                error_kind = ?error.kind(),
                raw_os_error = error.raw_os_error(),
                "protected Agent provider configuration read failed"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "provider_configuration_unavailable".into(),
                message: "The protected Agent provider configuration could not be read".into(),
            });
        }
    };
    let mut saved = BTreeMap::new();
    for pair in dotenvy::from_read_iter(bytes.as_slice()) {
        let (key, value) = pair.map_err(|_| ToolError::Sdk {
            sdk_kind: "provider_configuration_invalid".into(),
            message: "The protected Agent provider configuration is malformed".into(),
        })?;
        if saved.insert(key, value).is_some() {
            return Err(ToolError::Sdk {
                sdk_kind: "provider_configuration_invalid".into(),
                message: "The protected Agent provider configuration contains duplicate keys"
                    .into(),
            });
        }
    }
    for key in [
        crate::dispatch::phoenix_openai::BASE_URL_ENV,
        crate::dispatch::phoenix_openai::API_KEY_ENV,
        crate::dispatch::phoenix_openai::PROTOCOL_ENV,
    ] {
        if crate::dispatch::helpers::env_set_outside_dotenv(key) {
            if let Some(value) = crate::dispatch::helpers::env_non_empty(key) {
                saved.insert(key.into(), value);
            } else {
                saved.remove(key);
            }
        }
    }
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    #[test]
    fn provider_protocol_schema_is_populated_and_validated() {
        let fields = settings_fields();
        for field in &fields {
            assert!(!field.label.trim().is_empty());
            assert!(!field.description.trim().is_empty());
            if field.control == SettingsControl::Enum {
                assert!(!field.options.is_empty());
                for option in &field.options {
                    assert!(!option.label.trim().is_empty());
                    assert!(!option.value.is_empty());
                }
            }
        }
        let protocol = fields
            .iter()
            .find(|field| field.key == "LABBY_AGENT_PROVIDER_PROTOCOL")
            .unwrap();
        assert!(validate_string_field(protocol, "openai").is_ok());
        assert!(validate_string_field(protocol, "phoenix").is_ok());
        assert!(validate_string_field(protocol, "auto").is_err());
        assert!(validate_string_field(protocol, "").is_err());
        assert_eq!(
            crate::dispatch::phoenix_openai::ProviderProtocol::parse(None).unwrap(),
            crate::dispatch::phoenix_openai::ProviderProtocol::Phoenix
        );
    }

    #[tokio::test]
    async fn provider_configuration_resolves_protocol_with_connection() {
        let (_root, store, _) = crate::access::test_support::fixture().await;
        let path = store.storage_dir().join(".env");
        super::super::secure_file::replace_journal(&path, b"LABBY_PHOENIX_OPENAI_BASE_URL=https://standard.example/v1\nLABBY_AGENT_PROVIDER_PROTOCOL=openai\n").unwrap();
        crate::dispatch::helpers::with_env_keys_set_outside_dotenv(BTreeSet::new(), || {
            let (url, key, mode) = agent_provider_configuration(&store).unwrap();
            assert_eq!(url.as_deref(), Some("https://standard.example/v1"));
            assert_eq!(key, None);
            assert_eq!(mode.as_deref(), Some("openai"));
        });
    }

    #[tokio::test]
    async fn new_agent_connections_follow_saved_values_and_preserve_service_overrides() {
        let (_root, store, _) = crate::access::test_support::fixture().await;
        let path = store.storage_dir().join(".env");
        super::super::secure_file::replace_journal(&path, b"LABBY_PHOENIX_OPENAI_BASE_URL=https://saved.example/v1\nLABBY_PHOENIX_OPENAI_API_KEY=saved-secret\n").unwrap();
        let empty = BTreeSet::new();
        crate::dispatch::helpers::with_env_keys_set_outside_dotenv(empty, || {
            assert_eq!(
                agent_provider_values(&store).unwrap(),
                (
                    Some("https://saved.example/v1".into()),
                    Some("saved-secret".into())
                )
            );
        });
        crate::dispatch::helpers::with_env_override(
            HashMap::from([(
                "LABBY_PHOENIX_OPENAI_BASE_URL".into(),
                "https://managed.example/v1".into(),
            )]),
            || {
                crate::dispatch::helpers::with_env_keys_set_outside_dotenv(
                    BTreeSet::from(["LABBY_PHOENIX_OPENAI_BASE_URL".into()]),
                    || {
                        assert_eq!(
                            agent_provider_values(&store).unwrap().0.as_deref(),
                            Some("https://managed.example/v1")
                        );
                    },
                );
            },
        );
        super::super::secure_file::replace_journal(&path, b"MALFORMED private-secret LINE\n")
            .unwrap();
        let error = agent_provider_values(&store).unwrap_err().to_string();
        assert!(!error.contains("private-secret"));
    }

    #[test]
    fn malformed_dotenv_and_generated_catalog_fail_visibly() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        std::fs::write(&env, "MALFORMED LINE\nLABBY_LOG=labby=debug\n").unwrap();

        assert!(env_file_value_by_name(&env, "LABBY_LOG").is_err());
        assert!(build_env_schema_from_json("{").is_err());
        assert!(build_env_schema_from_json("[{}]").is_err());
        let duplicate = r#"[
          {"env_var":"LABBY_DUP","service":"lab","required":false,"secret":false,"description":"a","example":"1"},
          {"env_var":"LABBY_DUP","service":"lab","required":false,"secret":false,"description":"b","example":"2"}
        ]"#;
        assert!(build_env_schema_from_json(duplicate).is_err());
        let mut incomplete: Value = serde_json::from_str(include_str!(
            "../../../../../docs/generated/env-reference.json"
        ))
        .unwrap();
        incomplete.as_array_mut().unwrap().retain(|entry| {
            entry.get("env_var").and_then(Value::as_str) != Some("LABBY_MCP_HTTP_HOST")
        });
        assert!(build_env_schema_from_json(&incomplete.to_string()).is_err());
    }

    /// Finding 5: the process environment takes precedence over `.env`, so a
    /// variable set by the service manager makes a Settings save silently
    /// ineffective. The state must report the effective value and flag the
    /// override, and the shared env update must refuse the write.
    #[test]
    fn env_backed_field_reports_process_override_when_file_and_process_disagree() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        std::fs::write(&env, "LABBY_AUTH_ADMIN_EMAIL=a@example.com\n").unwrap();
        let fields = settings_fields_by_key();
        let field = &fields[ADMIN_EMAILS_KEY];
        let cfg = crate::config::LabConfig::default();
        let explicit = BTreeSet::new();
        let process_env = HashMap::from([(
            ADMIN_EMAILS_KEY.to_string(),
            "a@example.com,b@example.com".to_string(),
        )]);
        let update = || {
            vec![SettingsUpdateEntry {
                key: ADMIN_EMAILS_KEY.into(),
                value: json!(["a@example.com"]),
                previous: json!(["a@example.com", "b@example.com"]),
                unset: false,
                previous_present: true,
            }]
        };

        // Set outside `.env` (present before dotenv loaded): the running
        // server uses `a,b`, editing the file cannot change that.
        crate::dispatch::helpers::with_env_override(process_env.clone(), || {
            crate::dispatch::helpers::with_env_keys_set_outside_dotenv(
                BTreeSet::from([ADMIN_EMAILS_KEY.to_string()]),
                || {
                    let (value, source) = value_for_field(&cfg, field, &explicit, &env).unwrap();
                    assert_eq!(value, json!(["a@example.com", "b@example.com"]));
                    assert_eq!(source.source, SettingsSourceKind::Env);
                    assert_eq!(source.overridden_by_env.as_deref(), Some(ADMIN_EMAILS_KEY));
                    let error = env_entries_from_updates(&update()).unwrap_err();
                    assert_eq!(error.kind(), "invalid_param");
                    assert!(error.to_string().contains("process environment"), "{error}");
                },
            )
        });

        // The same disagreement after a Settings save that is waiting for a
        // restart is not an override: the file value stays editable.
        crate::dispatch::helpers::with_env_override(process_env, || {
            let (value, source) = value_for_field(&cfg, field, &explicit, &env).unwrap();
            assert_eq!(value, json!(["a@example.com"]));
            assert_eq!(source.overridden_by_env, None);
            assert!(env_entries_from_updates(&update()).is_ok());
        });
    }

    #[test]
    fn secret_marker_is_stable_and_opaque() {
        let first = secret_marker("human-chosen-key");
        let second = secret_marker("human-chosen-key");
        assert_eq!(first, second);
        assert_eq!(first["configured"], true);
        let fingerprint = first["fingerprint"].as_str().expect("opaque fingerprint");
        assert!(fingerprint.starts_with("opaque:"));
        assert!(!fingerprint.contains("human-chosen-key"));
        let unsalted = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(b"human-chosen-key"))
        );
        assert_ne!(fingerprint, unsalted);
    }

    #[test]
    fn settings_schema_keys_are_unique() {
        let schema = schema_response();
        let sections = schema
            .sections
            .iter()
            .map(|section| section.id)
            .collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        for field in schema.fields {
            assert!(seen.insert(field.key), "duplicate field {}", field.key);
            assert!(
                sections.contains(field.section),
                "field {} has no visible section",
                field.key
            );
        }
    }

    #[test]
    fn host_and_cors_lists_reject_values_that_cannot_work() {
        let fields = settings_fields();
        let hosts = fields
            .iter()
            .find(|field| field.key == "mcp.allowed_hosts")
            .unwrap();
        let origins = fields
            .iter()
            .find(|field| field.key == "api.cors_origins")
            .unwrap();
        for value in ["example.com", "example.com:8443", "127.0.0.1", "::1"] {
            assert!(
                validate_string_list_field(hosts, &[value.into()]).is_ok(),
                "{value}"
            );
        }
        for value in [
            "*",
            "https://example.com",
            "example.com/path",
            "user@example.com",
        ] {
            assert!(
                validate_string_list_field(hosts, &[value.into()]).is_err(),
                "{value}"
            );
        }
        for value in ["https://example.com", "http://localhost:3000"] {
            assert!(
                validate_string_list_field(origins, &[value.into()]).is_ok(),
                "{value}"
            );
        }
        for value in [
            "*",
            "example.com",
            "https://example.com/path",
            "https://user@example.com",
        ] {
            assert!(
                validate_string_list_field(origins, &[value.into()]).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn log_filter_settings_reject_invalid_directives() {
        let fields = settings_fields();
        for key in ["LABBY_LOG", "log.filter"] {
            let field = fields.iter().find(|field| field.key == key).unwrap();
            for value in ["info", "off", "labby=debug,labby_apis=warn"] {
                assert!(validate_string_field(field, value).is_ok(), "{value}");
            }
            assert!(validate_string_field(field, "labby=not-a-level").is_err());
        }
    }

    #[test]
    fn bind_host_settings_reject_ports_and_urls() {
        let fields = settings_fields();
        for key in ["LABBY_MCP_HTTP_HOST", "mcp.host"] {
            let field = fields.iter().find(|field| field.key == key).unwrap();
            for value in ["127.0.0.1", "::1", "labby.local"] {
                assert!(
                    validate_string_field(field, value).is_ok(),
                    "{key}: {value}"
                );
            }
            for value in ["", "localhost:8765", "https://labby.local", "host/path"] {
                assert!(
                    validate_string_field(field, value).is_err(),
                    "{key}: {value}"
                );
            }
        }
    }

    /// The settings editor must accept exactly the range config.toml accepts;
    /// a second hand-written ceiling silently rejects values the file allows.
    #[test]
    fn code_mode_timeout_setting_bounds_match_config_validation() {
        use labby_runtime::gateway_config::CodeModeConfig;
        let field = settings_fields()
            .into_iter()
            .find(|field| field.key == "code_mode.timeout_ms")
            .expect("code mode timeout setting");
        let max = field.max.expect("bounded setting");
        let accepted = CodeModeConfig {
            timeout_ms: u64::try_from(max).unwrap(),
            ..CodeModeConfig::default()
        };
        assert!(accepted.validate().is_ok(), "settings max must validate");
        let rejected = CodeModeConfig {
            timeout_ms: u64::try_from(max).unwrap() + 1,
            ..CodeModeConfig::default()
        };
        assert!(
            rejected.validate().is_err(),
            "settings max must be the config ceiling, not below it"
        );
        assert_eq!(field.min, Some(1));
        assert_eq!(
            max,
            i64::try_from(labby_runtime::gateway_config::MAX_CODE_MODE_TIMEOUT_MS).unwrap()
        );
    }

    #[test]
    fn code_mode_source_limit_is_editable_and_bounded() {
        let fields = settings_fields();
        let field = fields
            .iter()
            .find(|field| field.key == "code_mode.max_source_bytes")
            .expect("source limit setting");
        assert_eq!(field.write_policy, SettingsWritePolicy::Editable);
        assert_eq!(field.apply_mode, SettingsApplyMode::Partial);
        assert_eq!(field.min, Some(1024));
        assert_eq!(field.max, Some(1_048_576));
        assert_eq!(field.example, Some("1048576"));
    }

    #[test]
    fn auto_reconnect_setting_is_editable_immediate_bool() {
        let field = settings_fields()
            .into_iter()
            .find(|field| field.key == "gateway.auto_reconnect")
            .expect("auto reconnect setting");
        assert_eq!(field.control, SettingsControl::Bool);
        assert_eq!(field.write_policy, SettingsWritePolicy::Editable);
        assert_eq!(field.apply_mode, SettingsApplyMode::Immediate);
        assert_eq!(field.example, Some("false"));
    }

    #[test]
    fn dangerous_and_secret_config_is_not_editable_in_first_slice() {
        let fields = settings_fields();
        let mut keys = vec![
            "auth",
            "web.disable_auth",
            "upstream",
            "protected_mcp_routes",
            "deploy",
        ];
        // Gateway-owned fields are only declared in gateway builds.
        #[cfg(feature = "gateway")]
        keys.push("gateway.disable_spawn_guard");
        for key in keys {
            let field = fields.iter().find(|field| field.key == key).expect(key);
            assert_ne!(
                field.write_policy,
                SettingsWritePolicy::Editable,
                "{key} must not be scalar-editable"
            );
        }
    }

    #[test]
    fn env_override_metadata_is_present_for_shadowed_toml_fields() {
        let fields = settings_fields();
        for (key, env) in [
            ("log.filter", "LABBY_LOG"),
            ("log.format", "LABBY_LOG_FORMAT"),
            ("mcp.transport", "LABBY_MCP_TRANSPORT"),
            ("mcp.host", "LABBY_MCP_HTTP_HOST"),
            ("mcp.port", "LABBY_MCP_HTTP_PORT"),
            ("mcp.allowed_hosts", "LABBY_MCP_ALLOWED_HOSTS"),
            ("api.cors_origins", "LABBY_CORS_ORIGINS"),
            ("public_urls.app", "LABBY_PUBLIC_URL"),
            ("public_urls.mcp_gateway", "LABBY_MCP_GATEWAY_URL"),
        ] {
            assert_eq!(
                fields
                    .iter()
                    .find(|field| field.key == key)
                    .unwrap()
                    .env_override,
                Some(env),
                "{key} must advertise {env}"
            );
        }
    }

    #[test]
    fn redaction_removes_nested_secret_values() {
        let raw = json!({
            "oauth": { "client_secret": "super-secret" },
            "nested": [{ "api_key": "abc123" }],
            "safe": "visible"
        });
        let redacted = redact_value(raw);
        let serialized = serde_json::to_string(&redacted).unwrap();
        assert!(!serialized.contains("super-secret"));
        assert!(!serialized.contains("abc123"));
        assert!(serialized.contains("visible"));
    }

    #[test]
    fn config_update_rejects_readonly_and_secret_settings() {
        let entries = vec![SettingsUpdateEntry {
            key: "auth".into(),
            value: json!("********"),
            previous: json!(null),
            unset: false,
            previous_present: true,
        }];
        let err = config_patches_from_entries(&entries).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn env_update_accepts_only_allowlisted_core_env_keys() {
        let entries = vec![SettingsUpdateEntry {
            key: "LABBY_MCP_HTTP_PORT".into(),
            value: json!(8766),
            previous: json!(8765),
            unset: false,
            previous_present: true,
        }];
        let parsed = env_entries_from_updates(&entries).unwrap();
        assert_eq!(parsed[0].key, "LABBY_MCP_HTTP_PORT");
        assert_eq!(parsed[0].value, "8766");

        let rejected = vec![SettingsUpdateEntry {
            key: "LABBY_MCP_HTTP_TOKEN".into(),
            value: json!("secret"),
            previous: json!(null),
            unset: false,
            previous_present: true,
        }];
        assert!(env_entries_from_updates(&rejected).is_err());
    }

    #[test]
    fn env_port_rejects_values_outside_tcp_range() {
        let field = settings_fields()
            .into_iter()
            .find(|field| field.key == "LABBY_MCP_HTTP_PORT")
            .unwrap();
        assert_eq!((field.min, field.max), (Some(1), Some(65_535)));
        for value in [0, 65_536] {
            let entry = SettingsUpdateEntry {
                key: field.key.into(),
                value: json!(value),
                previous: json!(8765),
                unset: false,
                previous_present: true,
            };
            assert!(env_entries_from_updates(&[entry]).is_err());
        }
    }

    #[test]
    fn url_settings_reject_credentials_queries_and_invalid_hosts() {
        let field = settings_fields()
            .into_iter()
            .find(|field| field.key == "LABBY_PUBLIC_URL")
            .unwrap();
        for invalid in [
            "https://",
            "https://user:pass@example.com",
            "https://example.com/?token=1",
            "https://example.com/#section",
        ] {
            assert!(validate_string_field(&field, invalid).is_err(), "{invalid}");
        }
        assert!(validate_string_field(&field, "https://example.com/labby").is_ok());
    }

    #[test]
    fn env_previous_validation_accepts_matching_process_value_when_file_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let env_path = temp.path().join(".env");
        let entries = vec![SettingsUpdateEntry {
            key: "LABBY_LOG".into(),
            value: json!("labby=debug"),
            previous: json!("labby=info"),
            unset: false,
            previous_present: true,
        }];

        crate::dispatch::helpers::with_env_override(
            HashMap::from([("LABBY_LOG".to_string(), "labby=info".to_string())]),
            || validate_env_previous(&entries, &env_path),
        )
        .expect("process value should satisfy previous");
    }

    #[test]
    fn config_update_requires_previous_for_stale_protection() {
        let entries = vec![SettingsUpdateEntry {
            key: "mcp.port".into(),
            value: json!(8766),
            previous: json!(null),
            unset: false,
            previous_present: false,
        }];
        let err = config_patches_from_entries(&entries).unwrap_err();
        assert_eq!(err.kind(), "invalid_param");
    }

    #[test]
    fn dangerous_exposure_fields_are_not_normal_editable_settings() {
        let fields = settings_fields();
        let mut keys = vec!["admin.enabled"];
        // Gateway-owned fields are only declared in gateway builds.
        #[cfg(feature = "gateway")]
        keys.extend(["gateway_import_mode", "gateway.extra_stdio_commands"]);
        for key in keys {
            let field = fields.iter().find(|field| field.key == key).unwrap();
            assert_eq!(field.control, SettingsControl::ReadOnly);
            assert_eq!(
                field.write_policy,
                SettingsWritePolicy::DangerousFlowRequired
            );
        }
    }

    #[test]
    fn readonly_values_are_capped_for_advanced_state() {
        let value = json!((0..90).collect::<Vec<i32>>());
        let capped = cap_readonly_value(value, 0);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["kind"], "array");
        assert_eq!(capped["total_items"], 90);
    }

    #[test]
    fn readonly_string_capping_is_utf8_boundary_safe() {
        let value = json!(format!("{}é", "a".repeat(READONLY_MAX_STRING_BYTES - 1)));
        let capped = cap_readonly_value(value, 0);
        assert_eq!(capped["truncated"], true);
        assert_eq!(capped["kind"], "string");
        assert_eq!(
            capped["preview"].as_str().unwrap().len(),
            READONLY_MAX_STRING_BYTES - 1
        );
    }

    #[test]
    fn env_override_values_are_coerced_to_field_control() {
        let field = settings_fields()
            .into_iter()
            .find(|field| field.key == "mcp.port")
            .unwrap();
        assert_eq!(env_override_value(&field, "8766"), json!(8766));
    }

    #[test]
    fn env_schema_merges_generated_reference_and_plugin_meta() {
        let specs = env_schema().unwrap();
        let keys = vec!["LABBY_PUBLIC_URL", "LABBY_MCP_HTTP_TOKEN"];
        for key in keys {
            assert!(specs.iter().any(|spec| spec.key == key), "missing {key}");
        }
        assert!(
            !specs
                .iter()
                .any(|spec| spec.key.starts_with("LABBY_ACP_") || spec.key.starts_with("ACP_")),
            "no-acp builds must not advertise ACP env keys"
        );
        let token = specs
            .iter()
            .find(|spec| spec.key == "LABBY_MCP_HTTP_TOKEN")
            .unwrap();
        assert!(token.secret, "token must be secret");
    }

    #[test]
    fn env_schema_marks_supported_settings_editable() {
        let specs = env_schema().unwrap();
        for key in [
            "LABBY_LOG",
            "LABBY_PUBLIC_URL",
            "LABBY_MCP_GATEWAY_URL",
            "LABBY_PHOENIX_OPENAI_BASE_URL",
            "LABBY_PHOENIX_OPENAI_API_KEY",
        ] {
            assert!(
                specs.iter().find(|spec| spec.key == key).unwrap().editable,
                "{key} should be editable"
            );
        }
        assert!(
            !specs
                .iter()
                .find(|spec| spec.key == "LABBY_MCP_HTTP_TOKEN")
                .unwrap()
                .editable
        );
    }
}
