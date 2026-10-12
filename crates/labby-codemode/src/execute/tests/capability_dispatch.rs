use super::*;

#[tokio::test]
async fn dispatch_internal_call_resource_discovery_round_trips_uri() {
    let host = FixtureHost::new(Vec::new());
    let broker = CodeModeBroker::new(Some(&host));
    let discovered = broker
        .call_tool_id(
            "__lab_internal::list_resources",
            json!({"upstream": "alpha"}),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect("list resource metadata");
    let uri = discovered["resources"][0]["uri"].clone();
    let read = broker
        .call_tool_id(
            "__lab_internal::read_resource",
            json!({"uri": uri}),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect("read using discovered URI");
    assert_eq!(read["contents"][0]["uri"], uri);
    for params in [
        json!({}),
        json!({"upstream": 42}),
        json!({"upstream": ""}),
        json!({"upstream": "x".repeat(MAX_CAPABILITY_IDENTIFIER_BYTES + 1)}),
    ] {
        assert!(
            broker
                .call_tool_id(
                    "__lab_internal::list_resources",
                    params,
                    CodeModeCaller::TrustedLocal,
                    CodeModeSurface::Cli,
                    &ToolScope::default(),
                    ExecCtx::none(),
                )
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn dispatch_internal_call_read_resource_forwards_uri() {
    let host = FixtureHost::new(Vec::new());
    let broker = CodeModeBroker::new(Some(&host));

    let result = broker
        .call_tool_id(
            "__lab_internal::read_resource",
            json!({ "uri": "lab://upstream/qa-vm-service/qa-vm-service://skill" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect("read_resource must reach the host");

    assert_eq!(
        result["contents"][0]["uri"],
        "lab://upstream/qa-vm-service/qa-vm-service://skill"
    );
}

#[tokio::test]
async fn dispatch_internal_prompt_and_skill_calls_round_trip_validated_inputs() {
    let host = FixtureHost::new(Vec::new());
    let broker = CodeModeBroker::new(Some(&host));
    let scope = ToolScope::default();

    let prompt = broker
        .call_tool_id(
            "__lab_internal::get_prompt",
            json!({
                "prompt": "prompt::alpha::review",
                "arguments": { "tone": "strict", "max_items": 3 },
            }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect("get_prompt must reach the host with validated arguments");
    assert_eq!(prompt["prompt"], "prompt::alpha::review");
    assert_eq!(
        prompt["arguments"],
        json!({ "tone": "strict", "max_items": 3 })
    );
    assert_eq!(prompt["messages"][0]["content"], "fixture prompt");

    let skills = broker
        .call_tool_id(
            "__lab_internal::list_skills",
            json!({}),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect("list_skills must reach the host");
    let uri = skills["skills"][0]["uri"]
        .as_str()
        .expect("fixture Skill URI")
        .to_string();
    assert_eq!(uri, "skill://labby/fixture");

    let skill = broker
        .call_tool_id(
            "__lab_internal::get_skill",
            json!({ "uri": uri }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect("get_skill must reach the host with the discovered URI");
    assert_eq!(skill["skill"]["uri"], "skill://labby/fixture");

    let read = broker
        .call_tool_id(
            "__lab_internal::read_skill",
            json!({ "uri": "skill://labby/fixture/SKILL.md" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect("read_skill must reach the host with the requested file URI");
    assert_eq!(read["contents"][0]["uri"], "skill://labby/fixture/SKILL.md");
    assert_eq!(read["contents"][0]["text"], "fixture body");

    let invalid_prompt = broker
        .call_tool_id(
            "__lab_internal::get_prompt",
            json!({ "prompt": "prompt::alpha::review", "arguments": [] }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect_err("non-object prompt arguments must fail before host dispatch");
    assert_eq!(invalid_prompt.kind(), "invalid_param");

    let oversized_prompt = broker
        .call_tool_id(
            "__lab_internal::get_prompt",
            json!({
                "prompt": "x".repeat(MAX_CAPABILITY_IDENTIFIER_BYTES + 1),
                "arguments": {},
            }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect_err("oversized prompt identifier must fail before host dispatch");
    assert_eq!(oversized_prompt.kind(), "invalid_param");
    assert!(
        oversized_prompt
            .to_string()
            .contains("prompt identifier exceeds max length")
    );

    let missing_skill_uri = broker
        .call_tool_id(
            "__lab_internal::get_skill",
            json!({ "uri": "   " }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect_err("blank Skill URI must fail before host dispatch");
    assert_eq!(missing_skill_uri.kind(), "missing_param");
}

#[tokio::test]
async fn dispatch_internal_call_describe_types_returns_dts_for_matching_id() {
    let github_tool = CatalogDescriptor::tool(
        "github",
        "list_tags",
        "List repository tags",
        Some(json!({"type": "object", "properties": {"owner": {"type": "string"}}})),
        None,
    );
    let expected_dts = github_tool.dts.clone();
    assert!(
        !expected_dts.is_empty(),
        "fixture tool must have a generated .dts to assert against"
    );
    let host = FixtureHost::new(vec![github_tool]);
    let broker = CodeModeBroker::new(Some(&host));

    let result = broker
        .call_tool_id(
            "__lab_internal::describe_types",
            json!({ "id": "github::list_tags" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect("describe_types must succeed for a known id");

    assert_eq!(result, json!({ "dts": expected_dts }));
}

#[tokio::test]
async fn describe_types_reuses_execution_snapshot_for_broad_and_narrow_catalogs() {
    let catalog = || {
        let mut entries = (0..512)
            .map(|index| {
                CatalogDescriptor::tool(
                    "bulk",
                    &format!("tool_{index}"),
                    "Broad catalog fixture",
                    None,
                    None,
                )
            })
            .collect::<Vec<_>>();
        entries.push(CatalogDescriptor::tool(
            "linear-notification-worker",
            "prepare_developer_handoff",
            "Prepare a developer handoff",
            Some(json!({
                "type": "object",
                "properties": { "issue": { "type": "string" } },
                "required": ["issue"]
            })),
            None,
        ));
        entries
    };

    for scope in [
        ToolScope::default(),
        ToolScope::scoped_namespaces(vec!["linear-notification-worker".to_string()], vec![]),
    ] {
        let host = FixtureHost::new(catalog());
        let broker = CodeModeBroker::new(Some(&host));

        let _proxy = broker
            .build_code_mode_proxy(&CodeModeCaller::TrustedLocal, CodeModeSurface::Cli, &scope)
            .await
            .expect("discovery proxy must build");
        assert_eq!(
            host.list_tools_call_count(),
            1,
            "proxy construction should enumerate the catalog exactly once"
        );

        let result = broker
            .call_tool_id(
                "__lab_internal::describe_types",
                json!({ "id": "linear-notification-worker::prepare_developer_handoff" }),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &scope,
                ExecCtx::none(),
            )
            .await
            .expect("describe_types must resolve from the captured execution snapshot");

        assert!(
            result["dts"]
                .as_str()
                .is_some_and(|dts| dts.contains("issue")),
            "captured snapshot must retain the target parameter declaration: {result}"
        );
        assert_eq!(
            host.list_tools_call_count(),
            1,
            "describe_types must not re-enumerate the live catalog after discovery"
        );
    }
}

#[tokio::test]
async fn dispatch_internal_call_describe_types_returns_null_for_unknown_id() {
    let host = FixtureHost::new(vec![CatalogDescriptor::tool(
        "github",
        "list_tags",
        "List repository tags",
        None,
        None,
    )]);
    let broker = CodeModeBroker::new(Some(&host));

    let result = broker
        .call_tool_id(
            "__lab_internal::describe_types",
            json!({ "id": "gateway_alpha::status_get" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
        )
        .await
        .expect("an unknown id must fail open, not error");

    assert_eq!(result, json!({ "dts": null }));
}

/// Regression test for a real cross-scope disclosure bug: `describe_types`
/// originally looked up `id` over the unfiltered render, so a tool-scoped
/// caller could fetch a sibling tool's `.dts` by calling
/// `callTool("__lab_internal::describe_types", {id})` directly — bypassing
/// `codemode.describe()`'s own already-scoped local matching entirely,
/// since `__lab_internal::*` dispatch is never subject to `scope.allows()`.
/// A tools-only `ToolScope` (`namespaces: None`) means `list_tools` applies
/// no namespace filtering at all (see `FixtureHost::list_tools`, matching
/// real production), so both tools are present in the render and only the
/// `describe_types` handler's own `discovery_entry_visible` filter can
/// exclude the out-of-scope one.
#[tokio::test]
async fn dispatch_internal_call_describe_types_excludes_out_of_scope_sibling_tool() {
    let host = FixtureHost::new(vec![
        CatalogDescriptor::tool(
            "github",
            "allowed_tool",
            "An allowed tool",
            Some(json!({"type": "object"})),
            None,
        ),
        CatalogDescriptor::tool(
            "github",
            "forbidden_tool",
            "A forbidden tool",
            Some(json!({"type": "object"})),
            None,
        ),
    ]);
    let broker = CodeModeBroker::new(Some(&host));
    let scope = ToolScope::new(vec![], vec!["github::allowed_tool".to_string()]);
    let _proxy = broker
        .build_code_mode_proxy(&CodeModeCaller::TrustedLocal, CodeModeSurface::Cli, &scope)
        .await
        .expect("scoped discovery proxy must build");
    assert_eq!(host.list_tools_call_count(), 1);

    let allowed = broker
        .call_tool_id(
            "__lab_internal::describe_types",
            json!({ "id": "github::allowed_tool" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect("in-scope tool must still resolve");
    assert_ne!(
        allowed,
        json!({ "dts": null }),
        "the in-scope tool must still return real type info"
    );

    let forbidden = broker
        .call_tool_id(
            "__lab_internal::describe_types",
            json!({ "id": "github::forbidden_tool" }),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &scope,
            ExecCtx::none(),
        )
        .await
        .expect("an out-of-scope id must fail open, not error");
    assert_eq!(
        forbidden,
        json!({ "dts": null }),
        "a tool outside scope.tools must never disclose its .dts, even though it \
             shares a namespace with an allowed tool and is present in the unfiltered render"
    );
}
