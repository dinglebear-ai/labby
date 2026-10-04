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
/// - the host's render cache is only safe for CLI executions with no explicit
///   namespace scope (everything else builds live).
pub fn discovery_render_params(
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    scope: &ToolScope,
) -> (bool, bool) {
    let include_snippets = caller.can_use_snippets() && !scope.is_scoped();
    let use_cache = surface == CodeModeSurface::Cli && scope.allowed_namespaces().is_none();
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
            .filter_map(|id| crate::split_namespaced_id(id).map(|(ns, _)| ns.to_string()))
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
        return Ok(crate::types::namespaced_tool_id(canonical, tool));
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
    allowed_namespaces: std::collections::BTreeSet<String>,
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
