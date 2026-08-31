#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_action_runner.rs"]
mod mcp_action_runner;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::collections::{BTreeMap, BTreeSet};

use action_matrix::{ScenarioKind, ScenarioOwner, Surface, intents};
use mcp_action_runner::BuiltinMcpRunner;

const ACTION_CATALOG: &str = include_str!("../../../docs/generated/action-catalog.json");

fn mcp_intents() -> Vec<&'static action_matrix::CaseIntent> {
    intents()
        .iter()
        .filter(|intent| intent.applicable_surfaces.contains(&Surface::Mcp))
        .collect()
}

fn expected_service_tools() -> BTreeSet<String> {
    mcp_intents()
        .into_iter()
        .map(|intent| intent.service.clone())
        .collect()
}

fn result_text(result: &rmcp::model::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_mcp_visible_classification_has_one_bounded_execution_plan() {
    let cases = mcp_intents();
    assert_eq!(cases.len(), 146);
    let mut plans = BTreeMap::new();
    for intent in cases {
        let disposition = match intent.scenario_kind {
            ScenarioKind::ContractProbe => "metadata_probe",
            ScenarioKind::LiveInvoke => "live_success_or_stable_error",
            ScenarioKind::StatefulScenario => "isolated_workflow",
            ScenarioKind::DestructiveIsolated => "confirmation_bound_workflow",
            ScenarioKind::ConditionalOptional => "conditional_http_subject",
            ScenarioKind::ExternalOptional => "offline_error_path",
            ScenarioKind::ExcludedWithReason => "reviewed_exclusion",
        };
        assert!(plans.insert(intent.key(), disposition).is_none());
        assert!(!intent.scenario_id.is_empty());
        assert!(!intent.fixture_params.fixture.is_empty());
    }
    assert_eq!(plans.len(), 146);
}

#[test]
fn mcp_projection_and_security_axes_are_derived_from_canonical_metadata() {
    let catalog: Vec<action_matrix::CatalogAction> = serde_json::from_str(ACTION_CATALOG).unwrap();
    let catalog = action_matrix::catalog_map(&catalog).unwrap();
    for intent in mcp_intents() {
        let action = catalog[&intent.key()];
        assert!(action.surface_availability.mcp);
        if action.requires_admin {
            assert_eq!(action.required_scopes, ["lab:admin"]);
        }
        if action.destructive {
            let canonical = intent.canonical_action.as_ref().map_or(intent, |key| {
                intents().iter().find(|case| case.key() == *key).unwrap()
            });
            assert_eq!(canonical.scenario_kind, ScenarioKind::DestructiveIsolated);
        }
    }
}

#[tokio::test]
async fn raw_mode_catalog_is_exact_and_builtin_help_executes_live() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let fingerprint = runner.identity_fingerprint();
    assert_eq!(fingerprint.len(), 64);
    let advertised = runner.list_tool_names().await.expect("bounded tools/list");
    let all_services = expected_service_tools();
    let expected = all_services
        .iter()
        .filter(|service| service.as_str() != "lab_admin")
        .cloned()
        .collect::<BTreeSet<_>>();
    let advertised_services = advertised
        .intersection(&all_services)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(advertised_services, expected);
    assert!(
        !advertised.contains("lab_admin"),
        "local-only tool leaked over HTTP MCP"
    );
    for unexpected in ["acp", "deploy", "fleet", "marketplace", "registry", "stash"] {
        assert!(!advertised.contains(unexpected));
    }
    for service in expected {
        let result = runner
            .call(&service, "help", serde_json::Map::new())
            .await
            .unwrap_or_else(|error| panic!("{service}.help: {error}"));
        assert_ne!(
            result.is_error,
            Some(true),
            "{service}.help failed: {}",
            result_text(&result)
        );
    }
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn code_mode_hides_raw_service_tools_without_testing_code_mode_primitives() {
    let runner = BuiltinMcpRunner::start_code_mode()
        .await
        .expect("Code Mode MCP runner");
    let advertised = runner.list_tool_names().await.expect("bounded tools/list");
    let services = expected_service_tools();
    let visible_services = advertised
        .intersection(&services)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        visible_services,
        BTreeSet::from(["server_logs".to_string()])
    );
    assert!(advertised.contains("codemode"));
    let hidden = runner
        .call("doctor", "help", serde_json::Map::new())
        .await
        .expect("hidden execution returns a protocol result");
    assert_eq!(hidden.is_error, Some(true));
    assert!(result_text(&hidden).contains("hidden"));
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn live_errors_are_structured_redacted_and_terminal() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let unknown = runner
        .call("doctor", "definitely.unknown", serde_json::Map::new())
        .await
        .unwrap();
    assert_eq!(unknown.is_error, Some(true));
    let unknown_text = result_text(&unknown);
    assert!(unknown_text.contains("unknown_action"));
    assert!(unknown_text.contains("valid"));

    let mut invalid = serde_json::Map::new();
    invalid.insert("action".into(), serde_json::Value::Bool(true));
    let invalid = runner.call("doctor", "schema", invalid).await.unwrap();
    assert_eq!(invalid.is_error, Some(true));
    let text = result_text(&invalid).to_ascii_lowercase();
    assert!(text.contains("invalid_param") || text.contains("validation"));
    assert!(!text.contains("live-mcp-action-matrix-token"));

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn every_http_feasible_surface_action_reaches_live_dispatch() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let expected = mcp_intents()
        .into_iter()
        .filter(|intent| intent.scenario_owner == ScenarioOwner::SurfaceActionRunner)
        // lab_admin is intentionally local-only and therefore cannot be
        // exercised through the HTTP MCP route owned by this runner.
        .filter(|intent| intent.service != "lab_admin")
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 59);

    let mut consumed = BTreeSet::new();
    for intent in expected {
        let result = runner
            .call(&intent.service, &intent.action, serde_json::Map::new())
            .await
            .unwrap_or_else(|error| panic!("{} wire failure: {error}", intent.key()));
        let text = result_text(&result);
        assert!(
            result.is_error != Some(true) || !text.trim().is_empty(),
            "{} returned an empty error envelope",
            intent.key()
        );
        assert!(
            !text.contains("live-mcp-action-matrix-token"),
            "{} reflected the bearer secret",
            intent.key()
        );
        assert!(consumed.insert(intent.key()), "duplicate action execution");
    }
    assert_eq!(consumed.len(), 59);

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn project_bound_non_admin_identity_narrows_discovery_and_denies_execution() {
    let identity = live_identity::LiveIdentity::bootstrap("mcp-matrix-non-admin")
        .await
        .expect("public identity bootstrap");
    let tuple = mcp_action_runner::IdentityTuple::from_public(&identity.identity);
    let fingerprint = tuple.fingerprint();
    let missing = BuiltinMcpRunner::connect_project(identity.base(), "", tuple.clone()).await;
    assert!(
        missing.is_err(),
        "missing credential must fail initialization"
    );
    let runner = BuiltinMcpRunner::connect_project(
        identity.base(),
        identity.credential_for_request(),
        tuple,
    )
    .await
    .expect("project-bound MCP client");
    assert_eq!(runner.identity_fingerprint(), fingerprint);

    let tools = runner.list_tool_names().await.expect("scoped tools/list");
    // This Loadout has no upstreams. The protected gateway-subset route must
    // therefore reveal no raw operator service tools at all.
    assert_eq!(tools, BTreeSet::from(["gateway".to_string()]));
    assert!(!tools.contains("setup"));
    assert!(!tools.contains("lab_admin"));

    let denied = runner
        .call("setup", "state", serde_json::Map::new())
        .await
        .expect("hidden execution returns an MCP result");
    assert_eq!(denied.is_error, Some(true));
    let denial = result_text(&denied);
    let denial_kind = denial.to_ascii_lowercase();
    assert!(
        denial_kind.contains("hidden")
            || denial_kind.contains("scope")
            || denial_kind.contains("unknown")
            || denial_kind.contains("not_found"),
        "unexpected non-enumerating denial: {denial}"
    );
    assert!(!denial.contains(identity.credential_for_request()));

    runner.disconnect().await;
    let cleanup = identity.cleanup().await.expect("identity cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn read_only_non_admin_discovers_mixed_service_but_cannot_execute_admin_action() {
    let setup_policy = live_identity::policy(&["lab:read"])
        .replace("services = [\"gateway\"]", "services = [\"setup\"]");
    let identity = live_identity::LiveIdentity::bootstrap_with_policy(
        "mcp-matrix-read-only",
        300,
        &setup_policy,
    )
    .await
    .expect("public read-only identity bootstrap");
    let tuple = mcp_action_runner::IdentityTuple::from_public(&identity.identity);
    assert!(!tuple.scopes.iter().any(|scope| scope == "lab:admin"));
    let runner = BuiltinMcpRunner::connect_project(
        identity.base(),
        identity.credential_for_request(),
        tuple,
    )
    .await
    .expect("read-only protected MCP client");

    let tools = runner.list_tool_names().await.expect("scoped tools/list");
    assert!(
        tools.contains("setup"),
        "mixed-scope setup tool must be visible"
    );
    let setup_contract = runner
        .tool_contract("setup")
        .await
        .expect("setup descriptor")
        .expect("visible setup descriptor");
    assert!(
        !setup_contract.contains(identity.credential_for_request()),
        "discovery must not reflect the credential"
    );

    let denied = runner
        .call("setup", "services.status", serde_json::Map::new())
        .await
        .expect("scope denial is an MCP result");
    assert_eq!(denied.is_error, Some(true));
    let denial = result_text(&denied).to_ascii_lowercase();
    assert!(
        denial.contains("forbidden")
            || denial.contains("scope")
            || denial.contains("admin")
            || denial.contains("not_found"),
        "unexpected scope denial: {denial}"
    );
    assert!(!denial.contains(identity.credential_for_request()));

    runner.disconnect().await;
    let cleanup = identity.cleanup().await.expect("identity cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}
