use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Availability of one observed capability family. Unknown is distinct from a measured zero.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityObservationState {
    #[default]
    Unknown,
    Known,
    Stale,
    Failed,
}

/// Counts from a single capability observation, never inferred from a placeholder zero.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CapabilityFamilyObservation {
    pub state: CapabilityObservationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub discovered: Option<usize>,
    pub exposed: Option<usize>,
}

impl CapabilityFamilyObservation {
    /// One shared precedence rule: failures retain measurements, while never
    /// observed families retain absent counts rather than placeholder zeros.
    pub(crate) fn from_snapshot(
        counts: Option<(usize, usize)>,
        available: bool,
        stale: bool,
        error: Option<String>,
    ) -> Self {
        Self {
            state: if error.is_some() {
                CapabilityObservationState::Failed
            } else if counts.is_none() {
                CapabilityObservationState::Unknown
            } else if stale || !available {
                CapabilityObservationState::Stale
            } else {
                CapabilityObservationState::Known
            },
            discovered: counts.map(|counts| counts.0),
            exposed: counts.map(|counts| counts.1),
            error,
        }
    }

    pub(crate) fn observed(
        state: CapabilityObservationState,
        discovered: usize,
        exposed: usize,
    ) -> Self {
        Self {
            state,
            error: None,
            discovered: Some(discovered),
            exposed: Some(exposed),
        }
    }
}

/// Credential identity is deliberately excluded from serialized operator observations.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityObservationScope {
    #[default]
    Global,
    Credential,
}

/// Independently observed capability families for an upstream catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CapabilityObservation {
    pub scope: CapabilityObservationScope,
    pub tools: CapabilityFamilyObservation,
    pub resources: CapabilityFamilyObservation,
    pub prompts: CapabilityFamilyObservation,
    pub skills: CapabilityFamilyObservation,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema, Deserialize)]
pub struct SurfaceStateView {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub connected: bool,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema, Deserialize)]
pub struct SurfaceStatesView {
    #[serde(default)]
    pub cli: SurfaceStateView,
    #[serde(default)]
    pub api: SurfaceStateView,
    #[serde(default)]
    pub mcp: SurfaceStateView,
    #[serde(default)]
    pub webui: SurfaceStateView,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema, Deserialize)]
pub struct ServerWarningView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema, Deserialize)]
pub struct ServerConfigSummaryView {
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    /// Redacted executable for stdio transport (e.g. `uvx`, `npx`). `None` for HTTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Redacted args for stdio transport (e.g. `["github-chat-mcp"]`). Empty for HTTP.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema, Deserialize)]
pub struct ServerView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_observation: Option<CapabilityObservation>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub notification_incidents: std::collections::HashMap<String, String>,
    pub id: String,
    pub name: String,
    /// Operator-facing label; absent when the server has none. Presentation
    /// only — callers must keep using `id`/`name` to address the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub source: String,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub discovered_tool_count: usize,
    #[serde(default)]
    pub exposed_tool_count: usize,
    #[serde(default)]
    pub discovered_resource_count: usize,
    #[serde(default)]
    pub exposed_resource_count: usize,
    #[serde(default)]
    pub discovered_prompt_count: usize,
    #[serde(default)]
    pub exposed_prompt_count: usize,
    #[serde(default)]
    pub discovered_skill_count: usize,
    #[serde(default)]
    pub exposed_skill_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_skills: Option<bool>,
    #[serde(default)]
    pub surfaces: SurfaceStatesView,
    #[serde(default)]
    pub warnings: Vec<ServerWarningView>,
    #[serde(default)]
    pub config_summary: ServerConfigSummaryView,
    /// OS process id of the spawned stdio child, when connected. `None` for HTTP
    /// transports and for disconnected/disabled stdio servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}
