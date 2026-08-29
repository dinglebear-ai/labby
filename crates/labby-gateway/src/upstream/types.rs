//! Types for upstream MCP server proxy.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use rmcp::model::Tool;
use serde_json::Value;

/// Number of consecutive failures before marking an upstream unhealthy.
pub const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;

/// Base interval before the first reprobe of an open circuit.
pub const REPROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Maximum quarantine interval for a persistently broken upstream.
pub const MAX_REPROBE_INTERVAL: std::time::Duration = std::time::Duration::from_mins(30);

/// Exponential quarantine delay for an open circuit. The threshold failure
/// waits 30 seconds, then each additional failure doubles the delay up to 30
/// minutes. This prevents permanently broken OAuth/SSH peers from being
/// reprobed on every catalog request.
#[must_use]
pub fn reprobe_interval_for_failures(consecutive_failures: u32) -> std::time::Duration {
    let exponent = consecutive_failures
        .saturating_sub(CIRCUIT_BREAKER_THRESHOLD)
        .min(16);
    REPROBE_INTERVAL
        .saturating_mul(1_u32 << exponent)
        .min(MAX_REPROBE_INTERVAL)
}

/// A discovered upstream tool with its schema cached.
#[derive(Debug, Clone)]
pub struct UpstreamTool {
    /// The original tool definition from the upstream server.
    pub tool: Tool,
    /// Cached input schema as a JSON value for `schema` action proxying.
    pub input_schema: Option<Value>,
    /// Cached output schema as a JSON value for typed Code Mode returns.
    pub output_schema: Option<Value>,
    /// Name of the upstream server this tool belongs to.
    pub upstream_name: Arc<str>,
    /// Whether this tool is destructive for gateway-side safety gates.
    /// Missing annotations fail closed unless the upstream explicitly marks the
    /// tool read-only or non-destructive.
    pub destructive: bool,
}

/// Visibility metadata for one discovered upstream tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamToolExposureRow {
    pub name: String,
    pub description: Option<String>,
    pub exposed: bool,
    pub matched_by: Option<String>,
}

/// Cached, sanitized-at-source catalog data used by gateway enrichment.
///
/// This is intentionally schema-free and provenance-free. It is assembled from
/// the already-discovered in-memory catalog only, so callers can preview
/// enrichment without connecting to stdio upstreams or reading resources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpstreamEnrichmentCatalogEntry {
    pub upstream: String,
    pub tool_rows: Vec<UpstreamToolExposureRow>,
    pub resource_count: usize,
    pub prompt_count: usize,
}

/// Runtime metadata for process-backed upstream connections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpstreamRuntimeOwner {
    pub surface: String,
    pub subject: Option<String>,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub client_name: Option<String>,
    pub raw: Option<String>,
}

/// Runtime metadata for process-backed upstream connections.
///
/// On Unix, `pgid` holds the process group id used for `killpg` reaping.
/// On Windows, `job` owns a Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
/// set; closing it terminates the entire descendant tree. Both fields serve
/// the same role — each is only populated on its respective platform.
///
/// Cloning runtime metadata deliberately does not clone the Windows Job Object
/// handle ownership. Cloned metadata is used for logging and shutdown bookkeeping
/// only; the original value remains the authoritative owner and is closed once.
///
/// Default-constructed instances own no job. Only stdio-spawned connections
/// can contain one.
#[derive(Debug, Default)]
pub struct UpstreamRuntimeMetadata {
    pub pid: Option<u32>,
    /// Monotonic process generation for stdio-backed connections. Each respawn
    /// receives a new value so exit logs and invalidated requests can be tied
    /// to the exact child instance.
    pub generation: Option<u64>,
    /// Credential/client generation used to authenticate an OAuth connection.
    pub oauth_credential_generation: Option<u64>,
    pub pgid: Option<u32>,
    /// Opaque RAII owner for a Windows Job Object.
    #[cfg(windows)]
    pub(crate) job: Option<labby_winjob::JobObject>,
    pub started_at: Option<SystemTime>,
    pub origin: Option<String>,
    pub owner: Option<UpstreamRuntimeOwner>,
}

impl Clone for UpstreamRuntimeMetadata {
    fn clone(&self) -> Self {
        Self {
            pid: self.pid,
            generation: self.generation,
            oauth_credential_generation: self.oauth_credential_generation,
            pgid: self.pgid,
            #[cfg(windows)]
            job: None,
            started_at: self.started_at,
            origin: self.origin.clone(),
            owner: self.owner.clone(),
        }
    }
}

/// Runtime exposure policy applied to one upstream's discovered tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExposurePolicy {
    All,
    AllowList(Vec<ToolPattern>),
}

/// Runtime exposure policy applied to one upstream's validated Agent Skills.
///
/// This is deliberately a distinct capability type even though the legacy
/// `expose_skills` syntax currently compiles through the shared name matcher.
/// Keeping the type distinct prevents tool-only assumptions from leaking into
/// provider, provenance, requirement, and loadout rules as this policy grows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillExposurePolicy(ToolExposurePolicy);

/// One user-provided tool pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPattern {
    Exact(String),
    Wildcard(String),
}

impl ToolExposurePolicy {
    pub fn from_optional(patterns: Option<Vec<String>>) -> Result<Self, String> {
        match patterns {
            None => Ok(Self::All),
            Some(patterns) => Self::from_patterns(patterns),
        }
    }

    pub fn from_patterns(patterns: Vec<String>) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let trimmed = pattern.trim();
            if trimmed.is_empty() {
                return Err("exposure allowlist entries must not be empty".to_string());
            }
            if trimmed.contains('*') {
                compiled.push(ToolPattern::Wildcard(trimmed.to_string()));
            } else {
                compiled.push(ToolPattern::Exact(trimmed.to_string()));
            }
        }
        Ok(Self::AllowList(compiled))
    }

    #[must_use]
    pub fn matches(&self, tool_name: &str) -> bool {
        self.matched_by(tool_name).is_some()
    }

    #[must_use]
    pub fn matched_by(&self, tool_name: &str) -> Option<String> {
        match self {
            Self::All => Some("*".to_string()),
            Self::AllowList(patterns) => patterns.iter().find_map(|pattern| {
                pattern
                    .matches(tool_name)
                    .then(|| pattern.as_str().to_string())
            }),
        }
    }
}

impl SkillExposurePolicy {
    pub fn from_optional(patterns: Option<Vec<String>>) -> Result<Self, String> {
        ToolExposurePolicy::from_optional(patterns).map(Self)
    }

    pub fn from_patterns(patterns: Vec<String>) -> Result<Self, String> {
        ToolExposurePolicy::from_patterns(patterns).map(Self)
    }

    #[must_use]
    pub fn all() -> Self {
        Self(ToolExposurePolicy::All)
    }

    #[must_use]
    pub fn matches(&self, skill_name: &str) -> bool {
        self.0.matches(skill_name)
    }

    #[must_use]
    pub fn matched_by(&self, skill_name: &str) -> Option<String> {
        self.0.matched_by(skill_name)
    }

    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        matches!(self.0, ToolExposurePolicy::All)
    }
}

impl From<ToolExposurePolicy> for SkillExposurePolicy {
    fn from(policy: ToolExposurePolicy) -> Self {
        Self(policy)
    }
}

impl ToolPattern {
    #[must_use]
    pub fn matches(&self, candidate: &str) -> bool {
        match self {
            Self::Exact(value) => value == candidate,
            Self::Wildcard(pattern) => wildcard_matches(pattern, candidate),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Exact(value) | Self::Wildcard(value) => value.as_str(),
        }
    }
}

fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == candidate;
    }

    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let non_empty_parts: Vec<&str> = parts.into_iter().filter(|part| !part.is_empty()).collect();

    if non_empty_parts.is_empty() {
        return true;
    }

    // Use match_indices to advance the cursor only on char-boundary-aligned
    // byte offsets. `&str::match_indices` yields `(byte_offset, _)` pairs
    // where `byte_offset` is always a valid UTF-8 boundary, and each `part`
    // is itself a `&str` so `part.len()` is its UTF-8 byte length. This
    // keeps `cursor` on a valid boundary at all times — no slicing panic
    // possible for any valid UTF-8 candidate (including upstream tool
    // names containing multi-byte characters).
    let mut cursor: usize = 0;
    for (index, part) in non_empty_parts.iter().enumerate() {
        if index == 0 && anchored_start {
            if !candidate.starts_with(part) {
                return false;
            }
            cursor = part.len();
            continue;
        }

        match candidate
            .match_indices(*part)
            .find(|(idx, _)| *idx >= cursor)
        {
            Some((idx, _)) => cursor = idx + part.len(),
            None => return false,
        }
    }

    if anchored_end && let Some(last) = non_empty_parts.last() {
        return candidate.ends_with(last);
    }

    true
}

/// Capability-specific health buckets tracked independently for an upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpstreamCapability {
    /// Tool discovery and tool calls.
    Tools,
    /// Prompt listing and prompt retrieval.
    Prompts,
    /// Resource listing and resource reads.
    Resources,
    /// Agent Skills enumeration and retrieval (SEP-2640).
    Skills,
}

/// Health state of an upstream connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamHealth {
    /// Upstream is healthy and accepting requests.
    Healthy,
    /// Upstream has failed consecutively and is excluded from tool listing.
    Unhealthy {
        /// Number of consecutive failures.
        consecutive_failures: u32,
    },
}

impl UpstreamHealth {
    /// Whether this upstream should be included in tool listings.
    ///
    /// An upstream remains routable until its consecutive failures reach
    /// `CIRCUIT_BREAKER_THRESHOLD`. This is the inverse of `is_open`.
    #[must_use]
    pub const fn is_routable(self) -> bool {
        !self.is_open()
    }

    /// Whether this upstream has crossed the circuit breaker threshold.
    #[must_use]
    pub const fn is_open(self) -> bool {
        match self {
            Self::Healthy => false,
            Self::Unhealthy {
                consecutive_failures,
            } => consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD,
        }
    }
}

/// Snapshot of a single upstream server's state.
#[derive(Debug, Clone)]
pub struct UpstreamEntry {
    /// Human-readable name from config.
    pub name: Arc<str>,
    /// Discovered tools (keyed by tool name).
    pub tools: HashMap<String, UpstreamTool>,
    /// Exposure policy for discovered tools from this upstream.
    pub exposure_policy: ToolExposurePolicy,
    /// Exposure policy for this upstream's resources (`expose_resources`).
    ///
    /// Matched against the **bare, upstream-native** resource URI — the form
    /// cached in `resource_uris` and shown by `gateway.discovered_resources` —
    /// never the `lab://upstream/{name}/…` gateway rewrite.
    pub resource_exposure_policy: ToolExposurePolicy,
    /// Exposure policy for this upstream's prompts (`expose_prompts`).
    ///
    /// Matched against either the bare upstream prompt name or the
    /// `{upstream}/{name}` gateway-namespaced form — see
    /// `pool::entries::prompt_exposed`.
    pub prompt_exposure_policy: ToolExposurePolicy,
    /// Exposure policy for this upstream's Agent Skills (`expose_skills`).
    pub skill_exposure_policy: SkillExposurePolicy,
    /// Whether this upstream's Agent Skills are trusted for downstream proxying.
    pub proxy_skills: bool,
    /// Whether the most recent MCP initialize handshake advertised the Skills extension.
    /// `None` means the upstream has not completed a handshake in this pool yet.
    pub supports_skills: Option<bool>,
    /// Whether this upstream's resources are allowed to be proxied downstream.
    ///
    /// MCP App tools depend on their `ui://` resources being readable through the
    /// gateway, so top-level UI tool promotion also uses this flag.
    pub proxy_resources: bool,
    /// Last successfully discovered upstream prompt count.
    pub prompt_count: usize,
    /// Last successfully discovered upstream resource count.
    pub resource_count: usize,
    /// Number of skill candidates observed during the last successful `skills/list` walk,
    /// before Labby validation or exposure policy is applied.
    pub skill_count: usize,
    /// Cached validated skill names from the last successful skills/list walk.
    pub skill_names: Vec<String>,
    /// Cached upstream prompt names from the last successful list operation.
    pub prompt_names: Vec<String>,
    /// Cached upstream resource URIs from the last successful list operation.
    pub resource_uris: Vec<String>,
    /// Current tool-discovery/tool-call health state.
    pub tool_health: UpstreamHealth,
    /// Current prompt capability health state.
    pub prompt_health: UpstreamHealth,
    /// Current resource capability health state.
    pub resource_health: UpstreamHealth,
    /// Health of this upstream's skills capability.
    pub skill_health: UpstreamHealth,
    /// When the tools capability last became unhealthy.
    pub tool_unhealthy_since: Option<std::time::Instant>,
    /// When the prompts capability last became unhealthy.
    pub prompt_unhealthy_since: Option<std::time::Instant>,
    /// When the resources capability last became unhealthy.
    pub resource_unhealthy_since: Option<std::time::Instant>,
    /// When the skills capability first went unhealthy.
    pub skill_unhealthy_since: Option<std::time::Instant>,
    /// Most recent tools-capability failure detail.
    pub tool_last_error: Option<String>,
    /// Most recent prompts-capability failure detail.
    pub prompt_last_error: Option<String>,
    /// Most recent resources-capability failure detail.
    pub resource_last_error: Option<String>,
    /// Last skills-capability failure detail.
    pub skill_last_error: Option<String>,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{SkillExposurePolicy, ToolExposurePolicy, wildcard_matches};

    #[test]
    fn exact_and_wildcard_patterns_match_tool_names() {
        let policy = ToolExposurePolicy::from_patterns(vec![
            "search_repos".to_string(),
            "github_*".to_string(),
        ])
        .expect("policy");

        assert!(policy.matches("search_repos"));
        assert!(policy.matches("github_create_issue"));
        assert!(!policy.matches("delete_repo"));
    }

    #[test]
    fn missing_policy_defaults_to_all() {
        let policy = ToolExposurePolicy::from_optional(None).expect("policy");
        assert!(policy.matches("anything_at_all"));
    }

    #[test]
    fn skill_policy_preserves_legacy_pattern_semantics_behind_a_distinct_type() {
        let policy = SkillExposurePolicy::from_optional(Some(vec![
            "review-*".to_string(),
            "deploy".to_string(),
        ]))
        .expect("skill policy");

        assert!(!policy.is_unrestricted());
        assert!(policy.matches("review-pr"));
        assert!(policy.matches("deploy"));
        assert!(!policy.matches("delete-repository"));
        assert_eq!(policy.matched_by("review-pr").as_deref(), Some("review-*"));
    }

    #[test]
    fn missing_skill_policy_preserves_the_existing_expose_all_default() {
        let policy = SkillExposurePolicy::from_optional(None).expect("skill policy");

        assert!(policy.is_unrestricted());
        assert!(policy.matches("anything_at_all"));
    }

    #[test]
    fn wildcard_matching_supports_simple_globs() {
        assert!(wildcard_matches("github_*", "github_create_issue"));
        assert!(wildcard_matches("*_repo", "delete_repo"));
        assert!(wildcard_matches("search*repos", "search_public_repos"));
        assert!(!wildcard_matches("github_*", "gitlab_create_issue"));
    }

    #[test]
    fn wildcard_matches_does_not_panic_on_multibyte_char_boundary() {
        // Regression: pattern `f*o` against candidate `f∂o` previously
        // panicked at `candidate[cursor..].find(part)` because cursor=1
        // sits inside the 2-byte `∂` codepoint. The match_indices-based
        // implementation never byte-slices at hand-computed offsets.
        assert!(wildcard_matches("f*o", "f∂o"));
        assert!(!wildcard_matches("f*o", "f∂x"));
    }

    #[test]
    fn wildcard_matches_unicode_anchors() {
        assert!(wildcard_matches("*∂*", "prefix∂suffix"));
        assert!(wildcard_matches("∂*", "∂abc"));
        assert!(wildcard_matches("*∂", "abc∂"));
        assert!(wildcard_matches("a*b*c", "a∂b∂c"));
    }

    #[test]
    fn wildcard_matches_edge_cases() {
        assert!(wildcard_matches("*", ""));
        assert!(!wildcard_matches("a", ""));
        assert!(wildcard_matches("**", "anything"));
        // BIDI override (U+202E) is just another codepoint to the matcher.
        // Security normalization (rejecting BIDI/control chars at catalog
        // ingress) is out of scope for this fix — tracked separately.
        assert!(wildcard_matches("*\u{202E}*", "abc\u{202E}def"));
    }

    proptest::proptest! {
        #[test]
        fn wildcard_matches_never_panics(pattern in ".{0,32}", candidate in ".{0,128}") {
            // The only requirement is no panic. Return value is unconstrained —
            // any valid UTF-8 input must produce a bool without panicking.
            let _ = wildcard_matches(&pattern, &candidate);
        }

        #[test]
        fn wildcard_matches_star_injection_never_panics(
            parts in proptest::collection::vec(".{0,8}", 0..6),
            candidate in ".{0,64}",
        ) {
            let pattern = parts.join("*");
            let _ = wildcard_matches(&pattern, &candidate);
        }
    }
}

impl UpstreamEntry {
    /// Read the health for a specific upstream capability.
    #[must_use]
    pub const fn health_for(&self, capability: UpstreamCapability) -> UpstreamHealth {
        match capability {
            UpstreamCapability::Tools => self.tool_health,
            UpstreamCapability::Prompts => self.prompt_health,
            UpstreamCapability::Resources => self.resource_health,
            UpstreamCapability::Skills => self.skill_health,
        }
    }

    /// Update the health for a specific upstream capability.
    pub fn set_health_for(&mut self, capability: UpstreamCapability, health: UpstreamHealth) {
        match capability {
            UpstreamCapability::Tools => self.tool_health = health,
            UpstreamCapability::Prompts => self.prompt_health = health,
            UpstreamCapability::Resources => self.resource_health = health,
            UpstreamCapability::Skills => self.skill_health = health,
        }
    }

    /// Read the unhealthy timestamp for a specific upstream capability.
    #[must_use]
    pub const fn unhealthy_since_for(
        &self,
        capability: UpstreamCapability,
    ) -> Option<std::time::Instant> {
        match capability {
            UpstreamCapability::Tools => self.tool_unhealthy_since,
            UpstreamCapability::Prompts => self.prompt_unhealthy_since,
            UpstreamCapability::Resources => self.resource_unhealthy_since,
            UpstreamCapability::Skills => self.skill_unhealthy_since,
        }
    }

    /// Update the unhealthy timestamp for a specific upstream capability.
    pub fn set_unhealthy_since_for(
        &mut self,
        capability: UpstreamCapability,
        unhealthy_since: Option<std::time::Instant>,
    ) {
        match capability {
            UpstreamCapability::Tools => self.tool_unhealthy_since = unhealthy_since,
            UpstreamCapability::Prompts => self.prompt_unhealthy_since = unhealthy_since,
            UpstreamCapability::Resources => self.resource_unhealthy_since = unhealthy_since,
            UpstreamCapability::Skills => self.skill_unhealthy_since = unhealthy_since,
        }
    }

    /// Read the last failure detail for a specific upstream capability.
    #[must_use]
    pub fn last_error_for(&self, capability: UpstreamCapability) -> Option<&str> {
        match capability {
            UpstreamCapability::Tools => self.tool_last_error.as_deref(),
            UpstreamCapability::Prompts => self.prompt_last_error.as_deref(),
            UpstreamCapability::Resources => self.resource_last_error.as_deref(),
            UpstreamCapability::Skills => self.skill_last_error.as_deref(),
        }
    }

    /// Update the last failure detail for a specific upstream capability.
    pub fn set_last_error_for(
        &mut self,
        capability: UpstreamCapability,
        last_error: Option<String>,
    ) {
        match capability {
            UpstreamCapability::Tools => self.tool_last_error = last_error,
            UpstreamCapability::Prompts => self.prompt_last_error = last_error,
            UpstreamCapability::Resources => self.resource_last_error = last_error,
            UpstreamCapability::Skills => self.skill_last_error = last_error,
        }
    }
}
