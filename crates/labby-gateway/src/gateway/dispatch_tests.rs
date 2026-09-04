#![allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use labby_runtime::gateway_config::{
    ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig, ProtectedMcpRouteTarget, UpstreamConfig,
    UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};

use super::super::discovery::DiscoveredServer;
use super::super::manager::GatewayRuntimeHandle;
use super::super::params::{GatewayDiscoverParams, GatewayEnrichmentScope};
use super::super::types::McpClientTransportType;
use super::*;

#[cfg(feature = "skills")]
#[test]
fn skills_operator_projection_preserves_candidate_count_and_rejection_detail() {
    let operator = OperatorSkills {
        discovered_count: 2,
        rejected: vec![crate::upstream::pool::OperatorSkillRejection {
            uri: "skill://labby/rejected/SKILL.md".into(),
            reason: "invalid_frontmatter".into(),
            detail: "frontmatter `allowed-tools` must be a space-separated string".into(),
        }],
        ..OperatorSkills::default()
    };

    let projection = project_operator_skills(&operator);

    assert_eq!(projection.discovered_count, 2);
    assert!(projection.skills.is_empty());
    assert_eq!(projection.rejected[0]["reason"], "invalid_frontmatter");
    assert_eq!(
        projection.rejected[0]["detail"],
        "frontmatter `allowed-tools` must be a space-separated string"
    );
}

#[cfg(feature = "skills")]
#[test]
fn skills_operator_projection_reports_identity_and_the_exposure_decision() {
    use labby_runtime::skills::{SkillDescriptor, SkillId, SkillProviderId, SkillProviderKind};

    // `identity` and `exposure` are a documented contract
    // (docs/guides/SKILLS_AND_LOADOUTS.md). An operator needs the reason to
    // tell "no pattern matched" from "the upstream never advertised it".
    let provider = SkillProviderId::new(SkillProviderKind::McpUpstream, "github");
    let descriptor = SkillDescriptor {
        id: SkillId::new(provider, "skill://github/review/SKILL.md"),
        name: "review".into(),
        description: "review a diff".into(),
        source_uri: Some("skill://github/review/SKILL.md".into()),
        resource_count: 1,
        availability: labby_runtime::skills::SkillAvailabilitySummary::available(),
        requirements: labby_runtime::skills::SkillRequirementsSummary::default(),
        provider_metadata: serde_json::Map::new(),
    };
    let operator = OperatorSkills {
        discovered_count: 1,
        skills: vec![crate::upstream::pool::OperatorSkill {
            descriptor,
            exposure: crate::upstream::pool::SkillExposureDecision {
                exposed: true,
                reason: crate::upstream::pool::SkillExposureReason::MatchedPattern,
                matched_pattern: Some("review-*".into()),
            },
        }],
        ..OperatorSkills::default()
    };

    let projection = project_operator_skills(&operator);
    let row = &projection.skills[0];

    assert_eq!(row["exposed"], true);
    assert_eq!(row["exposure"]["status"], "exposed");
    assert_eq!(row["exposure"]["reason"], "matched_pattern");
    assert_eq!(row["exposure"]["matched_pattern"], "review-*");
    assert_eq!(
        row["identity"]["source_id"],
        "skill://github/review/SKILL.md"
    );
    assert_eq!(row["identity"]["provider"]["instance"], "github");
}

#[derive(Clone)]
struct DashboardCatalogResponder {
    discover_requests: std::sync::Arc<AtomicUsize>,
    list_requests: std::sync::Arc<AtomicUsize>,
    tool_count: std::sync::Arc<AtomicUsize>,
    delay_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Default for DashboardCatalogResponder {
    fn default() -> Self {
        Self {
            discover_requests: std::sync::Arc::new(AtomicUsize::new(0)),
            list_requests: std::sync::Arc::new(AtomicUsize::new(0)),
            tool_count: std::sync::Arc::new(AtomicUsize::new(1)),
            delay_ms: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

impl Respond for DashboardCatalogResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).expect("valid JSON-RPC request");
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .expect("JSON-RPC method");
        let id = body.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "server/discover" => {
                self.discover_requests.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_millis(
                        self.delay_ms.load(Ordering::SeqCst),
                    ))
                    .set_body_json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "resultType": "complete",
                            "supportedVersions": ["2026-07-28"],
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "dashboard-test", "version": "1.0.0"},
                            "ttlMs": 0,
                            "cacheScope": "private"
                        }
                    }))
            }
            "tools/list" => {
                self.list_requests.fetch_add(1, Ordering::SeqCst);
                let count = self.tool_count.load(Ordering::SeqCst);
                let tools: Vec<Value> = (0..count)
                    .map(|index| {
                        json!({
                            "name": if index == 0 {
                                "dashboard_echo".to_string()
                            } else {
                                format!("dashboard_echo_{index}")
                            },
                            "description": "dashboard discovery proof",
                            "inputSchema": {"type": "object"}
                        })
                    })
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"tools": tools}
                }))
            }
            other => ResponseTemplate::new(500)
                .set_body_string(format!("unexpected MCP method: {other}")),
        }
    }
}

#[test]
fn gateway_actions_include_management_surface() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"gateway.list"));
    assert!(names.contains(&"gateway.server.get"));
    assert!(names.contains(&"gateway.supported_services"));
    assert!(names.contains(&"gateway.protected_route.list"));
    assert!(names.contains(&"gateway.protected_route.get"));
    assert!(names.contains(&"gateway.protected_route.add"));
    assert!(names.contains(&"gateway.protected_route.update"));
    assert!(names.contains(&"gateway.protected_route.remove"));
    assert!(names.contains(&"gateway.protected_route.test"));
    assert!(names.contains(&"gateway.virtual_server.enable"));
    assert!(names.contains(&"gateway.virtual_server.disable"));
    assert!(names.contains(&"gateway.virtual_server.remove"));
    assert!(names.contains(&"gateway.virtual_server.quarantine.list"));
    assert!(names.contains(&"gateway.virtual_server.quarantine.restore"));
    assert!(names.contains(&"gateway.virtual_server.set_surface"));
    assert!(names.contains(&"gateway.virtual_server.get_mcp_policy"));
    assert!(names.contains(&"gateway.virtual_server.set_mcp_policy"));
    assert!(names.contains(&"gateway.service_config.get"));
    assert!(names.contains(&"gateway.service_config.set"));
    assert!(names.contains(&"gateway.service_actions"));
    assert!(names.contains(&"gateway.get"));
    assert!(names.contains(&"gateway.test"));
    assert!(names.contains(&"gateway.add"));
    assert!(names.contains(&"gateway.update"));
    assert!(names.contains(&"gateway.remove"));
    assert!(names.contains(&"gateway.reload"));
    assert!(names.contains(&"gateway.status"));
    assert!(names.contains(&"gateway.client_config.get"));
    assert!(names.contains(&"gateway.discovered_tools"));
    assert!(names.contains(&"gateway.discovered_resources"));
    assert!(names.contains(&"gateway.discovered_prompts"));
    assert!(names.contains(&"gateway.enrich.preview"));
    assert!(names.contains(&"gateway.enrich.apply"));
    assert!(names.contains(&"gateway.oauth.probe"));
    assert!(names.contains(&"gateway.oauth.start"));
    assert!(names.contains(&"gateway.oauth.status"));
    assert!(names.contains(&"gateway.oauth.clear"));
    assert!(names.contains(&"gateway.oauth.google_revoke"));
    assert!(names.contains(&"gateway.oauth.resource_lease.create"));
    assert!(names.contains(&"gateway.oauth.resource_lease.renew"));
    assert!(names.contains(&"gateway.oauth.resource_lease.release"));
    assert!(names.contains(&"gateway.mcp.enable"));
    assert!(names.contains(&"gateway.mcp.disable"));
    assert!(names.contains(&"gateway.mcp.restart"));
    assert!(names.contains(&"gateway.mcp.cleanup"));
    assert!(names.contains(&"gateway.public_urls.get"));

    for spec in ACTIONS {
        if matches!(
            spec.name,
            "gateway.code_mode.set"
                | "gateway.enrich.preview"
                | "gateway.enrich.apply"
                | "gateway.import"
                | "gateway.import_pending.approve"
                | "gateway.import_pending.reject"
                | "gateway.import_tombstones.clear"
                | "gateway.import_tombstones.restore"
                | "gateway.remove"
                | "gateway.mcp.cleanup"
                | "gateway.oauth.google_revoke"
                | "gateway.oauth.resource_lease.release"
        ) {
            continue;
        }
        assert!(
            !spec.destructive,
            "{} must not be destructive unless it risks permanent, hard-to-recreate data loss",
            spec.name
        );
    }
}

#[test]
fn resource_lease_actions_require_admin_scope() {
    for name in [
        "gateway.oauth.resource_lease.create",
        "gateway.oauth.resource_lease.renew",
        "gateway.oauth.resource_lease.release",
    ] {
        let spec = ACTIONS.iter().find(|spec| spec.name == name).unwrap();
        assert!(spec.requires_admin, "{name} must require lab:admin");
    }
}

#[tokio::test]
async fn resource_lease_actions_dispatch_typed_round_trip() {
    let registry = labby_auth::resource_registry::ResourceRegistry::new();
    let manager = test_manager().with_resource_registry(registry.clone());
    let created = dispatch_with_manager(
        &manager,
        "gateway.oauth.resource_lease.create",
        json!({
            "resource": "https://proxy.example:53147/mcp",
            "scopes": ["mcp:read", "mcp:write"],
            "ttl_secs": 120,
            "owner": "live-gateway-test"
        }),
    )
    .await
    .unwrap();
    let lease: labby_auth::resource_registry::ResourceLease =
        serde_json::from_value(created).unwrap();
    assert_eq!(lease.resource, "https://proxy.example:53147/mcp");
    assert_eq!(lease.scopes, vec!["mcp:read", "mcp:write"]);

    let renewed = dispatch_with_manager(
        &manager,
        "gateway.oauth.resource_lease.renew",
        json!({"id": lease.id, "ttl_secs": 240}),
    )
    .await
    .unwrap();
    let renewed: labby_auth::resource_registry::ResourceLease =
        serde_json::from_value(renewed).unwrap();
    assert!(renewed.expires_at_unix > lease.expires_at_unix);

    let released = dispatch_with_manager(
        &manager,
        "gateway.oauth.resource_lease.release",
        json!({"id": renewed.id}),
    )
    .await
    .unwrap();
    let released: super::super::types::ResourceLeaseReleaseView =
        serde_json::from_value(released).unwrap();
    assert!(released.released);
    assert_eq!(registry.lease_count(), 0);
}

#[tokio::test]
async fn resource_lease_unknown_and_released_ids_fail_clearly() {
    let manager = test_manager()
        .with_resource_registry(labby_auth::resource_registry::ResourceRegistry::new());
    for action in [
        "gateway.oauth.resource_lease.renew",
        "gateway.oauth.resource_lease.release",
    ] {
        let error = dispatch_with_manager(
            &manager,
            action,
            json!({"id": "unknown-opaque-id", "ttl_secs": 60}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "not_found");
    }
}

#[test]
fn manager_clones_share_registry_and_restart_does_not_restore_leases() {
    let registry = labby_auth::resource_registry::ResourceRegistry::new();
    let manager = test_manager().with_resource_registry(registry.clone());
    let clone = manager.clone();
    manager
        .resource_registry()
        .unwrap()
        .create_resource_lease(
            "https://proxy.example:53147/mcp",
            ["mcp:read"],
            std::time::Duration::from_mins(1),
            "restart-test",
        )
        .unwrap();
    assert_eq!(clone.resource_registry().unwrap().lease_count(), 1);

    let restarted = test_manager()
        .with_resource_registry(labby_auth::resource_registry::ResourceRegistry::new());
    assert_eq!(restarted.resource_registry().unwrap().lease_count(), 0);
    restarted
        .resource_registry()
        .unwrap()
        .create_resource_lease(
            "https://proxy.example:53147/mcp",
            ["mcp:read"],
            std::time::Duration::from_mins(1),
            "restart-test",
        )
        .unwrap();
    assert_eq!(restarted.resource_registry().unwrap().lease_count(), 1);
}

/// Import actions mutate gateway import state, but `destructive` does not mean
/// "mutates state" — it means unrecoverable data loss. Every import mutation
/// has an undo path: imports land disabled-by-default, a rejection writes a
/// tombstone, and `import_tombstones.clear` reverses the tombstone. They are
/// admin-gated via `requires_admin`, not confirmation-gated.
#[test]
fn import_mutations_are_not_destructive() {
    for name in [
        "gateway.import",
        "gateway.import_pending.approve",
        "gateway.import_pending.reject",
        "gateway.import_tombstones.clear",
        "gateway.import_tombstones.restore",
    ] {
        let spec = ACTIONS
            .iter()
            .find(|spec| spec.name == name)
            .unwrap_or_else(|| panic!("{name} action"));
        assert!(
            !spec.destructive,
            "{name} mutates import state but loses no unrecoverable data"
        );
    }
}

/// `gateway.enrich.preview` really does spawn a local provider subprocess
/// (`claude`/`codex`, see `enrichment/provider.rs`). Spawning a process is not
/// destructive under Lab's data-loss definition — the same reasoning that keeps
/// `gateway.test` non-destructive despite spawning stdio upstreams. The gate
/// for local execution is `requires_admin` plus the spawn allowlist.
#[test]
fn enrich_preview_spawns_providers_but_is_not_destructive() {
    let spec = ACTIONS
        .iter()
        .find(|spec| spec.name == "gateway.enrich.preview")
        .expect("gateway.enrich.preview action");

    assert!(!spec.destructive);
    assert!(spec.requires_admin);
}

/// `gateway.remove` deletes an operator-authored upstream config entry with no
/// undo path, so it clears the data-loss bar. `gateway.mcp.cleanup` only kills
/// runtime processes, which the definition explicitly calls out as recoverable.
#[test]
fn gateway_remove_is_destructive_but_runtime_cleanup_is_not() {
    let remove = ACTIONS
        .iter()
        .find(|spec| spec.name == "gateway.remove")
        .expect("gateway.remove action");
    assert!(remove.destructive, "gateway.remove deletes config");

    let cleanup = ACTIONS
        .iter()
        .find(|spec| spec.name == "gateway.mcp.cleanup")
        .expect("gateway.mcp.cleanup action");
    assert!(
        !cleanup.destructive,
        "killing runtime processes is recoverable"
    );
}

#[tokio::test]
async fn enrich_preview_dispatch_defaults_to_deterministic_provider() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.enrich.preview",
        json!({"upstreams": ["github"]}),
    )
    .await
    .expect("preview dispatch");

    assert_eq!(value["provider"], json!("deterministic"));
    assert_eq!(value["proposals"][0]["upstream"], json!("github"));
}

#[tokio::test]
async fn enrich_preview_dispatch_rejects_empty_selection() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.enrich.preview", json!({}))
        .await
        .expect_err("empty selection must fail");

    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn shared_gateway_oauth_actions_reject_subject_overrides_without_echoing_them() {
    let manager = test_manager();

    for action in [
        "gateway.oauth.start",
        "gateway.oauth.status",
        "gateway.oauth.clear",
        "gateway.oauth.wait",
    ] {
        let error = dispatch_with_manager(
            &manager,
            action,
            json!({
                "upstream": "example",
                "subject": "private-subject-marker",
            }),
        )
        .await
        .expect_err("shared OAuth actions must reject subject overrides before execution");

        assert_eq!(error.kind(), "invalid_param", "{action}");
        assert!(
            !error.to_string().contains("private-subject-marker"),
            "{action}"
        );
    }
}

#[tokio::test]
async fn enrich_apply_dispatch_persists_hint() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let preview = dispatch_with_manager(
        &manager,
        "gateway.enrich.preview",
        json!({"upstreams": ["github"]}),
    )
    .await
    .expect("preview");
    let hash = preview["proposals"][0]["metadata_hash"]
        .as_str()
        .expect("hash")
        .to_string();

    let applied = dispatch_with_manager(
        &manager,
        "gateway.enrich.apply",
        json!({
            "upstream": "github",
            "hint": "search repositories",
            "metadata_hash": hash,
        }),
    )
    .await
    .expect("apply");

    assert_eq!(applied["hint"], json!("search repositories"));
    assert_eq!(
        manager.current_config().await.upstream[0]
            .code_mode_hint
            .as_deref(),
        Some("search repositories")
    );
}

#[tokio::test]
async fn gateway_usage_metrics_returns_zeroed_view_with_no_calls() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let usage_store = std::sync::Arc::new(
        crate::usage::UsageStore::open(tempfile::tempdir().unwrap().path().join("usage.db"))
            .await
            .unwrap(),
    );
    let manager = manager.with_usage_store(usage_store);

    let result = dispatch_with_manager(&manager, "gateway.usage.metrics", json!({}))
        .await
        .expect("dispatch succeeds");
    assert_eq!(result["window_total_calls"], 0);
    assert_eq!(result["total_calls"], 0);
    assert_eq!(result["error_calls"], 0);
    assert_eq!(result["p95_elapsed_ms"], 0);
    assert_eq!(result["distinct_tools"], 0);
    assert_eq!(result["distinct_actors"], 0);
    assert_eq!(result["timeseries"], json!([]));
    assert_eq!(result["facets"]["tools"], json!([]));
}

#[tokio::test]
async fn gateway_usage_metrics_and_calls_expose_exact_filtered_contract() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let usage_store = std::sync::Arc::new(
        crate::usage::UsageStore::open(tempfile::tempdir().unwrap().path().join("usage.db"))
            .await
            .unwrap(),
    );
    for (ts_unix, actor, outcome) in [(1_000, "alice", "ok"), (1_100, "bob", "timeout")] {
        usage_store
            .record_call(crate::usage::UpstreamCallRecord {
                ts_unix,
                upstream_name: "github".to_string(),
                tool_name: "search_repos".to_string(),
                capability: "tools".to_string(),
                operation: "tool.call".to_string(),
                subject_scoped: false,
                actor: actor.to_string(),
                outcome: outcome.to_string(),
                elapsed_ms: if outcome == "ok" { 10 } else { 50 },
                response_bytes: Some(128),
            })
            .await
            .unwrap();
    }
    let manager = manager.with_usage_store(usage_store);

    let metrics = dispatch_with_manager(
        &manager,
        "gateway.usage.metrics",
        json!({
            "since_unix": 0,
            "until_unix": 1200,
            "tool": "github::search_repos",
            "capability": "tools",
            "operation": "tool.call",
            "subject_scoped": false,
            "actor": "bob",
            "outcome": "failed",
            "search": "timeout",
            "bucket_count": 2,
            "timezone_offset_minutes": -240,
            "include_facets": true
        }),
    )
    .await
    .expect("filtered metrics dispatch succeeds");

    assert_eq!(metrics["window_total_calls"], 2);
    assert_eq!(metrics["total_calls"], 1);
    assert_eq!(metrics["error_calls"], 1);
    assert_eq!(metrics["p50_elapsed_ms"], 50);
    assert_eq!(metrics["p95_elapsed_ms"], 50);
    assert_eq!(metrics["timeseries"].as_array().map(Vec::len), Some(2));
    assert_eq!(metrics["facets"]["actors"], json!(["alice", "bob"]));
    assert_eq!(metrics["facets"]["upstreams"], json!(["github"]));
    assert_eq!(metrics["facets"]["capabilities"], json!(["tools"]));
    assert_eq!(metrics["facets"]["operations"], json!(["tool.call"]));
    assert_eq!(metrics["facets"]["subject_scopes"], json!([false]));
    assert_eq!(metrics["slowest_tools"][0]["capability"], "tools");
    assert_eq!(metrics["slowest_tools"][0]["operation"], "tool.call");
    assert_eq!(metrics["slowest_tools"][0]["subject_scoped"], false);

    let calls = dispatch_with_manager(
        &manager,
        "gateway.usage.calls",
        json!({
            "tool": "github::search_repos",
            "capability": "tools",
            "operation": "tool.call",
            "subject_scoped": false,
            "actor": "bob",
            "outcome": "failed",
            "search": "timeout",
            "limit": 50,
            "include_total": true
        }),
    )
    .await
    .expect("filtered calls dispatch succeeds");
    assert_eq!(calls["total_matching"], 1);
    assert_eq!(calls["calls"].as_array().map(Vec::len), Some(1));
    assert_eq!(calls["calls"][0]["outcome"], "timeout");
}

#[tokio::test]
async fn gateway_usage_metrics_fails_closed_when_store_not_wired() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let error = dispatch_with_manager(&manager, "gateway.usage.metrics", json!({}))
        .await
        .expect_err("no usage store wired must fail, not silently return empty data");
    assert_eq!(error.kind(), "usage_store_unavailable");
}

#[tokio::test]
async fn gateway_usage_metrics_rejects_route_hidden_explicit_upstream() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let usage_store = std::sync::Arc::new(
        crate::usage::UsageStore::open(tempfile::tempdir().unwrap().path().join("usage.db"))
            .await
            .unwrap(),
    );
    let manager = manager.with_usage_store(usage_store);

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.usage.metrics",
        json!({"upstream": "github"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from([
                "gateway-alpha".to_string()
            ])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("route-hidden upstream must fail");

    assert_eq!(error.kind(), "unknown_upstream");
}

#[tokio::test]
async fn gateway_usage_calls_rejects_route_hidden_explicit_upstream() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let usage_store = std::sync::Arc::new(
        crate::usage::UsageStore::open(tempfile::tempdir().unwrap().path().join("usage.db"))
            .await
            .unwrap(),
    );
    let manager = manager.with_usage_store(usage_store);

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.usage.calls",
        json!({"upstream": "github"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from([
                "gateway-alpha".to_string()
            ])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("route-hidden upstream must fail");

    assert_eq!(error.kind(), "unknown_upstream");
}

#[tokio::test]
async fn gateway_status_rejects_route_hidden_explicit_upstream() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.status",
        json!({"name": "github"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from([
                "gateway-alpha".to_string()
            ])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("route-hidden upstream must fail");

    assert_eq!(error.kind(), "unknown_upstream");
}

#[tokio::test]
async fn gateway_list_only_returns_route_visible_upstreams() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            upstream_fixture(
                "github",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
            upstream_fixture(
                "gateway-alpha",
                Some("https://example2.invalid/mcp".to_string()),
                None,
            ),
        ])
        .await;

    let result = dispatch_with_manager_scoped(
        &manager,
        "gateway.list",
        json!({}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
            oauth_subject: None,
        },
    )
    .await
    .expect("scoped list succeeds");

    let rows = result.as_array().expect("gateway rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], json!("github"));
}

#[tokio::test]
async fn gateway_get_rejects_route_hidden_upstream() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.get",
        json!({"name": "github"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from([
                "gateway-alpha".to_string()
            ])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("route-hidden upstream must fail");

    assert_eq!(error.kind(), "unknown_upstream");
}

#[tokio::test]
async fn named_gateway_actions_reject_route_hidden_upstreams_before_dispatch() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;
    let scope = GatewayEnrichmentScope {
        route_visible_upstreams: Some(std::collections::BTreeSet::from([
            "gateway-alpha".to_string()
        ])),
        oauth_subject: None,
    };

    for (action, params) in [
        ("gateway.server.get", json!({"id": "github"})),
        ("gateway.test", json!({"name": "github"})),
        ("gateway.update", json!({"name": "github", "patch": {}})),
        ("gateway.remove", json!({"name": "github"})),
        ("gateway.client_config.get", json!({"name": "github"})),
        ("gateway.discovered_tools", json!({"name": "github"})),
        ("gateway.discovered_resources", json!({"name": "github"})),
        ("gateway.discovered_prompts", json!({"name": "github"})),
        ("gateway.mcp.enable", json!({"name": "github"})),
        ("gateway.mcp.disable", json!({"name": "github"})),
        ("gateway.mcp.restart", json!({"name": "github"})),
        ("gateway.mcp.cleanup", json!({"name": "github"})),
        ("gateway.oauth.start", json!({"upstream": "github"})),
        ("gateway.oauth.status", json!({"upstream": "github"})),
        ("gateway.oauth.clear", json!({"upstream": "github"})),
        (
            "gateway.oauth.google_revoke",
            json!({"upstream": "github", "confirm": true}),
        ),
        ("gateway.oauth.wait", json!({"upstream": "github"})),
        ("gateway.import_pending.reject", json!({"name": "github"})),
        ("gateway.import_tombstones.clear", json!({"name": "github"})),
        (
            "gateway.import_tombstones.restore",
            json!({"name": "github"}),
        ),
    ] {
        let error = dispatch_with_manager_scoped(&manager, action, params, scope.clone())
            .await
            .expect_err("route-hidden upstream must fail before action dispatch");
        assert_eq!(error.kind(), "unknown_upstream", "action: {action}");
    }
}

#[tokio::test]
async fn gateway_skills_list_is_restricted_to_route_visible_upstreams() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            upstream_fixture(
                "github",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
            upstream_fixture(
                "secret",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
        ])
        .await;
    let scope = GatewayEnrichmentScope {
        route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
        oauth_subject: None,
    };

    // A hidden upstream must be indistinguishable from an absent one: the
    // `not_found` probe would otherwise confirm that `secret` is configured.
    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.skills.list",
        json!({"upstream": "secret"}),
        scope.clone(),
    )
    .await
    .expect_err("route-hidden upstream must not be listable");
    assert_eq!(error.kind(), "unknown_upstream");

    let absent = dispatch_with_manager_scoped(
        &manager,
        "gateway.skills.list",
        json!({"upstream": "does-not-exist"}),
        scope,
    )
    .await
    .expect_err("absent upstream must not be listable either");
    assert_eq!(
        absent.kind(),
        "unknown_upstream",
        "hidden and absent upstreams must report the same kind"
    );
}

#[cfg(not(feature = "skills"))]
#[tokio::test]
async fn gateway_skills_list_reports_missing_feature_after_route_scope_accepts_upstream() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.skills.list",
        json!({"upstream": "github"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("a visible upstream still requires the compiled Skills feature");

    assert_eq!(error.kind(), "feature_not_compiled");
}

#[tokio::test]
async fn protected_route_rejects_unsaved_gateway_test_spec() {
    let manager = test_manager();
    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.test",
        json!({"spec": {"name": "candidate", "url": "https://example.invalid/mcp"}}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("protected subset must not execute an unsaved gateway spec");

    assert_eq!(error.kind(), "forbidden");
}

#[tokio::test]
async fn import_admin_lists_filter_to_route_visible_names_without_affecting_root() {
    let manager = test_manager();
    let source = |name: &str| {
        labby_runtime::gateway_config::ImportSource::new(
            "test-client",
            format!("/tmp/{name}.json"),
            "2026-08-27T00:00:00Z",
        )
        .with_server_name(name)
    };
    let mut visible_pending = upstream_fixture(
        "visible",
        Some("https://visible.invalid/mcp".to_string()),
        None,
    );
    visible_pending.imported_from = Some(source("visible"));
    let mut hidden_pending = upstream_fixture(
        "hidden",
        Some("https://hidden.invalid/mcp".to_string()),
        None,
    );
    hidden_pending.imported_from = Some(source("hidden"));
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            upstream_pending: vec![visible_pending, hidden_pending],
            upstream_import_tombstones: vec![
                labby_runtime::gateway_config::UpstreamImportTombstone::now(
                    "visible",
                    source("visible"),
                ),
                labby_runtime::gateway_config::UpstreamImportTombstone::now(
                    "hidden",
                    source("hidden"),
                ),
            ],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;
    let scoped = GatewayEnrichmentScope {
        route_visible_upstreams: Some(std::collections::BTreeSet::from(["visible".to_string()])),
        oauth_subject: None,
    };

    for action in [
        "gateway.import_pending.list",
        "gateway.import_tombstones.list",
    ] {
        let filtered = dispatch_with_manager_scoped(&manager, action, json!({}), scoped.clone())
            .await
            .expect("scoped import administration list succeeds");
        let rows = filtered.as_array().expect("filtered import rows");
        assert_eq!(rows.len(), 1, "action: {action}");
        assert_eq!(rows[0]["name"], json!("visible"), "action: {action}");

        let unscoped = dispatch_with_manager(&manager, action, json!({}))
            .await
            .expect("root import administration list succeeds");
        assert_eq!(
            unscoped.as_array().expect("unscoped import rows").len(),
            2,
            "action: {action}"
        );
    }
}

#[tokio::test]
async fn gateway_status_aggregate_only_returns_route_visible_upstreams() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            upstream_fixture(
                "github",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
            upstream_fixture(
                "gateway-alpha",
                Some("https://example2.invalid/mcp".to_string()),
                None,
            ),
        ])
        .await;

    let result = dispatch_with_manager_scoped(
        &manager,
        "gateway.status",
        json!({}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
            oauth_subject: None,
        },
    )
    .await
    .expect("scoped status succeeds");

    let rows = result.as_array().expect("status rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], json!("github"));
}

#[tokio::test]
async fn gateway_mcp_list_can_return_one_targeted_snapshot() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            upstream_fixture(
                "github",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
            upstream_fixture(
                "gateway-alpha",
                Some("https://example2.invalid/mcp".to_string()),
                None,
            ),
        ])
        .await;

    let result = dispatch_with_manager(
        &manager,
        "gateway.mcp.list",
        json!({"name": "gateway-alpha"}),
    )
    .await
    .expect("targeted runtime snapshot succeeds");

    let rows = result.as_array().expect("runtime rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], json!("gateway-alpha"));
}

#[tokio::test]
async fn gateway_mcp_list_rejects_route_hidden_explicit_upstream() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "github",
            Some("https://example.invalid/mcp".to_string()),
            None,
        )])
        .await;

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.mcp.list",
        json!({"name": "github"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from([
                "gateway-alpha".to_string()
            ])),
            oauth_subject: None,
        },
    )
    .await
    .expect_err("route-hidden upstream must fail");

    assert_eq!(error.kind(), "unknown_upstream");
}

#[tokio::test]
async fn gateway_mcp_list_aggregate_only_returns_route_visible_upstreams() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            upstream_fixture(
                "github",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
            upstream_fixture(
                "gateway-alpha",
                Some("https://example2.invalid/mcp".to_string()),
                None,
            ),
        ])
        .await;

    let result = dispatch_with_manager_scoped(
        &manager,
        "gateway.mcp.list",
        json!({}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
            oauth_subject: None,
        },
    )
    .await
    .expect("scoped runtime list succeeds");

    let rows = result.as_array().expect("runtime rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], json!("github"));
}

#[tokio::test]
async fn gateway_usage_metrics_scoped_aggregate_restricts_to_visible_upstreams() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            upstream_fixture(
                "github",
                Some("https://example.invalid/mcp".to_string()),
                None,
            ),
            upstream_fixture(
                "gateway-alpha",
                Some("https://example2.invalid/mcp".to_string()),
                None,
            ),
        ])
        .await;
    let usage_store = std::sync::Arc::new(
        crate::usage::UsageStore::open(tempfile::tempdir().unwrap().path().join("usage.db"))
            .await
            .unwrap(),
    );
    usage_store
        .record_call(crate::usage::UpstreamCallRecord {
            ts_unix: 1_000,
            upstream_name: "github".to_string(),
            tool_name: "search_repos".to_string(),
            capability: "tools".to_string(),
            operation: "tool.call".to_string(),
            subject_scoped: false,
            actor: "unattributed".to_string(),
            outcome: "ok".to_string(),
            elapsed_ms: 10,
            response_bytes: Some(128),
        })
        .await
        .unwrap();
    usage_store
        .record_call(crate::usage::UpstreamCallRecord {
            ts_unix: 1_001,
            upstream_name: "gateway-alpha".to_string(),
            tool_name: "status_get".to_string(),
            capability: "tools".to_string(),
            operation: "tool.call".to_string(),
            subject_scoped: false,
            actor: "unattributed".to_string(),
            outcome: "ok".to_string(),
            elapsed_ms: 10,
            response_bytes: Some(64),
        })
        .await
        .unwrap();
    let manager = manager.with_usage_store(usage_store);

    // No explicit `upstream` filter: aggregate query must still be scoped to
    // the route-visible set, not the whole store.
    let result = dispatch_with_manager_scoped(
        &manager,
        "gateway.usage.metrics",
        json!({}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from(["github".to_string()])),
            oauth_subject: None,
        },
    )
    .await
    .expect("scoped aggregate dispatch succeeds");

    assert_eq!(result["total_calls"], 1);
    assert_eq!(result["top_tools"][0]["upstream"], json!("github"));
}

#[test]
fn gateway_actions_include_servers_and_schema() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(
        names.contains(&"gateway.servers"),
        "missing gateway.servers; have {names:?}"
    );
    assert!(
        names.contains(&"gateway.schema"),
        "missing gateway.schema; have {names:?}"
    );
}

/// Test stub registry that knows a single `deploy` service with a small action
/// catalog. The host's real default-registry builder lives in `lab`, not
/// `labby-gateway`; gateway dispatch tests that exercise service-aware behavior
/// (`gateway.service_actions`, virtual-server enable/policy validation) inject
/// this so `service_meta`/`service_actions`/`contains_service` resolve `deploy`.
struct DeployTestRegistry;

static DEPLOY_TEST_META: labby_primitives::plugin::PluginMeta =
    labby_primitives::plugin::PluginMeta {
        name: "deploy",
        display_name: "Deploy",
        description: "deploy (test stub)",
        category: labby_primitives::plugin::Category::Bootstrap,
        docs_url: "",
        required_env: &[],
        optional_env: &[],
        default_port: None,
        supports_multi_instance: false,
    };

static FIXTURE_TEST_REQUIRED_ENV: &[labby_primitives::plugin::EnvVar] = &[
    labby_primitives::plugin::EnvVar {
        name: "FIXTURE_URL",
        description: "Fixture service URL",
        example: "http://127.0.0.1:9999",
        secret: false,
        ui: None,
    },
    labby_primitives::plugin::EnvVar {
        name: "FIXTURE_TOKEN",
        description: "Fixture service token",
        example: "secret",
        secret: true,
        ui: None,
    },
];

static FIXTURE_TEST_META: labby_primitives::plugin::PluginMeta =
    labby_primitives::plugin::PluginMeta {
        name: "fixture-service",
        display_name: "Fixture Service",
        description: "test-only metadata-backed service",
        category: labby_primitives::plugin::Category::Bootstrap,
        docs_url: "",
        required_env: FIXTURE_TEST_REQUIRED_ENV,
        optional_env: &[],
        default_port: Some(9999),
        supports_multi_instance: false,
    };

impl crate::registry::InProcessServiceRegistry for DeployTestRegistry {
    fn in_process_services(&self) -> Vec<Box<dyn crate::registry::InProcessService>> {
        Vec::new()
    }
}

impl crate::gateway::service_registry::GatewayServiceRegistry for DeployTestRegistry {
    fn service_names(&self) -> Vec<&'static str> {
        vec!["deploy", "fixture-service"]
    }

    fn contains_service(&self, name: &str) -> bool {
        matches!(name, "deploy" | "fixture-service")
    }

    fn service_actions(
        &self,
        name: &str,
    ) -> Option<Vec<crate::gateway::service_registry::ServiceActionInfo>> {
        (name == "deploy").then(|| {
            vec![
                crate::gateway::service_registry::ServiceActionInfo {
                    name: "deploy.plan",
                    description: "Plan a deployment",
                    destructive: false,
                    requires_admin: false,
                },
                crate::gateway::service_registry::ServiceActionInfo {
                    name: "deploy.apply",
                    description: "Apply a deployment",
                    destructive: true,
                    requires_admin: true,
                },
            ]
        })
    }

    fn service_meta(&self, name: &str) -> Option<&'static labby_primitives::plugin::PluginMeta> {
        match name {
            "deploy" => Some(&DEPLOY_TEST_META),
            "fixture-service" => Some(&FIXTURE_TEST_META),
            _ => None,
        }
    }
}

fn test_manager() -> GatewayManager {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    GatewayManager::new(path, GatewayRuntimeHandle::default())
        .with_builtin_service_registry(std::sync::Arc::new(DeployTestRegistry))
}

fn oauth_upstream_fixture(name: &str, enabled: bool) -> UpstreamConfig {
    UpstreamConfig {
        enabled,
        name: name.to_string(),
        url: Some("http://127.0.0.1:1/mcp".to_string()),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: Vec::new(),
        env: Default::default(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration: UpstreamOauthRegistration::Preregistered {
                client_id: "test-client".to_string(),
                client_secret_env: None,
            },
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        }),
        imported_from: None,
        priority: 1.0,
    }
}

#[tokio::test]
async fn gateway_code_mode_set_accepts_all_public_config_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let value = dispatch_with_manager(
        &manager,
        "gateway.code_mode.set",
        json!({
            "enabled": true,
            "mcp_ui_enabled": false,
            "trace_params": false,
            "result_shape_policy": "truncate",
            "timeout_ms": 5000,
            "max_response_bytes": 4096,
            "max_response_tokens": 1024,
            "token_estimate_divisor": 2,
            "max_log_entries": 10,
            "max_log_bytes": 2048
        }),
    )
    .await
    .expect("code mode config should update");

    assert_eq!(value["enabled"], true);
    assert_eq!(value["mcp_ui_enabled"], false);
    assert_eq!(value["trace_params"], false);
    assert_eq!(value["result_shape_policy"], "truncate");
    assert_eq!(value["timeout_ms"], 5000);
    assert_eq!(value["max_response_bytes"], 4096);
    assert_eq!(value["max_response_tokens"], 1024);
    assert_eq!(value["token_estimate_divisor"], 2);
    assert_eq!(value["max_log_entries"], 10);
    assert_eq!(value["max_log_bytes"], 2048);
}

#[tokio::test]
async fn gateway_code_mode_set_rejects_invalid_result_shape_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let err = dispatch_with_manager(
        &manager,
        "gateway.code_mode.set",
        json!({ "result_shape_policy": "redact" }),
    )
    .await
    .expect_err("invalid code mode shape policy should be rejected");
    let body = serde_json::to_value(&err).expect("serialize");

    assert_eq!(body["kind"], "invalid_param");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("redact")
    );
}

#[tokio::test]
async fn gateway_code_mode_set_rejects_invalid_public_config_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let err = dispatch_with_manager(
        &manager,
        "gateway.code_mode.set",
        json!({ "token_estimate_divisor": 0 }),
    )
    .await
    .expect_err("invalid code mode config should be rejected");
    let body = serde_json::to_value(&err).expect("serialize");

    assert_eq!(body["kind"], "invalid_param");
    assert!(
        body["message"]
            .as_str()
            .expect("message")
            .contains("token_estimate_divisor")
    );
}

#[tokio::test]
async fn gateway_public_urls_get_dispatches_from_catalog_action() {
    let manager = test_manager();
    let value = dispatch_with_manager(&manager, "gateway.public_urls.get", json!({}))
        .await
        .expect("public urls dispatches");

    assert!(value.get("effective_mcp_gateway").is_some());
}

#[tokio::test]
async fn gateway_servers_action_returns_not_found_when_no_pool() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.servers", json!({}))
        .await
        .expect_err("no pool configured");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(
        body["kind"], "not_found",
        "sdk_kind must be promoted to kind"
    );
}

#[tokio::test]
async fn gateway_schema_missing_name_returns_missing_param() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.schema", json!({}))
        .await
        .expect_err("missing name");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(body["kind"], "missing_param");
    assert_eq!(body["param"], "name");
}

#[tokio::test]
async fn gateway_schema_unknown_upstream_returns_not_found_envelope() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.schema", json!({"name": "nope"}))
        .await
        .expect_err("no pool configured");
    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(
        body["kind"], "not_found",
        "sdk_kind must be promoted to kind"
    );
}

#[tokio::test]
async fn gateway_schema_known_oauth_upstream_without_subject_fails_auth_instead_of_falling_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .replace_config_for_tests(vec![oauth_upstream_fixture("linear", true)])
        .await;
    runtime
        .swap(Some(std::sync::Arc::new(
            crate::upstream::pool::UpstreamPool::new(),
        )))
        .await;

    let error = dispatch_with_manager(&manager, "gateway.schema", json!({"name": "linear"}))
        .await
        .expect_err("known OAuth upstream requires a verified subject");

    assert_eq!(error.kind(), "auth_failed");
}

#[tokio::test]
async fn gateway_schema_route_scope_rejects_hidden_oauth_upstream_before_connecting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .replace_config_for_tests(vec![oauth_upstream_fixture("linear", true)])
        .await;
    runtime
        .swap(Some(std::sync::Arc::new(
            crate::upstream::pool::UpstreamPool::new(),
        )))
        .await;

    let error = dispatch_with_manager_scoped(
        &manager,
        "gateway.schema",
        json!({"name": "linear"}),
        GatewayEnrichmentScope {
            route_visible_upstreams: Some(std::collections::BTreeSet::from([
                "featureos".to_string()
            ])),
            oauth_subject: Some("alice".to_string()),
        },
    )
    .await
    .expect_err("route-hidden upstream must fail before outbound discovery");

    assert_eq!(error.kind(), "unknown_upstream");
}

#[tokio::test]
async fn disabled_oauth_upstream_is_not_eligible_for_subject_discovery() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![oauth_upstream_fixture("linear", false)])
        .await;

    assert!(manager.oauth_upstream_config("linear").await.is_none());
    assert!(manager.oauth_upstream_configs().await.is_empty());
}

#[tokio::test]
async fn non_positive_priority_oauth_upstream_is_not_eligible_for_subject_discovery() {
    let manager = test_manager();
    let mut upstream = oauth_upstream_fixture("linear", true);
    upstream.priority = 0.0;
    manager.replace_config_for_tests(vec![upstream]).await;

    assert!(manager.oauth_upstream_config("linear").await.is_none());
    assert!(manager.oauth_upstream_configs().await.is_empty());
}

#[tokio::test]
async fn gateway_servers_marks_oauth_catalog_rows_as_request_scoped_not_healthy_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .replace_config_for_tests(vec![oauth_upstream_fixture("linear", true)])
        .await;
    let pool = crate::upstream::pool::UpstreamPool::new();
    let name: std::sync::Arc<str> = std::sync::Arc::from("linear");
    pool.insert_entry_for_tests(
        "linear",
        crate::upstream::types::UpstreamEntry {
            name,
            tools: Default::default(),
            exposure_policy: crate::upstream::types::ToolExposurePolicy::All,
            resource_exposure_policy: crate::upstream::types::ToolExposurePolicy::All,
            prompt_exposure_policy: crate::upstream::types::ToolExposurePolicy::All,
            skill_exposure_policy: crate::upstream::types::SkillExposurePolicy::all(),
            proxy_skills: false,
            supports_skills: None,
            proxy_resources: false,
            prompt_count: 0,
            resource_count: 0,
            skill_count: 0,
            skill_names: Vec::new(),
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::upstream::types::UpstreamHealth::Healthy,
            prompt_health: crate::upstream::types::UpstreamHealth::Healthy,
            resource_health: crate::upstream::types::UpstreamHealth::Healthy,
            skill_health: crate::upstream::types::UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            skill_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
            skill_last_error: None,
        },
    )
    .await;
    runtime.swap(Some(std::sync::Arc::new(pool))).await;

    let value = dispatch_with_manager(&manager, "gateway.servers", json!({}))
        .await
        .expect("server listing");
    let row = &value["servers"][0];
    assert!(row["tool_count"].is_null());
    assert_eq!(row["tool_health"], "not_probed");
    assert_eq!(row["discovery_mode"], "request_scoped");
}

#[tokio::test]
async fn gateway_dispatch_rejects_synthetic_tool_execution_actions() {
    let manager = test_manager();

    for action in ["tool_execute", "tool_invoke", "code_mode", "invoke"] {
        let err = dispatch_with_manager(&manager, action, json!({}))
            .await
            .expect_err("synthetic top-level MCP tools are not gateway actions");
        assert_eq!(err.kind(), "unknown_action", "{action}");
    }
}

#[tokio::test]
async fn gateway_list_returns_array() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list");

    assert!(value.is_array());
    assert_eq!(value.as_array().expect("array").len(), 1);
    let row = &value.as_array().expect("array")[0];
    assert_eq!(row["discovered_tool_count"], 0);
    assert_eq!(row["exposed_tool_count"], 0);
    assert_eq!(row["discovered_resource_count"], 0);
    assert_eq!(row["exposed_resource_count"], 0);
    assert_eq!(row["discovered_prompt_count"], 0);
    assert_eq!(row["exposed_prompt_count"], 0);
}

#[tokio::test]
async fn gateway_client_config_get_returns_http_and_stdio_configs() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001/mcp".to_string()),
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
            UpstreamConfig {
                enabled: true,
                name: "fixture-stdio".to_string(),
                url: None,
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: Some("npx".to_string()),
                args: vec!["-y".to_string(), "fixture-server".to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
        ])
        .await;

    let http = dispatch_with_manager(
        &manager,
        "gateway.client_config.get",
        json!({"name":"fixture-http"}),
    )
    .await
    .expect("http client config");
    assert_eq!(http["name"], "fixture-http");
    assert_eq!(http["type"], "http");
    assert_eq!(http["url"], "http://127.0.0.1:9001/mcp");

    let stdio = dispatch_with_manager(
        &manager,
        "gateway.client_config.get",
        json!({"name":"fixture-stdio"}),
    )
    .await
    .expect("stdio client config");
    assert_eq!(stdio["name"], "fixture-stdio");
    assert_eq!(stdio["type"], "stdio");
    assert_eq!(stdio["command"], "npx");
    assert_eq!(stdio["args"], json!(["-y", "fixture-server"]));
}

fn protected_route_fixture(name: &str) -> ProtectedMcpRouteConfig {
    ProtectedMcpRouteConfig {
        name: name.to_string(),
        enabled: true,
        public_host: "mcp.example.com".to_string(),
        public_path: "/syslog".to_string(),
        upstream: None,
        backend_url: "http://100.64.0.10:3100".to_string(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: Vec::new(),
        health_path: None,
        target: None,
    }
}

fn protected_gateway_subset_route_fixture(name: &str) -> ProtectedMcpRouteConfig {
    ProtectedMcpRouteConfig {
        name: name.to_string(),
        enabled: true,
        public_host: "mcp.example.com".to_string(),
        public_path: "/ops".to_string(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: Vec::new(),
        health_path: None,
        target: Some(ProtectedMcpRouteTarget::GatewaySubset(
            ProtectedGatewaySubsetTarget {
                project_id: None,
                upstreams: vec!["gateway-alpha".to_string()],
                services: Vec::new(),
                expose_code_mode: false,
                loadout: None,
            },
        )),
    }
}

#[tokio::test]
async fn staged_route_update_preserves_project_binding_inside_mutation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut current = protected_gateway_subset_route_fixture("ops");
    let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = current.target.as_mut() else {
        panic!("fixture is gateway subset")
    };
    target.project_id = Some("project-current".to_string());
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![current],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let result = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_update",
        json!({
            "name": "ops",
            "route": protected_gateway_subset_route_fixture("ops"),
            "preserve_project_id": true,
        }),
    )
    .await
    .expect("preserve binding under manager mutation lock");

    assert_eq!(result["route"]["target"]["project_id"], "project-current");
}

#[tokio::test]
async fn direct_route_update_ignores_project_preservation_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![protected_route_fixture("syslog")],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;
    let mut replacement = protected_route_fixture("syslog");
    replacement.public_path = "/updated".to_string();

    let result = dispatch_with_manager(
        &manager,
        "gateway.protected_route.update",
        json!({
            "name": "syslog",
            "route": replacement,
            "preserve_project_id": true,
        }),
    )
    .await
    .expect("ordinary direct update remains compatible");

    assert_eq!(result["public_path"], "/updated");
    assert!(result["target"].is_null());
}

#[tokio::test]
async fn protected_route_dispatch_add_list_and_test_share_gateway_actions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let tested = dispatch_with_manager(
        &manager,
        "gateway.protected_route.test",
        json!({ "route": protected_route_fixture("syslog") }),
    )
    .await
    .expect("test route");
    assert_eq!(tested["resource"], "https://mcp.example.com/syslog");

    let added = dispatch_with_manager(
        &manager,
        "gateway.protected_route.add",
        json!({ "route": protected_route_fixture("syslog") }),
    )
    .await
    .expect("add route");
    assert_eq!(added["name"], "syslog");

    let listed = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("list routes");
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert_eq!(listed[0]["public_host"], "mcp.example.com");
}

#[tokio::test]
async fn protected_gateway_subset_hot_crud_requires_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let err = dispatch_with_manager(
        &manager,
        "gateway.protected_route.add",
        json!({ "route": protected_gateway_subset_route_fixture("ops") }),
    )
    .await
    .expect_err("gateway_subset add must not pretend to hot-mount scoped service");
    assert_eq!(err.kind(), "restart_required");

    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![protected_gateway_subset_route_fixture("ops")],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let err = dispatch_with_manager(
        &manager,
        "gateway.protected_route.update",
        json!({
            "name": "ops",
            "route": protected_gateway_subset_route_fixture("ops")
        }),
    )
    .await
    .expect_err("gateway_subset update must not leave stale scoped service mounted");
    assert_eq!(err.kind(), "restart_required");

    let err = dispatch_with_manager(
        &manager,
        "gateway.protected_route.remove",
        json!({ "name": "ops" }),
    )
    .await
    .expect_err("gateway_subset remove must not leave stale scoped service mounted");
    assert_eq!(err.kind(), "restart_required");
}

#[tokio::test]
async fn protected_gateway_subset_stage_actions_persist_without_hot_mounting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let staged = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_add",
        json!({ "route": protected_gateway_subset_route_fixture("ops") }),
    )
    .await
    .expect("stage add");
    assert_eq!(staged["restart_required"], true);
    assert_eq!(staged["pending_operation"], "add");

    let runtime = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("runtime routes");
    assert_eq!(runtime.as_array().expect("runtime array").len(), 0);

    let desired = dispatch_with_manager(&manager, "gateway.protected_route.list_state", json!({}))
        .await
        .expect("desired route state");
    assert_eq!(desired.as_array().expect("state array").len(), 1);
    assert_eq!(desired[0]["name"], "ops");
    assert_eq!(desired[0]["restart_required"], true);
    assert_eq!(desired[0]["pending_operation"], "add");
    assert_eq!(desired[0]["runtime_present"], false);
    assert_eq!(desired[0]["desired_present"], true);

    let removed = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_remove",
        json!({ "name": "ops" }),
    )
    .await
    .expect("remove staged add before restart");
    assert_eq!(removed["restart_required"], false);
    assert!(removed["pending_operation"].is_null());
    assert!(
        removed["restart_note"]
            .as_str()
            .is_some_and(|note| note.contains("no restart is required"))
    );

    let desired = dispatch_with_manager(&manager, "gateway.protected_route.list_state", json!({}))
        .await
        .expect("route state after staged removal");
    assert!(desired.as_array().expect("state array").is_empty());
}

#[tokio::test]
async fn mounted_loadout_update_can_be_staged_without_changing_runtime_projection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );

    let original = json!({
        "name": "ops",
        "description": "runtime projection",
        "upstreams": [],
        "services": [],
        "expose_tools": true,
        "expose_resources": true,
        "expose_prompts": true,
        "expose_skills": true,
        "expose_code_mode": false
    });
    dispatch_with_manager(
        &manager,
        "gateway.loadout.add",
        json!({ "loadout": original }),
    )
    .await
    .expect("add loadout");

    let mut route = protected_gateway_subset_route_fixture("ops-route");
    let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = route.target.as_mut() else {
        panic!("gateway subset fixture");
    };
    target.loadout = Some("ops".to_string());
    target.upstreams.clear();
    dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_add",
        json!({ "route": route }),
    )
    .await
    .expect("stage loadout route");

    let staged = dispatch_with_manager(
        &manager,
        "gateway.loadout.stage_patch",
        json!({ "name": "ops", "patch": { "expose_tools": false } }),
    )
    .await
    .expect("stage loadout patch");
    assert_eq!(staged["restart_required"], true);
    assert_eq!(staged["loadout"]["expose_tools"], false);

    let runtime = dispatch_with_manager(&manager, "gateway.loadout.list", json!({}))
        .await
        .expect("runtime loadouts");
    assert_eq!(runtime[0]["expose_tools"], true);

    let desired = dispatch_with_manager(&manager, "gateway.loadout.list_state", json!({}))
        .await
        .expect("desired loadouts");
    assert_eq!(desired[0]["expose_tools"], false);
    assert_eq!(desired[0]["restart_required"], true);
    assert_eq!(desired[0]["pending_operation"], "update");
}

#[tokio::test]
async fn staged_subset_is_not_promoted_by_unrelated_hot_persist_or_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let runtime_route = protected_gateway_subset_route_fixture("ops");
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![runtime_route.clone()],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let desired_direct = protected_route_fixture("ops");
    let staged = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_update",
        json!({ "name": "ops", "route": desired_direct }),
    )
    .await
    .expect("stage subset to direct conversion");
    assert_eq!(staged["restart_required"], true);

    // An unrelated hot-safe mutation persists the durable desired config. It
    // must not smuggle the staged route into the running snapshot/index.
    dispatch_with_manager(
        &manager,
        "gateway.loadout.add",
        json!({
            "loadout": {
                "name": "unrelated",
                "upstreams": [],
                "services": [],
                "expose_code_mode": true,
                "expose_tools": true,
                "expose_resources": true,
                "expose_prompts": true,
                "expose_skills": true
            }
        }),
    )
    .await
    .expect("unrelated hot loadout add");

    let runtime = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("runtime route list after unrelated persist");
    assert_eq!(runtime[0]["target"]["kind"], "gateway_subset");

    // An explicit gateway reload also reconciles hot-safe upstream/runtime
    // inputs only. Host-mounted subset routes still require a process restart.
    manager
        .reload_with_origin(None, None)
        .await
        .expect("gateway reload with staged route");
    let runtime = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("runtime route list after reload");
    assert_eq!(runtime[0]["target"]["kind"], "gateway_subset");

    let desired = dispatch_with_manager(&manager, "gateway.protected_route.list_state", json!({}))
        .await
        .expect("desired route state after reload");
    assert_eq!(desired[0]["target"], Value::Null);
    assert_eq!(desired[0]["restart_required"], true);
    assert_eq!(desired[0]["pending_operation"], "update");
}

#[tokio::test]
async fn hot_route_crud_rejects_still_mounted_subset_after_desired_conversion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![protected_gateway_subset_route_fixture("ops")],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_update",
        json!({ "name": "ops", "route": protected_route_fixture("ops") }),
    )
    .await
    .expect("stage subset to direct conversion");

    for (action, params) in [
        (
            "gateway.protected_route.update",
            json!({ "name": "ops", "route": protected_route_fixture("ops") }),
        ),
        ("gateway.protected_route.remove", json!({ "name": "ops" })),
    ] {
        let error = dispatch_with_manager(&manager, action, params)
            .await
            .expect_err("still-mounted subset must reject hot mutation");
        assert_eq!(error.kind(), "restart_required", "action={action}");
    }

    dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_remove",
        json!({ "name": "ops" }),
    )
    .await
    .expect("stage desired removal while runtime subset remains");
    let error = dispatch_with_manager(
        &manager,
        "gateway.protected_route.add",
        json!({ "route": protected_route_fixture("ops") }),
    )
    .await
    .expect_err("same-name direct add must not replace mounted subset hot");
    assert_eq!(error.kind(), "restart_required");

    let replacement = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_add",
        json!({ "route": protected_route_fixture("ops") }),
    )
    .await
    .expect("stage direct replacement for mounted subset");
    assert_eq!(replacement["restart_required"], true);
    assert_eq!(replacement["pending_operation"], "update");
}

#[tokio::test]
async fn staged_subset_rename_freezes_route_collection_and_keeps_followup_edits_staged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![protected_gateway_subset_route_fixture("ops-old")],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let renamed = protected_route_fixture("ops-direct");
    let staged = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_update",
        json!({ "name": "ops-old", "route": renamed }),
    )
    .await
    .expect("stage subset rename to direct route");
    assert_eq!(staged["restart_required"], true);
    assert_eq!(staged["pending_operation"], "add");

    let runtime = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("runtime route list");
    assert_eq!(runtime.as_array().expect("runtime routes").len(), 1);
    assert_eq!(runtime[0]["name"], "ops-old");
    assert_eq!(runtime[0]["target"]["kind"], "gateway_subset");

    let state = dispatch_with_manager(&manager, "gateway.protected_route.list_state", json!({}))
        .await
        .expect("route state after rename");
    let rows = state.as_array().expect("route state array");
    let desired_new = rows
        .iter()
        .find(|row| row["name"] == "ops-direct")
        .expect("desired renamed route");
    assert_eq!(desired_new["restart_required"], true);
    assert_eq!(desired_new["pending_operation"], "add");
    let runtime_old = rows
        .iter()
        .find(|row| row["name"] == "ops-old")
        .expect("runtime old route");
    assert_eq!(runtime_old["restart_required"], true);
    assert_eq!(runtime_old["pending_operation"], "remove");

    let mut extra = protected_route_fixture("extra-direct");
    extra.public_path = "/extra".to_string();
    let error = dispatch_with_manager(
        &manager,
        "gateway.protected_route.add",
        json!({ "route": extra.clone() }),
    )
    .await
    .expect_err("hot direct add must not bypass a pending route restart transaction");
    assert_eq!(error.kind(), "restart_required");

    let staged_extra = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_add",
        json!({ "route": extra.clone() }),
    )
    .await
    .expect("stage direct route into pending restart transaction");
    assert_eq!(staged_extra["restart_required"], true);
    assert_eq!(staged_extra["pending_operation"], "add");

    extra.backend_url = "http://100.64.0.11:3100".to_string();
    let edited_extra = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_update",
        json!({ "name": "extra-direct", "route": extra }),
    )
    .await
    .expect("edit staged direct route while subset restart debt remains");
    assert_eq!(edited_extra["restart_required"], true);
    assert_eq!(edited_extra["pending_operation"], "add");

    let cancelled_extra = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_remove",
        json!({ "name": "extra-direct" }),
    )
    .await
    .expect("cancel staged direct route while rename debt remains");
    assert_eq!(cancelled_extra["restart_required"], true);
    assert!(cancelled_extra["pending_operation"].is_null());
    assert!(
        cancelled_extra["restart_note"]
            .as_str()
            .is_some_and(|note| note.contains("other protected route changes"))
    );

    let runtime = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("runtime routes remain frozen");
    assert_eq!(runtime.as_array().expect("runtime routes").len(), 1);
    assert_eq!(runtime[0]["name"], "ops-old");
}

#[tokio::test]
async fn clearing_last_subset_restart_debt_publishes_accumulated_direct_route_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let mut tools = protected_route_fixture("tools");
    tools.public_path = "/tools".to_string();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            protected_mcp_routes: vec![tools.clone()],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let staged_subset = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_add",
        json!({ "route": protected_gateway_subset_route_fixture("ops") }),
    )
    .await
    .expect("stage subset add");
    assert_eq!(staged_subset["restart_required"], true);

    tools.public_path = "/tools-v2".to_string();
    let staged_direct = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_update",
        json!({ "name": "tools", "route": tools.clone() }),
    )
    .await
    .expect("stage direct update behind subset restart debt");
    assert_eq!(staged_direct["restart_required"], true);

    let cancelled = dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_remove",
        json!({ "name": "ops" }),
    )
    .await
    .expect("cancel final subset change");
    assert_eq!(cancelled["restart_required"], false);
    assert!(cancelled["pending_operation"].is_null());

    let runtime = dispatch_with_manager(&manager, "gateway.protected_route.list", json!({}))
        .await
        .expect("runtime routes after debt clears");
    let rows = runtime.as_array().expect("runtime route array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "tools");
    assert_eq!(rows[0]["public_path"], "/tools-v2");

    let state = dispatch_with_manager(&manager, "gateway.protected_route.list_state", json!({}))
        .await
        .expect("state after debt clears");
    assert_eq!(state[0]["restart_required"], false);
    assert!(state[0]["pending_operation"].is_null());
    assert_eq!(state[0]["public_path"], "/tools-v2");
}

#[tokio::test]
async fn staged_loadout_revert_to_runtime_clears_loadout_restart_debt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let original = json!({
        "name": "ops",
        "description": "runtime projection",
        "upstreams": [],
        "services": [],
        "expose_tools": true,
        "expose_resources": true,
        "expose_prompts": true,
        "expose_skills": true,
        "expose_code_mode": false
    });
    dispatch_with_manager(
        &manager,
        "gateway.loadout.add",
        json!({ "loadout": original }),
    )
    .await
    .expect("add runtime loadout");

    let mut route = protected_gateway_subset_route_fixture("ops-route");
    let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = route.target.as_mut() else {
        panic!("gateway subset fixture");
    };
    target.loadout = Some("ops".to_string());
    target.upstreams.clear();
    dispatch_with_manager(
        &manager,
        "gateway.protected_route.stage_add",
        json!({ "route": route }),
    )
    .await
    .expect("stage route that will mount loadout");

    let changed = dispatch_with_manager(
        &manager,
        "gateway.loadout.stage_patch",
        json!({ "name": "ops", "patch": { "expose_tools": false } }),
    )
    .await
    .expect("stage changed loadout");
    assert_eq!(changed["restart_required"], true);

    let reverted = dispatch_with_manager(
        &manager,
        "gateway.loadout.stage_patch",
        json!({ "name": "ops", "patch": { "expose_tools": true } }),
    )
    .await
    .expect("revert loadout to runtime projection");
    assert_eq!(reverted["restart_required"], false);
    assert!(reverted["pending_operation"].is_null());
    assert!(
        reverted["restart_note"]
            .as_str()
            .is_some_and(|note| note.contains("no restart is required"))
    );

    let loadouts = dispatch_with_manager(&manager, "gateway.loadout.list_state", json!({}))
        .await
        .expect("loadout state");
    assert_eq!(loadouts[0]["restart_required"], false);
    assert!(loadouts[0]["pending_operation"].is_null());

    // The route itself is still a staged add, so its independent restart debt
    // remains visible.
    let routes = dispatch_with_manager(&manager, "gateway.protected_route.list_state", json!({}))
        .await
        .expect("route state");
    assert_eq!(routes[0]["restart_required"], true);
}

#[tokio::test]
async fn gateway_server_get_returns_custom_gateway_row() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let value = dispatch_with_manager(&manager, "gateway.server.get", json!({"id":"fixture-http"}))
        .await
        .expect("server get");

    assert_eq!(value["id"], "fixture-http");
    assert_eq!(value["source"], "custom_gateway");
}

#[tokio::test]
async fn gateway_list_and_mcp_runtime_are_snapshot_only_until_status_refresh() {
    let server = MockServer::start().await;
    let responder = DashboardCatalogResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "dashboard-http".to_string(),
            url: Some(format!("{}/mcp", server.uri())),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;
    runtime
        .swap(Some(std::sync::Arc::new(
            crate::upstream::pool::UpstreamPool::new(),
        )))
        .await;

    let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list");
    let row = value
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "dashboard-http")
        .expect("dashboard row");

    assert_eq!(row["discovered_tool_count"], 0);
    assert_eq!(row["exposed_tool_count"], 0);
    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 0);

    let runtime_value = dispatch_with_manager(&manager, "gateway.mcp.list", json!({}))
        .await
        .expect("runtime snapshot");
    let runtime_row = runtime_value
        .as_array()
        .expect("runtime array")
        .iter()
        .find(|item| item["name"] == "dashboard-http")
        .expect("runtime row");
    assert_eq!(runtime_row["discovered_tool_count"], 0);
    assert_eq!(runtime_row["exposed_tool_count"], 0);
    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 0);

    manager
        .refresh_gateway_status_catalog(&GatewayEnrichmentScope::default(), None)
        .await
        .expect("catalog refresh");
    let refreshed = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("refreshed list");
    let refreshed_row = refreshed
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "dashboard-http")
        .expect("dashboard row");
    assert_eq!(refreshed_row["discovered_tool_count"], 1);
    assert_eq!(refreshed_row["exposed_tool_count"], 1);
    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 1);

    let refreshed_runtime = dispatch_with_manager(&manager, "gateway.mcp.list", json!({}))
        .await
        .expect("refreshed runtime snapshot");
    let refreshed_runtime_row = refreshed_runtime
        .as_array()
        .expect("runtime array")
        .iter()
        .find(|item| item["name"] == "dashboard-http")
        .expect("runtime row");
    assert_eq!(refreshed_runtime_row["discovered_tool_count"], 1);
    assert_eq!(refreshed_runtime_row["exposed_tool_count"], 1);
    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn gateway_status_catalog_refresh_reprobes_healthy_upstream_tool_growth() {
    let server = MockServer::start().await;
    let responder = DashboardCatalogResponder::default();
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "dashboard-http".to_string(),
            url: Some(format!("{}/mcp", server.uri())),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;
    runtime
        .swap(Some(std::sync::Arc::new(
            crate::upstream::pool::UpstreamPool::new(),
        )))
        .await;

    let first = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("first list");
    let first_row = first
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "dashboard-http")
        .expect("dashboard row");
    assert_eq!(first_row["discovered_tool_count"], 0);
    assert_eq!(responder.list_requests.load(Ordering::SeqCst), 0);

    manager
        .refresh_gateway_status_catalog(&GatewayEnrichmentScope::default(), None)
        .await
        .expect("catalog refresh");
    let initial = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("initial refreshed list");
    let initial_row = initial
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "dashboard-http")
        .expect("dashboard row");
    assert_eq!(initial_row["discovered_tool_count"], 1);
    assert_eq!(responder.list_requests.load(Ordering::SeqCst), 1);

    responder.tool_count.store(3, Ordering::SeqCst);
    let stale = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("cached list");
    let stale_row = stale
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "dashboard-http")
        .expect("dashboard row");
    assert_eq!(stale_row["discovered_tool_count"], 1);
    assert_eq!(responder.list_requests.load(Ordering::SeqCst), 1);

    manager
        .refresh_gateway_status_catalog(
            &GatewayEnrichmentScope::default(),
            Some("different-upstream"),
        )
        .await
        .expect("filtered catalog refresh");
    assert_eq!(responder.list_requests.load(Ordering::SeqCst), 1);

    manager
        .refresh_gateway_status_catalog(
            &GatewayEnrichmentScope {
                route_visible_upstreams: Some(std::collections::BTreeSet::from([
                    "different-upstream".to_string(),
                ])),
                oauth_subject: None,
            },
            None,
        )
        .await
        .expect("scoped catalog refresh");
    assert_eq!(responder.list_requests.load(Ordering::SeqCst), 1);

    manager
        .refresh_gateway_status_catalog(&GatewayEnrichmentScope::default(), None)
        .await
        .expect("catalog refresh");

    let refreshed = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("refreshed list");
    let refreshed_row = refreshed
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "dashboard-http")
        .expect("dashboard row");
    assert_eq!(refreshed_row["discovered_tool_count"], 3);
    assert_eq!(refreshed_row["exposed_tool_count"], 3);
    assert_eq!(responder.list_requests.load(Ordering::SeqCst), 2);
    assert_eq!(responder.discover_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_fleet_catalog_refresh_coalesces_while_one_is_inflight() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let scope = GatewayEnrichmentScope::default();
    let refresh_key = format!(
        "{:?}\0{:?}",
        scope.route_visible_upstreams, scope.oauth_subject
    );
    manager
        .mcp_catalog_refresh_inflight
        .lock()
        .await
        .insert(refresh_key);

    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        manager.refresh_gateway_status_catalog(&scope, None),
    )
    .await
    .expect("a concurrent refresh should attach to the active fleet warm-up")
    .expect("coalesced refresh result");
}

#[tokio::test]
async fn fleet_catalog_refresh_does_not_coalesce_distinct_actor_scopes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let first = GatewayEnrichmentScope {
        route_visible_upstreams: None,
        oauth_subject: Some("actor-a".into()),
    };
    let second = GatewayEnrichmentScope {
        route_visible_upstreams: None,
        oauth_subject: Some("actor-b".into()),
    };
    let first_key = format!(
        "{:?}\0{:?}",
        first.route_visible_upstreams, first.oauth_subject
    );
    manager
        .mcp_catalog_refresh_inflight
        .lock()
        .await
        .insert(first_key);

    manager
        .refresh_gateway_status_catalog(&second, None)
        .await
        .expect("distinct actor refresh");
}

#[tokio::test]
async fn fleet_catalog_refresh_retries_immediately_after_terminal_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = GatewayManager::new(
        dir.path().join("config.toml"),
        GatewayRuntimeHandle::default(),
    );
    let scope = GatewayEnrichmentScope::default();
    let key = format!(
        "{:?}\0{:?}",
        scope.route_visible_upstreams, scope.oauth_subject
    );
    manager
        .mcp_catalog_refresh_failures
        .lock()
        .await
        .insert(key.clone());

    manager
        .refresh_gateway_status_catalog(&scope, None)
        .await
        .expect("retry refresh");
    assert!(
        !manager
            .mcp_catalog_refresh_failures
            .lock()
            .await
            .contains(&key)
    );
    assert!(
        !manager
            .mcp_catalog_refresh_inflight
            .lock()
            .await
            .contains(&key)
    );
}

#[tokio::test]
async fn fleet_catalog_refresh_continues_after_response_timeout() {
    let server = MockServer::start().await;
    let responder = DashboardCatalogResponder::default();
    responder.delay_ms.store(75, Ordering::SeqCst);
    Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/mcp"))
        .respond_with(responder.clone())
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(dir.path().join("config.toml"), runtime.clone());
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "slow-http".into(),
            url: Some(format!("{}/mcp", server.uri())),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: Default::default(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;
    manager
        .config
        .write()
        .await
        .gateway
        .mcp_list_warm_timeout_ms = Some(10);
    runtime
        .swap(Some(std::sync::Arc::new(
            crate::upstream::pool::UpstreamPool::new(),
        )))
        .await;

    manager
        .refresh_gateway_status_catalog(&GatewayEnrichmentScope::default(), None)
        .await
        .expect("timeout is advisory");
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let rows = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list");
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "slow-http")
        .unwrap();
    assert_eq!(row["discovered_tool_count"], 1);
}

#[tokio::test]
async fn gateway_list_surfaces_cached_custom_gateway_summary_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let runtime = GatewayRuntimeHandle::default();
    let manager = GatewayManager::new(path, runtime.clone());

    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "noxa".to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("noxa".to_string()),
            args: vec!["mcp".to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: false,
            expose_tools: Some(vec!["scrape".to_string()]),
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let pool = crate::upstream::pool::UpstreamPool::new();
    let upstream_name: std::sync::Arc<str> = std::sync::Arc::from("noxa");
    let mut tools = std::collections::HashMap::new();
    for name in ["scrape", "crawl"] {
        let schema = std::sync::Arc::new(serde_json::Map::new());
        let tool = rmcp::model::Tool::new(name, format!("{name} description"), schema);
        tools.insert(
            name.to_string(),
            crate::upstream::types::UpstreamTool {
                tool,
                input_schema: None,
                output_schema: None,
                upstream_name: std::sync::Arc::clone(&upstream_name),
                destructive: false,
            },
        );
    }
    pool.insert_entry_for_tests(
        "noxa",
        crate::upstream::types::UpstreamEntry {
            name: std::sync::Arc::clone(&upstream_name),
            tools,
            exposure_policy: crate::upstream::types::ToolExposurePolicy::from_patterns(vec![
                "scrape".to_string(),
            ])
            .expect("policy"),
            resource_exposure_policy: crate::upstream::types::ToolExposurePolicy::All,
            prompt_exposure_policy: crate::upstream::types::ToolExposurePolicy::All,
            skill_exposure_policy: crate::upstream::types::SkillExposurePolicy::all(),
            proxy_skills: false,
            supports_skills: None,
            proxy_resources: true,
            prompt_count: 3,
            resource_count: 4,
            skill_count: 0,
            skill_names: Vec::new(),
            prompt_names: Vec::new(),
            resource_uris: Vec::new(),
            tool_health: crate::upstream::types::UpstreamHealth::Healthy,
            prompt_health: crate::upstream::types::UpstreamHealth::Healthy,
            resource_health: crate::upstream::types::UpstreamHealth::Healthy,
            skill_health: crate::upstream::types::UpstreamHealth::Healthy,
            tool_unhealthy_since: None,
            prompt_unhealthy_since: None,
            resource_unhealthy_since: None,
            skill_unhealthy_since: None,
            tool_last_error: None,
            prompt_last_error: None,
            resource_last_error: None,
            skill_last_error: None,
        },
    )
    .await;
    runtime.swap(Some(std::sync::Arc::new(pool))).await;

    let value = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list");
    let row = value
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == "noxa")
        .expect("noxa row");

    assert_eq!(row["discovered_tool_count"], 2);
    assert_eq!(row["exposed_tool_count"], 1);
    assert_eq!(row["discovered_resource_count"], 4);
    assert_eq!(row["exposed_resource_count"], 4);
    assert_eq!(row["discovered_prompt_count"], 3);
    assert_eq!(row["exposed_prompt_count"], 3);
}

// Re-fixtured post-gateway-pivot: backed by the kept `deploy` service and a real
// `deploy.plan` action (the policy validator checks `allowed_actions` against the
// service's compiled action catalog, so the action must actually exist for
// `deploy`). The original `server.info` belonged to a retired service fixture.
#[tokio::test]
async fn virtual_server_policy_validation_uses_service_name() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy-primary".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.set_mcp_policy",
        json!({"id":"deploy-primary","allowed_actions":["deploy.plan"]}),
    )
    .await
    .expect("set policy");

    assert_eq!(value["allowed_actions"][0], "deploy.plan");
}

#[test]
fn supported_services_lists_metadata_backed_lab_gateways() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"gateway.supported_services"));
}

#[tokio::test]
async fn supported_services_payload_is_an_array() {
    let manager = test_manager();
    let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
        .await
        .expect("supported services");

    let _services = value.as_array().expect("array");
}

// CANNOT be re-fixtured without a production change (out of test-only scope): it
// relies on `filter_built_in_upstream_apis(reg, false)` removing `deploy`/`setup`,
// but post-pivot that filter is a documented no-op — no `BuiltInUpstreamApi`
// services remain (all surviving services are `BootstrapOperator`). See
// `registry::tests::upstream_api_filter_is_noop_after_gateway_pivot`. So `deploy`
// and `setup` are never omitted and the assertions can't hold. Re-enabling needs a
// real `BuiltInUpstreamApi` service to exist again — a production change.
#[tokio::test]
#[ignore = "filter_built_in_upstream_apis is a no-op post-pivot (no BuiltInUpstreamApi services left); deploy/setup are never omitted — prod change required"]
async fn supported_services_omits_upstreams_when_policy_disabled() {
    // NOTE: the default-registry builder + upstream-API filter live in the `lab`
    // binary, not `labby-gateway`. This test is permanently `#[ignore]`d (the filter
    // is a no-op post-pivot), so an `EmptyServiceRegistry` keeps it compiling here.
    let registry = std::sync::Arc::new(crate::gateway::service_registry::EmptyServiceRegistry);
    let manager = test_manager().with_builtin_service_registry(registry);
    let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
        .await
        .expect("supported services");

    let services = value.as_array().expect("array");
    assert!(!services.iter().any(|service| service["key"] == "deploy"));
    assert!(!services.iter().any(|service| service["key"] == "setup"));
}

// CANNOT be re-fixtured without a production change (out of test-only scope): same
// root cause as `supported_services_omits_upstreams_when_policy_disabled` —
// `filter_built_in_upstream_apis(reg, false)` is a no-op post-pivot, so `deploy`
// stays in the registry and `service_actions` returns its catalog instead of
// erroring. Re-enabling needs a real `BuiltInUpstreamApi` service.
#[tokio::test]
#[ignore = "filter_built_in_upstream_apis is a no-op post-pivot; deploy is never filtered out so service_actions does not error — prod change required"]
async fn service_actions_rejects_disabled_upstream_service() {
    // See note above: builder + filter live in `lab`; permanently ignored here.
    let registry = std::sync::Arc::new(crate::gateway::service_registry::EmptyServiceRegistry);
    let manager = test_manager().with_builtin_service_registry(registry);
    let err = dispatch_with_manager(
        &manager,
        "gateway.service_actions",
        json!({"service": "deploy"}),
    )
    .await
    .expect_err("disabled service should be unknown");

    assert_eq!(err.kind(), "invalid_param");
}

// CANNOT be re-fixtured without a production change (out of test-only scope): same
// root cause — `filter_built_in_upstream_apis(reg, false)` is a no-op post-pivot, so
// `deploy` remains a registered service and enabling its virtual server succeeds
// instead of returning `not_found`. Re-enabling needs a real `BuiltInUpstreamApi`
// service.
#[tokio::test]
#[ignore = "filter_built_in_upstream_apis is a no-op post-pivot; deploy stays registered so virtual_server.enable succeeds — prod change required"]
async fn virtual_server_enable_rejects_disabled_upstream_service() {
    // See note above: builder + filter live in `lab`; permanently ignored here.
    let registry = std::sync::Arc::new(crate::gateway::service_registry::EmptyServiceRegistry);
    let manager = test_manager().with_builtin_service_registry(registry);
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: false,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let err = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.enable",
        json!({"id": "deploy"}),
    )
    .await
    .expect_err("disabled upstream virtual server should be unavailable");

    assert_eq!(err.kind(), "not_found");
}

// Re-fixtured post-gateway-pivot: backed by the kept/registered `deploy` service.
#[tokio::test]
async fn enabling_virtual_server_marks_existing_server_row_enabled() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: false,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.enable",
        json!({"id": "deploy"}),
    )
    .await
    .expect("enable");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["enabled"], true);
}

// CANNOT be re-fixtured without a production change (out of test-only scope): it
// drives `gateway.service_config.set` with `GATEWAY_ALPHA_*` values, which only succeeds for
// a `service_meta`-resolvable service that declares those env fields. Post-pivot the
// only resolvable service is `deploy`, which declares zero env fields, so the set is
// rejected before a service row can be created. Needs a service_meta service with
// env fields.
#[tokio::test]
async fn enabling_virtual_server_creates_missing_service_row() {
    let manager = test_manager();

    dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "fixture-service",
            "values": {
                "FIXTURE_URL": "http://127.0.0.1:9999",
                "FIXTURE_TOKEN": "token"
            }
        }),
    )
    .await
    .expect("set service config");

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.enable",
        json!({"id": "fixture-service"}),
    )
    .await
    .expect("enable missing virtual server");

    assert_eq!(value["id"], "fixture-service");
    assert_eq!(value["source"], "in_process");
    assert_eq!(value["enabled"], true);
    assert_eq!(value["surfaces"]["mcp"]["enabled"], true);
}

#[tokio::test]
async fn disabling_virtual_server_keeps_server_row_visible_but_disabled() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.disable",
        json!({"id": "deploy"}),
    )
    .await
    .expect("disable");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["enabled"], false);

    let list = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list after disable");
    assert!(
        list.as_array()
            .expect("array")
            .iter()
            .any(|server| server["id"] == "deploy" && server["enabled"] == false)
    );
}

// CANNOT be re-fixtured without a production change (out of test-only scope):
// `gateway.service_config.set` with `GATEWAY_ALPHA_*` requires a service_meta-resolvable
// service that declares those env fields. Only `deploy` resolves post-pivot and it
// declares none, so the write is rejected. Needs a service_meta service with env
// fields.
#[tokio::test]
async fn setting_service_config_writes_canonical_env_backed_fields() {
    let manager = test_manager();

    let value = dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "fixture-service",
            "values": {
                "FIXTURE_URL": "http://127.0.0.1:9999",
                "FIXTURE_TOKEN": "token"
            }
        }),
    )
    .await
    .expect("set service config");

    assert_eq!(value["service"], "fixture-service");
    assert_eq!(value["configured"], true);
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "FIXTURE_URL" && field["present"] == true)
    );
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "FIXTURE_TOKEN" && field["present"] == true)
    );
}

// CANNOT be re-fixtured without a production change (out of test-only scope):
// `gateway.service_config.set` with `GATEWAY_ALPHA_*` requires a service_meta-resolvable
// service that declares those env fields. Only `deploy` resolves post-pivot and it
// declares none, so the write is rejected before the read-back can be exercised.
// Needs a service_meta service with env fields.
#[tokio::test]
async fn configured_but_disabled_service_can_be_read_back_for_editing() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "fixture-service".to_string(),
                service: "fixture-service".to_string(),
                enabled: false,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig::default(),
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "fixture-service",
            "values": {
                "FIXTURE_URL": "http://127.0.0.1:9999",
                "FIXTURE_TOKEN": "token"
            }
        }),
    )
    .await
    .expect("set service config");

    let value = dispatch_with_manager(
        &manager,
        "gateway.service_config.get",
        json!({"service": "fixture-service"}),
    )
    .await
    .expect("get service config");

    assert_eq!(value["service"], "fixture-service");
    assert_eq!(value["configured"], true);
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "FIXTURE_URL"
                && field["value_preview"] == "http://127.0.0.1:9999")
    );
    assert!(
        value["fields"]
            .as_array()
            .expect("fields")
            .iter()
            .any(|field| field["name"] == "FIXTURE_TOKEN" && field["secret"] == true)
    );
}

#[tokio::test]
async fn setting_virtual_server_surface_updates_visible_server_row() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    mcp: true,
                    ..labby_runtime::gateway_config::VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.set_surface",
        json!({"id": "deploy", "surface": "api", "enabled": true}),
    )
    .await
    .expect("set surface");

    assert_eq!(value["id"], "deploy");
    assert_eq!(value["surfaces"]["api"]["enabled"], true);
}

// Re-fixtured post-gateway-pivot: backed by the kept `deploy` service and its real
// `deploy.plan` action (the policy validator checks allowed_actions against the
// service's compiled catalog, so the action must exist for `deploy`).
#[tokio::test]
async fn setting_virtual_server_mcp_policy_persists_allowed_actions() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    cli: false,
                    api: false,
                    mcp: true,
                    webui: false,
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.set_mcp_policy",
        json!({"id": "deploy", "allowed_actions": ["deploy.plan"]}),
    )
    .await
    .expect("set mcp policy");

    assert_eq!(value["allowed_actions"], json!(["deploy.plan"]));

    let reloaded = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.get_mcp_policy",
        json!({"id": "deploy"}),
    )
    .await
    .expect("get mcp policy");

    assert_eq!(reloaded["allowed_actions"], json!(["deploy.plan"]));
}

// Re-fixtured post-gateway-pivot: assert against the kept `deploy` service's real
// `deploy.plan` action instead of the retired fixture's `server.info`.
#[tokio::test]
async fn service_actions_returns_compiled_action_catalog() {
    let manager = test_manager();
    let value = dispatch_with_manager(
        &manager,
        "gateway.service_actions",
        json!({"service": "deploy"}),
    )
    .await
    .expect("service actions");

    let actions = value.as_array().expect("array");
    assert!(actions.iter().any(|action| action["name"] == "deploy.plan"));
}

#[tokio::test]
async fn gateway_get_rejects_missing_name() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.get", json!({}))
        .await
        .expect_err("missing name should fail");

    assert_eq!(err.kind(), "invalid_param");
}

/// `gateway.test` with a `spec` whose `command` field names a stdio upstream
/// **executes that command as a real child process**.  This test uses `echo` so
/// the subprocess exits cleanly on all platforms.  See docs/services/UPSTREAM.md §"Testing
/// with Stdio Upstreams" and the SECURITY NOTE in the `gateway.test` handler.
#[tokio::test]
async fn gateway_test_spec_stdio_executes_command_and_name_routes_to_config() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![
            UpstreamConfig {
                enabled: true,
                name: "fixture-http".to_string(),
                url: Some("http://127.0.0.1:9001".to_string()),
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
            UpstreamConfig {
                enabled: true,
                name: "configured-stdio".to_string(),
                url: None,
                transport: None,
                socket_path: None,
                headers: Default::default(),
                bearer_token_env: None,
                command: Some("echo".to_string()),
                args: vec!["hello".to_string()],
                env: std::collections::BTreeMap::new(),
                proxy_resources: false,
                proxy_prompts: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                proxy_skills: false,
                expose_skills: None,
                code_mode_hint: None,
                oauth: None,
                imported_from: None,
                priority: 1.0,
            },
        ])
        .await;

    let named = dispatch_with_manager(&manager, "gateway.test", json!({"name": "fixture-http"}))
        .await
        .expect("named test");
    // Stdio gateways test freely — no ack required.
    let named_stdio = dispatch_with_manager(
        &manager,
        "gateway.test",
        json!({"name": "configured-stdio"}),
    )
    .await
    .expect("configured stdio test");
    let proposed = dispatch_with_manager(
        &manager,
        "gateway.test",
        json!({"spec": {
            "name": "fixture-stdio",
            "command": "npx",
            "args": ["hello"]
        }}),
    )
    .await
    .expect("spec test");

    assert_eq!(named["name"], "fixture-http");
    assert_eq!(named_stdio["name"], "configured-stdio");
    assert_eq!(proposed["name"], "fixture-stdio");
}

#[tokio::test]
async fn gateway_mutations_call_manager_methods() {
    let manager = test_manager();

    let added = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "fixture-http",
            "url": "https://fixture.example.com/mcp",
            "bearer_token_env": "FIXTURE_HTTP_TOKEN"
        }}),
    )
    .await
    .expect("add");
    assert_eq!(added["config"]["name"], "fixture-http");
    assert_eq!(added["config"]["bearer_token_env"], "FIXTURE_HTTP_TOKEN");

    let public = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "deepwiki",
            "url": "https://mcp.deepwiki.com/mcp"
        }}),
    )
    .await
    .expect("add no-auth http");
    assert_eq!(public["config"]["name"], "deepwiki");
    assert_eq!(public["config"]["bearer_token_env"], Value::Null);

    let updated = dispatch_with_manager(
        &manager,
        "gateway.update",
        json!({"name": "fixture-http", "patch": {"proxy_resources": true}}),
    )
    .await
    .expect("update");
    assert_eq!(updated["config"]["proxy_resources"], true);

    let status = dispatch_with_manager(&manager, "gateway.status", json!({}))
        .await
        .expect("status");
    assert!(status.is_array());

    let removed =
        dispatch_with_manager(&manager, "gateway.remove", json!({"name": "fixture-http"}))
            .await
            .expect("remove");
    assert_eq!(removed["config"]["name"], "fixture-http");

    let reloaded = dispatch_with_manager(&manager, "gateway.reload", json!({}))
        .await
        .expect("reload");
    assert!(reloaded.get("tools_changed").is_some());
}

#[tokio::test]
async fn gateway_add_stdio_needs_no_ack() {
    let manager = test_manager();

    let added = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "fixture-stdio",
            "command": "npx",
            "args": ["hello"]
        }}),
    )
    .await
    .expect("stdio add without ack");

    assert_eq!(added["config"]["name"], "fixture-stdio");
}

#[tokio::test]
async fn gateway_update_stdio_needs_no_ack() {
    let manager = test_manager();
    dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {
            "name": "fixture-stdio",
            "command": "npx",
            "args": ["hello"]
        }}),
    )
    .await
    .expect("add stdio");

    let updated = dispatch_with_manager(
        &manager,
        "gateway.update",
        json!({"name": "fixture-stdio", "patch": {"proxy_resources": true}}),
    )
    .await
    .expect("stdio update without ack");

    assert_eq!(updated["config"]["proxy_resources"], true);
}

#[tokio::test]
async fn virtual_server_remove_deletes_configured_service_row() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "stale-service".to_string(),
                service: "missing-service".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    mcp: true,
                    ..labby_runtime::gateway_config::VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let removed = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.remove",
        json!({"id": "stale-service"}),
    )
    .await
    .expect("remove virtual server");

    assert_eq!(removed["id"], "stale-service");
    assert_eq!(removed["warnings"][0]["code"], "unknown_service");

    let remaining = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list after remove");
    assert_eq!(remaining.as_array().expect("array").len(), 0);
}

// Re-fixtured post-gateway-pivot: the quarantined virtual server is backed by the
// kept/registered `deploy` service, so restore returns it to the active list.
#[tokio::test]
async fn virtual_server_quarantine_list_and_restore_round_trip() {
    let manager = test_manager();
    manager
        .seed_config_unchecked_for_tests(labby_runtime::gateway_config::GatewayConfig {
            quarantined_virtual_servers: vec![labby_runtime::gateway_config::VirtualServerConfig {
                id: "deploy".to_string(),
                service: "deploy".to_string(),
                enabled: true,
                surfaces: labby_runtime::gateway_config::VirtualServerSurfacesConfig {
                    mcp: true,
                    ..labby_runtime::gateway_config::VirtualServerSurfacesConfig::default()
                },
                mcp_policy: None,
            }],
            ..labby_runtime::gateway_config::GatewayConfig::default()
        })
        .await;

    let quarantined = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.quarantine.list",
        json!({}),
    )
    .await
    .expect("list quarantine");
    assert_eq!(quarantined.as_array().expect("array").len(), 1);
    assert_eq!(quarantined[0]["id"], "deploy");

    let restored = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.quarantine.restore",
        json!({"id": "deploy"}),
    )
    .await
    .expect("restore quarantine");
    assert_eq!(restored["id"], "deploy");

    let remaining = dispatch_with_manager(
        &manager,
        "gateway.virtual_server.quarantine.list",
        json!({}),
    )
    .await
    .expect("list after restore");
    assert_eq!(remaining.as_array().expect("array").len(), 0);

    let listed = dispatch_with_manager(&manager, "gateway.list", json!({}))
        .await
        .expect("list active");
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert_eq!(listed[0]["id"], "deploy");
}

#[tokio::test]
async fn invalid_gateway_specs_return_validation_errors() {
    let manager = test_manager();

    let invalid_url = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {"name": "bad", "url": "ftp://example.com"}}),
    )
    .await
    .expect_err("invalid scheme");
    assert_eq!(invalid_url.kind(), "invalid_param");

    let invalid_transport = dispatch_with_manager(
        &manager,
        "gateway.add",
        json!({"spec": {"name": "bad", "url": "http://127.0.0.1:9001", "command": "node"}}),
    )
    .await
    .expect_err("invalid transport");
    assert_eq!(invalid_transport.kind(), "invalid_param");
}

#[tokio::test]
async fn only_reload_promises_to_pick_up_changed_bearer_token_env_vars() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: "fixture-http".to_string(),
            url: Some("http://127.0.0.1:9001".to_string()),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: Some("FIXTURE_HTTP_TOKEN".to_string()),
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let status = dispatch_with_manager(&manager, "gateway.status", json!({}))
        .await
        .expect("status");
    assert!(status.is_array());

    let help = dispatch_with_manager(&manager, "help", json!({}))
        .await
        .expect("help");
    assert_eq!(help["service"], "gateway");
    assert!(
        help.to_string().contains("gateway.reload"),
        "reload should remain the explicit env-refresh action"
    );
}

#[tokio::test]
async fn public_urls_action_dispatches_to_manager() {
    let manager = test_manager();

    let value = dispatch_with_manager(&manager, "gateway.public_urls.get", json!({}))
        .await
        .expect("public urls");

    assert!(value.get("app").is_some());
    assert!(value.get("mcp_gateway").is_some());
    assert!(value.get("effective_mcp_gateway").is_some());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gateway_mcp_cleanup_dispatch_returns_cleanup_payload() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let manager = test_manager();
    let upstream_name = "cleanup-dispatch";
    let runtime_arg = "cleanup-dispatch-mcp";
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    use std::os::unix::process::CommandExt;
    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Keep this stand-in out of nextest's process group so the test
    // process survives when cleanup kills the child's process group.
    command.process_group(0);
    let mut child = command.spawn().expect("spawn github chat stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;
    wait_for_cleanup_match(&manager, upstream_name).await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.mcp.cleanup",
        json!({
            "name": upstream_name,
            "aggressive": false,
            "dry_run": false
        }),
    )
    .await
    .expect("cleanup dispatch");

    assert_eq!(value["upstream"], upstream_name);
    assert_eq!(value["aggressive"], false);
    assert!(
        value["gateway_killed"]
            .as_u64()
            .expect("gateway_killed as u64")
            >= 1
    );

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("github-chat stand-in process was not terminated by dispatch cleanup");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gateway_mcp_disable_with_cleanup_returns_gateway_and_cleanup_payload() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let manager = test_manager();
    let upstream_name = "disable-dispatch";
    let runtime_arg = "disable-dispatch-mcp";
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The cleanup path kills process groups for child runtimes. Keep this
    // stand-in out of nextest's process group so the test process survives.
    command.process_group(0);
    let mut child = command.spawn().expect("spawn github chat stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;
    wait_for_cleanup_match(&manager, upstream_name).await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.mcp.disable",
        json!({
            "name": upstream_name,
            "cleanup": true,
            "aggressive": false
        }),
    )
    .await
    .expect("disable dispatch");

    assert_eq!(value["gateway"]["config"]["name"], upstream_name);
    assert_eq!(value["gateway"]["config"]["enabled"], false);
    assert_eq!(value["cleanup"]["upstream"], upstream_name);

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("github-chat stand-in process was not terminated by disable cleanup");
}

#[tokio::test]
async fn gateway_mcp_restart_rejects_a_disabled_upstream_without_enabling_it() {
    let manager = test_manager();
    manager
        .replace_config_for_tests(vec![upstream_fixture(
            "disabled-restart",
            Some("http://127.0.0.1:1/mcp".to_string()),
            None,
        )])
        .await;

    let error = dispatch_with_manager(
        &manager,
        "gateway.mcp.restart",
        json!({"name": "disabled-restart"}),
    )
    .await
    .expect_err("disabled upstream restart must fail");

    assert!(matches!(error, ToolError::InvalidParam { .. }));
    assert!(
        !manager
            .upstream_config("disabled-restart")
            .await
            .expect("disabled upstream")
            .enabled
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn gateway_mcp_restart_cleans_the_old_runtime_and_returns_enabled() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let manager = test_manager();
    let upstream_name = "restart-dispatch";
    let runtime_arg = "restart-dispatch-mcp";
    manager
        .replace_config_for_tests(vec![UpstreamConfig {
            enabled: true,
            name: upstream_name.to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some("uvx".to_string()),
            args: vec![runtime_arg.to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }])
        .await;

    let mut command = Command::new("python3");
    command
        .args(["-c", "import time; time.sleep(60)", runtime_arg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.process_group(0);
    let mut child = command.spawn().expect("spawn restart stand-in");

    tokio::time::sleep(Duration::from_millis(150)).await;
    wait_for_cleanup_match(&manager, upstream_name).await;

    let value = dispatch_with_manager(
        &manager,
        "gateway.mcp.restart",
        json!({"name": upstream_name, "aggressive": false}),
    )
    .await
    .expect("restart dispatch");

    assert_eq!(value["gateway"]["config"]["name"], upstream_name);
    assert_eq!(value["gateway"]["config"]["enabled"], true);
    assert_eq!(value["cleanup"]["upstream"], upstream_name);

    for _ in 0..20 {
        if child.try_wait().expect("try_wait").is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(child.kill());
    panic!("restart stand-in process was not terminated");
}

#[cfg(target_os = "linux")]
async fn wait_for_cleanup_match(manager: &GatewayManager, upstream_name: &str) {
    use std::time::Duration;

    for _ in 0..40 {
        let value = dispatch_with_manager(
            manager,
            "gateway.mcp.cleanup",
            json!({
                "name": upstream_name,
                "aggressive": false,
                "dry_run": true
            }),
        )
        .await
        .expect("cleanup dry-run dispatch");
        if value["gateway_matched"]
            .as_u64()
            .expect("gateway_matched as u64")
            >= 1
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("stand-in process was not visible to cleanup matcher");
}

#[test]
fn discovery_url_preview_redacts_secret_url_parts() {
    assert_eq!(
        redact_url_preview("https://user:pass@example.com/mcp?token=secret#frag"),
        "https://example.com/mcp"
    );
    assert_eq!(redact_url_preview("not a url token=secret"), "<redacted>");
}

// ── shape_discovered_views unit tests ──────────────────────────────────

fn make_discovered_http(name: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: UpstreamConfig {
            name: name.to_string(),
            enabled: false,
            url: Some("http://127.0.0.1:9000".to_string()),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: std::collections::BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: true,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        },
        source_client: "cursor".to_string(),
        source_path: "/home/user/.cursor/mcp.json".to_string(),
        env_key_count: 0,
    }
}

fn make_discovered_stdio(name: &str, command: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: UpstreamConfig {
            name: name.to_string(),
            enabled: false,
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: Some(command.to_string()),
            args: vec!["--serve".to_string()],
            env: std::collections::BTreeMap::new(),
            proxy_resources: true,
            proxy_prompts: true,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        },
        source_client: "claude-code".to_string(),
        source_path: "/home/user/.claude/settings.json".to_string(),
        env_key_count: 2,
    }
}

#[test]
fn shape_http_server_gets_http_transport_no_command_preview() {
    let discovered = vec![make_discovered_http("my-http-server")];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let existing: HashSet<String> = HashSet::new();
    let params = GatewayDiscoverParams::default();

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].transport, McpClientTransportType::Http);
    assert!(views[0].command_preview.is_none());
    assert_eq!(views[0].name, "my-http-server");
}

#[test]
fn shape_stdio_server_gets_stdio_transport_and_command_preview_first_token() {
    let discovered = vec![make_discovered_stdio(
        "my-stdio-server",
        "npx --yes some-mcp",
    )];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let existing: HashSet<String> = HashSet::new();
    let params = GatewayDiscoverParams::default();

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].transport, McpClientTransportType::Stdio);
    assert_eq!(views[0].command_preview.as_deref(), Some("npx"));
}

#[test]
fn shape_already_configured_true_when_name_in_existing_set() {
    let discovered = vec![make_discovered_http("configured-server")];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let mut existing: HashSet<String> = HashSet::new();
    existing.insert("configured-server".to_string());
    let params = GatewayDiscoverParams {
        include_existing: true,
        ..GatewayDiscoverParams::default()
    };

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert!(views[0].already_configured);
}

#[test]
fn shape_include_existing_false_filters_out_already_configured_servers() {
    let discovered = vec![
        make_discovered_http("new-server"),
        make_discovered_http("existing-server"),
    ];
    let cfg = labby_runtime::gateway_config::GatewayConfig::default();
    let mut existing: HashSet<String> = HashSet::new();
    existing.insert("existing-server".to_string());
    let params = GatewayDiscoverParams {
        include_existing: false,
        ..GatewayDiscoverParams::default()
    };

    let views = shape_discovered_views(discovered, &cfg, &existing, &params);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name, "new-server");
    assert!(!views[0].already_configured);
}

// ── handle_import and handle_discover validation branch tests ──────────

#[tokio::test]
async fn gateway_import_rejects_empty_params() {
    let manager = test_manager();
    let err = dispatch_with_manager(&manager, "gateway.import", json!({}))
        .await
        .expect_err("empty import params should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_import_rejects_both_all_and_names() {
    let manager = test_manager();
    let err = dispatch_with_manager(
        &manager,
        "gateway.import",
        json!({"all": true, "names": ["some-server"]}),
    )
    .await
    .expect_err("both all and names should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_import_rejects_unknown_client_kind() {
    let manager = test_manager();
    let err = dispatch_with_manager(
        &manager,
        "gateway.import",
        json!({"all": true, "clients": ["not-a-real-client"]}),
    )
    .await
    .expect_err("unknown client kind should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_discover_rejects_unknown_client_kind() {
    let manager = test_manager();
    let err = dispatch_with_manager(
        &manager,
        "gateway.discover",
        json!({"clients": ["typo-client"]}),
    )
    .await
    .expect_err("unknown client kind in discover should fail");
    assert_eq!(err.kind(), "invalid_param");
}

#[tokio::test]
async fn gateway_import_result_has_correct_shape() {
    // Verify the ImportResultView shape: all=true on empty discovery
    // returns ImportResultView with empty imported/skipped/errors
    let manager = test_manager();
    // Pin discovery at an empty home. Otherwise this walks the developer's real
    // editor configs, and `all=true` tries to import whatever it finds there.
    let home = tempfile::tempdir().expect("tempdir");
    let _home_guard = crate::gateway::discovery::TestHomeDirGuard::set(home.path().to_path_buf());
    let result = dispatch_with_manager(&manager, "gateway.import", json!({"all": true}))
        .await
        .expect("all=true should succeed even with no discovered servers");
    // The result should be an object (ImportResultView), not an array
    assert!(
        result.is_object(),
        "import result should be an object with imported/skipped/errors"
    );
    assert!(
        result.get("imported").is_some(),
        "should have imported field"
    );
}

// --- lab-l3cm regression: public dispatch() must handle built-ins before manager resolution ---

/// `gateway::dispatch("help", …)` must succeed even when no gateway manager
/// is installed.  The old code called `require_gateway_manager()` first,
/// which returned `internal_error` in that situation.
#[tokio::test]
async fn gateway_dispatch_help_succeeds_without_manager() {
    let old = super::super::client::swap_gateway_manager_for_test(None);
    let result = dispatch("help", serde_json::json!({})).await;
    super::super::client::swap_gateway_manager_for_test(old);

    let value = result.expect("help must not require a gateway manager");
    assert_eq!(value["service"], "gateway");
    assert!(
        value["actions"].is_array(),
        "help response must contain an actions array"
    );
}

/// `gateway::dispatch("schema", {action: "gateway.list"})` must succeed even
/// when no gateway manager is installed.
#[tokio::test]
async fn gateway_dispatch_schema_succeeds_without_manager() {
    let old = super::super::client::swap_gateway_manager_for_test(None);
    let result = dispatch("schema", serde_json::json!({"action": "gateway.list"})).await;
    super::super::client::swap_gateway_manager_for_test(old);

    let value = result.expect("schema must not require a gateway manager");
    assert_eq!(value["action"], "gateway.list");
}

/// `gateway::dispatch("schema", {})` with a missing `action` param must
/// return `missing_param`, not `internal_error`.
#[tokio::test]
async fn gateway_dispatch_schema_missing_param_without_manager() {
    let old = super::super::client::swap_gateway_manager_for_test(None);
    let err = dispatch("schema", serde_json::json!({}))
        .await
        .expect_err("schema without action param must fail");
    super::super::client::swap_gateway_manager_for_test(old);

    let body = serde_json::to_value(&err).expect("serialize");
    assert_eq!(body["kind"], "missing_param");
    assert_eq!(body["param"], "action");
}
fn upstream_fixture(name: &str, url: Option<String>, command: Option<String>) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        enabled: false,
        url,
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command,
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: None,
        imported_from: None,
        priority: 1.0,
    }
}

fn make_http_server(name: &str, url: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: upstream_fixture(name, Some(url.to_string()), None),
        source_client: "test".to_string(),
        source_path: "/tmp/test.json".to_string(),
        env_key_count: 0,
    }
}

fn make_stdio_server(name: &str, command: &str) -> DiscoveredServer {
    DiscoveredServer {
        name: name.to_string(),
        spec: upstream_fixture(name, None, Some(command.to_string())),
        source_client: "test".to_string(),
        source_path: "/tmp/test.json".to_string(),
        env_key_count: 2,
    }
}

#[test]
fn http_server_gets_http_transport() {
    let views = shape_discovered_views(
        vec![make_http_server("srv", "https://example.com/mcp")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &HashSet::new(),
        &GatewayDiscoverParams::default(),
    );
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0].transport, McpClientTransportType::Http));
    assert!(views[0].command_preview.is_none());
}

#[test]
fn stdio_server_gets_stdio_transport_and_command_preview() {
    let views = shape_discovered_views(
        vec![make_stdio_server("srv", "npx @some/mcp-server")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &HashSet::new(),
        &GatewayDiscoverParams::default(),
    );
    assert_eq!(views.len(), 1);
    assert!(matches!(views[0].transport, McpClientTransportType::Stdio));
    assert_eq!(views[0].command_preview.as_deref(), Some("npx"));
}

#[test]
fn already_configured_flag_set_when_name_in_existing() {
    let mut existing = HashSet::new();
    existing.insert("known-server".to_string());
    let views = shape_discovered_views(
        vec![make_http_server("known-server", "https://h/m")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &existing,
        &GatewayDiscoverParams {
            include_existing: true,
            clients: vec![],
        },
    );
    assert_eq!(views.len(), 1);
    assert!(views[0].already_configured);
}

#[test]
fn include_existing_false_filters_out_configured_servers() {
    let mut existing = HashSet::new();
    existing.insert("known-server".to_string());
    let views = shape_discovered_views(
        vec![make_http_server("known-server", "https://h/m")],
        &labby_runtime::gateway_config::GatewayConfig::default(),
        &existing,
        &GatewayDiscoverParams::default(), // include_existing defaults to false
    );
    assert!(views.is_empty());
}
