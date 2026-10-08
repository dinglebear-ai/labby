use super::*;

#[tokio::test]
async fn dispatch_internal_call_describe_types_missing_id_is_missing_param() {
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));

    let err = broker
        .call_tool_id(
            "__lab_internal::describe_types",
            json!({}),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect_err("a missing id must be a caller error, not folded into \"unknown id\"");
    assert_eq!(err.kind(), "missing_param");
}

#[tokio::test]
async fn dispatch_internal_call_describe_types_propagates_host_error() {
    let host = FixtureHost::failing();
    let broker = CodeModeBroker::new(Some(&host));

    let error = broker
        .call_tool_id(
            "__lab_internal::describe_types",
            json!({ "id": "github::list_tags" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect_err("a host list_tools failure must remain observable to describe()");
    assert_eq!(error.kind(), "upstream_connect_error");
    assert!(
        error.to_string().contains("simulated list_tools failure"),
        "the original lookup failure must be preserved: {error}"
    );
}

#[tokio::test]
async fn dispatch_internal_call_truncates_oversized_query_instead_of_erroring() {
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let oversized = "q".repeat(MAX_SEMANTIC_QUERY_BYTES * 4);
    let result = broker
        .call_tool_id(
            "__lab_internal::semantic_rank",
            json!({ "query": oversized, "limit": 5 }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await;
    let value = result.expect("oversized query must be truncated, not errored");
    assert_eq!(value, json!({ "ranked": [] }));
}

async fn subject_scoped_registry() -> labby_openapi::OpenApiRegistry {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("openapi.json");
    std::fs::write(
            &path,
            r#"{"openapi":"3.0.0","info":{"title":"Fixture","version":"1.0.0"},"paths":{"/me":{"get":{"operationId":"getMe","responses":{"200":{"description":"ok"}}}}}}"#,
        )
        .expect("write spec");
    let cfg = labby_openapi::OpenApiProviderConfig {
        specs: vec![labby_openapi::OpenApiSpecConfig {
            label: "vendor".into(),
            spec_source: labby_openapi::SpecSource::Path(path),
            base_url: "https://api.example.com".parse().unwrap(),
            allowed_operations: vec!["getMe".into()],
            credential: None,
            oauth_upstream: Some("vendor-oauth".into()),
        }],
    };
    let registry = labby_openapi::OpenApiRegistry::load(cfg, Duration::from_secs(2)).await;
    drop(dir);
    registry
}

#[tokio::test]
async fn subject_scoped_openapi_allows_authenticated_executor_but_not_missing_subject_or_scoped_route()
 {
    let registry = subject_scoped_registry().await;
    assert!(registry.has_subject_scoped_operations());
    let user = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some("alice".into()),
    };
    assert!(openapi_provider_allowed(
        &user,
        &ToolScope::default(),
        &registry
    ));

    let missing_subject = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: None,
    };
    assert!(!openapi_provider_allowed(
        &missing_subject,
        &ToolScope::default(),
        &registry
    ));

    let empty_subject = CodeModeCaller::Scoped {
        capabilities: CodeModeCallerCapabilities {
            can_read: true,
            can_execute: true,
            can_use_snippets: false,
            is_admin: false,
        },
        sub: Some(String::new()),
    };
    assert!(!openapi_provider_allowed(
        &empty_subject,
        &ToolScope::default(),
        &registry
    ));
    assert!(!openapi_provider_allowed(
        &user,
        &ToolScope::scoped_namespaces(vec!["vendor".into()], vec![]),
        &registry
    ));
}

#[test]
fn local_providers_require_unscoped_admin_scope() {
    assert!(local_providers_allowed(
        &CodeModeCaller::TrustedLocal,
        &ToolScope::default()
    ));
    assert!(local_providers_allowed(
        &CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: true,
                is_admin: true,
            },
            sub: Some("admin".to_string()),
        },
        &ToolScope::default()
    ));
    assert!(!local_providers_allowed(
        &CodeModeCaller::Scoped {
            capabilities: CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: false,
                is_admin: false,
            },
            sub: Some("user".to_string()),
        },
        &ToolScope::default()
    ));
    assert!(!local_providers_allowed(
        &CodeModeCaller::TrustedLocal,
        &ToolScope::scoped_namespaces(vec!["github".to_string()], vec![])
    ));
    assert!(!local_providers_allowed(
        &CodeModeCaller::TrustedLocal,
        &ToolScope::new(vec![], vec!["github::list_pull_requests".to_string()])
    ));
}
