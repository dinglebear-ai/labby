#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
//! Code Mode runtime readiness + tool resolution tests.
#![allow(clippy::panic)]

use labby_codemode::{CodeModeCaller, CodeModeHost, CodeModeSurface, ToolScope};
use labby_runtime::error::ToolError;
use serde_json::json;
use tracing_subscriber::layer::SubscriberExt;

use super::*;
use crate::gateway::code_mode::catalog_cache;

#[tokio::test]
async fn code_mode_host_resource_read_connects_a_cold_upstream() {
    let mut upstream = fixture_http_upstream("alpha");
    upstream.proxy_resources = true;
    let (manager, _) = code_mode_manager_with_pool(upstream).await;
    let error = CodeModeHost::read_resource(
        &manager,
        "lab://upstream/alpha/fixture://skill".to_string(),
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
    )
    .await
    .expect_err("unreachable fixture must fail connection, not report a missing resource");
    assert!(
        matches!(error, ToolError::Sdk { sdk_kind, .. } if sdk_kind == "upstream_connect_error")
    );
}

#[tokio::test]
async fn code_mode_host_resource_read_checks_scope_before_connecting() {
    let mut upstream = fixture_http_upstream("alpha");
    upstream.proxy_resources = true;
    let (manager, pool) = code_mode_manager_with_pool(upstream).await;
    let error = CodeModeHost::read_resource(
        &manager,
        "lab://upstream/alpha/fixture://skill".to_string(),
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::scoped_namespaces(vec!["beta".to_string()], Vec::new()),
    )
    .await
    .expect_err("out-of-scope resource must be refused");
    assert!(matches!(error, ToolError::Sdk { sdk_kind, .. } if sdk_kind == "forbidden"));
    assert_eq!(pool.connection_count_for_tests().await, 0);
}

#[tokio::test]
async fn search_tools_seeds_cold_lazy_runtime_before_searching() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        })
        .await;

    manager
        .ensure_search_runtime_ready(true, None, None)
        .await
        .expect_err("failed live discovery returns an actionable error");

    let pool = manager
        .current_pool()
        .await
        .expect("manager keeps a shared lazy pool installed");
    assert!(pool.cached_upstream_summary("alpha").await.is_some());
}

#[tokio::test]
async fn unauthenticated_code_mode_readiness_never_discovers_oauth_catalog() {
    let (manager, pool) =
        code_mode_manager_with_pool(fixture_oauth_upstream("private", "http://127.0.0.1:9/mcp"))
            .await;

    let tools = manager
        .code_mode_catalog_tools(true, None, None)
        .await
        .expect("OAuth upstreams are skipped without a subject");

    assert!(tools.is_empty());
    assert!(pool.healthy_tools().await.is_empty());
    assert_eq!(pool.connection_count_for_tests().await, 0);
}

#[tokio::test]
async fn scoped_code_mode_catalog_fails_when_allowed_upstream_is_unhealthy() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "ping"))
        .await;
    let allowed = std::collections::BTreeSet::from(["alpha".to_string()]);

    let err = manager
        .code_mode_catalog_tools_allowed(true, None, None, Some(&allowed))
        .await
        .expect_err("healthy disallowed upstreams must not mask scoped connect failures");

    match err {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "upstream_connect_error");
            assert!(message.contains("alpha"));
            assert!(!message.contains("beta"));
        }
        other => panic!("expected upstream_connect_error sdk error, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_code_mode_upstream_tool_hides_priority_zero_upstreams() {
    let mut upstream = fixture_http_upstream("suppressed");
    upstream.priority = 0.0;
    let (manager, pool) = code_mode_manager_with_pool(upstream).await;
    pool.insert_entry_for_tests(
        "suppressed",
        healthy_entry_with_tool("suppressed", "secret-tool"),
    )
    .await;

    let err = manager
        .resolve_code_mode_upstream_tool("suppressed", "secret-tool", None, None)
        .await
        .expect_err("priority=0 upstream tools must not be invokable by code mode id");

    match err {
        ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "unknown_tool"),
        other => panic!("expected unknown_tool sdk error, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_code_mode_upstream_tool_resolves_requested_upstream() {
    // resolve_code_mode_upstream_tool requires the codemode surface, gated
    // solely by code_mode.enabled, to be active.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        })
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let tool = manager
        .resolve_code_mode_upstream_tool("alpha", "ping", None, None)
        .await
        .expect("code mode should resolve requested upstream");

    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn admin_tool_browser_search_and_describe_use_the_live_manager_catalog() {
    let (manager, pool) = code_mode_manager_with_pool(fixture_http_upstream("alpha")).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let searched = manager
        .search_admin_tools(Some("admin".to_string()), "alpha ping", 50)
        .await
        .expect("search live manager catalog");
    assert_eq!(searched.results.len(), 1);
    assert_eq!(searched.results[0].id, "alpha::ping");

    let described = manager
        .describe_admin_tool(Some("admin".to_string()), "alpha::ping")
        .await
        .expect("describe live manager catalog");
    assert_eq!(described.id, "alpha::ping");
    assert!(described.typescript.is_some());
    assert_eq!(pool.connection_count_for_tests().await, 0);
}

// Regression: the Cloudflare-parity surface exposes search+execute under
// `code_mode.enabled` (RootSynthetic). `execute`'s callTool must resolve
// upstream tools when `code_mode.enabled` is the active flag — the single
// toggle that exposes the surface. A prior merge gated resolution on a
// separate flag, so execute could never call a tool when the surface was
// exposed via code_mode (the only way it is exposed). The test suite did
// not cover this path, so it passed while the live server rejected callTool.
#[tokio::test]
async fn resolve_upstream_tool_works_with_code_mode_enabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![fixture_http_upstream("alpha")],
            ..GatewayConfig::default()
        })
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let tool = manager
        .resolve_code_mode_upstream_tool("alpha", "ping", None, None)
        .await
        .expect("execute callTool must resolve when code_mode surface is enabled");

    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_resolves_cached_tool_without_code_mode() {
    let upstream = fixture_http_upstream("alpha");
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new());
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(path, runtime);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: false,
                ..CodeModeConfig::default()
            },
            upstream: vec![upstream],
            ..GatewayConfig::default()
        })
        .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let (upstream, tool) = manager
        .resolve_raw_upstream_tool("ping", None, None)
        .await
        .expect("raw proxy resolution should not require code_mode");

    assert_eq!(upstream, "alpha");
    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_honors_qualified_upstream_name() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "ping"))
        .await;

    let (upstream, tool) = manager
        .resolve_raw_upstream_tool("beta::ping", None, None)
        .await
        .expect("qualified raw tool should resolve requested upstream");

    assert_eq!(upstream, "beta");
    assert_eq!(tool.tool.name.as_ref(), "ping");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_scoped_hides_priority_zero_upstreams() {
    let mut upstream = fixture_http_upstream("suppressed");
    upstream.priority = 0.0;
    let (manager, pool) = code_mode_manager_with_upstreams(vec![upstream]).await;
    pool.insert_entry_for_tests("suppressed", healthy_entry_with_tool("suppressed", "ping"))
        .await;
    let allowed = std::collections::BTreeSet::from(["suppressed".to_string()]);

    let err = manager
        .resolve_raw_upstream_tool_scoped("suppressed::ping", Some(&allowed), None, None)
        .await
        .expect_err("priority=0 upstream tools must not be invokable through scoped raw proxy");

    match err {
        ToolError::Sdk { sdk_kind, .. } => assert_eq!(sdk_kind, "unknown_tool"),
        other => panic!("expected unknown_tool sdk error, got {other:?}"),
    }
}

#[tokio::test]
async fn advertised_subject_scoped_oauth_tool_normalizes_metadata_and_executes() {
    let upstream = fixture_oauth_upstream("private", "http://unused.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "alice",
        vec![rmcp::model::Tool::new(
            "private_ping".to_string(),
            "### Private ping \u{2066}documentation\u{2069}",
            Arc::new(serde_json::Map::from_iter([
                ("type".to_string(), json!("object")),
                ("required".to_string(), json!(["query"])),
                (
                    "properties".to_string(),
                    json!({
                        "query": {
                            "type": "string",
                            "description": "### Query \u{2066}documentation\u{2069}",
                            "enum": ["\u{2066}exact\u{2069}"]
                        }
                    }),
                ),
            ])),
        )],
    )
    .await;
    let caller = CodeModeCaller::Scoped {
        capabilities: labby_codemode::CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some("alice".to_string()),
    };

    let advertised = CodeModeHost::list_tools(
        &manager,
        &caller,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
        false,
        false,
    )
    .await
    .expect("subject catalog is advertised");
    assert!(
        advertised
            .entries
            .iter()
            .any(|entry| entry.id == "private::private_ping")
    );

    let resolved = manager
        .resolve_code_mode_upstream_tool("private", "private_ping", None, Some("alice"))
        .await
        .expect("advertised Code Mode tool resolves");
    assert_eq!(resolved.tool.name.as_ref(), "private_ping");
    let description = resolved.tool.description.as_deref().expect("description");
    assert!(!description.contains("###"));
    assert!(!description.contains('\u{2066}'));
    assert!(!description.contains('\u{2069}'));
    let query_schema = &resolved.tool.input_schema["properties"]["query"];
    let query_description = query_schema["description"].as_str().unwrap();
    assert!(!query_description.contains("###"));
    assert!(!query_description.contains('\u{2066}'));
    assert!(!query_description.contains('\u{2069}'));
    // Documentation is normalized, but schema values remain exact. Both the
    // discovery contract and the final peer check must use this representation.
    assert_eq!(query_schema["enum"], json!(["\u{2066}exact\u{2069}"]));
    for selector in ["private_ping", "private::private_ping"] {
        let (owner, resolved) = manager
            .resolve_raw_upstream_tool(selector, None, Some("alice"))
            .await
            .expect("advertised raw tool resolves");
        assert_eq!(owner, "private");
        assert_eq!(resolved.tool.name.as_ref(), "private_ping");
    }

    CodeModeHost::call_tool(
        &manager,
        "private::private_ping",
        json!({"query": "\u{2066}exact\u{2069}"}),
        &caller,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect("advertised subject-scoped tool executes through its subject peer");
}

#[tokio::test]
async fn annotated_read_only_fixture_is_searchable_describable_and_callable() {
    let upstream = fixture_oauth_upstream("fixture", "http://unused.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    let annotated = |name: &str, annotations: rmcp::model::ToolAnnotations| {
        let mut tool = rmcp::model::Tool::new(
            name.to_string(),
            format!("{name} fixture"),
            Arc::new(serde_json::Map::new()),
        );
        tool.annotations = Some(annotations);
        tool
    };
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![
            annotated(
                "provider_status",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            ),
            rmcp::model::Tool::new(
                "unannotated".to_string(),
                "unannotated fixture",
                Arc::new(serde_json::Map::new()),
            ),
            annotated(
                "contradictory",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(true),
            ),
        ],
    )
    .await;
    let caller = CodeModeCaller::Scoped {
        capabilities: labby_codemode::CodeModeCallerCapabilities {
            can_read: true,
            can_execute: false,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some("reader".to_string()),
    };
    let scope = ToolScope::default().read_only();

    let render = CodeModeHost::list_tools(
        &manager,
        &caller,
        CodeModeSurface::Mcp,
        &scope,
        false,
        false,
    )
    .await
    .expect("read-only fixture catalog");
    assert_eq!(
        render
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture::provider_status"]
    );

    let searched = labby_codemode::search_visible_tools(&render.entries, &scope, "provider", 10)
        .expect("search annotated fixture");
    assert_eq!(searched.results[0].id, "fixture::provider_status");
    let described =
        labby_codemode::describe_visible_tool(&render.entries, &scope, "fixture::provider_status")
            .expect("describe annotated fixture");
    assert_eq!(described.id, "fixture::provider_status");

    CodeModeHost::call_tool(
        &manager,
        "fixture::provider_status",
        json!({}),
        &caller,
        CodeModeSurface::Mcp,
        &scope,
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect("call annotated read-only fixture");
    let resource = CodeModeHost::read_resource(
        &manager,
        "lab://upstream/fixture/fixture://skill".to_string(),
        &caller,
        CodeModeSurface::Mcp,
        &scope,
    )
    .await
    .expect("read proxied fixture resource");
    assert_eq!(resource["contents"], json!([]));

    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "reader",
        vec![
            annotated(
                "provider_status",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            ),
            annotated(
                "operation_status",
                rmcp::model::ToolAnnotations::new()
                    .read_only(true)
                    .destructive(false),
            ),
        ],
    )
    .await;
    let refreshed = CodeModeHost::list_tools(
        &manager,
        &caller,
        CodeModeSurface::Mcp,
        &scope,
        false,
        false,
    )
    .await
    .expect("refreshed read-only fixture catalog");
    assert_eq!(
        refreshed
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture::operation_status", "fixture::provider_status"]
    );
}

#[tokio::test]
async fn code_mode_enabled_reads_code_mode_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let manager = GatewayManager::new(path, GatewayRuntimeHandle::default());

    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            ..GatewayConfig::default()
        })
        .await;

    // PRESENCE: code_mode_enabled() reflects code_mode.enabled = true
    assert!(
        manager.code_mode_enabled().await,
        "code_mode_enabled() must return true when code_mode.enabled = true"
    );
}

#[tokio::test]
async fn code_mode_host_list_tools_honors_scoped_namespaces() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "pong"))
        .await;

    let render = CodeModeHost::list_tools(
        &manager,
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::scoped_namespaces(vec!["alpha".to_string()], Vec::new()),
        false,
        false,
    )
    .await
    .expect("scoped Code Mode host catalog");

    let ids = render
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["alpha::ping"]);
}

#[tokio::test]
async fn code_mode_catalog_preserves_upstream_output_schema_for_describe_types() {
    let output_schema = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "message": { "type": "string" }
        },
        "required": ["ok"],
        "additionalProperties": false
    });
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    pool.insert_entry_for_tests(
        "alpha",
        healthy_entry_with_typed_tool("alpha", "typed", output_schema.clone()),
    )
    .await;

    let render = CodeModeHost::list_tools(
        &manager,
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Mcp,
        &ToolScope::default(),
        false,
        false,
    )
    .await
    .expect("Code Mode host catalog");
    let entry = render
        .entries
        .iter()
        .find(|entry| entry.id == "alpha::typed")
        .expect("typed tool entry");

    assert_eq!(entry.output_schema, Some(output_schema));
    assert!(
        entry.dts.contains("type AlphaTypedOutput = {"),
        "dts must define a concrete output type, got: {}",
        entry.dts
    );
    assert!(
        entry.dts.contains("ok: boolean;"),
        "dts must render output properties, got: {}",
        entry.dts
    );
    assert!(
        !entry.signature.contains("Promise<unknown>"),
        "signature must not degrade typed output to unknown: {}",
        entry.signature
    );
}

#[tokio::test]
async fn code_mode_host_list_tools_for_mcp_does_not_block_on_cold_unhealthy_upstreams() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind hanging upstream fixture");
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _socket = socket;
                tokio::time::sleep(Duration::from_secs(5)).await;
            });
        }
    });

    let mut hanging = fixture_http_upstream("alpha");
    hanging.url = Some(format!("http://{addr}/mcp"));
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![hanging, fixture_http_upstream("beta")]).await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "ping"))
        .await;

    let render = tokio::time::timeout(
        Duration::from_millis(100),
        CodeModeHost::list_tools(
            &manager,
            &CodeModeCaller::Scoped {
                capabilities: labby_codemode::CodeModeCallerCapabilities {
                    can_read: true,
                    can_execute: true,
                    can_use_snippets: false,
                    is_admin: false,
                },
                sub: Some("user-1".to_string()),
            },
            CodeModeSurface::Mcp,
            &ToolScope::default(),
            false,
            false,
        ),
    )
    .await
    .expect("MCP proxy generation must not wait for cold upstream refresh")
    .expect("MCP Code Mode proxy generation should use current healthy tools");

    let ids = render
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["beta::ping"]);
}

#[tokio::test]
async fn code_mode_host_blocks_destructive_calls_for_read_only_callers() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let mut entry = healthy_entry_with_tool("alpha", "delete");
    entry
        .tools
        .get_mut("delete")
        .expect("fixture tool")
        .destructive = true;
    pool.insert_entry_for_tests("alpha", entry).await;

    let err = CodeModeHost::call_tool(
        &manager,
        "alpha::delete",
        json!({}),
        &CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
            sub: Some("user-1".to_string()),
        },
        CodeModeSurface::Mcp,
        &ToolScope::new(Vec::new(), Vec::new()),
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect_err("read-only caller must not execute destructive tool");

    assert_eq!(err.kind(), "forbidden");
    assert!(err.user_message().contains("alpha::delete"));
}

/// The read-only Code Mode gate rests entirely on the upstream's own
/// `readOnlyHint`; the operator-held `trusted_read_only_tools` allowlist that
/// used to be a second required conjunct is retired. These two tests pin both
/// directions of what is now the only gate, so a future change to it cannot
/// widen read-only execution unnoticed.
#[tokio::test]
async fn code_mode_host_blocks_unannotated_tools_for_read_only_callers() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    // Not destructive, and carrying no annotations at all — the ordinary shape
    // of an upstream tool that never declared its safety.
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;

    let err = CodeModeHost::call_tool(
        &manager,
        "alpha::ping",
        json!({}),
        &CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
            sub: Some("user-1".to_string()),
        },
        CodeModeSurface::Mcp,
        &ToolScope::new(Vec::new(), Vec::new()),
        labby_codemode::ExecCtx::none(),
    )
    .await
    .expect_err("a read-only caller must not execute an unannotated tool");

    // Two layers deny this, and the catalog is the first: an unannotated tool is
    // not admitted to a read-only caller's catalog at all, so resolution fails
    // before the execution gate is consulted. `forbidden` would mean the catalog
    // admitted it and only the gate caught it; either is a denial, and asserting
    // both keeps this test honest if the layering ever shifts.
    assert!(
        matches!(err.kind(), "not_found" | "forbidden"),
        "a read-only caller must be denied an unannotated tool, got kind {}: {}",
        err.kind(),
        err.user_message()
    );
}

#[tokio::test]
async fn code_mode_host_admits_annotated_read_only_tools_without_an_operator_allowlist() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let mut entry = healthy_entry_with_tool("alpha", "ping");
    entry
        .tools
        .get_mut("ping")
        .expect("fixture tool")
        .tool
        .annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
    pool.insert_entry_for_tests("alpha", entry).await;

    // The fixture upstream is not reachable, so the call still fails — but it
    // must fail past the policy gate, not at it. Nothing here configures a
    // `trusted_read_only_tools` allowlist, which is the point: the annotation
    // alone is now sufficient.
    let outcome = CodeModeHost::call_tool(
        &manager,
        "alpha::ping",
        json!({}),
        &CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
            sub: Some("user-1".to_string()),
        },
        CodeModeSurface::Mcp,
        &ToolScope::new(Vec::new(), Vec::new()),
        labby_codemode::ExecCtx::none(),
    )
    .await;

    if let Err(err) = outcome {
        assert!(
            !err.user_message().contains("not explicitly annotated"),
            "an annotated read-only tool must clear the read-only gate, got: {}",
            err.user_message()
        );
    }
}

#[tokio::test]
async fn mcp_tool_error_result_does_not_poison_upstream_connection_health() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("unifi")]).await;
    pool.insert_tool_error_server_for_tests("unifi", "controller rejected the request")
        .await;

    let err = manager
        .execute_upstream_tool("unifi", "unifi", json!({}))
        .await
        .expect_err("is_error=true must remain a Code Mode tool error");

    assert_eq!(err.kind(), "tool_error");
    assert_eq!(
        pool.upstream_tool_last_error("unifi").await,
        None,
        "a successful MCP response must not count as a connection-health failure"
    );
}

#[tokio::test]
async fn cortex_exact_schema_rejects_bad_fields_before_upstream_dispatch() {
    // Minimal composed-schema fixture. This test proves the *wiring* facts:
    // a cached upstream inputSchema is enforced before dispatch (invalid_param)
    // and a pre-dispatch failure never touches upstream health. Full keyword
    // regression coverage (the original ~65-line Cortex schema) lives in
    // `labby-codemode`'s `tests_ids_schema.rs`.
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("cortex")]).await;
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action"],
        "properties": {
            "action": { "type": "string" },
            "project": { "type": "string" }
        },
        "oneOf": [
            {
                "properties": { "action": { "const": "project_context" } },
                "required": ["action", "project"]
            }
        ]
    });
    let upstream_name: Arc<str> = Arc::from("cortex");
    let tool = rmcp::model::Tool::new(
        "cortex".to_string(),
        "Cortex action dispatcher",
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "cortex",
        fixture_upstream_entry(
            "cortex",
            HashMap::from([(
                "cortex".to_string(),
                UpstreamTool {
                    tool,
                    input_schema: Some(schema),
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;

    for params in [
        // additionalProperties: false rejects the unknown field.
        json!({"action": "project_context", "project": "/repo", "since": "x"}),
        // oneOf requires `project` alongside `project_context`.
        json!({"action": "project_context"}),
    ] {
        let error = CodeModeHost::call_tool(
            &manager,
            "cortex::cortex",
            params,
            &CodeModeCaller::Scoped {
                capabilities: labby_codemode::CodeModeCallerCapabilities::default(),
                sub: Some("user-1".to_string()),
            },
            CodeModeSurface::Mcp,
            &ToolScope::default(),
            labby_codemode::ExecCtx::none(),
        )
        .await
        .expect_err("schema mismatch must fail before the upstream call");
        assert_eq!(error.kind(), "invalid_param");
    }

    assert_eq!(
        pool.upstream_tool_last_error("cortex").await,
        None,
        "pre-dispatch schema failures must not affect upstream health"
    );
}

#[tokio::test]
async fn mcp_invalid_params_map_to_code_mode_invalid_param_without_health_failure() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("cortex")]).await;
    let message =
        "invalid project_context arguments: unknown field `since`, expected project, tool, limit";
    pool.insert_mcp_error_server_for_tests(
        "cortex",
        rmcp::model::ErrorData::invalid_params(message, None),
    )
    .await;

    let err = manager
        .execute_upstream_tool("cortex", "cortex", json!({}))
        .await
        .expect_err("invalid params must remain a Code Mode caller error");

    match err {
        ToolError::Sdk {
            sdk_kind,
            message: actual,
        } => {
            assert_eq!(sdk_kind, "invalid_param");
            assert_eq!(actual, message);
        }
        other => panic!("expected invalid_param sdk error, got {other:?}"),
    }
    assert_eq!(
        pool.upstream_tool_last_error("cortex").await,
        None,
        "valid MCP errors must not poison upstream health"
    );
    assert!(
        pool.upstream_tool_health("cortex")
            .await
            .expect("health entry")
            .is_routable(),
        "upstream must remain routable after invalid params"
    );
}

#[tokio::test]
async fn palette_catalog_discovers_configured_upstream_tools() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let tools = Arc::new(tokio::sync::RwLock::new(vec!["ping".to_string()]));
    assert!(
        pool.healthy_tools_for_upstream("alpha").await.is_empty(),
        "fixture starts as a lazy-seeded upstream without cached tools"
    );
    pool.insert_live_tool_server_for_tests("alpha", tools).await;

    let catalog = manager
        .palette_catalog(&crate::gateway::palette::PaletteCaller::admin(
            Some("admin"),
            Some("req-1"),
        ))
        .await
        .expect("catalog builds");

    assert_eq!(catalog.entries.len(), 1);
    let crate::gateway::palette::LauncherEntryView::McpTool(entry) = &catalog.entries[0] else {
        panic!("expected mcp tool entry");
    };
    assert_eq!(entry.id, "mcp:alpha::ping");
    assert_eq!(entry.source, "alpha");
    assert_eq!(entry.tool, "ping");
    assert!(
        entry.input_schema.is_none(),
        "catalog rows must not retain exact schemas"
    );
    assert!(!catalog.truncated);
}

#[tokio::test]
async fn palette_catalog_caps_cross_upstream_projection_but_exact_lookup_remains_available() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    for upstream in ["alpha", "beta"] {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tools = (0..600)
            .map(|index| {
                let name = format!("tool_{index:04}");
                let tool = rmcp::model::Tool::new(
                    name.clone(),
                    "bounded palette fixture",
                    Arc::new(serde_json::Map::new()),
                );
                (
                    name,
                    UpstreamTool {
                        tool,
                        input_schema: Some(json!({"type": "object"})),
                        output_schema: None,
                        upstream_name: Arc::clone(&upstream_name),
                        destructive: false,
                    },
                )
            })
            .collect();
        pool.insert_entry_for_tests(upstream, fixture_upstream_entry(upstream, tools))
            .await;
    }

    let caller = crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1"));
    let catalog = manager
        .palette_catalog_snapshot(&caller)
        .await
        .expect("bounded catalog");
    assert_eq!(catalog.entries.len(), 1_000);
    assert!(catalog.truncated);
    assert!(catalog.entries.iter().all(|entry| match entry {
        crate::gateway::palette::LauncherEntryView::McpTool(entry) => entry.input_schema.is_none(),
        crate::gateway::palette::LauncherEntryView::LabbyAction(_) => false,
    }));

    let searched = manager
        .palette_catalog_snapshot_matching(
            &caller,
            &crate::gateway::palette::PaletteSearchQuery::new("mcp:beta").expect("valid query"),
        )
        .await
        .expect("query is applied before the cross-upstream cap");
    assert_eq!(searched.entries.len(), 600);
    assert!(!searched.truncated);
    assert!(searched.entries.iter().any(|entry| match entry {
        crate::gateway::palette::LauncherEntryView::McpTool(entry) => {
            entry.id == "mcp:beta::tool_0599"
        }
        crate::gateway::palette::LauncherEntryView::LabbyAction(_) => false,
    }));

    let exact = manager
        .palette_catalog_snapshot_for_tool(&caller, "mcp:beta::tool_0599")
        .await
        .expect("exact lookup outside bounded catalog");
    assert_eq!(exact.entries.len(), 1);
    assert!(!exact.truncated);
}

#[tokio::test]
async fn palette_search_filters_before_single_upstream_catalog_cap() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let mut tools = (0..1_100)
        .map(|index| {
            let name = format!("tool_{index:04}");
            let tool = rmcp::model::Tool::new(
                name.clone(),
                "ordinary fixture",
                Arc::new(serde_json::Map::new()),
            );
            (
                name,
                UpstreamTool {
                    tool,
                    input_schema: None,
                    output_schema: None,
                    upstream_name: Arc::clone(&upstream_name),
                    destructive: false,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let name = "zzzz_unique_match".to_string();
    tools.insert(
        name.clone(),
        UpstreamTool {
            tool: rmcp::model::Tool::new(
                name,
                "ordinary fixture",
                Arc::new(serde_json::Map::new()),
            ),
            input_schema: None,
            output_schema: None,
            upstream_name,
            destructive: false,
        },
    );
    pool.insert_entry_for_tests("alpha", fixture_upstream_entry("alpha", tools))
        .await;

    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("unique_match").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert_eq!(searched.entries.len(), 1);
    assert!(matches!(
        &searched.entries[0],
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:alpha::zzzz_unique_match"
    ));
}

#[tokio::test]
async fn palette_search_global_cap_keeps_later_exact_match_over_weak_matches() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let weak = (0..1_000)
        .map(|index| {
            let name = format!("weak_{index:04}");
            (
                name.clone(),
                UpstreamTool {
                    tool: rmcp::model::Tool::new(
                        name,
                        "contains needle somewhere",
                        Arc::new(serde_json::Map::new()),
                    ),
                    input_schema: None,
                    output_schema: None,
                    upstream_name: Arc::clone(&upstream_name),
                    destructive: false,
                },
            )
        })
        .collect();
    pool.insert_entry_for_tests("alpha", fixture_upstream_entry("alpha", weak))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "needle"))
        .await;

    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("needle").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert!(searched.entries.iter().any(|entry| matches!(
        entry,
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:beta::needle"
    )));
}

#[tokio::test]
async fn palette_search_reports_truncation_when_global_inspection_budget_is_exhausted() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    for upstream in ["alpha", "beta"] {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tools = (0..6_000)
            .map(|index| {
                let name = format!("tool_{index:04}");
                (
                    name.clone(),
                    UpstreamTool {
                        tool: rmcp::model::Tool::new(
                            name,
                            "ordinary fixture",
                            Arc::new(serde_json::Map::new()),
                        ),
                        input_schema: None,
                        output_schema: None,
                        upstream_name: Arc::clone(&upstream_name),
                        destructive: false,
                    },
                )
            })
            .collect();
        pool.insert_entry_for_tests(upstream, fixture_upstream_entry(upstream, tools))
            .await;
    }
    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("absent").expect("valid query"),
        )
        .await
        .expect("bounded search");
    assert!(searched.entries.is_empty());
    assert!(searched.truncated);
}

#[tokio::test]
async fn palette_search_matches_description_subsequences_before_catalog_cap() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha")]).await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let tool = rmcp::model::Tool::new(
        "otherwise_hidden",
        "Deploy Production Safely",
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([(
                "otherwise_hidden".to_string(),
                UpstreamTool {
                    tool,
                    input_schema: None,
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;

    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("dps").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert_eq!(searched.entries.len(), 1);
}

#[tokio::test]
async fn palette_search_scores_only_the_visible_sanitized_description() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    let upstream_name: Arc<str> = Arc::from("alpha");
    let hidden = rmcp::model::Tool::new(
        "hidden",
        format!("{}needle", "x".repeat(512)),
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([(
                "hidden".to_string(),
                UpstreamTool {
                    tool: hidden,
                    input_schema: None,
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "needle"))
        .await;
    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("needle").expect("valid query"),
        )
        .await
        .expect("search catalog");
    assert_eq!(searched.entries.len(), 1);
    assert!(matches!(
        &searched.entries[0],
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:beta::needle"
    ));
}

#[tokio::test]
async fn palette_search_many_delayed_oauth_upstreams_has_one_bounded_deadline() {
    use labby_auth::upstream::cache::OauthClientCache;
    use labby_auth::upstream::manager::UpstreamOauthManager;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed OAuth fixture");
    let addr = listener.local_addr().expect("listener address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_for_server = Arc::clone(&accepted);
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            accepted_for_server.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let _socket = socket;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });
    let upstreams = (0..32)
        .map(|index| {
            fixture_oauth_upstream(&format!("oauth_{index:02}"), &format!("http://{addr}/mcp"))
        })
        .collect::<Vec<_>>();
    let dir = tempfile::tempdir().expect("tempdir");
    let (sqlite, key, redirect_uri) = fixture_oauth_resources(&dir).await;
    let managers = Arc::new(dashmap::DashMap::new());
    for upstream in &upstreams {
        managers.insert(
            upstream.name.clone(),
            UpstreamOauthManager::new(
                sqlite.clone(),
                key.clone(),
                upstream.clone(),
                redirect_uri.clone(),
            ),
        );
    }
    let cache = OauthClientCache::new(Arc::clone(&managers));
    let runtime = GatewayRuntimeHandle::default();
    let pool = Arc::new(UpstreamPool::new().with_oauth_client_cache(cache.clone()));
    runtime.swap(Some(Arc::clone(&pool))).await;
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime)
        .with_upstream_oauth_managers(managers)
        .with_oauth_client_cache(cache)
        .with_oauth_resources(sqlite, key, redirect_uri);
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: upstreams.clone(),
            ..GatewayConfig::default()
        })
        .await;
    pool.install_test_subject_tools_for_upstream(
        &upstreams[0],
        "admin",
        vec![rmcp::model::Tool::new(
            "needle",
            "fast OAuth match",
            Arc::new(serde_json::Map::new()),
        )],
    )
    .await;
    let started = std::time::Instant::now();
    let searched = manager
        .palette_catalog_snapshot_matching(
            &crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1")),
            &crate::gateway::palette::PaletteSearchQuery::new("needle").expect("valid query"),
        )
        .await
        .expect("deadline degrades to a partial catalog");
    let elapsed = started.elapsed();
    server.abort();

    assert!(searched.entries.iter().any(|entry| matches!(
        entry,
        crate::gateway::palette::LauncherEntryView::McpTool(entry)
            if entry.id == "mcp:oauth_00::needle"
    )));
    assert!(
        elapsed < Duration::from_secs(3),
        "many delayed upstreams must share one deadline: {elapsed:?}"
    );
    assert!(
        accepted.load(Ordering::SeqCst) < 32,
        "catalog fanout must bound simultaneous delayed connection work"
    );
}

#[tokio::test]
async fn palette_catalog_scoped_caller_only_sees_allowed_upstreams() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "search"))
        .await;

    let caller = crate::gateway::palette::PaletteCaller::scoped_read_only(
        Some("user"),
        Some("request-1"),
        vec!["beta".to_string()],
    );
    let catalog = manager
        .palette_catalog(&caller)
        .await
        .expect("scoped palette catalog should build for allowed upstream");

    let ids = catalog
        .entries
        .iter()
        .map(|entry| match entry {
            crate::gateway::palette::LauncherEntryView::McpTool(entry) => entry.id.as_str(),
            crate::gateway::palette::LauncherEntryView::LabbyAction(entry) => entry.id.as_str(),
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["mcp:beta::search"]);
}

#[tokio::test]
async fn palette_catalog_scope_and_fingerprint_follow_visible_schema() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "pong"))
        .await;

    let admin = crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1"));
    let scoped = crate::gateway::palette::PaletteCaller::scoped_read_only(
        Some("user"),
        Some("req-2"),
        vec!["alpha".to_string()],
    );

    let admin_catalog = manager
        .palette_catalog(&admin)
        .await
        .expect("admin catalog");
    let scoped_catalog = manager
        .palette_catalog(&scoped)
        .await
        .expect("scoped catalog");
    assert_eq!(admin_catalog.entries.len(), 2);
    assert_eq!(scoped_catalog.entries.len(), 1);
    assert_ne!(admin_catalog.fingerprint, scoped_catalog.fingerprint);

    let upstream_name: Arc<str> = Arc::from("alpha");
    let tool = rmcp::model::Tool::new(
        "ping".to_string(),
        "changed schema",
        Arc::new(serde_json::Map::new()),
    );
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([(
                "ping".to_string(),
                UpstreamTool {
                    tool,
                    input_schema: Some(json!({"type": "object", "required": ["q"]})),
                    output_schema: None,
                    upstream_name,
                    destructive: false,
                },
            )]),
        ),
    )
    .await;

    let changed = manager
        .palette_catalog(&scoped)
        .await
        .expect("changed catalog");
    assert_ne!(scoped_catalog.fingerprint, changed.fingerprint);
}

fn palette_contract_hash(
    catalog: &crate::gateway::palette::LauncherCatalogView,
    id: &str,
) -> String {
    catalog
        .entries
        .iter()
        .find_map(|entry| match entry {
            crate::gateway::palette::LauncherEntryView::McpTool(entry) if entry.id == id => {
                Some(entry.contract_hash.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing palette entry {id}"))
}

#[tokio::test]
async fn palette_execute_binds_oauth_catalog_and_call_to_the_same_subject() {
    let upstream = fixture_oauth_upstream("private", "http://unused.invalid/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    let subject_tool = |property: &str| {
        let properties =
            serde_json::Map::from_iter([(property.to_string(), json!({"type": "string"}))]);
        let mut tool = rmcp::model::Tool::new(
            "private_ping".to_string(),
            "private ping",
            Arc::new(serde_json::Map::from_iter([(
                "properties".to_string(),
                serde_json::Value::Object(properties),
            )])),
        );
        tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
        tool
    };
    pool.install_test_subject_tools_for_upstream(&upstream, "alice", vec![subject_tool("alice")])
        .await;
    pool.install_test_subject_tools_for_upstream(&upstream, "bob", vec![subject_tool("bob")])
        .await;

    let alice = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-123"));
    let bob = crate::gateway::palette::PaletteCaller::admin(Some("bob"), Some("req-bob"));
    let alice_catalog = manager
        .palette_catalog(&alice)
        .await
        .expect("alice catalog");
    let bob_catalog = manager.palette_catalog(&bob).await.expect("bob catalog");
    let id = "mcp:private::private_ping";
    let alice_hash = palette_contract_hash(&alice_catalog, id);
    let bob_hash = palette_contract_hash(&bob_catalog, id);
    assert_ne!(
        alice_hash, bob_hash,
        "subject-specific schemas must not cross"
    );

    let response = manager
        .palette_execute(
            &alice,
            crate::gateway::palette::PaletteExecuteRequest {
                id: id.to_string(),
                params: json!({"token": "TOKEN-CANARY"}),
                confirm_destructive: false,
                expected_contract_hash: alice_hash.clone(),
            },
        )
        .await
        .expect("Alice executes against Alice's subject connection");

    assert_eq!(response.receipt.request_id, "req-123");
    assert_eq!(
        serde_json::to_value(&response.receipt).unwrap()["executionMode"],
        "exact"
    );
    assert_eq!(response.receipt.tool_id, id);
    assert_eq!(response.receipt.contract_hash, alice_hash);
    let receipt = serde_json::to_string(&response.receipt).expect("receipt serializes");
    for forbidden in [
        "alice",
        "TOKEN-CANARY",
        "oauth",
        "params",
        "result",
        "llmInvocations",
        "auditId",
    ] {
        assert!(
            !receipt.contains(forbidden),
            "receipt leaked {forbidden}: {receipt}"
        );
    }
}

#[tokio::test]
async fn palette_execute_does_not_reuse_an_invalidated_oauth_subject_connection() {
    let upstream = fixture_oauth_upstream("private", "http://127.0.0.1:9/mcp");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    let mut tool = rmcp::model::Tool::new(
        "private_ping".to_string(),
        "private ping",
        Arc::new(serde_json::Map::new()),
    );
    tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
    pool.install_test_subject_tools_for_upstream(&upstream, "alice", vec![tool])
        .await;
    let alice = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-revoked"));
    let catalog = manager
        .palette_catalog(&alice)
        .await
        .expect("catalog before revocation");
    let contract_hash = palette_contract_hash(&catalog, "mcp:private::private_ping");

    pool.invalidate_oauth_subject_sessions("private", "alice", "credential revoked")
        .await;
    let error = manager
        .palette_execute(
            &alice,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:private::private_ping".to_string(),
                params: json!({}),
                expected_contract_hash: contract_hash,
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("revoked subject connection must not be reused");
    assert!(
        matches!(
            error.kind(),
            "upstream_connect_error" | "network_error" | "auth_failed"
        ),
        "unexpected revocation error: {error:?}"
    );
}

#[tokio::test]
async fn palette_execute_rechecks_the_published_config_after_catalog_preview() {
    let mut upstream = fixture_http_upstream("alpha");
    let (manager, pool) = code_mode_manager_with_pool(upstream.clone()).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "ping"))
        .await;
    let caller = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-reload"));
    let catalog = manager
        .palette_catalog(&caller)
        .await
        .expect("catalog before reload");
    let contract_hash = palette_contract_hash(&catalog, "mcp:alpha::ping");

    upstream.priority = 0.0;
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                ..CodeModeConfig::default()
            },
            upstream: vec![upstream],
            ..GatewayConfig::default()
        })
        .await;
    let error = manager
        .palette_execute(
            &caller,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::ping".to_string(),
                params: json!({}),
                expected_contract_hash: contract_hash,
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("disabled published revision must win over the preview");
    assert_eq!(error.kind(), "not_found");
}

#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "Serialize tracing capture across awaits on this current-thread test runtime"
)]
async fn palette_execute_fails_closed_when_the_previewed_contract_changes() {
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("github")]).await;
    pool.insert_entry_for_tests("github", healthy_entry_with_tool("github", "search_issues"))
        .await;
    let alice = crate::gateway::palette::PaletteCaller::admin(Some("alice"), Some("req-drift"));
    let catalog = manager
        .palette_catalog(&alice)
        .await
        .expect("preview catalog");
    let id = "mcp:github::search_issues";
    let old_hash = palette_contract_hash(&catalog, id);

    let mut changed = pool.healthy_tools_for_upstream("github").await;
    changed[0].input_schema = Some(json!({
        "type": "object",
        "properties": {"query": {"type": "string"}}
    }));
    pool.insert_entry_for_tests(
        "github",
        fixture_upstream_entry(
            "github",
            HashMap::from([("search_issues".to_string(), changed.remove(0))]),
        ),
    )
    .await;

    let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let buffer = crate::test_support::SharedBuf::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .without_time(),
    );
    let tracing_guard = tracing::subscriber::set_default(subscriber);
    let error = manager
        .palette_execute(
            &alice,
            crate::gateway::palette::PaletteExecuteRequest {
                id: id.to_string(),
                params: json!({"query": "bug"}),
                expected_contract_hash: old_hash,
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("changed contract must fail closed");
    assert_eq!(error.kind(), "contract_changed");
    drop(tracing_guard);
    let logs = crate::test_support::captured_logs(&buffer);
    assert!(
        !logs.contains("upstream.request"),
        "contract drift dispatched an upstream request: {logs}"
    );
}

#[tokio::test]
async fn palette_execute_rejects_cross_upstream_scope_and_destructive_reclassification() {
    let (manager, pool) = code_mode_manager_with_upstreams(vec![
        fixture_http_upstream("alpha"),
        fixture_http_upstream("beta"),
    ])
    .await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "safe"))
        .await;
    pool.insert_entry_for_tests("beta", healthy_entry_with_tool("beta", "other"))
        .await;
    let caller = crate::gateway::palette::PaletteCaller {
        caller: CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: false,
                is_admin: false,
            },
            sub: Some("alice".to_string()),
        },
        caller_auth: labby_runtime::caller_auth::PropagatedCallerAuth {
            sub: Some("alice".to_string()),
            scopes: vec![
                "mcp:read".to_string(),
                "mcp:write".to_string(),
                "gateway:alpha".to_string(),
            ],
            trusted_local: false,
            access_principal_id: None,
            private_context_token: None,
        },
        scope: ToolScope::scoped_namespaces(vec!["alpha".to_string()], Vec::new()),
        owner: crate::gateway::shared::make_api_runtime_owner(Some("alice"), Some("req-scope")),
        oauth_subject: "alice".to_string(),
    };

    let error = manager
        .palette_execute(
            &caller,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:beta::other".to_string(),
                params: json!({}),
                expected_contract_hash: "a".repeat(64),
                confirm_destructive: false,
            },
        )
        .await
        .expect_err("cross-upstream call denied");
    assert_eq!(error.kind(), "forbidden");

    let catalog = manager
        .palette_catalog(&caller)
        .await
        .expect("scoped catalog");
    let old_hash = palette_contract_hash(&catalog, "mcp:alpha::safe");
    let mut reclassified = pool.healthy_tools_for_upstream("alpha").await;
    reclassified[0].destructive = true;
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([("safe".to_string(), reclassified.remove(0))]),
        ),
    )
    .await;
    let error = manager
        .palette_execute(
            &caller,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::safe".to_string(),
                params: json!({}),
                expected_contract_hash: old_hash,
                confirm_destructive: true,
            },
        )
        .await
        .expect_err("destructive reclassification is contract drift");
    assert_eq!(error.kind(), "contract_changed");
}

#[tokio::test]
async fn palette_execute_rejects_unknown_hidden_destructive_and_read_only_calls() {
    let mut suppressed = fixture_http_upstream("suppressed");
    suppressed.priority = 0.0;
    let (manager, pool) =
        code_mode_manager_with_upstreams(vec![fixture_http_upstream("alpha"), suppressed]).await;
    pool.insert_entry_for_tests("alpha", healthy_entry_with_tool("alpha", "delete"))
        .await;
    pool.insert_entry_for_tests(
        "suppressed",
        healthy_entry_with_tool("suppressed", "secret"),
    )
    .await;

    let mut destructive = pool.healthy_tools_for_upstream("alpha").await;
    destructive[0].destructive = true;
    pool.insert_entry_for_tests(
        "alpha",
        fixture_upstream_entry(
            "alpha",
            HashMap::from([("delete".to_string(), destructive.remove(0))]),
        ),
    )
    .await;

    let admin = crate::gateway::palette::PaletteCaller::admin(Some("admin"), Some("req-1"));
    let read_only = crate::gateway::palette::PaletteCaller::scoped_read_only(
        Some("user"),
        Some("req-2"),
        vec!["alpha".to_string()],
    );
    let catalog = manager
        .palette_catalog(&admin)
        .await
        .expect("admin catalog");
    let destructive_hash = palette_contract_hash(&catalog, "mcp:alpha::delete");

    let err = manager
        .palette_execute(
            &admin,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:missing::tool".to_string(),
                params: json!({}),
                confirm_destructive: false,
                expected_contract_hash: "a".repeat(64),
            },
        )
        .await
        .expect_err("unknown id rejected");
    assert_eq!(err.kind(), "not_found");

    let err = manager
        .palette_execute(
            &admin,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:suppressed::secret".to_string(),
                params: json!({}),
                confirm_destructive: false,
                expected_contract_hash: "a".repeat(64),
            },
        )
        .await
        .expect_err("priority zero hidden");
    assert_eq!(err.kind(), "not_found");

    let err = manager
        .palette_execute(
            &read_only,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::delete".to_string(),
                params: json!({}),
                confirm_destructive: true,
                expected_contract_hash: destructive_hash.clone(),
            },
        )
        .await
        .expect_err("read-only rejected");
    assert_eq!(err.kind(), "forbidden");

    let err = manager
        .palette_execute(
            &admin,
            crate::gateway::palette::PaletteExecuteRequest {
                id: "mcp:alpha::delete".to_string(),
                params: json!({}),
                confirm_destructive: false,
                expected_contract_hash: destructive_hash,
            },
        )
        .await
        .expect_err("destructive confirmation required");
    assert_eq!(err.kind(), "confirmation_required");
}

// ── Semantic search (fail-open embedding blend) ──────────────────────────────

#[tokio::test]
async fn semantic_rank_returns_empty_when_unconfigured() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let result = manager
        .semantic_rank(
            "hello".to_string(),
            5,
            &CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
        )
        .await
        .unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn semantic_search_cooldown_blocks_immediate_retry_after_failure() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    manager.record_semantic_search_failure("test failure").await;
    assert!(!manager.semantic_search_available().await);
}

#[tokio::test]
async fn semantic_search_recovery_clears_cooldown() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    manager.record_semantic_search_failure("test failure").await;
    assert!(!manager.semantic_search_available().await);
    manager.record_semantic_search_recovery().await;
    assert!(manager.semantic_search_available().await);
}

#[tokio::test]
async fn ensure_embeddings_for_fingerprint_is_noop_when_unconfigured() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let entries = Vec::new(); // empty catalog — also exercises the cold-start-empty-catalog path
    let result = manager
        .ensure_embeddings_for_fingerprint("some-fingerprint", &entries)
        .await;
    assert!(result.is_empty());
    assert!(
        manager
            .cached_embeddings("some-fingerprint")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn catalog_embeddings_stay_cold_when_semantic_search_unconfigured() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    // Default config has semantic_search.tei_url = None.
    let render = manager
        .list_tools(
            &CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            false,
            false,
        )
        .await
        .unwrap();
    // The embedding cache must remain empty — ensure_embeddings_for_fingerprint
    // returns immediately for an unconfigured host.
    assert!(
        manager
            .cached_embeddings(&render.embedding_fingerprint)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn semantic_rank_never_returns_ids_outside_scope_filtered_catalog() {
    // semantic_rank's own internal build_tools_render call uses the SAME
    // `scope` parameter it was given, and its ranking set is additionally
    // filtered with the same `kind == Snippet || scope.allows(...)` test the
    // sandbox's own discovery catalog uses — so an id excluded by that scope
    // is structurally never present in the vectors handed to
    // `rank_by_similarity` in the first place.
    //
    // This unit test exercises the unconfigured (no TEI) path, which
    // already proves semantic_rank cannot fabricate ids independent of
    // build_tools_render's scope-filtered output regardless of scope — a
    // live, multi-upstream, TEI-backed confirmation of the same invariant
    // is covered by the plan's manual smoke test (Task 7 Step 6).
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let restrictive_scope = ToolScope::scoped_namespaces(vec![], vec![]);
    let result = manager
        .semantic_rank(
            "anything".to_string(),
            5,
            &CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &restrictive_scope,
        )
        .await
        .unwrap();
    assert!(result.is_empty());
}
#[tokio::test]
async fn ensure_embeddings_unreachable_tei_fails_open_and_records_cooldown() {
    let (manager, _pool) = code_mode_manager_with_upstreams(Vec::new()).await;
    let mut cfg = manager.code_mode_config().await;
    cfg.semantic_search.tei_url = Some("http://127.0.0.1:1".to_string());
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: cfg,
            ..GatewayConfig::default()
        })
        .await;
    let entries = vec![labby_codemode::ToolDescriptor::tool(
        "alpha",
        "ping",
        "Ping the alpha upstream",
        None,
        None,
    )];
    assert!(manager.semantic_search_available().await);
    let result = manager
        .ensure_embeddings_for_fingerprint("fp-test", &entries)
        .await;
    assert!(result.is_empty(), "fail-open returns empty vectors");
    assert!(
        !manager.semantic_search_available().await,
        "failure must start the cooldown"
    );
}

/// A cold HTTP MCP upstream for the one-shot CLI catalog tests: answers the
/// modern lifecycle (`server/discover`), lists one fixed tool, and can delay
/// `tools/list` or `prompts/list` to shape probe timing. Request counters let
/// tests prove cache hits are connect-free.
#[derive(Clone)]
struct OneShotHttpResponder {
    tool: &'static str,
    list_tools_delay: Duration,
    list_prompts_delay: Duration,
    list_tools_requests: Arc<AtomicUsize>,
}

impl OneShotHttpResponder {
    fn new(tool: &'static str, list_tools_delay: Duration) -> Self {
        Self {
            tool,
            list_tools_delay,
            list_prompts_delay: Duration::ZERO,
            list_tools_requests: Arc::default(),
        }
    }

    fn with_prompts_delay(mut self, delay: Duration) -> Self {
        self.list_prompts_delay = delay;
        self
    }

    fn list_tools_requests(&self) -> usize {
        self.list_tools_requests.load(Ordering::SeqCst)
    }
}

impl wiremock::Respond for OneShotHttpResponder {
    fn respond(&self, request: &wiremock::Request) -> wiremock::ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(serde_json::Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
        match method {
            "server/discover" => wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": {"tools": {}, "prompts": {}},
                    "serverInfo": {"name": "one-shot-fixture", "version": "1.0.0"},
                    "ttlMs": 0,
                    "cacheScope": "private"
                }
            })),
            "notifications/initialized" => wiremock::ResponseTemplate::new(202),
            "tools/list" => {
                self.list_tools_requests.fetch_add(1, Ordering::SeqCst);
                wiremock::ResponseTemplate::new(200)
                    .set_delay(self.list_tools_delay)
                    .set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"tools": [{
                            "name": self.tool,
                            "description": "one-shot catalog fixture",
                            "inputSchema": {"type": "object"}
                        }]}
                    }))
            }
            "prompts/list" => wiremock::ResponseTemplate::new(200)
                .set_delay(self.list_prompts_delay)
                .set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"prompts": []}
                })),
            other => wiremock::ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

async fn cold_http_upstream(
    name: &str,
    responder: OneShotHttpResponder,
) -> (wiremock::MockServer, UpstreamConfig) {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder)
        .mount(&server)
        .await;
    let mut upstream = fixture_http_upstream(name);
    upstream.url = Some(format!("{}/mcp", server.uri()));
    (server, upstream)
}

/// An upstream that accepts the connection and then refuses the MCP handshake.
///
/// Deliberately not `fixture_http_upstream`'s closed port: a connect to a
/// closed port is refused instantly on Unix but sits in SYN retransmit on
/// Windows, so that upstream can end a run either as a genuine failure or as
/// still in flight. Tests that assert the *failure* arm specifically — as the
/// negative cache must, since only a real failure may be suppressed — need a
/// fixture that fails the same way on every platform. Accepting the connection
/// and answering 500 does that.
async fn refusing_http_upstream(name: &str) -> (wiremock::MockServer, UpstreamConfig) {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut upstream = fixture_http_upstream(name);
    upstream.url = Some(format!("{}/mcp", server.uri()));
    (server, upstream)
}

/// A socket that accepts and never answers, so a connect stalls until its
/// discovery timeout (far beyond any budget used here).
async fn stalled_http_upstream(name: &str) -> UpstreamConfig {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stalled upstream fixture");
    let addr = listener.local_addr().expect("listener addr");
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _socket = socket;
                tokio::time::sleep(Duration::from_mins(2)).await;
            });
        }
    });
    let mut upstream = fixture_http_upstream(name);
    upstream.url = Some(format!("http://{addr}/mcp"));
    upstream
}

/// A manager with a fresh (cold) pool, the given Code Mode timeout, the
/// one-shot catalog cache redirected to `cache_path`, and the discovery
/// concurrency pinned to the product default of 3 so the tests do not depend
/// on whatever the machine's process default happens to be. (An ambient
/// `LABBY_UPSTREAM_DISCOVERY_CONCURRENCY` still overrides the config value.)
async fn one_shot_manager_at(
    upstreams: Vec<UpstreamConfig>,
    timeout_ms: u64,
    cache_path: PathBuf,
) -> (GatewayManager, Arc<UpstreamPool>) {
    one_shot_manager_with_concurrency(upstreams, timeout_ms, cache_path, 3).await
}

async fn one_shot_manager_with_concurrency(
    upstreams: Vec<UpstreamConfig>,
    timeout_ms: u64,
    cache_path: PathBuf,
    concurrency: usize,
) -> (GatewayManager, Arc<UpstreamPool>) {
    let (mut manager, pool) = code_mode_manager_with_upstreams(upstreams.clone()).await;
    manager
        .seed_config_unchecked_for_tests(GatewayConfig {
            code_mode: CodeModeConfig {
                enabled: true,
                timeout_ms,
                ..CodeModeConfig::default()
            },
            gateway: labby_runtime::gateway_config::GatewayPreferences {
                upstream_discovery_concurrency: Some(concurrency),
                ..labby_runtime::gateway_config::GatewayPreferences::default()
            },
            upstream: upstreams,
            ..GatewayConfig::default()
        })
        .await;
    manager.set_code_mode_catalog_cache_path_for_tests(cache_path);
    (manager, pool)
}

async fn received_request_count(server: &wiremock::MockServer) -> usize {
    server
        .received_requests()
        .await
        .map_or(0, |requests| requests.len())
}

fn tool_ids(tools: &[UpstreamTool]) -> Vec<String> {
    tools
        .iter()
        .map(|tool| format!("{}::{}", tool.upstream_name, tool.tool.name))
        .collect()
}

/// Outer liveness guard for a budget-bounded one-shot catalog call.
///
/// These tests prove that the cold-connect budget, not a per-upstream
/// discovery timeout, is what bounds the wait. The guard therefore has to stay
/// below that discovery timeout — 30s for the stalled HTTP fixtures, since
/// `UpstreamPool::new` uses the 30s `DEFAULT_REQUEST_TIMEOUT` — so a budget
/// that stopped working still fails the test rather than merely running long.
///
/// Within that ceiling it should be as generous as possible: the calls being
/// guarded finish in a few hundred milliseconds, and the guard is not the
/// assertion. The returned catalog and the emitted warning carry the meaning.
/// Earlier values as low as 5s left only a scheduling hiccup of headroom on a
/// loaded machine, which is a flake waiting to happen and already bit the
/// sibling Windows shard once.
const BUDGET_GUARD: Duration = Duration::from_secs(20);

/// Run `future` while capturing tracing output, returning its result and the
/// captured JSON log lines.
#[allow(clippy::await_holding_lock)] // TRACING_TEST_LOCK must span the captured await
async fn with_captured_logs<T>(future: impl Future<Output = T>) -> (T, String) {
    let _tracing_lock = crate::test_support::TRACING_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let buffer = crate::test_support::SharedBuf::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .without_time(),
    );
    let tracing_guard = tracing::subscriber::set_default(subscriber);
    let output = future.await;
    drop(tracing_guard);
    (output, crate::test_support::captured_logs(&buffer))
}

/// The budget warning line, if any, from captured logs.
fn budget_warning(logs: &str) -> Option<&str> {
    logs.lines()
        .find(|line| line.contains("cold-connect budget exhausted"))
}

/// One-shot CLI catalog: dead and stalled upstreams ahead of a genuinely cold
/// healthy one must not starve it. Uncached upstreams are probed concurrently
/// under a budget derived from the Code Mode timeout (half of 6s here), the
/// stalled straggler is named and omitted for this run, and the upstream that
/// completed is persisted so the next run does not pay for it again. Serial
/// probing would first wait out the stalled upstream's 30s discovery timeout,
/// which the 15s guard rejects.
///
/// How the refused upstream is classified is deliberately not asserted: a
/// connect to a closed port is refused immediately on Unix but can stay
/// pending on Windows, so it may end the run either as a failure or as still
/// in flight. Only that it never lands in the catalog or the cache matters
/// here; the failure-reporting path is pinned by
/// `one_shot_cli_catalog_errors_when_every_uncached_upstream_fails_fast`.
#[tokio::test]
async fn one_shot_cli_catalog_bounds_cold_connects_and_persists_completed_upstreams() {
    let stalled = stalled_http_upstream("alpha").await;
    // `fixture_http_upstream` points at 127.0.0.1:9, which nothing listens on.
    let dead = fixture_http_upstream("beta");
    let responder = OneShotHttpResponder::new("ping", Duration::ZERO);
    let (_server, healthy) = cold_http_upstream("omega", responder.clone()).await;
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");
    let (manager, _pool) = one_shot_manager_at(
        vec![stalled.clone(), dead.clone(), healthy.clone()],
        6_000,
        cache_path.clone(),
    )
    .await;

    let (tools, logs) = with_captured_logs(tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    ))
    .await;
    let tools = tools
        .expect("one-shot catalog must not wait out a stalled upstream's discovery timeout")
        .expect("stalled and dead upstreams are omitted from the proxy, not an error");

    assert_eq!(tool_ids(&tools), vec!["omega::ping"]);
    assert_eq!(responder.list_tools_requests(), 1);
    let warning = budget_warning(&logs).expect("the budget cutoff must be logged");
    assert!(
        warning.contains("in_flight_upstreams") && warning.contains("alpha"),
        "the stalled upstream must be named as in flight: {warning}"
    );
    assert!(
        !warning.contains("omega"),
        "a completed upstream is not unfinished: {warning}"
    );

    let cache = catalog_cache::CatalogCache::load_from(&cache_path);
    assert_eq!(
        cache
            .fresh_tools("omega", &catalog_cache::fingerprint(&healthy))
            .map(|tools| tools.len()),
        Some(1),
        "an upstream that completed must be persisted even though another stalled"
    );
    for (name, config) in [("alpha", &stalled), ("beta", &dead)] {
        assert!(
            cache
                .fresh_tools(name, &catalog_cache::fingerprint(config))
                .is_none(),
            "{name} did not connect and must not be cached, so the next run retries it"
        );
    }
}

/// Partial means partial, not empty: when the budget ends before any upstream
/// connected and nothing was served from cache, the one-shot catalog is an
/// error naming what was still connecting, never a silently empty proxy.
#[tokio::test]
async fn one_shot_cli_catalog_errors_when_nothing_connects_within_the_budget() {
    let stalled = stalled_http_upstream("alpha").await;
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let (manager, _pool) = one_shot_manager_at(
        vec![stalled],
        400,
        cache_dir.path().join("codemode-catalog.json"),
    )
    .await;

    let error = tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    )
    .await
    .expect("the budget must bound the wait")
    .expect_err("an empty catalog at the deadline is an error");
    match error {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "upstream_connect_error");
            assert!(
                message.contains("still connecting") && message.contains("alpha"),
                "the error must name the stalled upstream: {message}"
            );
        }
        other => panic!("expected upstream_connect_error, got {other:?}"),
    }
}

/// The same rule holds without a deadline: if every uncached upstream fails
/// fast and nothing was cached, the run errors and names each failure, rather
/// than handing the sandbox an empty proxy with exit code 0.
#[tokio::test]
async fn one_shot_cli_catalog_errors_when_every_uncached_upstream_fails_fast() {
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let (manager, _pool) = one_shot_manager_at(
        vec![
            fixture_http_upstream("beta"),
            fixture_http_upstream("gamma"),
        ],
        10_000,
        cache_dir.path().join("codemode-catalog.json"),
    )
    .await;

    let error = tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    )
    .await
    .expect("connection refused fails fast")
    .expect_err("all-failed with an empty cache is an error");
    match error {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "upstream_connect_error");
            assert!(
                message.contains("beta") && message.contains("gamma"),
                "the error must name every failed upstream: {message}"
            );
        }
        other => panic!("expected upstream_connect_error, got {other:?}"),
    }
}

/// Concurrent probes settle out of order, yet the catalog must follow the
/// configured upstream order; and a fresh process (new manager and pool, same
/// cache file) must serve a fresh cache entry without connecting again.
#[tokio::test]
async fn one_shot_cli_catalog_keeps_config_order_and_serves_repeat_runs_from_cache() {
    let slow = OneShotHttpResponder::new("slow_tool", Duration::from_millis(300));
    let fast = OneShotHttpResponder::new("fast_tool", Duration::ZERO);
    let (_slow_server, zeta) = cold_http_upstream("zeta", slow.clone()).await;
    let (_fast_server, alpha) = cold_http_upstream("alpha", fast.clone()).await;
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");
    let upstreams = vec![zeta, alpha];

    let (first_manager, _pool) =
        one_shot_manager_at(upstreams.clone(), 10_000, cache_path.clone()).await;
    let first = first_manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect("both cold upstreams connect");
    assert_eq!(
        tool_ids(&first),
        vec!["zeta::slow_tool", "alpha::fast_tool"]
    );
    assert_eq!(slow.list_tools_requests(), 1);
    assert_eq!(fast.list_tools_requests(), 1);

    let slow_requests = received_request_count(&_slow_server).await;
    let fast_requests = received_request_count(&_fast_server).await;

    let (second_manager, _pool) = one_shot_manager_at(upstreams, 10_000, cache_path).await;
    let second = second_manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect("fresh cache entries need no connects");
    assert_eq!(tool_ids(&second), tool_ids(&first));
    assert_eq!(
        slow.list_tools_requests(),
        1,
        "a fresh cache entry must short-circuit without a connect"
    );
    assert_eq!(fast.list_tools_requests(), 1);
    assert_eq!(
        (
            received_request_count(&_slow_server).await,
            received_request_count(&_fast_server).await
        ),
        (slow_requests, fast_requests),
        "a cache hit must send no request at all, not even discovery"
    );
}

/// A cached upstream keeps the run partial rather than failed when a
/// straggler misses the budget: the cached tools are served without a
/// connect and the straggler is named.
#[tokio::test]
async fn one_shot_cli_catalog_serves_cached_upstreams_when_a_straggler_misses_the_budget() {
    let responder = OneShotHttpResponder::new("ping", Duration::ZERO);
    let (_server, healthy) = cold_http_upstream("omega", responder.clone()).await;
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");
    let (warm_manager, _pool) =
        one_shot_manager_at(vec![healthy.clone()], 10_000, cache_path.clone()).await;
    warm_manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect("warm the cache");
    assert_eq!(responder.list_tools_requests(), 1);

    let stalled = stalled_http_upstream("alpha").await;
    let (manager, _pool) = one_shot_manager_at(vec![stalled, healthy], 400, cache_path).await;
    let (tools, logs) = with_captured_logs(tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    ))
    .await;
    let tools = tools
        .expect("the budget must bound the wait")
        .expect("a cached upstream keeps the catalog partial, not failed");

    assert_eq!(tool_ids(&tools), vec!["omega::ping"]);
    assert_eq!(
        responder.list_tools_requests(),
        1,
        "the cached upstream must be served without a connect"
    );
    let warning = budget_warning(&logs).expect("the budget cutoff must be logged");
    assert!(
        warning.contains("alpha"),
        "straggler must be named: {warning}"
    );
}

/// An upstream whose tools landed before the budget ended is connected even if
/// the connect's trailing prompt-cache refresh is what the deadline cut off:
/// its tools are served and cached, and nothing is reported as unfinished.
#[tokio::test]
async fn one_shot_cli_catalog_keeps_an_upstream_whose_tools_landed_before_the_cutoff() {
    let responder = OneShotHttpResponder::new("ping", Duration::ZERO)
        .with_prompts_delay(Duration::from_mins(2));
    let (_server, mut healthy) = cold_http_upstream("omega", responder).await;
    healthy.proxy_prompts = true;
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");
    let (manager, _pool) =
        one_shot_manager_at(vec![healthy.clone()], 4_000, cache_path.clone()).await;

    let (tools, logs) = with_captured_logs(tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    ))
    .await;
    let tools = tools
        .expect("the budget must bound the wait")
        .expect("tools that landed before the cutoff are served");

    assert_eq!(tool_ids(&tools), vec!["omega::ping"]);
    assert!(
        budget_warning(&logs).is_none(),
        "a harvested upstream is not unfinished: {logs}"
    );
    let cache = catalog_cache::CatalogCache::load_from(&cache_path);
    assert_eq!(
        cache
            .fresh_tools("omega", &catalog_cache::fingerprint(&healthy))
            .map(|tools| tools.len()),
        Some(1)
    );
}

/// OAuth upstreams stay subject-scoped on the one-shot path: with a subject
/// their tools are served from the subject cache, without one they are
/// skipped, and they are never written to the on-disk catalog cache.
#[tokio::test]
async fn one_shot_cli_catalog_keeps_oauth_upstreams_subject_scoped_and_uncached() {
    let upstream = fixture_oauth_upstream("private", "http://unused.invalid/mcp");
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");
    let (manager, pool) =
        one_shot_manager_at(vec![upstream.clone()], 10_000, cache_path.clone()).await;
    pool.install_test_subject_tools_for_upstream(
        &upstream,
        "alice",
        vec![rmcp::model::Tool::new(
            "private_ping".to_string(),
            "private ping".to_string(),
            Arc::new(serde_json::Map::new()),
        )],
    )
    .await;

    let with_subject = manager
        .code_mode_catalog_tools_cached(None, Some("alice"))
        .await
        .expect("subject-scoped tools are served");
    assert_eq!(tool_ids(&with_subject), vec!["private::private_ping"]);

    let without_subject = manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect("OAuth upstreams are skipped without a subject");
    assert!(without_subject.is_empty());

    let cache = catalog_cache::CatalogCache::load_from(&cache_path);
    assert!(
        cache
            .fresh_tools("private", &catalog_cache::fingerprint(&upstream))
            .is_none(),
        "subject-scoped catalogs must never reach the shared cache"
    );
}

/// Probes that never got a discovery slot are reported as not attempted, not
/// as still connecting: with one slot and a stalled upstream ahead of a
/// healthy one, the healthy one is never contacted and the error says so.
#[tokio::test]
async fn one_shot_cli_catalog_names_unattempted_upstreams_when_stalled_probes_fill_the_slots() {
    let stalled = stalled_http_upstream("alpha").await;
    let responder = OneShotHttpResponder::new("ping", Duration::ZERO);
    let (_server, healthy) = cold_http_upstream("omega", responder.clone()).await;
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let (manager, _pool) = one_shot_manager_with_concurrency(
        vec![stalled, healthy],
        1_000,
        cache_dir.path().join("codemode-catalog.json"),
        1,
    )
    .await;

    let error = tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    )
    .await
    .expect("the budget must bound the wait")
    .expect_err("nothing connected, so the catalog is an error");
    assert_eq!(responder.list_tools_requests(), 0, "omega never got a slot");
    match error {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "upstream_connect_error");
            assert!(
                message.contains("still connecting") && message.contains("alpha"),
                "alpha was in flight: {message}"
            );
            assert!(
                message.contains("not attempted") && message.contains("omega"),
                "omega was never attempted: {message}"
            );
        }
        other => panic!("expected upstream_connect_error, got {other:?}"),
    }
}

/// A fresh cache entry with zero tools (a resource- or prompt-only upstream)
/// still counts as served from cache: a straggler missing the budget leaves
/// the run partial with an empty tool list, not failed.
#[tokio::test]
async fn one_shot_cli_catalog_treats_a_cached_zero_tool_upstream_as_served() {
    let quiet = fixture_http_upstream("omega");
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");
    catalog_cache::merge_and_store(
        cache_path.clone(),
        vec![catalog_cache::CatalogCacheUpdate {
            upstream_name: "omega".to_string(),
            fingerprint: catalog_cache::fingerprint(&quiet),
            tools: Vec::new(),
        }],
        Vec::new(),
    )
    .await;

    let stalled = stalled_http_upstream("alpha").await;
    let (manager, _pool) = one_shot_manager_at(vec![stalled, quiet], 400, cache_path).await;
    let (tools, logs) = with_captured_logs(tokio::time::timeout(
        BUDGET_GUARD,
        manager.code_mode_catalog_tools_cached(None, None),
    ))
    .await;
    let tools = tools
        .expect("the budget must bound the wait")
        .expect("a cached zero-tool upstream keeps the run partial, not failed");
    assert!(tools.is_empty());
    let warning = budget_warning(&logs).expect("the straggler must be logged");
    assert!(
        warning.contains("alpha"),
        "straggler must be named: {warning}"
    );
}

/// A genuinely failing upstream must not cost a connect on every invocation.
///
/// `fixture_http_upstream` points at the discard port, so the probe fails fast
/// rather than stalling — this is the failure path, distinct from the
/// budget-exhaustion paths, and the only one the negative cache may suppress.
#[tokio::test]
async fn a_failed_probe_is_suppressed_on_the_next_one_shot_run() {
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");

    let (dead_server, dead) = refusing_http_upstream("dead").await;
    let (server, healthy) =
        cold_http_upstream("healthy", OneShotHttpResponder::new("ping", Duration::ZERO)).await;
    let (manager, _pool) =
        one_shot_manager_at(vec![dead.clone(), healthy], 4_000, cache_path.clone()).await;

    let first = manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect("first run should serve the healthy upstream");
    assert_eq!(
        first
            .iter()
            .map(|tool| tool.tool.name.as_ref())
            .collect::<Vec<_>>(),
        vec!["ping"]
    );

    // The failure is now on disk, and the healthy upstream is cached, so the
    // second run must reach neither the network nor the dead upstream.
    let cache = catalog_cache::CatalogCache::load_from(&cache_path);
    assert!(
        cache.probe_suppressed("dead", &catalog_cache::fingerprint(&dead)),
        "a failed probe must leave a negative entry"
    );

    let (second, logs) =
        with_captured_logs(manager.code_mode_catalog_tools_cached(None, None)).await;
    let second = second.expect("second run should still serve the cached healthy upstream");
    assert_eq!(
        second
            .iter()
            .map(|tool| tool.tool.name.as_ref())
            .collect::<Vec<_>>(),
        vec!["ping"]
    );
    assert!(
        logs.contains("suppressed"),
        "a suppressed upstream must be reported, not silently dropped: {logs}"
    );
    drop(server);
    drop(dead_server);
}

/// Suppression must never be the reason a catalog comes back empty.
#[tokio::test]
async fn an_entirely_suppressed_fleet_is_an_error_not_an_empty_catalog() {
    let cache_dir = tempfile::tempdir().expect("tempdir");
    let cache_path = cache_dir.path().join("codemode-catalog.json");

    let (dead_server, dead) = refusing_http_upstream("dead").await;
    let (manager, _pool) = one_shot_manager_at(vec![dead], 4_000, cache_path.clone()).await;

    manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect_err("a fleet with nothing reachable is already an error");

    let error = manager
        .code_mode_catalog_tools_cached(None, None)
        .await
        .expect_err("suppression must not turn that into a silent empty catalog");
    let message = format!("{error}");
    assert!(
        message.contains("suppressed") && message.contains("dead"),
        "the error must name the suppressed upstream: {message}"
    );
    drop(dead_server);
}
