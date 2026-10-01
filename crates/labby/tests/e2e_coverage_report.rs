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
        "LiveRestartPersistence" => 5,
        "CrossSurfaceParity" => 6,
        "PackagedArtifactVerified" => 7,
        _ => return None,
    })
}

/// The per-case bar a single surface must clear for a declared minimum.
///
/// A per-case sweep records what one surface did, so a higher declaration
/// does not require that sweep alone to prove a restart, surface agreement, or
/// packaged artifact. Those proofs belong to their independently hashed shards.
/// A journey may record genuinely stronger observed evidence for a case; keep
/// that rank in the report while retaining this sweep floor and the separate
/// shard checks below. Without this floor higher declarations would be
/// unsatisfiable in a single-surface sweep. A surface that legitimately cannot
/// reach live state still answers its exact declared dedicated contract.
fn per_surface_floor(minimum: action_matrix::EvidenceLevel) -> u8 {
    let rank = minimum as u8;
    rank.min(action_matrix::EvidenceLevel::LiveStateTransition as u8)
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
                            achieved >= per_surface_floor(intent.minimum_evidence)
                                || dedicated_contract,
                            "{} evidence {} is below {:?}",
                            event.case_id,
                            event.achieved_evidence,
                            intent.minimum_evidence
                        );
                        event
                    })
                    .collect::<Vec<_>>()
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
        let mut accepted = event("action::Api::browser:browser.call", false, false);
        accepted.outcome_kind =
            "dedicated_contract:requires_live_consented_browser_document:stale_document".into();
        assert!(is_accepted_dedicated_contract(
            "browser:browser.call",
            crate::action_matrix::Surface::Api,
            &accepted
        ));

        let mut arbitrary = accepted.clone();
        arbitrary.outcome_kind =
            "dedicated_contract:requires_live_consented_browser_document:internal_error".into();
        assert!(!is_accepted_dedicated_contract(
            "browser:browser.call",
            crate::action_matrix::Surface::Api,
            &arbitrary
        ));
        assert!(!is_accepted_dedicated_contract(
            "gateway:gateway.get",
            crate::action_matrix::Surface::Api,
            &accepted
        ));
    }

    #[test]
    fn evidence_parser_accepts_every_declared_level_and_rejects_unknown_values() {
        use crate::action_matrix::EvidenceLevel;
        for level in [
            EvidenceLevel::MetadataOnly,
            EvidenceLevel::RouterReachable,
            EvidenceLevel::LiveErrorPath,
            EvidenceLevel::LiveSuccess,
            EvidenceLevel::LiveStateTransition,
            EvidenceLevel::LiveRestartPersistence,
            EvidenceLevel::CrossSurfaceParity,
            EvidenceLevel::PackagedArtifactVerified,
        ] {
            assert_eq!(
                super::evidence_rank(&format!("{level:?}")),
                Some(level as u8)
            );
        }
        for unknown in ["", "FutureEvidence", "live_restart_persistence", "8"] {
            assert_eq!(super::evidence_rank(unknown), None);
        }
        assert_eq!(
            super::per_surface_floor(EvidenceLevel::LiveRestartPersistence),
            4
        );
        assert_eq!(
            super::per_surface_floor(EvidenceLevel::CrossSurfaceParity),
            4
        );
        assert_eq!(
            super::per_surface_floor(EvidenceLevel::PackagedArtifactVerified),
            4
        );
    }

    fn write_fixture_event(root: &std::path::Path, event: &CaseEvent) -> std::path::PathBuf {
        use sha2::Digest as _;
        let path = root.join("cases").join(format!(
            "{}.json",
            hex::encode(sha2::Sha256::digest(event.case_id.as_bytes()))
        ));
        std::fs::write(&path, serde_json::to_vec(event).unwrap()).unwrap();
        path
    }

    /// Synthetic reporting inputs exercise the real joiner, never product
    /// runtime evidence. Every declared case and shard is present and run-bound.
    fn reporting_fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for directory in ["cases", "shards", "artifacts", "tmp"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
        }
        let levels = [
            "MetadataOnly",
            "RouterReachable",
            "LiveErrorPath",
            "LiveSuccess",
            "LiveStateTransition",
        ];
        for intent in crate::action_matrix::intents() {
            for surface in &intent.applicable_surfaces {
                if super::action_surface_shard(*surface).is_none() {
                    continue;
                }
                let mut case = event(
                    &format!("action::{surface:?}::{}", intent.key()),
                    false,
                    false,
                );
                case.achieved_evidence =
                    levels[usize::from(super::per_surface_floor(intent.minimum_evidence))].into();
                // Genuine journey observations may be stronger than the sweep
                // floor. Include every stronger vocabulary value in this replay.
                case.achieved_evidence = match intent.action.as_str() {
                    "stash.save_text" => "LiveRestartPersistence",
                    "stash.read_text" => "CrossSurfaceParity",
                    "stash.move" => "PackagedArtifactVerified",
                    _ => &case.achieved_evidence,
                }
                .to_owned();
                case.outcome_kind = "synthetic_report_fixture".into();
                write_fixture_event(root.path(), &case);
            }
        }
        for route in crate::route_matrix::route_cases().unwrap() {
            let mut case = event(&format!("route::{}", route.key()), false, false);
            case.kind = "route".into();
            write_fixture_event(root.path(), &case);
        }
        for shard in [
            "live-http-cli-api",
            "live-mcp-parity",
            "live-identity-protected-restart",
        ] {
            use sha2::Digest as _;
            let bytes = format!("synthetic reporting shard {shard}\n");
            std::fs::write(root.path().join(format!("{shard}.log")), &bytes).unwrap();
            let completion = serde_json::json!({
                "schema_version":1, "run_id":"run", "seed":"1", "build_identity":"build",
                "shard":shard, "status":"passed",
                "sha256":hex::encode(sha2::Sha256::digest(bytes.as_bytes())),
            });
            std::fs::write(
                root.path().join("shards").join(format!("{shard}.json")),
                serde_json::to_vec(&completion).unwrap(),
            )
            .unwrap();
        }
        root
    }

    async fn replay_report(
        root: &std::path::Path,
        overrides: &[(&str, &str)],
    ) -> std::process::Output {
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .env("HOME", root)
            .env("LABBY_HOME", root.join(".labby"))
            .env("TMPDIR", root.join("tmp"));
        for name in ["PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .kill_on_drop(true)
            .arg("--exact")
            .arg("exact_catalog_join_emits_versioned_coverage_report")
            .arg("--nocapture")
            .env("LABBY_E2E_REPORT", root.join("artifacts/coverage.json"))
            .env("LABBY_E2E_CASE_DIR", root.join("cases"))
            .env("LABBY_E2E_SHARD_DIR", root.join("shards"))
            .env(
                "LABBY_E2E_DECLARED_SHARDS",
                "live-http-cli-api,live-mcp-parity,live-identity-protected-restart",
            )
            .env("LABBY_E2E_RUN_ID", "run")
            .env("LABBY_E2E_SEED", "1")
            .env("LABBY_E2E_BUILD_IDENTITY", "build")
            .env("LABBY_E2E_CLEANUP_STATUS", "passed")
            .env("LABBY_E2E_EVIDENCE_STATUS", "passed")
            .envs(overrides.iter().copied());
        tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
            .await
            .expect("report replay deadline")
            .expect("report replay process")
    }

    fn assert_report_rejected(output: &std::process::Output, reason: &str) {
        assert!(
            !output.status.success(),
            "invalid reporting fixture was accepted"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(reason)
                || String::from_utf8_lossy(&output.stderr).contains(reason),
            "unexpected report rejection: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn report_mode_replay_accepts_stronger_evidence_and_keeps_join_guards() {
        let fixture = reporting_fixture();
        let root = fixture.path();
        let output = replay_report(root, &[]).await;
        assert!(
            output.status.success(),
            "report replay failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("artifacts/coverage.json")).unwrap())
                .unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(
            report["actions"].as_array().unwrap().len(),
            crate::action_matrix::EXPECTED_ACTIONS
        );
        assert_eq!(
            report["routes"].as_array().unwrap().len(),
            crate::route_matrix::PINNED_ROUTE_COUNT
        );
        assert_eq!(report["cleanup_status"], "passed");
        assert_eq!(report["evidence_status"], "passed");
        assert_eq!(report["shards"].as_object().unwrap().len(), 3);
        for (action, level) in [
            ("stash.save_text", "LiveRestartPersistence"),
            ("stash.read_text", "CrossSurfaceParity"),
            ("stash.move", "PackagedArtifactVerified"),
        ] {
            let row = report["actions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["key"] == format!("stash:{action}"))
                .unwrap();
            assert!(
                row["execution_outcomes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|event| event["achieved_evidence"] == level)
            );
        }
        let mut case = event("action::Api::stash:stash.move", false, false);
        case.achieved_evidence = "FutureEvidence".into();
        let path = write_fixture_event(root, &case);
        let original = std::fs::read(root.join("artifacts/coverage.json")).unwrap();
        assert_report_rejected(
            &replay_report(root, &[]).await,
            "unknown evidence FutureEvidence",
        );
        case.achieved_evidence = "LiveSuccess".into();
        write_fixture_event(root, &case);
        assert_report_rejected(
            &replay_report(root, &[]).await,
            "below LiveRestartPersistence",
        );
        std::fs::remove_file(path).unwrap();
        assert_report_rejected(
            &replay_report(root, &[]).await,
            "missing required per-case event",
        );
        case.achieved_evidence = "PackagedArtifactVerified".into();
        write_fixture_event(root, &case);
        case.case_id = "action::Api::stash:stash.unregistered".into();
        let extra = write_fixture_event(root, &case);
        assert_report_rejected(&replay_report(root, &[]).await, "unjoined case evidence");
        std::fs::remove_file(extra).unwrap();
        let log = root.join("live-identity-protected-restart.log");
        let original_log = std::fs::read(&log).unwrap();
        std::fs::write(&log, "modified after shard completion").unwrap();
        assert_report_rejected(&replay_report(root, &[]).await, "shard log hash mismatch");
        std::fs::write(log, original_log).unwrap();
        assert_report_rejected(
            &replay_report(root, &[("LABBY_E2E_CLEANUP_STATUS", "failed")]).await,
            "cleanup did not pass",
        );
        assert_report_rejected(
            &replay_report(root, &[("LABBY_E2E_EVIDENCE_STATUS", "failed")]).await,
            "evidence audit did not pass",
        );
        assert_eq!(
            std::fs::read(root.join("artifacts/coverage.json")).unwrap(),
            original,
            "rejected replay must not replace the successful report"
        );
    }
}
