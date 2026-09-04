#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/route_matrix.rs"]
mod route_matrix;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

#[derive(Serialize)]
struct Report<'a> {
    schema_version: u32,
    run_id: &'a str,
    seed: &'a str,
    build_identity: &'a str,
    feature_identity: &'static str,
    fixture_identity: &'static str,
    reproduction: &'static str,
    actions: Vec<ActionRow>,
    routes: Vec<RouteRow>,
    exclusions: Vec<ExclusionRow>,
    shards: BTreeMap<String, ShardCompletion>,
    cleanup_status: &'static str,
    evidence_status: &'static str,
}

#[derive(Serialize)]
struct ActionRow {
    key: String,
    classification: String,
    scenario: String,
    surfaces: Vec<String>,
    minimum_evidence: String,
    evidence_shards: Vec<String>,
    execution_outcomes: Vec<CaseEvent>,
}
#[derive(Serialize)]
struct RouteRow {
    key: String,
    classification: String,
    handler: String,
    runtime_condition: Option<String>,
    evidence_shards: Vec<String>,
    execution_outcome: CaseEvent,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct CaseEvent {
    schema_version: u32,
    run_id: String,
    seed: String,
    build_identity: String,
    case_id: String,
    kind: String,
    achieved_evidence: String,
    handler_success: bool,
    denial_only: bool,
    outcome_kind: String,
    cleanup_ok: bool,
}
#[derive(Serialize)]
struct ExclusionRow {
    key: String,
    reason: String,
    owner: String,
}
#[derive(Serialize)]
struct ShardCompletion {
    sha256: String,
    status: String,
    seed: String,
    build_identity: String,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn evidence_rank(value: &str) -> Option<u8> {
    Some(match value {
        "MetadataOnly" => 0,
        "RouterReachable" => 1,
        "LiveErrorPath" => 2,
        "LiveSuccess" => 3,
        "LiveStateTransition" => 4,
        _ => return None,
    })
}

fn action_surface_shard(surface: action_matrix::Surface) -> Option<&'static str> {
    match surface {
        action_matrix::Surface::Mcp => Some("live-mcp-parity"),
        // `browser-live` proves a representative end-to-end UI journey and is
        // retained as its own hashed shard. It does not emit one evidence file
        // for every catalog action exposed by the generic Web UI dispatcher.
        action_matrix::Surface::WebUi => None,
        action_matrix::Surface::Cli | action_matrix::Surface::Api => Some("live-http-cli-api"),
    }
}

fn take_required_event(events: &mut BTreeMap<String, CaseEvent>, case_id: &str) -> CaseEvent {
    events
        .remove(case_id)
        .unwrap_or_else(|| panic!("missing required per-case event {case_id}"))
}

fn validate_event_semantics(event: &CaseEvent) -> Result<(), String> {
    if !event.cleanup_ok {
        return Err(format!("case cleanup failed: {}", event.case_id));
    }
    if event.denial_only && event.handler_success {
        return Err(format!(
            "denial-only evidence cannot prove handler success: {}",
            event.case_id
        ));
    }
    Ok(())
}

fn is_accepted_dedicated_contract(
    key: &str,
    surface: action_matrix::Surface,
    event: &CaseEvent,
) -> bool {
    event.achieved_evidence == "LiveErrorPath"
        && event
            .outcome_kind
            .strip_prefix("dedicated_contract:")
            .and_then(|details| details.rsplit_once(':'))
            .is_some_and(|(reason, error_kind)| {
                action_scenarios::dedicated_contract_reason_for(key, surface) == Some(reason)
                    && action_scenarios::dedicated_contract_accepts_for(key, surface, error_kind)
            })
}

fn load_case_events() -> BTreeMap<String, CaseEvent> {
    let mut events = BTreeMap::new();
    for entry in fs::read_dir(required("LABBY_E2E_CASE_DIR")).expect("case evidence dir") {
        let path = entry.expect("case evidence entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let event: CaseEvent = serde_json::from_slice(&fs::read(&path).expect("case evidence"))
            .expect("case evidence json");
        assert_eq!(event.schema_version, 1);
        assert_eq!(event.run_id, required("LABBY_E2E_RUN_ID"));
        assert_eq!(event.seed, required("LABBY_E2E_SEED"));
        assert_eq!(event.build_identity, required("LABBY_E2E_BUILD_IDENTITY"));
        validate_event_semantics(&event).unwrap_or_else(|error| panic!("{error}"));
        assert!(events.insert(event.case_id.clone(), event).is_none());
    }
    events
}

#[test]
fn exact_catalog_join_emits_versioned_coverage_report() {
    let reporting = std::env::var_os("LABBY_E2E_REPORT").is_some();
    let declared_shards = reporting.then(|| {
        required("LABBY_E2E_DECLARED_SHARDS")
            .split(',')
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    });
    let mut case_events = if reporting {
        load_case_events()
    } else {
        BTreeMap::new()
    };
    let mut seen = BTreeSet::new();
    let mut exclusions = Vec::new();
    let actions = action_matrix::intents()
        .iter()
        .map(|intent| {
            let key = intent.key();
            assert!(seen.insert(key.clone()), "duplicate action {key}");
            if matches!(
                intent.scenario_kind,
                action_matrix::ScenarioKind::ExternalOptional
                    | action_matrix::ScenarioKind::ExcludedWithReason
            ) {
                exclusions.push(ExclusionRow {
                    key: key.clone(),
                    reason: format!("{:?}", intent.scenario_kind),
                    owner: format!("{:?}", intent.scenario_owner),
                });
            }
            let execution_outcomes = if !reporting {
                Vec::new()
            } else {
                intent
                    .applicable_surfaces
                    .iter()
                    .filter(|surface| {
                        action_surface_shard(**surface).is_some_and(|shard| {
                            declared_shards
                                .as_ref()
                                .is_none_or(|declared| declared.contains(shard))
                        })
                    })
                    .map(|surface| {
                        let case_id = format!("action::{surface:?}::{key}");
                        let event = take_required_event(&mut case_events, &case_id);
                        let achieved =
                            evidence_rank(&event.achieved_evidence).unwrap_or_else(|| {
                                panic!("unknown evidence {}", event.achieved_evidence)
                            });
                        let dedicated_contract =
                            is_accepted_dedicated_contract(&key, *surface, &event);
                        assert!(
                            achieved >= intent.minimum_evidence as u8 || dedicated_contract,
                            "{} evidence {} is below {:?}",
                            event.case_id,
                            event.achieved_evidence,
                            intent.minimum_evidence
                        );
                        event
                    })
                    .collect()
            };
            ActionRow {
                key,
                classification: format!("{:?}", intent.scenario_kind),
                scenario: intent.scenario_id.clone(),
                surfaces: intent
                    .applicable_surfaces
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect(),
                minimum_evidence: format!("{:?}", intent.minimum_evidence),
                evidence_shards: intent
                    .applicable_surfaces
                    .iter()
                    .filter_map(|surface| action_surface_shard(*surface))
                    .filter(|shard| {
                        declared_shards
                            .as_ref()
                            .is_none_or(|declared| declared.contains(*shard))
                    })
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                execution_outcomes,
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), action_matrix::EXPECTED_ACTIONS);
    let routes = route_matrix::route_cases()
        .expect("route matrix")
        .into_iter()
        .map(|case| {
            let key = case.key();
            let execution_outcome = if !reporting {
                CaseEvent {
                    schema_version: 1,
                    run_id: String::new(),
                    seed: String::new(),
                    build_identity: String::new(),
                    case_id: String::new(),
                    kind: String::new(),
                    achieved_evidence: String::new(),
                    handler_success: false,
                    denial_only: false,
                    outcome_kind: String::new(),
                    cleanup_ok: true,
                }
            } else {
                take_required_event(&mut case_events, &format!("route::{key}"))
            };
            RouteRow {
                key,
                classification: format!("{:?}", case.class),
                handler: case.descriptor.handler_identity,
                runtime_condition: case.descriptor.runtime_condition,
                evidence_shards: vec!["live-http-cli-api".to_owned()],
                execution_outcome,
            }
        })
        .collect::<Vec<_>>();
    assert!(!routes.is_empty());
    let Some(output) = std::env::var_os("LABBY_E2E_REPORT") else {
        return;
    };
    assert!(
        case_events.is_empty(),
        "unjoined case evidence: {:?}",
        case_events.keys()
    );
    let mut shards = BTreeMap::new();
    for entry in fs::read_dir(required("LABBY_E2E_SHARD_DIR")).expect("shard dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(path).expect("completion")).expect("completion json");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["run_id"], required("LABBY_E2E_RUN_ID"));
        assert_eq!(value["seed"], required("LABBY_E2E_SEED"));
        assert_eq!(
            value["build_identity"],
            required("LABBY_E2E_BUILD_IDENTITY")
        );
        let name = value["shard"].as_str().expect("shard name").to_string();
        let log = PathBuf::from(required("LABBY_E2E_SHARD_DIR"))
            .parent()
            .expect("run root")
            .join(format!("{name}.log"));
        let actual_hash = hex::encode(sha2::Sha256::digest(
            fs::read(&log).unwrap_or_else(|error| panic!("read {}: {error}", log.display())),
        ));
        assert_eq!(value["sha256"], actual_hash, "shard log hash mismatch");
        assert!(
            shards
                .insert(
                    name,
                    ShardCompletion {
                        sha256: value["sha256"].as_str().expect("hash").into(),
                        status: value["status"].as_str().expect("status").into(),
                        seed: value["seed"].as_str().expect("seed").into(),
                        build_identity: value["build_identity"].as_str().expect("build").into(),
                    }
                )
                .is_none()
        );
    }
    let declared = declared_shards.expect("reporting has declared shards");
    assert_eq!(shards.keys().cloned().collect::<BTreeSet<_>>(), declared);
    assert!(shards.values().all(|s| s.status == "passed"
        && s.sha256.len() == 64
        && s.seed == required("LABBY_E2E_SEED")
        && s.build_identity == required("LABBY_E2E_BUILD_IDENTITY")));
    for required_shard in actions
        .iter()
        .flat_map(|row| row.evidence_shards.iter())
        .chain(routes.iter().flat_map(|row| row.evidence_shards.iter()))
    {
        assert!(
            shards.contains_key(required_shard),
            "missing row evidence shard {required_shard}"
        );
    }
    let run_id = required("LABBY_E2E_RUN_ID");
    let seed = required("LABBY_E2E_SEED");
    let build = required("LABBY_E2E_BUILD_IDENTITY");
    let report = Report {
        schema_version: 1,
        run_id: &run_id,
        seed: &seed,
        build_identity: &build,
        feature_identity: "all-features",
        fixture_identity: "catalog-v1",
        reproduction: "just live-e2e <tier> <seed>",
        actions,
        routes,
        exclusions,
        shards,
        cleanup_status: match required("LABBY_E2E_CLEANUP_STATUS").as_str() {
            "passed" => "passed",
            other => panic!("cleanup did not pass: {other}"),
        },
        evidence_status: match required("LABBY_E2E_EVIDENCE_STATUS").as_str() {
            "passed" => "passed",
            other => panic!("evidence audit did not pass: {other}"),
        },
    };
    let bytes = serde_json::to_vec_pretty(&report).expect("serialize");
    assert!(bytes.len() < 4 * 1024 * 1024);
    let output = PathBuf::from(output);
    fs::create_dir_all(output.parent().expect("parent")).expect("mkdir");
    fs::write(output, bytes).expect("write");
}

#[cfg(test)]
mod tests {
    use super::{
        CaseEvent, is_accepted_dedicated_contract, take_required_event, validate_event_semantics,
    };
    use std::collections::BTreeMap;

    fn event(id: &str, handler_success: bool, denial_only: bool) -> CaseEvent {
        CaseEvent {
            schema_version: 1,
            run_id: "run".into(),
            seed: "1".into(),
            build_identity: "build".into(),
            case_id: id.into(),
            kind: "action".into(),
            achieved_evidence: "LiveErrorPath".into(),
            handler_success,
            denial_only,
            outcome_kind: "authorization_denial".into(),
            cleanup_ok: true,
        }
    }

    #[test]
    #[should_panic(expected = "missing required per-case event action::Api::doctor:help")]
    fn deleting_one_required_case_event_fails_the_join() {
        take_required_event(&mut BTreeMap::new(), "action::Api::doctor:help");
    }

    #[test]
    fn denial_only_event_is_not_handler_success() {
        let denial = event("action::Api::setup:install", false, true);
        assert!(validate_event_semantics(&denial).is_ok());
        let mislabeled = event("action::Api::setup:install", true, true);
        assert!(validate_event_semantics(&mislabeled).is_err());
    }

    #[test]
    fn dedicated_contract_requires_the_exact_action_reason_and_error_kind() {
        let mut accepted = event("action::Mcp::gateway:gateway.clients.list", false, false);
        accepted.outcome_kind =
            "dedicated_contract:catalog_dispatch_mismatch:unknown_action".into();
        assert!(is_accepted_dedicated_contract(
            "gateway:gateway.clients.list",
            crate::action_matrix::Surface::Api,
            &accepted
        ));

        let mut arbitrary = accepted.clone();
        arbitrary.outcome_kind =
            "dedicated_contract:catalog_dispatch_mismatch:internal_error".into();
        assert!(!is_accepted_dedicated_contract(
            "gateway:gateway.clients.list",
            crate::action_matrix::Surface::Api,
            &arbitrary
        ));
        assert!(!is_accepted_dedicated_contract(
            "gateway:gateway.get",
            crate::action_matrix::Surface::Api,
            &accepted
        ));
    }
}
