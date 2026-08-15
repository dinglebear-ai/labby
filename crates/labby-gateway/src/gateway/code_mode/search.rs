//! Host-side Code Mode discovery catalog construction.
//!
//! Projects the gateway's live `UpstreamTool` set (plus snippet metadata) into
//! the crate-neutral `ToolDescriptor` catalog and serves it through the
//! manager-level render cache. Called from `code_mode_host.rs`'s
//! `CodeModeHost::list_tools` impl.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use labby_codemode::snippet::store::{SnippetInfo, builtin_snippet_dir, list_snippets};
use labby_codemode::{ToolDescriptor, ToolScope, ToolsRender, serialized_catalog_size};
use sha2::{Digest, Sha256};

use crate::gateway::manager::GatewayManager;
use crate::gateway::projection::{sanitize_schema, sanitize_tool_text};
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};
use labby_runtime::CodeModeConfig;
use labby_runtime::error::ToolError;
use labby_runtime::lab_home;

/// Hash of a tool's callable shape (description + input/output schema), so the
/// catalog render cache invalidates on a schema/description change even when
/// the upstream keeps the tool's name unchanged — a rename-only fingerprint
/// would otherwise keep serving a stale `.dts` from `codemode.describe()`.
fn tool_shape_digest(tool: &UpstreamTool) -> String {
    let payload = serde_json::json!({
        "description": tool.tool.description,
        "input_schema": tool.input_schema,
        "output_schema": tool.output_schema,
    });
    let serialized = serde_json::to_string(&payload).unwrap_or_default();
    let digest = Sha256::digest(serialized.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build (or serve from cache) the Code Mode discovery catalog for the proxy.
///
/// `use_cache` selects the on-disk one-shot CLI catalog cache vs the live
/// cold-connect path; `allowed_upstreams` scopes the projected tool set.
pub(crate) async fn build_tools_render(
    manager: &GatewayManager,
    allow_cold_connect: bool,
    owner: &UpstreamRuntimeOwner,
    oauth_subject: Option<&str>,
    allowed_upstreams: Option<&BTreeSet<String>>,
    scope: &ToolScope,
    include_snippets: bool,
    use_cache: bool,
) -> Result<ToolsRender, ToolError> {
    let raw_tools = if use_cache {
        manager
            .code_mode_catalog_tools_cached(Some(owner), oauth_subject)
            .await?
    } else {
        manager
            .code_mode_catalog_tools_allowed(
                allow_cold_connect,
                Some(owner),
                oauth_subject,
                allowed_upstreams,
            )
            .await?
    };
    let code_mode_config = manager.code_mode_config().await;
    catalog_from_tools(
        manager,
        filter_tools_for_access(raw_tools, scope, &code_mode_config),
        include_snippets,
    )
    .await
}

fn filter_tools_for_access(
    tools: Vec<UpstreamTool>,
    scope: &ToolScope,
    config: &CodeModeConfig,
) -> Vec<UpstreamTool> {
    if !scope.is_read_only() {
        return tools;
    }
    tools
        .into_iter()
        .filter(|tool| super::code_mode_host::tool_is_trusted_read_only(config, tool))
        .collect()
}

async fn catalog_from_tools(
    manager: &GatewayManager,
    raw_tools: Vec<UpstreamTool>,
    include_snippets: bool,
) -> Result<ToolsRender, ToolError> {
    // --- catalog render cache ---
    // Compute a cheap fingerprint from the sorted healthy tool ids. This detects
    // upstream additions/removals/renames without needing a pool generation
    // counter. The sort makes the fingerprint order-independent.
    let snippet_fingerprint = if include_snippets {
        snippet_directory_fingerprint("admin")
            .await?
            .unwrap_or_else(|| "snippets:absent".to_string())
    } else {
        "snippets:hidden".to_string()
    };

    let fingerprint = {
        let mut ids: Vec<String> = raw_tools
            .iter()
            .map(|t| {
                format!(
                    "{}::{}::{}",
                    t.upstream_name,
                    t.tool.name,
                    tool_shape_digest(t)
                )
            })
            .collect();
        ids.sort_unstable();
        format!("tools:\n{}\n{snippet_fingerprint}", ids.join("\n"))
    };

    if let Some((entries, catalog_json, serialized_size)) =
        manager.cached_catalog_render(&fingerprint).await
    {
        tracing::debug!(
            surface = "dispatch",
            service = labby_codemode::SERVICE,
            action = "catalog.build",
            entry_count = entries.len(),
            "Code Mode discovery catalog served from render cache"
        );
        // Best-effort embedding warm-up on the cache-hit path too — a no-op
        // unless semantic search is configured and the embedding cache is
        // cold for this fingerprint (fail-open; never fails catalog serving).
        let _warmed = manager
            .ensure_embeddings_for_fingerprint(&fingerprint, &entries)
            .await;
        return Ok(ToolsRender {
            fingerprint,
            entries,
            catalog_json,
            serialized_size,
        });
    }

    // Cache miss — build entries (includes `generate_tool_types` per entry).
    let mut entries = raw_tools
        .into_iter()
        .map(|tool| {
            let upstream = tool.upstream_name.to_string();
            let name = tool.tool.name.to_string();
            let description = tool
                .tool
                .description
                .as_ref()
                .map(|description| description.to_string())
                .unwrap_or_default();
            ToolDescriptor::tool(
                &upstream,
                &name,
                &sanitize_tool_text(&description, 2048),
                sanitize_schema(tool.input_schema),
                sanitize_schema(tool.output_schema),
            )
        })
        .collect::<Vec<_>>();

    if include_snippets {
        let snippets = snippet_metadata_for_catalog(manager, &snippet_fingerprint).await?;
        entries.extend(snippets.iter().map(ToolDescriptor::snippet));
    }

    entries.sort_by(|a, b| {
        a.kind.cmp(&b.kind).then_with(|| {
            a.namespace
                .cmp(&b.namespace)
                .then_with(|| a.name.cmp(&b.name))
        })
    });

    // The catalog is injected as `const tools` into the javy runner and never
    // enters the model context, so it is served complete and uncapped.
    let serialized_size = serialized_catalog_size(&entries)?;
    let catalog_json = serde_json::to_string(&entries).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize Code Mode discovery catalog: {err}"),
    })?;
    // Wrap ONCE here — every consumer below (the stored cache entry, the
    // returned render, and any later `describe_types` re-fetch of this same
    // fingerprint) shares this allocation via a cheap Arc clone instead of a
    // deep copy of the whole catalog.
    let entries: std::sync::Arc<[ToolDescriptor]> = std::sync::Arc::from(entries);
    let catalog_json: std::sync::Arc<str> = std::sync::Arc::from(catalog_json);

    manager
        .store_catalog_render_cache(super::CatalogRenderCache {
            fingerprint: fingerprint.clone(),
            entries: std::sync::Arc::clone(&entries),
            catalog_json: std::sync::Arc::clone(&catalog_json),
            serialized_size,
        })
        .await;

    // Best-effort catalog embedding warm-up: never blocks or fails catalog
    // construction (`ensure_embeddings_for_fingerprint` is fail-open by
    // contract). Deliberately awaited inline (not spawned) so the FIRST
    // `semantic_rank` call after a catalog change doesn't pay the cold-embed
    // cost on its own critical path — this list_tools call pays it instead.
    // `list_tools` is already cached for the CLI/unscoped path and is not
    // latency-critical, so this tradeoff is accepted rather than using a
    // detached `tokio::spawn`.
    let _warmed = manager
        .ensure_embeddings_for_fingerprint(&fingerprint, &entries)
        .await;

    Ok(ToolsRender {
        fingerprint,
        entries,
        catalog_json,
        serialized_size,
    })
}

async fn snippet_metadata_for_catalog(
    manager: &GatewayManager,
    fingerprint: &str,
) -> Result<Vec<SnippetInfo>, ToolError> {
    if let Some(snippets) = manager.cached_snippet_metadata(fingerprint).await {
        return Ok(snippets);
    }

    let lab_home = lab_home();
    let builtin_dir = builtin_snippet_dir();
    let snippets = tokio::task::spawn_blocking(move || list_snippets(&lab_home, &builtin_dir))
        .await
        .map_err(|err| {
            ToolError::internal_message(format!("snippet metadata task failed: {err}"))
        })??;

    manager
        .store_snippet_metadata_cache(super::SnippetMetadataCache {
            fingerprint: fingerprint.to_string(),
            entries: snippets.clone(),
        })
        .await;
    Ok(snippets)
}

async fn snippet_directory_fingerprint(policy: &str) -> Result<Option<String>, ToolError> {
    let lab_home = lab_home();
    let user_dir = labby_codemode::snippet::store::user_snippet_dir(&lab_home);
    let builtin_dir = builtin_snippet_dir();
    let policy = policy.to_string();
    tokio::task::spawn_blocking(move || {
        let mut parts = vec![format!("snippet_policy:{policy}")];
        let mut saw_dir = false;
        for dir in [user_dir, builtin_dir] {
            match directory_fingerprint_part(&dir)? {
                Some(part) => {
                    saw_dir = true;
                    parts.push(part);
                }
                None => parts.push(format!("{}:absent", dir.display())),
            }
        }
        Ok::<_, ToolError>(saw_dir.then(|| parts.join("\n")))
    })
    .await
    .map_err(|err| ToolError::internal_message(format!("snippet fingerprint task failed: {err}")))?
}

fn directory_fingerprint_part(dir: &Path) -> Result<Option<String>, ToolError> {
    let metadata = match std::fs::metadata(dir) {
        Ok(metadata) => metadata,
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            return Ok(None);
        }
        Err(err) => {
            return Err(ToolError::internal_message(format!(
                "read snippets directory `{}` metadata failed: {err}",
                dir.display()
            )));
        }
    };
    if !metadata.is_dir() {
        return Ok(None);
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let entries = directory_entries_fingerprint(dir)?;
    Ok(Some(format!(
        "{}:{}:{}:{}",
        normalize_path(dir),
        modified,
        metadata.len(),
        entries.join("|")
    )))
}

fn directory_entries_fingerprint(dir: &Path) -> Result<Vec<String>, ToolError> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        ToolError::internal_message(format!(
            "read snippets directory `{}` failed: {err}",
            dir.display()
        ))
    })?;
    let mut parts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            ToolError::internal_message(format!(
                "read snippets directory `{}` entry failed: {err}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|err| {
            ToolError::internal_message(format!(
                "read snippet entry `{}` metadata failed: {err}",
                path.display()
            ))
        })?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        parts.push(format!(
            "{}:{}:{}:{}",
            entry.file_name().to_string_lossy(),
            metadata.is_file(),
            metadata.len(),
            modified
        ));
    }
    parts.sort_unstable();
    Ok(parts)
}

fn normalize_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path))
        .display()
        .to_string()
}

#[cfg(all(test, unix))]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use super::*;
    use std::sync::Arc;

    // ── Issue #210 (lab-41e7m.3): catalog output-shape coverage ─────────────
    //
    // These pin the sanitize → ToolDescriptor::tool path that the cache-miss
    // branch of `catalog_from_tools` runs per upstream tool.

    /// An upstream `output_schema` reaches the descriptor and renders a real
    /// `Promise<T>` in both the one-line signature and the `.d.ts`.
    #[test]
    fn upstream_output_schema_reaches_descriptor_and_dts() {
        let output_schema = serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"]
        });

        let descriptor = ToolDescriptor::tool(
            "fixture",
            "query",
            &sanitize_tool_text("Query data", 2048),
            sanitize_schema(Some(serde_json::json!({ "type": "object" }))),
            sanitize_schema(Some(output_schema.clone())),
        );

        assert_eq!(descriptor.output_schema, Some(output_schema));
        assert!(
            descriptor.signature.contains("Promise<FixtureQueryOutput>"),
            "{}",
            descriptor.signature
        );
        assert!(
            descriptor.dts.contains("type FixtureQueryOutput = {"),
            "typed output must render a structural type, not `unknown`: {}",
            descriptor.dts
        );
        assert!(
            descriptor.dts.contains("ok: boolean;"),
            "{}",
            descriptor.dts
        );
    }

    /// Absent output schema is rendered truthfully as `unknown` — no
    /// fabricated type.
    #[test]
    fn missing_output_schema_renders_unknown_not_a_fabricated_type() {
        let descriptor = ToolDescriptor::tool(
            "fixture",
            "query",
            "Query data",
            sanitize_schema(Some(serde_json::json!({ "type": "object" }))),
            sanitize_schema(None),
        );

        assert_eq!(descriptor.output_schema, None);
        assert!(
            descriptor
                .dts
                .contains("type FixtureQueryOutput = unknown;"),
            "{}",
            descriptor.dts
        );
    }

    /// Pathological schemas must not panic or leak: an oversized schema is
    /// dropped to `None` by `sanitize_schema`'s 512 KB input-size gate
    /// (`MAX_SCHEMA_BYTES`) — a different mechanism from the type renderer's
    /// expansion budget (and the type
    /// falls back to `unknown`); a malformed one renders defensively.
    #[test]
    fn pathological_output_schemas_degrade_without_panic() {
        let oversized = serde_json::json!({
            "type": "object",
            "description": "x".repeat(600_000)
        });
        let descriptor = ToolDescriptor::tool(
            "fixture",
            "query",
            "Query data",
            None,
            sanitize_schema(Some(oversized)),
        );
        assert_eq!(
            descriptor.output_schema, None,
            "oversized schema must drop to None"
        );
        assert!(
            descriptor
                .dts
                .contains("type FixtureQueryOutput = unknown;"),
            "{}",
            descriptor.dts
        );

        let malformed = serde_json::json!({
            "type": 42,
            "properties": "not-an-object",
            "items": { "$ref": "#/definitions/missing" }
        });
        let descriptor = ToolDescriptor::tool(
            "fixture",
            "query",
            "Query data",
            None,
            sanitize_schema(Some(malformed.clone())),
        );
        assert_eq!(
            descriptor.output_schema,
            Some(malformed),
            "malformed-but-small schemas are relayed; only the TYPE render degrades"
        );
        assert!(
            descriptor
                .dts
                .contains("type FixtureQueryOutput = Record<string, unknown>;"),
            "malformed type/properties must degrade to a defensive open record, never a fabricated type: {}",
            descriptor.dts
        );
    }

    #[test]
    fn read_only_catalog_filter_is_fail_closed() {
        let named = Arc::<str>::from("fixture");
        let make = |annotations: Option<rmcp::model::ToolAnnotations>| {
            let mut tool = rmcp::model::Tool::new(
                "query".to_string(),
                "Query data",
                Arc::new(serde_json::Map::new()),
            );
            tool.annotations = annotations;
            UpstreamTool {
                tool,
                input_schema: None,
                output_schema: None,
                upstream_name: Arc::clone(&named),
                destructive: false,
            }
        };
        let tools = vec![
            make(None),
            make(Some(rmcp::model::ToolAnnotations::new().destructive(false))),
            make(Some(rmcp::model::ToolAnnotations::new().read_only(true))),
        ];

        let config = CodeModeConfig {
            trusted_read_only_tools: vec!["fixture::query".to_string()],
            ..CodeModeConfig::default()
        };
        let read_only_scope = ToolScope::default().read_only();
        let filtered = filter_tools_for_access(tools, &read_only_scope, &config);
        assert_eq!(filtered.len(), 1);
        assert!(super::super::code_mode_host::tool_is_trusted_read_only(
            &config,
            &filtered[0]
        ));
    }

    #[test]
    fn read_only_catalog_requires_operator_trust_in_addition_to_hint() {
        let named = Arc::<str>::from("fixture");
        let mut tool = rmcp::model::Tool::new(
            "query".to_string(),
            "Query data",
            Arc::new(serde_json::Map::new()),
        );
        tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
        let tool = UpstreamTool {
            tool,
            input_schema: None,
            output_schema: None,
            upstream_name: named,
            destructive: false,
        };

        let read_only_scope = ToolScope::default().read_only();
        assert!(
            filter_tools_for_access(
                vec![tool.clone()],
                &read_only_scope,
                &CodeModeConfig::default()
            )
            .is_empty()
        );

        let trusted = CodeModeConfig {
            trusted_read_only_tools: vec!["fixture::query".to_string()],
            ..CodeModeConfig::default()
        };
        assert_eq!(
            filter_tools_for_access(vec![tool], &read_only_scope, &trusted).len(),
            1
        );
    }

    #[test]
    fn inaccessible_snippet_directory_is_absent_from_fingerprint() {
        use std::os::unix::fs::PermissionsExt;

        let blocked_parent = tempfile::tempdir().expect("temporary parent");
        let snippet_dir = blocked_parent.path().join("snippets");
        std::fs::create_dir(&snippet_dir).expect("snippet directory");
        std::fs::set_permissions(
            blocked_parent.path(),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("block parent traversal");

        let result = directory_fingerprint_part(&snippet_dir);

        std::fs::set_permissions(
            blocked_parent.path(),
            std::fs::Permissions::from_mode(0o700),
        )
        .expect("restore parent traversal");
        assert_eq!(
            result.expect("inaccessible directory should fail open"),
            None
        );
    }
}
