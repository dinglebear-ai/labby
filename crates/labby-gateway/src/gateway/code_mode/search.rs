//! Host-side Code Mode discovery catalog construction.
//!
//! Projects the gateway's live `UpstreamTool` set (plus snippet metadata) into
//! the crate-neutral `ToolDescriptor` catalog and serves it through the
//! manager-level render cache. Called from `code_mode_host.rs`'s
//! `CodeModeHost::list_tools` impl.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use labby_codemode::snippet::store::{SnippetInfo, builtin_snippet_dir, list_snippets};
use labby_codemode::{CodeModeToolSafety, ToolDescriptor, ToolScope, ToolsRender};
use sha2::{Digest, Sha256};

use crate::gateway::manager::GatewayManager;
use crate::gateway::projection::{sanitize_schema, sanitize_tool_text};
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};
use labby_runtime::error::ToolError;
use labby_runtime::lab_home;

/// Hash of a tool's callable shape (description + input/output schema), so the
/// catalog render cache invalidates on a schema/description change even when
/// the upstream keeps the tool's name unchanged — a rename-only fingerprint
/// would otherwise keep serving a stale `.dts` from `codemode.describe()`.
fn tool_shape_digest(tool: &UpstreamTool) -> String {
    let safety = normalized_tool_safety(tool);
    let payload = serde_json::json!({
        "description": tool.tool.description,
        "input_schema": tool.input_schema,
        "output_schema": tool.output_schema,
        "safety": safety,
    });
    let serialized = serde_json::to_string(&payload).unwrap_or_default();
    let digest = Sha256::digest(serialized.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn normalized_tool_safety(tool: &UpstreamTool) -> Option<CodeModeToolSafety> {
    let annotations = tool.tool.annotations.as_ref();
    let destructive_hint = annotations.and_then(|value| value.destructive_hint);
    let destructive = if destructive_hint == Some(true) || tool.destructive {
        Some(true)
    } else if destructive_hint == Some(false) {
        Some(false)
    } else {
        None
    };
    let read_only = (annotations.and_then(|value| value.read_only_hint) == Some(true)
        && destructive != Some(true))
    .then_some(true);
    let safety = CodeModeToolSafety {
        read_only,
        destructive,
    };
    (!safety.is_empty()).then_some(safety)
}

#[cfg(all(test, unix))]
fn embedding_corpus_fingerprint(tools: &[UpstreamTool]) -> String {
    let mut corpus = tools
        .iter()
        .map(|tool| {
            format!(
                "{}::{}\0{}",
                tool.upstream_name,
                tool.tool.name,
                tool.tool.description.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    corpus.sort_unstable();
    let mut digest = Sha256::new();
    for item in corpus {
        digest.update((item.len() as u64).to_be_bytes());
        digest.update(item.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn rendered_embedding_corpus_fingerprint(entries: &[ToolDescriptor]) -> String {
    let mut corpus = entries
        .iter()
        .map(|entry| format!("{}\0{}", entry.id, entry.description))
        .collect::<Vec<_>>();
    corpus.sort_unstable();
    let mut digest = Sha256::new();
    for item in corpus {
        digest.update((item.len() as u64).to_be_bytes());
        digest.update(item.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn render_from_cached_catalog(
    fingerprint: String,
    entries: std::sync::Arc<[ToolDescriptor]>,
    catalog_json: std::sync::Arc<str>,
    serialized_size: usize,
) -> ToolsRender {
    let embedding_fingerprint = rendered_embedding_corpus_fingerprint(&entries);
    ToolsRender {
        fingerprint,
        embedding_fingerprint,
        entries,
        catalog_json,
        serialized_size,
    }
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
    catalog_from_tools(
        manager,
        filter_tools_for_access(raw_tools, scope),
        include_snippets,
    )
    .await
}

fn filter_tools_for_access(tools: Vec<UpstreamTool>, scope: &ToolScope) -> Vec<UpstreamTool> {
    if !scope.is_read_only() {
        return tools;
    }
    tools
        .into_iter()
        .filter(super::code_mode_host::tool_is_explicitly_read_only)
        .collect()
}

pub(super) async fn catalog_from_tools(
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
        let render =
            render_from_cached_catalog(fingerprint, entries, catalog_json, serialized_size);
        // Best-effort embedding warm-up on the cache-hit path too — a no-op
        // unless semantic search is configured and the embedding cache is
        // cold for this fingerprint (fail-open; never fails catalog serving).
        let _warmed = manager
            .ensure_embeddings_for_fingerprint(&render.embedding_fingerprint, &render.entries)
            .await;
        return Ok(render);
    }

    let flight = manager.catalog_render_flight(&fingerprint).await;
    let _build_guard = flight.build.lock().await;
    if let Some((entries, catalog_json, serialized_size)) =
        manager.cached_catalog_render(&fingerprint).await
    {
        return Ok(render_from_cached_catalog(
            fingerprint,
            entries,
            catalog_json,
            serialized_size,
        ));
    }
    if let Some(cache) = flight.result.lock().await.clone() {
        return Ok(render_from_cached_catalog(
            fingerprint,
            cache.entries,
            cache.catalog_json,
            cache.serialized_size,
        ));
    }

    // Cache miss — build entries (includes `generate_tool_types` per entry).
    let mut entries = raw_tools
        .into_iter()
        .map(|tool| {
            let safety = normalized_tool_safety(&tool);
            let upstream = tool.upstream_name.to_string();
            let name = tool.tool.name.to_string();
            let description = tool
                .tool
                .description
                .as_ref()
                .map(|description| description.to_string())
                .unwrap_or_default();
            ToolDescriptor::tool_with_safety(
                &upstream,
                &name,
                &sanitize_tool_text(&description, 2048),
                sanitize_schema(tool.input_schema),
                sanitize_schema(tool.output_schema),
                safety,
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
    let embedding_fingerprint = rendered_embedding_corpus_fingerprint(&entries);

    // The catalog is injected as `const tools` into the javy runner and never
    // enters the model context, so it is served complete and uncapped.
    let catalog_json = serde_json::to_string(&entries).map_err(|err| ToolError::Sdk {
        sdk_kind: "internal_error".to_string(),
        message: format!("failed to serialize Code Mode discovery catalog: {err}"),
    })?;
    let serialized_size = catalog_json.len();
    // Wrap ONCE here — every consumer below (the stored cache entry, the
    // returned render, and any later `describe_types` re-fetch of this same
    // fingerprint) shares this allocation via a cheap Arc clone instead of a
    // deep copy of the whole catalog.
    let entries: std::sync::Arc<[ToolDescriptor]> = std::sync::Arc::from(entries);
    let catalog_json: std::sync::Arc<str> = std::sync::Arc::from(catalog_json);

    let cache = super::CatalogRenderCache {
        fingerprint: fingerprint.clone(),
        entries: std::sync::Arc::clone(&entries),
        catalog_json: std::sync::Arc::clone(&catalog_json),
        serialized_size,
    };
    *flight.result.lock().await = Some(cache.clone());
    manager.store_catalog_render_cache(cache).await;

    // Best-effort catalog embedding warm-up: never blocks or fails catalog
    // construction (`ensure_embeddings_for_fingerprint` is fail-open by
    // contract). Deliberately awaited inline (not spawned) so the FIRST
    // `semantic_rank` call after a catalog change doesn't pay the cold-embed
    // cost on its own critical path — this list_tools call pays it instead.
    // `list_tools` is already cached for the CLI/unscoped path and is not
    // latency-critical, so this tradeoff is accepted rather than using a
    // detached `tokio::spawn`.
    let _warmed = manager
        .ensure_embeddings_for_fingerprint(&embedding_fingerprint, &entries)
        .await;

    Ok(ToolsRender {
        fingerprint,
        embedding_fingerprint,
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

    fn safety_fixture(annotations: Option<rmcp::model::ToolAnnotations>) -> UpstreamTool {
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
            upstream_name: Arc::from("fixture"),
            destructive: false,
        }
    }

    #[test]
    fn safety_normalization_is_compact_and_fail_closed() {
        assert_eq!(normalized_tool_safety(&safety_fixture(None)), None);
        assert_eq!(
            normalized_tool_safety(&safety_fixture(Some(
                rmcp::model::ToolAnnotations::new().read_only(true)
            ))),
            Some(CodeModeToolSafety {
                read_only: Some(true),
                destructive: None,
            })
        );
        assert_eq!(
            normalized_tool_safety(&safety_fixture(Some(
                rmcp::model::ToolAnnotations::new().destructive(false)
            ))),
            Some(CodeModeToolSafety {
                read_only: None,
                destructive: Some(false),
            })
        );

        let mut contradictory = safety_fixture(Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(true),
        ));
        contradictory.destructive = true;
        assert_eq!(
            normalized_tool_safety(&contradictory),
            Some(CodeModeToolSafety {
                read_only: None,
                destructive: Some(true),
            })
        );
    }

    #[test]
    fn safety_changes_render_identity_but_not_embedding_identity() {
        let plain = safety_fixture(None);
        let annotated = safety_fixture(Some(rmcp::model::ToolAnnotations::new().read_only(true)));
        assert_ne!(tool_shape_digest(&plain), tool_shape_digest(&annotated));
        assert_eq!(
            embedding_corpus_fingerprint(std::slice::from_ref(&plain)),
            embedding_corpus_fingerprint(std::slice::from_ref(&annotated))
        );

        let mut renamed = annotated;
        renamed.tool.description = Some("Different ranking text".into());
        assert_ne!(
            embedding_corpus_fingerprint(std::slice::from_ref(&plain)),
            embedding_corpus_fingerprint(std::slice::from_ref(&renamed))
        );
    }

    #[test]
    fn snippet_membership_changes_rendered_embedding_identity() {
        use labby_codemode::snippet::store::{SnippetInfo, SnippetSource};

        let tool = ToolDescriptor::tool("fixture", "query", "Query data", None, None);
        let snippet = ToolDescriptor::snippet(&SnippetInfo {
            tools: None,
            name: "summarize".to_string(),
            description: Some("Summarize results".to_string()),
            tags: vec![],
            inputs: Default::default(),
            source: SnippetSource::User,
            path: "summarize.md".into(),
            shadowed: false,
        });

        assert_ne!(
            rendered_embedding_corpus_fingerprint(std::slice::from_ref(&tool)),
            rendered_embedding_corpus_fingerprint(&[tool, snippet])
        );
    }

    #[tokio::test]
    async fn concurrent_same_identity_renders_share_arc_allocations() {
        let dir = tempfile::tempdir().expect("temporary config root");
        let manager = GatewayManager::new(
            dir.path().join("config.toml"),
            crate::gateway::runtime::GatewayRuntimeHandle::default(),
        );
        let tool = safety_fixture(Some(rmcp::model::ToolAnnotations::new().read_only(true)));
        let renders = futures::future::join_all(
            (0..16).map(|_| catalog_from_tools(&manager, vec![tool.clone()], false)),
        )
        .await
        .into_iter()
        .map(|result| result.expect("render succeeds"))
        .collect::<Vec<_>>();

        for render in &renders[1..] {
            assert!(Arc::ptr_eq(&renders[0].entries, &render.entries));
            assert!(Arc::ptr_eq(&renders[0].catalog_json, &render.catalog_json));
        }
        assert_eq!(
            renders[0].entries[0].safety,
            Some(CodeModeToolSafety {
                read_only: Some(true),
                destructive: None,
            })
        );
        assert!(renders[0].catalog_json.contains("\"read_only\":true"));
    }

    #[tokio::test]
    async fn render_flights_release_on_cancellation_and_do_not_block_other_keys() {
        let dir = tempfile::tempdir().expect("temporary config root");
        let manager = GatewayManager::new(
            dir.path().join("config.toml"),
            crate::gateway::runtime::GatewayRuntimeHandle::default(),
        );
        let first = manager.catalog_render_flight("first").await;
        let second = manager.catalog_render_flight("second").await;
        assert!(!Arc::ptr_eq(&first, &second));

        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
        let leader = tokio::spawn({
            let first = Arc::clone(&first);
            async move {
                let _guard = first.build.lock().await;
                let _ = locked_tx.send(());
                std::future::pending::<()>().await;
            }
        });
        locked_rx.await.expect("leader acquired flight");

        let second_guard =
            tokio::time::timeout(std::time::Duration::from_millis(100), second.build.lock())
                .await
                .expect("different fingerprint must not head-of-line block");
        drop(second_guard);
        leader.abort();
        drop(leader.await);
        let first_guard =
            tokio::time::timeout(std::time::Duration::from_millis(100), first.build.lock())
                .await
                .expect("cancelled leader must release its flight");
        drop(first_guard);
    }

    #[tokio::test]
    #[ignore = "4,000-tool cold-render performance budget"]
    async fn four_thousand_tool_cold_render_stays_within_budget() {
        let dir = tempfile::tempdir().expect("temporary config root");
        let manager = GatewayManager::new(
            dir.path().join("config.toml"),
            crate::gateway::runtime::GatewayRuntimeHandle::default(),
        );
        let tools = (0..4_000)
            .map(|index| {
                let mut tool =
                    safety_fixture(Some(rmcp::model::ToolAnnotations::new().read_only(true)));
                tool.tool.name = format!("tool_{index}").into();
                tool
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let render = catalog_from_tools(&manager, tools, false)
            .await
            .expect("4k cold render");
        let elapsed = started.elapsed();
        let bytes_per_tool = render.serialized_size / render.entries.len();

        eprintln!(
            "4k cold render: elapsed_ms={} serialized_bytes={} bytes_per_tool={bytes_per_tool}",
            elapsed.as_millis(),
            render.serialized_size
        );
        assert!(elapsed < std::time::Duration::from_secs(10));
        assert!(render.serialized_size < 4_000_000);
        assert!(bytes_per_tool < 1_000);
    }

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
            make(Some(
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            )),
        ];

        let read_only_scope = ToolScope::default().read_only();
        let filtered = filter_tools_for_access(tools, &read_only_scope);
        assert_eq!(filtered.len(), 1);
        assert!(super::super::code_mode_host::tool_is_explicitly_read_only(
            &filtered[0]
        ));
    }

    #[test]
    fn read_only_catalog_uses_standard_mcp_safety_annotations() {
        let named = Arc::<str>::from("fixture");
        let mut tool = rmcp::model::Tool::new(
            "query".to_string(),
            "Query data",
            Arc::new(serde_json::Map::new()),
        );
        tool.annotations = Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false),
        );
        let tool = UpstreamTool {
            tool,
            input_schema: None,
            output_schema: None,
            upstream_name: named,
            destructive: false,
        };

        let read_only_scope = ToolScope::default().read_only();
        assert_eq!(
            filter_tools_for_access(vec![tool], &read_only_scope).len(),
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
