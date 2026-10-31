#![cfg(feature = "gateway")]
#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/mcp_action_runner.rs"]
mod mcp_action_runner;

mod support {
    pub(crate) use crate::evidence;
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use std::collections::{BTreeMap, BTreeSet};

use action_matrix::{EvidenceLevel, ScenarioKind, Surface, compiled_intents, intents};
use mcp_action_runner::BuiltinMcpRunner;

const ACTION_CATALOG: &str = include_str!("../../../docs/generated/action-catalog.json");

fn mcp_intents() -> Vec<&'static action_matrix::CaseIntent> {
    compiled_intents()
        .filter(|intent| intent.applicable_surfaces.contains(&Surface::Mcp))
        .collect()
}

fn expected_service_tools() -> BTreeSet<String> {
    mcp_intents()
        .into_iter()
        .filter(|intent| cfg!(feature = "fs") || intent.service != "fs")
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

fn result_error_kind(text: &str) -> Option<String> {
    fn find_kind(value: &serde_json::Value) -> Option<&str> {
        value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                value
                    .as_object()
                    .and_then(|object| object.values().find_map(find_kind))
            })
    }

    serde_json::from_str(text)
        .ok()
        .as_ref()
        .and_then(find_kind)
        .map(str::to_owned)
}

fn mcp_action_data(text: &str, service: &str, action: &str) -> serde_json::Value {
    let envelope: serde_json::Value = serde_json::from_str(text).expect("MCP command JSON");
    assert_eq!(envelope["ok"], true, "{service}:{action}: {envelope}");
    assert_eq!(envelope["service"], service);
    assert_eq!(envelope["action"], action);
    envelope.get("data").expect("MCP command payload").clone()
}

fn mcp_transition_params(intent: &action_matrix::CaseIntent) -> serde_json::Value {
    let mut params = action_scenarios::fixture_params(intent);
    match intent.key().as_str() {
        "gateway:gateway.code_mode.set" => params = serde_json::json!({"timeout_ms": 6000}),
        "gateway:gateway.loadout.update" => {
            params["loadout"]["expose_tools"] = serde_json::json!(false)
        }
        "gateway:gateway.mcp.restart" => {
            params = serde_json::json!({"name":"matrix-restart-owned", "wait_ms":5000})
        }
        "setup:draft.commit" | "setup:finalize" => params = serde_json::json!({"force":true}),
        "projects:projects.update" => params["name"] = serde_json::json!("matrix-project-updated"),
        _ => {}
    }
    params
}

#[test]
fn replacement_cases_request_values_distinct_from_their_owned_baselines() {
    for (key, field, baseline, expected) in [
        (
            "gateway:gateway.code_mode.set",
            "timeout_ms",
            serde_json::json!(5000),
            serde_json::json!(6000),
        ),
        (
            "gateway:gateway.loadout.update",
            "expose_tools",
            serde_json::json!(true),
            serde_json::json!(false),
        ),
    ] {
        let intent = mcp_intents()
            .into_iter()
            .find(|intent| intent.key() == key)
            .expect("required replacement case");
        let params = mcp_transition_params(intent);
        let payload = if key.ends_with("loadout.update") {
            &params["loadout"]
        } else {
            &params
        };
        assert_eq!(payload[field], expected, "{key} requests its exact target");
        assert_ne!(
            payload[field], baseline,
            "{key} cannot be an idempotent write"
        );
    }
}

#[test]
fn both_commit_entry_points_request_owned_value_replacement() {
    for key in ["setup:draft.commit", "setup:finalize"] {
        let intent = mcp_intents()
            .into_iter()
            .find(|intent| intent.key() == key)
            .expect("required commit case");
        assert_eq!(
            mcp_transition_params(intent)["force"],
            true,
            "{key} must replace the prior owned log value"
        );
    }
}

struct OwnedEnvRepair {
    env: std::path::PathBuf,
    backup: std::path::PathBuf,
}

impl OwnedEnvRepair {
    fn begin(runner: &BuiltinMcpRunner) -> Self {
        Self::begin_owned(&runner.owned_labby_home())
    }

    fn begin_owned(home: &std::path::Path) -> Self {
        let env = home.join(".env");
        let backup = home.join(".env.matrix-repair-backup");
        assert!(
            !backup.exists(),
            "repair backup must not replace another file"
        );
        std::fs::rename(&env, &backup).expect("move only the owned environment aside");
        Self { env, backup }
    }
}

impl Drop for OwnedEnvRepair {
    fn drop(&mut self) {
        // Even assertion unwinding restores only this runner's environment.
        // Authentication databases and other durable files are never rewound.
        if self.backup.exists() {
            if self.env.exists() {
                std::fs::remove_file(&self.env).expect("remove owned repair output");
            }
            std::fs::rename(&self.backup, &self.env).expect("restore owned environment");
        }
    }
}

#[test]
fn repair_fixture_restores_owned_environment_after_assertion_unwind() {
    let home = tempfile::tempdir().expect("isolated repair fixture");
    let env = home.path().join(".env");
    std::fs::write(&env, "LABBY_LOG=labby=debug\n").expect("owned original environment");
    let failure = std::panic::catch_unwind(|| {
        let _backup = OwnedEnvRepair::begin_owned(home.path());
        assert!(!env.exists());
        std::fs::write(&env, "").expect("simulated actual repair output");
        panic!("simulated subsequent oracle failure");
    });
    assert!(failure.is_err());
    assert_eq!(
        std::fs::read_to_string(&env).expect("restored owned environment"),
        "LABBY_LOG=labby=debug\n"
    );
    assert!(!home.path().join(".env.matrix-repair-backup").exists());
}

async fn owned_restart_pid(runner: &BuiltinMcpRunner) -> u64 {
    let result = runner
        .call(
            "gateway",
            "gateway.mcp.list",
            serde_json::Map::from_iter([(
                "name".into(),
                serde_json::json!("matrix-restart-owned"),
            )]),
        )
        .await
        .expect("owned runtime readback");
    assert_ne!(result.is_error, Some(true));
    let rows = mcp_action_data(&result_text(&result), "gateway", "gateway.mcp.list");
    let row = rows
        .as_array()
        .expect("runtime rows")
        .iter()
        .find(|row| row["name"] == "matrix-restart-owned")
        .expect("exact owned runtime");
    assert_eq!(row["connected"], true, "owned fixture must be connected");
    assert!(
        row["discovered_tool_count"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "owned fixture must advertise tools"
    );
    row["pid"].as_u64().expect("owned live child PID")
}

async fn assert_mcp_transition_readback(
    runner: &BuiltinMcpRunner,
    intent: &action_matrix::CaseIntent,
    mutation_text: &str,
    restart_before: Option<u64>,
    env_before: Option<&[u8]>,
) -> bool {
    let key = intent.key();
    let key_for_read = key.as_str();
    let read = |service: &'static str, action: &'static str, params: serde_json::Value| async move {
        let result = runner
            .call(
                service,
                action,
                params.as_object().cloned().expect("readback params object"),
            )
            .await
            .unwrap_or_else(|error| panic!("{key_for_read} readback wire failure: {error}"));
        (result.is_error != Some(true), result_text(&result))
    };

    match key.as_str() {
        "gateway:gateway.add" | "gateway:gateway.update" => {
            let (ok, text) = read(
                "gateway",
                "gateway.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            if key.ends_with("update") {
                assert!(
                    text.contains("127.0.0.1:10"),
                    "{key} patch was not observable: {text}"
                );
            }
            true
        }
        "gateway:gateway.code_mode.set" => {
            let (ok, text) = read("gateway", "gateway.code_mode.get", serde_json::json!({})).await;
            assert!(ok && !text.trim().is_empty(), "{key} readback: {text}");
            assert_eq!(
                result_error_kind(mutation_text),
                None,
                "{key} returned an error-shaped success"
            );
            let config = mcp_action_data(&text, "gateway", "gateway.code_mode.get");
            assert_eq!(config["timeout_ms"], 6000, "{key} persisted timeout");
            true
        }
        "gateway:gateway.loadout.add"
        | "gateway:gateway.loadout.patch"
        | "gateway:gateway.loadout.update" => {
            let (ok, text) = read(
                "gateway",
                "gateway.loadout.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            if key.ends_with("patch") {
                let loadout = mcp_action_data(&text, "gateway", "gateway.loadout.get");
                assert_eq!(loadout["expose_tools"], false, "{key} patch readback");
            }
            if key.ends_with("update") {
                let loadout = mcp_action_data(&text, "gateway", "gateway.loadout.get");
                assert_eq!(loadout["expose_tools"], false, "{key} replacement readback");
            }
            true
        }
        "gateway:gateway.loadout.remove" => {
            let (ok, text) = read(
                "gateway",
                "gateway.loadout.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "gateway:gateway.remove" => {
            let (ok, text) = read(
                "gateway",
                "gateway.get",
                serde_json::json!({"name":"matrix-remove-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "gateway:gateway.mcp.disable" | "gateway:gateway.mcp.enable" => {
            let (ok, text) = read(
                "gateway",
                "gateway.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(ok, "{key} readback: {text}");
            let gateway = mcp_action_data(&text, "gateway", "gateway.get");
            assert_eq!(gateway["config"]["name"], "matrix-owned");
            assert_eq!(
                gateway["config"]["enabled"],
                key.ends_with("enable"),
                "{key} must change the persisted enabled state: {text}"
            );
            true
        }
        "gateway:gateway.mcp.restart" => {
            let mutation = mcp_action_data(mutation_text, "gateway", "gateway.mcp.restart");
            assert_eq!(mutation["completed"], true, "owned restart must settle");
            let before = restart_before.expect("owned restart baseline PID");
            let after = owned_restart_pid(runner).await;
            assert_ne!(before, after, "restart must replace the owned child");
            #[cfg(target_os = "linux")]
            assert!(
                !std::path::Path::new(&format!("/proc/{before}")).exists(),
                "old owned child must be reaped"
            );
            assert!(
                runner
                    .list_tool_names()
                    .await
                    .expect("replacement published tools")
                    .contains("fixture.echo"),
                "replacement fixture tool must be published"
            );
            let echo = runner
                .call(
                    "matrix-restart-owned::fixture.echo",
                    "restart-readback",
                    serde_json::Map::from_iter([(
                        "marker".into(),
                        serde_json::json!("owned-restart-live"),
                    )]),
                )
                .await
                .expect("replacement tool call");
            assert_ne!(
                echo.is_error,
                Some(true),
                "{}",
                runner.wire_failure(
                    "gateway:gateway.mcp.restart",
                    "replacement_echo_readback",
                    "matrix-restart-owned::fixture.echo",
                    &result_text(&echo),
                    &[],
                )
            );
            let echo: serde_json::Value =
                serde_json::from_str(&result_text(&echo)).expect("fixture echo JSON");
            assert_eq!(
                echo["arguments"]["params"]["marker"], "owned-restart-live",
                "replacement must execute the actual fixture tool"
            );
            true
        }
        "gateway:gateway.protected_route.add" | "gateway:gateway.protected_route.update" => {
            let (ok, text) = read(
                "gateway",
                "gateway.protected_route.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            if key.ends_with("update") {
                let route = mcp_action_data(&text, "gateway", "gateway.protected_route.get");
                assert_eq!(
                    route["public_path"], "/matrix-owned",
                    "{key} replacement readback"
                );
            }
            true
        }
        "gateway:gateway.protected_route.remove" => {
            let (ok, text) = read(
                "gateway",
                "gateway.protected_route.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "gateway:gateway.reload" => {
            assert!(
                mutation_text.contains("completed") && mutation_text.contains("true"),
                "{key} did not prove reload completion: {mutation_text}"
            );
            true
        }
        "snippets:snippets.replay" => {
            let run: serde_json::Value = serde_json::from_str(mutation_text).expect("replay JSON");
            let run = action_scenarios::snippet_response_data("snippets.replay", &run);
            assert_eq!(run["receipt_status"], "persisted", "replay receipt: {run}");
            let id = run["execution_id"]
                .as_str()
                .expect("replay execution identifier");
            let (ok, text) = read(
                "snippets",
                "snippets.receipt",
                serde_json::json!({"execution_id":id}),
            )
            .await;
            assert!(ok, "{key} retained replay readback: {text}");
            let response: serde_json::Value =
                serde_json::from_str(&text).expect("replay receipt JSON");
            let receipt = action_scenarios::snippet_response_data("snippets.receipt", &response);
            assert_eq!(
                receipt["execution_id"], id,
                "{key} retained replay identifier"
            );
            true
        }
        "snippets:snippets.create" => {
            let (ok, text) = read(
                "snippets",
                "snippets.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                ok && text.contains("matrix-owned"),
                "{key} readback: {text}"
            );
            true
        }
        "snippets:snippets.remove" => {
            let (ok, text) = read(
                "snippets",
                "snippets.get",
                serde_json::json!({"name":"matrix-owned"}),
            )
            .await;
            assert!(
                !ok && text.contains("not_found"),
                "{key} absence proof: {text}"
            );
            true
        }
        "setup:draft.set" => {
            let (ok, text) = read("setup", "draft.get", serde_json::json!({})).await;
            assert!(ok && text.contains("LABBY_LOG"), "{key} readback: {text}");
            true
        }
        "setup:draft.discard" => {
            let (ok, text) = read("setup", "draft.get", serde_json::json!({})).await;
            assert!(
                ok && !text.contains("LABBY_LOG"),
                "{key} absence proof: {text}"
            );
            true
        }
        "setup:settings.update" => {
            let mutation = mcp_action_data(mutation_text, "setup", "settings.update");
            assert_eq!(
                mutation["changed"], true,
                "settings mutation must change the policy"
            );
            assert_eq!(
                mutation["previous"]["services"]["built_in_upstream_apis_enabled"],
                true
            );
            let (ok, text) = read(
                "setup",
                "settings.state",
                serde_json::json!({"section":"features"}),
            )
            .await;
            assert!(ok, "{key} readback: {text}");
            let settings = mcp_action_data(&text, "setup", "settings.state");
            assert_eq!(
                settings["values"]["services.built_in_upstream_apis_enabled"], false,
                "persisted settings must match the requested disabled policy"
            );
            true
        }
        "setup:draft.commit" | "setup:finalize" => {
            let mutation = mcp_action_data(mutation_text, "setup", &intent.action);
            let audit_failures = mutation["audit"]["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|finding| finding["severity"] == "fail")
                .take(8)
                .map(|finding| serde_json::json!({
                    "service": finding["service"],
                    "check": finding["check"],
                    "severity": finding["severity"],
                    "message": finding["message"].as_str().unwrap_or("[missing]").chars().take(128).collect::<String>(),
                }))
                .collect::<Vec<_>>();
            assert_ne!(
                mutation["ok"],
                false,
                "{}",
                runner.wire_failure(
                    &key,
                    "commit_audit_rejection",
                    &intent.action,
                    &serde_json::json!({"audit_failures": audit_failures}).to_string(),
                    &[],
                )
            );
            assert!(
                mutation["written"]
                    .as_u64()
                    .is_some_and(|written| written > 0),
                "{key} must write the staged owned key"
            );
            assert_eq!(
                mutation["audit_pass_count"], mutation["audit_total_count"],
                "commit requires a complete passing audit"
            );
            let env = std::fs::read(runner.owned_labby_home().join(".env"))
                .expect("committed owned environment");
            assert!(
                env.as_slice() != env_before.expect("owned environment baseline"),
                "commit must change durable environment bytes"
            );
            let expected = if intent.action == "finalize" {
                "LABBY_LOG=labby=trace"
            } else {
                "LABBY_LOG=labby=debug"
            };
            assert!(
                std::str::from_utf8(&env)
                    .expect("owned environment UTF-8")
                    .lines()
                    .any(|line| line == expected),
                "commit must persist the exact staged nonsecret log value"
            );
            assert!(
                !runner.owned_labby_home().join(".env.draft").exists(),
                "successful commit must clear its durable draft"
            );
            let (ok, text) = read("setup", "draft.get", serde_json::json!({})).await;
            assert!(
                ok && !text.contains("LABBY_LOG"),
                "committed draft must also be absent through dispatch"
            );
            true
        }
        "setup:repair" => {
            let mutation = mcp_action_data(mutation_text, "setup", "repair");
            assert_eq!(
                mutation["changed"], true,
                "repair must fix an induced owned defect"
            );
            assert_eq!(mutation["ran_repair"], true);
            assert_eq!(
                std::fs::read(runner.owned_labby_home().join(".env"))
                    .expect("repaired owned environment"),
                Vec::<u8>::new(),
                "repair creates the missing environment without invented values"
            );
            let (ok, text) = read("setup", "check", serde_json::json!({})).await;
            assert!(ok, "repair independent check failed");
            let check = mcp_action_data(&text, "setup", "check");
            let env_check = check["checks"]
                .as_array()
                .expect("setup checks")
                .iter()
                .find(|check| check["name"] == "env_file")
                .expect("environment file check");
            assert_eq!(
                env_check["ok"], true,
                "independent check must observe the repaired file"
            );
            true
        }
        // A multi-user mutation is only proven when the owning surface can
        // read the new state back through its own authority, so each of these
        // reads the collection the mutation changed and asserts the change is
        // visible rather than trusting the mutation's own response.
        "access:access.gateway_credential.bind" | "access:access.gateway_credential.revoke" => {
            let (ok, text) = read(
                "access",
                "access.gateway_credential.list",
                serde_json::json!({"team_id": live_labby::LiveLabbyGuard::HARNESS_TEAM_ID}),
            )
            .await;
            assert!(ok, "{key} readback: {text}");
            // Revocation is a tombstone, not a deletion: the binding stays
            // listed so an operator can see it was withdrawn.
            let binding = text
                .split("\"binding_id\"")
                .find(|entry| entry.contains("matrix-upstream"))
                .unwrap_or_else(|| panic!("{key} readback lost the binding: {text}"));
            let expected_status = if key.ends_with("bind") {
                "\"status\":\"active\""
            } else {
                "\"status\":\"revoked\""
            };
            assert!(
                binding.contains(expected_status),
                "{key} binding status did not follow the mutation: {text}"
            );
            true
        }
        "access:access.team.create" | "access:access.team.suspend" => {
            let (ok, text) = read("access", "access.team.list", serde_json::json!({})).await;
            assert!(ok, "{key} readback: {text}");
            let listed = mcp_action_data(&text, "access", "access.team.list");
            let params = action_scenarios::fixture_params(intent);
            let team = listed["teams"]
                .as_array()
                .expect("Team collection")
                .iter()
                .find(|team| team["team_id"] == params["team_id"])
                .expect("mutated Team remains listed");
            assert_eq!(
                team["status"],
                if key.ends_with("suspend") {
                    "suspended"
                } else {
                    "active"
                },
                "{key} must persist the requested Team status"
            );
            true
        }
        "access:access.team_project.assign" => {
            let params = mcp_transition_params(intent);
            let mutation = mcp_action_data(mutation_text, "access", "access.team_project.assign");
            for key in ["team_id", "project_id", "role"] {
                assert_eq!(
                    mutation[key], params[key],
                    "assignment must target the exact owned pair"
                );
            }
            assert!(
                mutation["assignment_epoch"]
                    .as_u64()
                    .is_some_and(|epoch| epoch > 0)
            );
            let (ok, text) = read("projects", "projects.get", params.clone()).await;
            assert!(ok, "{key} exact assignment readback: {text}");
            let project = mcp_action_data(&text, "projects", "projects.get");
            for key in ["team_id", "project_id", "role"] {
                assert_eq!(
                    project[key], params[key],
                    "persisted assignment must match the requested owned role"
                );
            }
            true
        }
        "agents:agents.create" | "agents:agents.delete" => {
            let (ok, text) = read("agents", "agents.list", serde_json::json!({})).await;
            assert!(ok, "{key} readback: {text}");
            let present = text.contains("matrix-agent");
            assert_eq!(
                present,
                key.ends_with("create"),
                "{key} agent visibility did not follow the mutation: {text}"
            );
            true
        }
        "projects:projects.create"
        | "projects:projects.update"
        | "projects:projects.archive"
        | "projects:projects.activate" => {
            // Archived Projects intentionally leave projects.list. Read the
            // exact owned Project so unrelated visible rows cannot prove it.
            let params = mcp_transition_params(intent);
            let (ok, text) = read("projects", "projects.get", params.clone()).await;
            assert!(ok, "{key} readback: {text}");
            let project = mcp_action_data(&text, "projects", "projects.get");
            assert_eq!(project["project_id"], params["project_id"]);
            assert_eq!(project["team_id"], params["team_id"]);
            assert_eq!(
                project["status"],
                if key.ends_with("archive") {
                    "disabled"
                } else {
                    "active"
                },
                "{key} must persist the requested Project status"
            );
            if key.ends_with("create") || key.ends_with("update") {
                assert_eq!(project["name"], params["name"]);
            }
            true
        }
        _ => false,
    }
}

/// Team-scoped Gateway policy may reference only upstreams that carry an
/// active Team credential binding, so a loadout or protected-route transition
/// must bind the harness Team to its upstream first. The binding is
/// host-custodied metadata; no secret travels through this call.
async fn bind_harness_team_upstream(runner: &BuiltinMcpRunner, upstream: &str, intent_key: &str) {
    let params = serde_json::json!({
        "team_id": live_labby::LiveLabbyGuard::HARNESS_TEAM_ID,
        "upstream_name": upstream,
        "binding_id": format!("matrix-{upstream}-binding"),
    });
    let result = runner
        .call(
            "access",
            "access.gateway_credential.bind",
            params.as_object().cloned().expect("binding params"),
        )
        .await
        .unwrap_or_else(|error| panic!("{intent_key} credential binding wire failure: {error}"));
    assert_ne!(
        result.is_error,
        Some(true),
        "{intent_key} credential binding failed: {}",
        result_text(&result)
    );
}

async fn prepare_mcp_transition(runner: &BuiltinMcpRunner, intent: &action_matrix::CaseIntent) {
    if intent.key() == "access:access.team_project.assign" {
        let mut params = mcp_transition_params(intent);
        assert_eq!(params["role"], "member");
        params["role"] = serde_json::json!("viewer");
        let baseline = runner
            .call(
                "access",
                "access.team_project.assign",
                params.as_object().unwrap().clone(),
            )
            .await
            .expect("owned viewer assignment prerequisite");
        assert_ne!(baseline.is_error, Some(true));
        let baseline = mcp_action_data(
            &result_text(&baseline),
            "access",
            "access.team_project.assign",
        );
        for key in ["team_id", "project_id", "role"] {
            assert_eq!(baseline[key], params[key]);
        }
        assert!(
            baseline["assignment_epoch"]
                .as_u64()
                .is_some_and(|epoch| epoch > 0)
        );
        // This projection joins the exact Team assignment; effective membership
        // can mask its role with a higher direct Project membership.
        let before = runner
            .call(
                "projects",
                "projects.get",
                params.as_object().unwrap().clone(),
            )
            .await
            .expect("owned assignment baseline readback");
        assert_ne!(before.is_error, Some(true));
        let before = mcp_action_data(&result_text(&before), "projects", "projects.get");
        for key in ["team_id", "project_id", "role"] {
            assert_eq!(before[key], params[key]);
        }
    }

    if matches!(
        intent.key().as_str(),
        "setup:draft.commit" | "setup:finalize"
    ) {
        // Earlier gateway cases deliberately use an unreachable HTTP target.
        // Disable only that owned target before the real setup audit; the
        // reachable restart fixture remains enabled and is still audited.
        let result = runner
            .call(
                "gateway",
                "gateway.update",
                serde_json::json!({"name":"matrix-owned", "patch":{"enabled":false}})
                    .as_object()
                    .cloned()
                    .expect("owned audit prerequisite"),
            )
            .await
            .expect("disable unreachable owned upstream before audit");
        assert_ne!(result.is_error, Some(true));
    }
    if intent.key() == "projects:projects.update" {
        let params = action_scenarios::fixture_params(intent);
        let before = runner
            .call(
                "projects",
                "projects.get",
                params
                    .as_object()
                    .cloned()
                    .expect("Project baseline params"),
            )
            .await
            .expect("Project baseline readback");
        assert_ne!(before.is_error, Some(true));
        let before = mcp_action_data(&result_text(&before), "projects", "projects.get");
        assert_eq!(
            before["name"], params["name"],
            "owned Project starts with create name"
        );
        assert_ne!(
            before["name"],
            mcp_transition_params(intent)["name"],
            "Project update must change the persisted name"
        );
    }
    // Team-scoped Gateway policy may reference only upstreams carrying an
    // active Team credential binding, so bind the harness Team before any
    // loadout or protected-route action, not just its transition prerequisite.
    if intent.action.starts_with("gateway.loadout.")
        || intent.action.starts_with("gateway.protected_route.")
    {
        bind_harness_team_upstream(runner, "matrix-owned", &intent.key()).await;
    }
    let prerequisite = match intent.key().as_str() {
        // Start on the opposite side so a successful no-op cannot satisfy the
        // enabled-state readback and be recorded as a live state transition.
        "gateway:gateway.mcp.disable" | "gateway:gateway.mcp.enable" => Some((
            "gateway.update",
            serde_json::json!({
                "name": "matrix-owned",
                "patch": {"enabled": intent.action.ends_with("disable")}
            }),
        )),
        "gateway:gateway.code_mode.set" => Some((
            "gateway.code_mode.set",
            serde_json::json!({"timeout_ms": 5000}),
        )),
        "setup:settings.update" => Some((
            "settings.update",
            serde_json::json!({"services.built_in_upstream_apis_enabled": true}),
        )),
        "setup:draft.commit" | "setup:finalize" | "setup:draft.discard" => Some((
            "draft.set",
            serde_json::json!({"force": true, "entries": [{
                "key": "LABBY_LOG",
                "value": if intent.action == "finalize" { "labby=trace" } else { "labby=debug" }
            }]}),
        )),
        "gateway:gateway.remove" => Some((
            "gateway.add",
            serde_json::json!({"spec": {
                "name": "matrix-remove-owned",
                "url": "http://127.0.0.1:9/mcp"
            }}),
        )),
        "gateway:gateway.loadout.update" => Some((
            "gateway.loadout.add",
            serde_json::json!({"loadout": {
                "name": "matrix-owned",
                "upstreams": ["matrix-owned"],
                "services": [],
                "expose_tools": true
            }}),
        )),
        "gateway:gateway.protected_route.update" => Some((
            "gateway.protected_route.add",
            serde_json::json!({"route": {
                "name": "matrix-owned",
                "enabled": true,
                "public_host": "matrix.invalid",
                "public_path": "/matrix-before",
                "upstream": "matrix-owned",
                "backend_url": "",
                "scopes": []
            }}),
        )),
        _ => None,
    };
    let Some((action, params)) = prerequisite else {
        return;
    };
    let service = if intent.service == "setup" {
        "setup"
    } else {
        "gateway"
    };
    let result = runner
        .call(
            service,
            action,
            params.as_object().cloned().expect("prerequisite params"),
        )
        .await
        .unwrap_or_else(|error| panic!("{} prerequisite wire failure: {error}", intent.key()));
    assert_ne!(
        result.is_error,
        Some(true),
        "{} prerequisite failed: {}",
        intent.key(),
        result_text(&result)
    );
    if matches!(
        intent.key().as_str(),
        "setup:draft.commit" | "setup:finalize" | "setup:draft.discard"
    ) {
        let expected_log = if intent.action == "finalize" {
            "labby=trace"
        } else {
            "labby=debug"
        };
        let before = runner
            .call("setup", "draft.get", serde_json::Map::new())
            .await
            .expect("staged draft baseline readback");
        assert_ne!(before.is_error, Some(true));
        let before = mcp_action_data(&result_text(&before), "setup", "draft.get");
        assert_eq!(
            before["entries"],
            serde_json::json!([{"key":"LABBY_LOG", "value":expected_log}]),
            "{} must start with the exact owned staged entry",
            intent.key()
        );
    }
    if intent.key() == "setup:settings.update" {
        let before = runner
            .call(
                "setup",
                "settings.state",
                serde_json::json!({"section":"features"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .await
            .expect("settings baseline readback");
        assert_ne!(before.is_error, Some(true));
        let before = mcp_action_data(&result_text(&before), "setup", "settings.state");
        assert_eq!(
            before["values"]["services.built_in_upstream_apis_enabled"], true,
            "settings policy must begin enabled"
        );
    }
    let baseline = match intent.key().as_str() {
        "gateway:gateway.code_mode.set" => Some((
            "gateway.code_mode.get",
            serde_json::json!({}),
            "timeout_ms",
            serde_json::json!(5000),
        )),
        "gateway:gateway.loadout.update" => Some((
            "gateway.loadout.get",
            serde_json::json!({"name":"matrix-owned"}),
            "expose_tools",
            serde_json::json!(true),
        )),
        "gateway:gateway.protected_route.update" => Some((
            "gateway.protected_route.get",
            serde_json::json!({"name":"matrix-owned"}),
            "public_path",
            serde_json::json!("/matrix-before"),
        )),
        _ => None,
    };
    if let Some((action, params, field, expected)) = baseline {
        let before = runner
            .call(
                "gateway",
                action,
                params.as_object().cloned().expect("baseline params"),
            )
            .await
            .expect("transition baseline readback");
        assert_ne!(before.is_error, Some(true));
        let before = mcp_action_data(&result_text(&before), "gateway", action);
        assert_eq!(
            before[field],
            expected,
            "{} must start with a different persisted value",
            intent.key()
        );
    }
    if matches!(
        intent.action.as_str(),
        "gateway.mcp.disable" | "gateway.mcp.enable"
    ) {
        let before = runner
            .call(
                "gateway",
                "gateway.get",
                serde_json::Map::from_iter([("name".into(), serde_json::json!("matrix-owned"))]),
            )
            .await
            .expect("toggle prerequisite readback");
        assert_ne!(before.is_error, Some(true));
        let before = mcp_action_data(&result_text(&before), "gateway", "gateway.get");
        assert_eq!(
            before["config"]["enabled"],
            intent.action.ends_with("disable"),
            "toggle must start with the opposite persisted state"
        );
    }
}

#[test]
fn every_mcp_visible_classification_has_one_bounded_execution_plan() {
    let cases = mcp_intents();
    let case_count = cases.len();
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
    assert_eq!(plans.len(), case_count);
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
    let mut expected = all_services
        .iter()
        .filter(|service| {
            !matches!(
                service.as_str(),
                "lab_admin" | "artifact_publish" | "bundles" | "jobs" | "sources" | "uploads"
            )
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if !cfg!(target_os = "linux") {
        expected.remove("stash");
    }
    let advertised_services = advertised
        .intersection(&all_services)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(advertised_services, expected);
    assert!(
        !advertised.contains("lab_admin"),
        "local-only tool leaked over HTTP MCP"
    );
    for unexpected in ["acp", "deploy", "fleet", "marketplace", "registry"] {
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
        if cfg!(target_os = "linux") {
            BTreeSet::from([
                "gateway".to_string(),
                "server_logs".to_string(),
                "snippets".to_string(),
                "stash".to_string(),
            ])
        } else {
            BTreeSet::from([
                "gateway".to_string(),
                "server_logs".to_string(),
                "snippets".to_string(),
            ])
        }
    );
    assert!(advertised.contains("codemode"));
    let validated = runner
        .call(
            "snippets",
            "snippets.validate",
            serde_json::json!({
                "name": "native-mcp-parity",
                "body": "async () => ({ ok: true })"
            })
            .as_object()
            .unwrap()
            .clone(),
        )
        .await
        .expect("native snippet validation over HTTP MCP");
    assert_ne!(
        validated.is_error,
        Some(true),
        "{}",
        result_text(&validated)
    );
    assert_eq!(
        validated.structured_content.as_ref().unwrap()["data"]["valid"],
        true
    );
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
    let owned_restart = tempfile::tempdir().expect("owned restart ledger directory");
    let fixture = serde_json::to_string(env!("CARGO_BIN_EXE_stdio-mcp-fixture"))
        .expect("fixture path TOML string");
    let pid_file =
        serde_json::to_string(&owned_restart.path().join("restart.pid")).expect("owned PID path");
    let config = format!(
        "[code_mode]\nenabled = false\n[gateway]\nextra_stdio_commands = [{fixture}]\n[[upstream]]\nname = \"matrix-restart-owned\"\ntransport = \"stdio\"\ncommand = {fixture}\nargs = [\"--pid-file\", {pid_file}]\n"
    );
    let runner = BuiltinMcpRunner::start_with_config(Some(&config))
        .await
        .expect("live MCP runner with owned restart fixture");
    action_scenarios::initialize_browser_fixture(runner.http_base_url()).await;
    let expected = mcp_intents()
        .into_iter()
        // lab_admin is intentionally local-only. artifact_publish is owned by a
        // protected team route and requires a bound user grant. Neither can be
        // exercised through the root HTTP MCP route owned by this runner.
        .filter(|intent| !matches!(intent.service.as_str(), "lab_admin" | "artifact_publish"))
        .collect::<Vec<_>>();
    let expected_count = expected.len();

    // Also prove guarded receipts on a separate owned admin operator route
    // that exposes native Snippets while the synthetic MCP surface is enabled.
    // The raw root runner covers the disabled advertisement regime. The scoped
    // route does not opt into Project asset execution; one verified credential
    // and route own every receipt.
    let snippet_identity =
        live_identity::LiveIdentity::bootstrap_snippet_receipt_harness("mcp-matrix-snippet-replay")
            .await
            .expect("isolated snippet receipt identity");
    let snippet_runner = BuiltinMcpRunner::connect_project(
        snippet_identity.base(),
        snippet_identity.credential_for_request(),
        mcp_action_runner::IdentityTuple::from_public(&snippet_identity.identity),
    )
    .await
    .expect("raw scoped snippet receipt MCP route");
    let snippet_canaries = snippet_identity.exact_secret_canaries();
    let snippet_identity = std::sync::Mutex::new(snippet_identity);
    assert_eq!(
        snippet_runner
            .list_tool_names()
            .await
            .expect("snippet route tools/list"),
        BTreeSet::from([
            "gateway".to_owned(),
            "snippets".to_owned(),
            "mcp_app".to_owned()
        ])
    );

    let mut consumed = BTreeSet::new();
    for intent in expected {
        prepare_mcp_transition(&runner, intent).await;
        let action_runner = if matches!(
            intent.action.as_str(),
            "snippets.preview" | "snippets.replay"
        ) {
            &snippet_runner
        } else {
            &runner
        };
        let restart_before = if intent.key() == "gateway:gateway.mcp.restart" {
            Some(owned_restart_pid(action_runner).await)
        } else {
            None
        };
        let env_before = matches!(
            intent.key().as_str(),
            "setup:draft.commit" | "setup:finalize"
        )
        .then(|| {
            std::fs::read(action_runner.owned_labby_home().join(".env")).unwrap_or_else(|error| {
                assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::NotFound,
                    "owned environment baseline read"
                );
                Vec::new()
            })
        });
        let repair_backup =
            (intent.key() == "setup:repair").then(|| OwnedEnvRepair::begin(action_runner));
        let fixture_runner = action_runner;
        let matrix_action = intent.key();
        let fixture_matrix_action = matrix_action.as_str();
        let fixture_canaries = &snippet_canaries;
        let fixture_identity = &snippet_identity;
        let prepared = action_scenarios::prepare_snippet_receipt_case(
            &intent.action,
            |action, params| async move {
                let result = fixture_runner
                    .call(
                        "snippets",
                        action,
                        params.as_object().cloned().expect("snippet fixture params"),
                    )
                    .await
                    .unwrap_or_else(|error| {
                        panic!(
                            "{}\n{}",
                            fixture_runner.wire_failure(
                                fixture_matrix_action,
                                "snippet_receipt_fixture",
                                action,
                                &error,
                                fixture_canaries,
                            ),
                            fixture_identity
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner())
                                .diagnostic_daemon_evidence(),
                        )
                    });
                let text = result_text(&result);
                assert_ne!(
                    result.is_error,
                    Some(true),
                    "{action} receipt fixture failed: {text}"
                );
                action_scenarios::snippet_mcp_action_data(
                    serde_json::from_str(&text).expect("snippet fixture JSON"),
                    action,
                )
            },
        )
        .await;
        let prepared_case = prepared.is_some();
        let params = prepared
            .unwrap_or_else(|| mcp_transition_params(intent))
            .as_object()
            .cloned()
            .expect("fixture params are an object");
        let result = action_runner
            .call(&intent.service, &intent.action, params)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{}\n{}",
                    action_runner.wire_failure(
                        &intent.key(),
                        "matrix_action",
                        &intent.action,
                        &error,
                        &snippet_canaries,
                    ),
                    snippet_identity
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .diagnostic_daemon_evidence(),
                )
            });
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
        if prepared_case && result.is_error != Some(true) {
            let value = action_scenarios::snippet_mcp_action_data(
                serde_json::from_str(&text).expect("snippet action JSON"),
                &intent.action,
            );
            action_scenarios::assert_snippet_receipt_case(&intent.action, &value);
        }
        let succeeded = result.is_error != Some(true);
        let transition_observed = succeeded
            && matches!(
                intent.scenario_kind,
                ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated
            )
            && assert_mcp_transition_readback(
                action_runner,
                intent,
                &text,
                restart_before,
                env_before.as_deref(),
            )
            .await;
        drop(repair_backup);
        let evidence = if succeeded {
            match intent.scenario_kind {
                ScenarioKind::ContractProbe => EvidenceLevel::MetadataOnly,
                ScenarioKind::LiveInvoke => EvidenceLevel::LiveSuccess,
                ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated => {
                    if transition_observed {
                        EvidenceLevel::LiveStateTransition
                    } else {
                        EvidenceLevel::LiveSuccess
                    }
                }
                ScenarioKind::ConditionalOptional => EvidenceLevel::RouterReachable,
                ScenarioKind::ExternalOptional | ScenarioKind::ExcludedWithReason => {
                    EvidenceLevel::LiveSuccess
                }
            }
        } else {
            EvidenceLevel::LiveErrorPath
        };
        let error_kind = (!succeeded)
            .then(|| result_error_kind(&text))
            .flatten()
            .unwrap_or_else(|| "mcp_error".to_owned());
        let dedicated = (!succeeded)
            .then(|| action_scenarios::dedicated_contract_reason_for(&intent.key(), Surface::Mcp))
            .flatten()
            .filter(|_| {
                action_scenarios::dedicated_contract_accepts_for(
                    &intent.key(),
                    Surface::Mcp,
                    &error_kind,
                )
            });
        action_scenarios::ActionOutcome {
            key: intent.key(),
            surface: Surface::Mcp,
            disposition: action_scenarios::disposition(intent),
            evidence,
            owner: intent.scenario_owner,
            outcome_kind: dedicated.map_or_else(
                || {
                    if succeeded {
                        "live_result"
                    } else {
                        &error_kind
                    }
                    .to_owned()
                },
                |reason| format!("dedicated_contract:{reason}:{error_kind}"),
            ),
            recovery: "isolated_runner".into(),
            side_effects: if succeeded {
                "owned_state"
            } else {
                "none_observed"
            }
            .into(),
            canary_free: !text.contains(action_scenarios::SECRET_CANARY),
        }
        .record();
        assert!(consumed.insert(intent.key()), "duplicate action execution");
    }
    assert_eq!(consumed.len(), expected_count);

    let cleanup = snippet_runner.finish().await;
    assert!(
        cleanup.is_clean(),
        "snippet client cleanup: {:?}",
        cleanup.failures
    );
    let cleanup = snippet_identity
        .into_inner()
        .unwrap_or_else(|poison| poison.into_inner())
        .cleanup()
        .await
        .expect("snippet identity cleanup");
    assert!(
        cleanup.is_clean(),
        "snippet identity cleanup: {:?}",
        cleanup.failures
    );
    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn artifact_publish_service_requires_the_protected_team_route_contract() {
    let runner = BuiltinMcpRunner::start().await.expect("live MCP runner");
    let intents = mcp_intents()
        .into_iter()
        .filter(|intent| intent.service == "artifact_publish")
        .collect::<Vec<_>>();
    assert_eq!(intents.len(), 3);

    for intent in intents {
        let params = action_scenarios::fixture_params(intent)
            .as_object()
            .cloned()
            .expect("fixture params are an object");
        let result = runner
            .call(&intent.service, &intent.action, params)
            .await
            .expect("root MCP returns a protocol result");
        let body = result_text(&result);
        assert_eq!(result.is_error, Some(true));
        let error_kind = result_error_kind(&body).unwrap_or_else(|| "mcp_error".to_owned());
        assert!(
            action_scenarios::dedicated_contract_accepts_for(
                &intent.key(),
                Surface::Mcp,
                &error_kind,
            ),
            "unexpected root-route error kind {error_kind}: {body}"
        );
        let reason = action_scenarios::dedicated_contract_reason_for(&intent.key(), Surface::Mcp)
            .expect("dedicated protected-route contract");
        action_scenarios::ActionOutcome {
            key: intent.key(),
            surface: Surface::Mcp,
            disposition: action_scenarios::disposition(intent),
            evidence: EvidenceLevel::LiveErrorPath,
            owner: intent.scenario_owner,
            outcome_kind: format!("dedicated_contract:{reason}:{error_kind}"),
            recovery: "isolated_root_route_probe".into(),
            side_effects: "none_observed".into(),
            canary_free: !body.contains(action_scenarios::SECRET_CANARY),
        }
        .record();
    }

    let cleanup = runner.finish().await;
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[cfg(feature = "lab-admin")]
#[tokio::test]
#[cfg(feature = "lab-admin")]
async fn local_stdio_executes_all_lab_admin_intents_before_recording_evidence() {
    let root = tempfile::tempdir().expect("local stdio MCP root");
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    std::fs::create_dir_all(root.path().join(".labby")).unwrap();
    std::fs::write(
        root.path().join(".labby/config.toml"),
        "[code_mode]\nenabled = false\n",
    )
    .unwrap();
    let mut command = support::isolated_command(root.path());
    command.env("LABBY_ADMIN_ENABLED", "1").arg("mcp");
    let runner = BuiltinMcpRunner::start_stdio(command)
        .await
        .expect("local stdio MCP runner");
    let tools = runner.list_tool_names().await.expect("stdio tools/list");
    assert!(
        tools.contains("lab_admin"),
        "local-only tool missing: {tools:?}"
    );

    let intents = mcp_intents()
        .into_iter()
        .filter(|intent| intent.service == "lab_admin")
        .collect::<Vec<_>>();
    let intent_count = intents.len();
    assert_eq!(intents.len(), 3);
    let mut consumed = BTreeSet::new();
    for intent in intents {
        let params = action_scenarios::fixture_params(intent)
            .as_object()
            .cloned()
            .expect("fixture params are an object");
        let result = runner
            .call("lab_admin", &intent.action, params)
            .await
            .unwrap_or_else(|error| panic!("{} stdio wire failure: {error}", intent.key()));
        let text = result_text(&result);
        assert_ne!(
            result.is_error,
            Some(true),
            "{} stdio execution failed: {text}",
            intent.key()
        );
        assert!(
            !text.trim().is_empty(),
            "{} returned no evidence",
            intent.key()
        );
        assert!(!text.contains(action_scenarios::SECRET_CANARY));

        action_scenarios::ActionOutcome {
            key: intent.key(),
            surface: Surface::Mcp,
            disposition: action_scenarios::disposition(intent),
            evidence: match intent.scenario_kind {
                ScenarioKind::ContractProbe => EvidenceLevel::MetadataOnly,
                ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated => {
                    EvidenceLevel::LiveSuccess
                }
                _ => EvidenceLevel::LiveSuccess,
            },
            owner: intent.scenario_owner,
            outcome_kind: "live_stdio_result".into(),
            recovery: "isolated_stdio_runner".into(),
            side_effects: "owned_state".into(),
            canary_free: true,
        }
        .record();
        assert!(consumed.insert(intent.key()), "duplicate stdio execution");
    }
    assert_eq!(consumed.len(), intent_count);
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
    assert_eq!(
        tools,
        BTreeSet::from(["gateway".to_string(), "mcp_app".to_string()])
    );
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
        .call(
            "setup",
            "settings.state",
            serde_json::Map::from_iter([(
                "section".to_string(),
                serde_json::Value::String("core".to_string()),
            )]),
        )
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

#[tokio::test]
async fn selected_daemon_snippet_keeps_local_source_and_durable_receipts() {
    let guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .start()
        .await
        .expect("disposable snippet daemon");
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".labby/snippets")).unwrap();
    std::fs::create_dir_all(home.path().join("tmp")).unwrap();
    // The selected daemon deliberately has no copy of either local snippet.
    for (name, declaration, raw) in [
        ("remote-legacy", "", false),
        ("remote-deny-all", "tools: []\n", false),
        ("remote-bare-js", "", true),
    ] {
        let body = if raw {
            "async () => ({ok:true, source:'local-only', markdown:'```js'})".to_owned()
        } else {
            format!(
                "---\nname: {name}\ndescription: Local selected source\n{declaration}---\n```js\nasync () => ({{ok:true, source:'local-only'}})\n```\n"
            )
        };
        let extension = if raw { "js" } else { "md" };
        std::fs::write(
            home.path()
                .join(format!(".labby/snippets/{name}.{extension}")),
            &body,
        )
        .unwrap();
        let mut command = tokio::process::Command::from(live_labby::isolated_command(home.path()));
        // This exercises the root MCP endpoint; Team-bound CLI selection is a separate guarded contract.
        let mut authorization = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
        guard.authorize_cli(&mut authorization);
        for (key, value) in authorization.as_std().get_envs() {
            if let Some(value) = value {
                command.env(key, value);
            }
        }
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            command.args(["--json", "snippet", "run", name]).output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let response = response.get("data").unwrap_or(&response);
        assert_eq!(response["result"]["source"], "local-only", "{response}");
        assert_eq!(response["receipt_status"], "persisted", "{response}");
        assert!(
            response["execution_id"]
                .as_str()
                .unwrap()
                .starts_with("snippet_"),
            "{response}"
        );
        assert!(
            !guard
                .root()
                .join(format!("labby-home/snippets/{name}.md"))
                .exists(),
            "inline source must not publish into the daemon store"
        );
    }
    assert!(guard.finish().await.is_clean());
}

#[tokio::test]
async fn selected_daemon_snippet_failure_keeps_receipt_identity() {
    let guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_E2E_BOOTSTRAP_STATIC_OWNER", "1")
        .start()
        .await
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".labby/snippets")).unwrap();
    std::fs::create_dir_all(home.path().join("tmp")).unwrap();
    std::fs::write(
        home.path().join(".labby/snippets/remote-failure.js"),
        "async () => { throw new Error('synthetic execution failure'); }",
    )
    .unwrap();
    let mut command = tokio::process::Command::from(live_labby::isolated_command(home.path()));
    let mut authorization = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
    guard.authorize_cli(&mut authorization);
    for (key, value) in authorization.as_std().get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        command
            .args(["--json", "snippet", "run", "remote-failure"])
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!output.status.success());
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let response: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(
        response["error"]["receipt_status"], "persisted",
        "{response}"
    );
    assert!(
        response["error"]["execution_id"]
            .as_str()
            .unwrap()
            .starts_with("snippet_"),
        "{response}"
    );
    assert!(guard.finish().await.is_clean());
}
