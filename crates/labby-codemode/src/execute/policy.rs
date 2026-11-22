use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;

use crate::namespace_alias::{namespaced_tool_id, split_namespaced_id};

use super::*;

pub(super) fn execution_allowed(caller: &CodeModeCaller, scope: &ToolScope) -> bool {
    caller.can_execute() || (scope.is_read_only() && caller.can_read())
}

/// Whether Labby's runner-reserved local Code Mode providers (`state`/`git`)
/// are injected and callable for this caller + scope: unscoped admin/trusted
/// callers only.
///
/// Exposed `pub` (re-exported from the crate root) so the MCP surface can apply
/// the same capability boundary. Local-provider calls dispatch through their
/// own decision/record hooks (`runner_drive.rs::enqueue_local_provider_call`),
/// but the current host does not persist or replay them.
#[must_use]
pub fn local_providers_allowed(caller: &CodeModeCaller, scope: &ToolScope) -> bool {
    caller.is_admin() && !scope.is_scoped()
}

/// Whether the OpenAPI local provider may be present for this execution.
/// Static/no-auth operations retain the operator-only boundary at dispatch;
/// authenticated non-admin callers gain access only when at least one loaded
/// operation is explicitly backed by subject-scoped OAuth.
#[must_use]
pub(crate) fn openapi_provider_allowed(
    caller: &CodeModeCaller,
    scope: &ToolScope,
    registry: &labby_openapi::OpenApiRegistry,
) -> bool {
    if scope.is_scoped() {
        return false;
    }
    local_providers_allowed(caller, scope)
        || (caller.can_execute()
            && caller.subject().is_some_and(|subject| !subject.is_empty())
            && registry.has_subject_scoped_operations())
}

/// Truncate a semantic query to at most [`MAX_SEMANTIC_QUERY_BYTES`], cutting
/// on a char boundary so the result stays valid UTF-8. Never errors — an
/// oversized query degrades to its prefix, mirroring the fail-open posture of
/// the rest of the semantic path.
pub(super) fn clamp_semantic_query(mut query: String) -> String {
    if query.len() > MAX_SEMANTIC_QUERY_BYTES {
        // `str::floor_char_boundary` is nightly-only; walk back from the cap
        // (at most 3 steps — UTF-8 sequences are ≤ 4 bytes) to the nearest
        // boundary. `is_char_boundary(0)` is always true, so this terminates.
        let mut boundary = MAX_SEMANTIC_QUERY_BYTES;
        while !query.is_char_boundary(boundary) {
            boundary -= 1;
        }
        query.truncate(boundary);
    }
    query
}

/// The `(include_snippets, use_cache)` discovery-render parameters for a Code
/// Mode execution's `caller`/`surface`/`scope`.
///
/// This is the single source of truth for the formulas `build_code_mode_proxy`
/// uses when rendering the sandbox's own discovery catalog. Hosts that
/// recompute the same render out-of-band (e.g. a gateway's `semantic_rank`
/// recomputing the scope-filtered entry set) MUST call this instead of
/// restating the formulas — the semantic-scope security invariant rests on
/// the two sites never diverging.
///
/// - snippets are included only for snippet-capable callers on unscoped runs;
/// - CLI executions use the one-shot catalog when unscoped or trusted-local.
///   Explicitly scoped authenticated callers use their live catalog.
pub fn discovery_render_params(
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    scope: &ToolScope,
) -> (bool, bool) {
    let include_snippets = caller.can_use_snippets() && !scope.is_scoped();
    // A local CLI without a gateway pool also uses the one-shot catalog for a
    // snippet's declared tools. The host limits upstreams before any probe.
    let use_cache = surface == CodeModeSurface::Cli
        && (scope.allowed_namespaces().is_none() || matches!(caller, CodeModeCaller::TrustedLocal));
    (include_snippets, use_cache)
}

/// Whether a rendered catalog entry is visible to the sandbox's discovery
/// catalog under `scope`. Callable tools must pass `scope.allows`; non-tool
/// catalog metadata does not consume tool grants and must already be filtered
/// by its owning source before it reaches this shared catalog.
///
/// This is discovery visibility only. Returning `true` for a metadata kind
/// never grants a dispatch/load capability; those operations remain separately
/// authorized by their own runtime paths.
///
/// Single source of truth for the post-render entry filter shared by
/// `build_code_mode_proxy` and any host recomputing the same scope-filtered
/// entry set (e.g. a gateway's `semantic_rank`) — see
/// [`discovery_render_params`] for why divergence here is a security bug,
/// not a style issue.
pub fn discovery_entry_visible(entry: &CatalogDescriptor, scope: &ToolScope) -> bool {
    match entry.kind {
        CodeModeCatalogKind::Tool => scope.allows(&entry.namespace, &entry.name),
        _ => true,
    }
}

pub(super) fn catalog_kind_enabled(
    kind: CodeModeCatalogKind,
    search: &CodeModeSearchConfig,
) -> bool {
    let configured = match kind {
        CodeModeCatalogKind::Tool => Some(CodeModeSearchKind::Tool),
        CodeModeCatalogKind::Snippet => Some(CodeModeSearchKind::Snippet),
        CodeModeCatalogKind::Prompt => Some(CodeModeSearchKind::Prompt),
        CodeModeCatalogKind::Skill => Some(CodeModeSearchKind::Skill),
        CodeModeCatalogKind::Command => Some(CodeModeSearchKind::Command),
        CodeModeCatalogKind::Subagent => Some(CodeModeSearchKind::Subagent),
        // Resource remains a compatibility-only discovery family and is not
        // part of the configurable six-kind policy.
        CodeModeCatalogKind::Resource => None,
    };
    configured.is_none_or(|kind| search.kinds.contains(&kind))
}

pub(super) fn personal_catalog_entry_enabled(
    entry: &CatalogDescriptor,
    scope: &ToolScope,
    search: &CodeModeSearchConfig,
) -> bool {
    search
        .sources
        .contains(&CodeModeSearchSource::PersonalLabby)
        && discovery_entry_visible(entry, scope)
        && catalog_kind_enabled(entry.kind, search)
}

pub(super) fn remove_soft_warning_if_it_breaks_budget(
    response: &mut CodeModeExecutionResponse,
    config: &CodeModeConfig,
) {
    let has_warning = response
        .result_shaping
        .as_ref()
        .and_then(|metadata| metadata.warning.as_ref())
        .is_some();
    if !has_warning
        || response_within_budget(
            response,
            config.max_response_bytes,
            config.max_response_tokens,
            config.token_estimate_divisor,
        )
    {
        return;
    }

    if let Some(metadata) = response.result_shaping.as_mut() {
        metadata.warning = None;
    }
    if config.result_shape_policy == CodeModeResultShapePolicy::Off {
        response.result_shaping = None;
    }
}

pub(super) fn ui_resource_uri(ui_meta: &Value) -> Option<&str> {
    ui_meta.get("resourceUri").and_then(Value::as_str)
}

/// Resolve a `callTool` id against this execution's scope.
///
/// A namespace that is only a case/separator alias of exactly one scoped
/// namespace (discovery shows `claude-macpoo` as `claude_macpoo`) resolves to
/// it. Anything else outside the scope fails with guidance naming what the
/// scope does allow.
pub(super) fn scoped_call_id(
    scope: &ToolScope,
    raw: &str,
    namespace: &str,
    tool: &str,
) -> Result<String, CodeModeCallError> {
    if scope.allows(namespace, tool) {
        return Ok(raw.to_string());
    }
    // Candidate namespaces: the explicit namespace scope, else namespaces
    // named by `upstream::tool` entries in the tools allowlist.
    let allowed_namespaces = scope.allowed_namespaces().cloned();
    let candidates = allowed_namespaces.clone().unwrap_or_else(|| {
        scope
            .allowed_tools()
            .into_iter()
            .flatten()
            .filter_map(|id| split_namespaced_id(id).map(|(ns, _)| ns.to_string()))
            .collect()
    });
    let resolved =
        match crate::resolve_namespace_alias(namespace, candidates.iter().map(String::as_str)) {
            crate::NamespaceResolution::Resolved(canonical) if canonical != namespace => {
                Some(canonical)
            }
            _ => None,
        };
    if let Some(canonical) = resolved
        && scope.allows(canonical, tool)
    {
        return Ok(namespaced_tool_id(canonical, tool));
    }
    let namespace_in_scope = allowed_namespaces
        .as_ref()
        .is_none_or(|allowed| resolved.is_some() || allowed.contains(namespace));
    Err(out_of_scope_call_error(
        scope,
        raw,
        namespace,
        namespace_in_scope,
        allowed_namespaces.unwrap_or_default(),
    ))
}

/// The `unknown_tool` error for a `callTool` id outside the execution scope,
/// naming what the scope does allow.
pub(super) fn out_of_scope_call_error(
    scope: &ToolScope,
    raw: &str,
    namespace: &str,
    namespace_in_scope: bool,
    allowed_namespaces: BTreeSet<String>,
) -> CodeModeCallError {
    const MAX_LISTED_TOOLS: usize = 25;
    let shown = crate::display_name(raw);
    let message = if namespace_in_scope {
        let tools = scope.allowed_tools().cloned().unwrap_or_default();
        let listed = crate::backtick_list(tools.iter().take(MAX_LISTED_TOOLS).map(String::as_str));
        let more_suffix = if tools.len() > MAX_LISTED_TOOLS {
            format!(" (and {} more)", tools.len() - MAX_LISTED_TOOLS)
        } else {
            String::new()
        };
        format!(
            "tool `{shown}` is outside this Code Mode execution capability set: it is not in the `tools` allowlist. Allowed: {listed}{more_suffix}. Call one of those, or rerun without the `tools` filter."
        )
    } else {
        format!(
            "tool `{shown}` is outside this Code Mode execution capability set: upstream `{}` is not in scope. {} Use codemode.search() to find a valid `upstream::tool` id.",
            crate::display_name(namespace),
            crate::unknown_namespace_guidance(namespace, &allowed_namespaces)
        )
    };
    CodeModeCallError::new("unknown_tool", message).with_tool(shown)
}

/// Access mode applied by a Code Mode tool scope.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeToolAccess {
    /// Permit only tools the host independently verifies as read-only.
    ReadOnly,
    /// Permit normal scoped tool access.
    #[default]
    Full,
}

/// Namespace/tool allowlist and access mode applied to one Code Mode execution.
#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolScope {
    namespaces: Option<BTreeSet<String>>,
    tools: BTreeSet<String>,
    access: CodeModeToolAccess,
    #[serde(default)]
    resources_disabled: bool,
    // None means the surface supplied no Skills binding; Some(None) explicitly disables it.
    #[serde(skip)]
    request_skill_context: Option<Option<String>>,
}

impl fmt::Debug for ToolScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolScope")
            .field("namespaces", &self.namespaces)
            .field("tools", &self.tools)
            .field("access", &self.access)
            .field("resources_disabled", &self.resources_disabled)
            .finish_non_exhaustive()
    }
}

impl ToolScope {
    /// Create an unscoped-by-default tool scope from optional namespace/tool filters.
    #[must_use]
    pub fn new(namespaces: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(None, namespaces, tools)
    }

    /// Create a scope where an empty namespace list means no namespaces are allowed.
    #[must_use]
    pub fn scoped_namespaces(namespaces: Vec<String>, tools: Vec<String>) -> Self {
        Self::new_inner(Some(BTreeSet::new()), namespaces, tools)
    }

    fn new_inner(
        scoped_default: Option<BTreeSet<String>>,
        namespaces: Vec<String>,
        tools: Vec<String>,
    ) -> Self {
        fn clean_set(values: Vec<String>) -> BTreeSet<String> {
            values
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        }
        let namespaces = clean_set(namespaces);
        Self {
            namespaces: if namespaces.is_empty() {
                scoped_default
            } else {
                Some(namespaces)
            },
            tools: clean_set(tools),
            access: CodeModeToolAccess::Full,
            resources_disabled: false,
            request_skill_context: None,
        }
    }

    /// Bind a request-local Skills context, or prohibit Skills for this request.
    #[must_use]
    pub fn with_request_skill_context(mut self, token: Option<&str>) -> Self {
        self.request_skill_context = Some(token.map(str::to_owned));
        self
    }

    /// A present request binding takes precedence over local defaults.
    #[must_use]
    pub fn request_skill_context(&self) -> Option<Option<&str>> {
        self.request_skill_context.as_ref().map(Option::as_deref)
    }

    /// Apply the route's upstream resource visibility to discovery and reads.
    #[must_use]
    pub fn with_resources_visible(mut self, visible: bool) -> Self {
        self.resources_disabled = !visible;
        self
    }

    /// Whether upstream resource discovery and reads are enabled.
    #[must_use]
    pub fn allows_resources(&self) -> bool {
        !self.resources_disabled
    }

    /// Restrict this execution to tools that the host can prove are read-only.
    /// Hosts must enforce this again against the live tool descriptor at the
    /// invocation boundary; discovery filtering alone is not authorization.
    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.access = CodeModeToolAccess::ReadOnly;
        self
    }

    /// Return whether the scope is restricted to verified read-only tools.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.access == CodeModeToolAccess::ReadOnly
    }

    /// Return whether a namespace/tool pair is included by the configured filters.
    #[must_use]
    pub fn allows(&self, namespace: &str, tool: &str) -> bool {
        (self
            .namespaces
            .as_ref()
            .is_none_or(|namespaces| namespaces.contains(namespace)))
            && (self.tools.is_empty()
                || self.tools.contains(tool)
                || self.tools.contains(&namespaced_tool_id(namespace, tool)))
    }

    /// Return whether any namespace, tool, or read-only restriction is active.
    #[must_use]
    pub fn is_scoped(&self) -> bool {
        self.namespaces.is_some()
            || self.resources_disabled
            || !self.tools.is_empty()
            || self.access == CodeModeToolAccess::ReadOnly
    }

    /// Borrow the `tools` allowlist (bare names or `namespace::tool` ids), if
    /// tool scoping is active. Mirrors [`Self::allowed_namespaces`].
    #[must_use]
    pub fn allowed_tools(&self) -> Option<&BTreeSet<String>> {
        (!self.tools.is_empty()).then_some(&self.tools)
    }

    /// Borrow the explicit namespace allowlist, if namespace scoping is active.
    #[must_use]
    pub fn allowed_namespaces(&self) -> Option<&BTreeSet<String>> {
        self.namespaces.as_ref()
    }

    /// Return a stable serialized fingerprint of this scope for ownership checks and caches.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut value = serde_json::json!({
            "access": match self.access {
                CodeModeToolAccess::ReadOnly => "read_only",
                CodeModeToolAccess::Full => "full",
            },
            "namespaces": self.namespaces.as_ref().map(|set| set.iter().cloned().collect::<Vec<_>>()),
            "tools": self.tools.iter().cloned().collect::<Vec<_>>(),
        });
        if self.resources_disabled {
            value["resources_disabled"] = Value::Bool(true);
        }
        value.to_string()
    }
}

pub(crate) fn source_capability_within_lookup(source: &str, lookup: &str) -> bool {
    if source == lookup {
        return true;
    }

    // Promotion sources are process-local. Different opaque fingerprints cannot
    // establish which tools or access mode the lookup allows.
    let (Ok(source), Ok(lookup)) = (
        serde_json::from_str::<ToolScope>(source),
        serde_json::from_str::<ToolScope>(lookup),
    ) else {
        return false;
    };
    if (lookup.is_read_only() && !source.is_read_only())
        || (!lookup.allows_resources() && source.allows_resources())
    {
        return false;
    }
    if let Some(namespaces) = &lookup.namespaces
        && source
            .namespaces
            .as_ref()
            .is_none_or(|set| !set.is_subset(namespaces))
    {
        return false;
    }
    lookup.tools.is_empty()
        || (!source.tools.is_empty()
            && source.tools.iter().all(|tool| {
                if let Some((namespace, name)) = split_namespaced_id(tool) {
                    lookup.allows(namespace, name)
                } else {
                    lookup.tools.contains(tool)
                        || source.namespaces.as_ref().is_some_and(|namespaces| {
                            namespaces
                                .iter()
                                .all(|namespace| lookup.allows(namespace, tool))
                        })
                }
            }))
}
