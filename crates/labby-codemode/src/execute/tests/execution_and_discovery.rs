use super::*;

#[test]
fn read_only_scope_can_run_without_granting_write_execution() {
    let unscoped_caller = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities::default(),
        sub: Some("unscoped".to_string()),
    };
    let read_caller = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            ..CodeModeCallerCapabilities::default()
        },
        sub: Some("reader".to_string()),
    };

    assert!(!execution_allowed(
        &unscoped_caller,
        &ToolScope::default().read_only()
    ));
    assert!(!execution_allowed(&read_caller, &ToolScope::default()));
    assert!(execution_allowed(
        &read_caller,
        &ToolScope::default().read_only()
    ));
    assert!(!read_caller.can_execute());
}

#[test]
fn personal_catalog_filter_tracks_live_source_and_kind_policy() {
    let tool = CatalogDescriptor::tool("github", "issues", "issues", None, None);
    let skill = CatalogDescriptor::metadata(
        CodeModeCatalogKind::Skill,
        "personal",
        "skill://personal/review",
        "review",
        "review skill",
        Vec::new(),
    );
    let resource = CatalogDescriptor::metadata(
        CodeModeCatalogKind::Resource,
        "github",
        "resource://github/readme",
        "readme",
        "compat resource",
        Vec::new(),
    );
    let scope = ToolScope::default();
    let mut search = CodeModeSearchConfig::default();
    assert!(personal_catalog_entry_enabled(&tool, &scope, &search));
    assert!(personal_catalog_entry_enabled(&skill, &scope, &search));
    assert!(personal_catalog_entry_enabled(&resource, &scope, &search));

    search.kinds.remove(&CodeModeSearchKind::Skill);
    assert!(personal_catalog_entry_enabled(&tool, &scope, &search));
    assert!(!personal_catalog_entry_enabled(&skill, &scope, &search));
    assert!(personal_catalog_entry_enabled(&resource, &scope, &search));

    search.sources.remove(&CodeModeSearchSource::PersonalLabby);
    assert!(!personal_catalog_entry_enabled(&tool, &scope, &search));
    assert!(!personal_catalog_entry_enabled(&resource, &scope, &search));
}

#[tokio::test]
async fn call_tool_id_routes_lab_internal_namespace_before_scope_check() {
    // A ToolScope that allows nothing should still let `__lab_internal::*`
    // through, because it's intercepted before the scope.allows() check.
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let empty_scope = ToolScope::scoped_namespaces(vec![], vec![]);
    let result = broker
        .call_tool_id(
            "__lab_internal::semantic_rank",
            json!({ "query": "test", "limit": 5 }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &empty_scope,
            ExecCtx::none(),
        )
        .await;
    // NoopHost's semantic_rank always returns Ok(vec![]), so this must
    // succeed with an empty ranked list, not a `forbidden`/`unknown_tool`
    // scope error.
    let value = result.expect("internal dispatch must bypass scope.allows()");
    assert_eq!(value, json!({ "ranked": [] }));
}

#[tokio::test]
async fn call_tool_id_rejects_unknown_internal_tool() {
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let scope = ToolScope::default();
    let result = broker
        .call_tool_id(
            "__lab_internal::not_a_real_internal_tool",
            json!({}),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await;
    assert!(result.is_err());
}

#[test]
fn large_result_warning_preserves_the_result() {
    let value = Value::String("x".repeat(7998));
    let shaped = shape_final_result(
        Some(value.clone()),
        CodeModeResultShapePolicy::Truncate,
        24 * 1024,
        6000,
        4,
    );
    let mut response = response_with_result(shaped.result.expect("shaped result"));
    response.result_shaping = Some(shaped.metadata);
    let config = CodeModeConfig {
        max_response_bytes: 24 * 1024,
        max_response_tokens: 6000,
        token_estimate_divisor: 4,
        ..CodeModeConfig::default()
    };

    remove_soft_warning_if_it_breaks_budget(&mut response, &config);

    assert_eq!(response.result, Some(value));
    assert!(
        response
            .result_shaping
            .as_ref()
            .and_then(|metadata| metadata.warning.as_deref())
            .is_some_and(|warning| warning.contains("large result"))
    );
}

#[tokio::test]
async fn broker_enforces_configured_source_limit_before_runner_start() {
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let config = CodeModeConfig {
        max_source_bytes: 1024,
        ..CodeModeConfig::default()
    };
    let code = format!("async () => \"{}\"", "x".repeat(2048));

    let error = broker
        .execute_with_raw_response(
            &code,
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            config,
            ToolScope::default(),
            None,
        )
        .await
        .expect_err("configured lower source limit must reject before runner start");

    assert!(format!("{error}").contains("code exceeds max length 1024 bytes"));
}

#[test]
fn execution_timeout_reserves_response_delivery_margin() {
    assert_eq!(execution_timeout(30_000), Duration::from_millis(29_500));
    assert_eq!(execution_timeout(500), Duration::from_millis(500));
    assert_eq!(execution_timeout(1), Duration::from_millis(1));
}

#[test]
fn oversized_response_drops_soft_warning_before_hard_truncation() {
    let value = Value::String("x".repeat(7998));
    let shaped = shape_final_result(
        Some(value.clone()),
        CodeModeResultShapePolicy::Truncate,
        24 * 1024,
        6000,
        4,
    );
    let mut response = response_with_result(shaped.result.expect("shaped result"));
    response.result_shaping = Some(shaped.metadata);
    response.logs.push("y".repeat(24 * 1024));
    let config = CodeModeConfig {
        max_response_bytes: 24 * 1024,
        max_response_tokens: 6000,
        token_estimate_divisor: 4,
        result_shape_policy: CodeModeResultShapePolicy::Truncate,
        ..CodeModeConfig::default()
    };

    remove_soft_warning_if_it_breaks_budget(&mut response, &config);

    assert_eq!(response.result, Some(value));
    assert!(
        response
            .result_shaping
            .as_ref()
            .is_some_and(|metadata| metadata.warning.is_none())
    );
}

#[test]
fn apply_ui_opt_in_unwraps_and_attaches_captured_link() {
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    *broker.ui_capture.lock().unwrap() = Some(UiLink {
        ui_meta: json!({ "resourceUri": "ui://axon/status-dashboard" }),
    });
    let mut response = response_with_result(json!({ "__ui": { "degraded": false } }));
    broker.apply_ui_opt_in(&mut response);
    // Inner payload is surfaced as `result`, wrapper removed.
    assert_eq!(response.result, Some(json!({ "degraded": false })));
    assert_eq!(
        response.ui.as_ref().expect("widget attached").ui_meta["resourceUri"],
        "ui://axon/status-dashboard"
    );
}

#[test]
fn apply_ui_opt_in_without_optin_is_noop() {
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    let mut response = response_with_result(json!({ "degraded": false }));
    broker.apply_ui_opt_in(&mut response);
    assert_eq!(response.result, Some(json!({ "degraded": false })));
    assert!(
        response.ui.is_none(),
        "no captured widget → no widget attached"
    );
}

#[test]
fn apply_ui_opt_in_surfaces_direct_ui_tool_result() {
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    *broker.ui_capture.lock().unwrap() = Some(UiLink {
        ui_meta: json!({ "resourceUri": "ui://ytdl-mcp/youtube-search.html" }),
    });

    let mut response = response_with_result(json!({
        "query": "phish",
        "limit": 1,
        "results": []
    }));

    broker.apply_ui_opt_in(&mut response);

    assert_eq!(
        response.ui.as_ref().expect("widget attached").ui_meta["resourceUri"],
        "ui://ytdl-mcp/youtube-search.html"
    );
}

#[test]
fn clamp_semantic_query_leaves_small_queries_untouched() {
    assert_eq!(clamp_semantic_query("hello".to_string()), "hello");
    let exactly_max = "a".repeat(MAX_SEMANTIC_QUERY_BYTES);
    assert_eq!(clamp_semantic_query(exactly_max.clone()), exactly_max);
}

#[test]
fn clamp_semantic_query_truncates_oversized_ascii_without_error() {
    let oversized = "a".repeat(MAX_SEMANTIC_QUERY_BYTES + 1000);
    let clamped = clamp_semantic_query(oversized);
    assert_eq!(clamped.len(), MAX_SEMANTIC_QUERY_BYTES);
}

#[test]
fn clamp_semantic_query_truncates_on_char_boundary() {
    // 4-byte scorpions straddling the cap: the clamp must land on a char
    // boundary (valid UTF-8), never split a code point.
    let oversized = "\u{1F982}".repeat(MAX_SEMANTIC_QUERY_BYTES / 4 + 10);
    let clamped = clamp_semantic_query(oversized);
    assert!(clamped.len() <= MAX_SEMANTIC_QUERY_BYTES);
    assert_eq!(
        clamped.len(),
        MAX_SEMANTIC_QUERY_BYTES - MAX_SEMANTIC_QUERY_BYTES % 4
    );
    assert!(clamped.chars().all(|c| c == '\u{1F982}'));
}

#[tokio::test]
async fn artifact_search_returns_query_backed_entries_outside_injected_catalog() {
    let host = FixtureHost::new(Vec::new()).with_search_entries(vec![CatalogDescriptor::metadata(
        CodeModeCatalogKind::Skill,
        "public_depot",
        "depot:skill:fixture",
        "fixture skill",
        "query-backed result",
        vec!["skill".to_string()],
    )]);
    let broker = CodeModeBroker::new(Some(&host));
    let value = broker
        .call_tool_id(
            "__lab_internal::artifact_search",
            json!({ "query": "fixture", "limit": 5, "kinds": ["skill"] }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect("query-backed search succeeds");

    assert_eq!(value["entries"][0]["id"], "depot:skill:fixture");
    assert_eq!(value["entries"][0]["kind"], "skill");
    assert_eq!(value["incompleteSources"], json!([]));
    assert_eq!(
        value["entries"][0]["path"],
        "skill.public_depot.fixture_skill"
    );
}

#[tokio::test]
async fn artifact_search_returns_one_extra_entry_to_signal_truncation() {
    let entries = (0..3)
        .map(|index| {
            CatalogDescriptor::metadata(
                CodeModeCatalogKind::Skill,
                "public_depot",
                &format!("skill-{index}"),
                &format!("fixture-{index}"),
                "fixture",
                Vec::new(),
            )
        })
        .collect();
    let host = FixtureHost::new(Vec::new()).with_search_entries(entries);
    let broker = CodeModeBroker::new(Some(&host));
    let value = broker
        .call_tool_id(
            "__lab_internal::artifact_search",
            json!({ "query": "fixture", "limit": 1, "kinds": ["skill"] }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .unwrap();
    assert_eq!(value["entries"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn raw_tool_call_boundary_rejects_undeclared_and_out_of_route_tools() {
    let host = FixtureHost::new(vec![
        CatalogDescriptor::tool("alpha", "tool1", "allowed", None, None),
        CatalogDescriptor::tool("alpha", "other_tool", "undeclared sibling", None, None),
        CatalogDescriptor::tool("beta", "tool2", "out of route", None, None),
    ]);
    let broker = CodeModeBroker::new(Some(&host));
    let scope =
        ToolScope::scoped_namespaces(vec!["alpha".to_string()], vec!["alpha::tool1".to_string()]);

    for id in ["alpha::other_tool", "beta::tool2"] {
        let error = broker
            .call_tool_id(
                id,
                json!({}),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &scope,
                ExecCtx::none(),
            )
            .await
            .expect_err("raw callTool target outside the effective snippet scope must fail");
        assert_eq!(error.kind(), "unknown_tool");
        assert!(
            error
                .to_string()
                .contains("outside this Code Mode execution capability set"),
            "scope rejection must happen before host dispatch: {error:?}"
        );
    }

    let allowed = broker
        .call_tool_id(
            "alpha::tool1",
            json!({}),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect_err("fixture host intentionally rejects real dispatch after scope admission");
    assert!(
        allowed
            .to_string()
            .contains("FixtureHost does not dispatch real tool calls"),
        "the declared in-route tool must pass the scope boundary and reach the host"
    );
}
