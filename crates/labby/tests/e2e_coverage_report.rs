#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/route_matrix.rs"]
mod route_matrix;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::Serialize;
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
}
#[derive(Serialize)]
struct RouteRow {
    key: String,
    classification: String,
    handler: String,
    runtime_condition: Option<String>,
    evidence_shards: Vec<String>,
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

#[test]
fn exact_catalog_join_emits_versioned_coverage_report() {
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
                    .map(|surface| match format!("{surface:?}").as_str() {
                        "Mcp" => "live-mcp-parity",
                        _ => "live-http-cli-api",
                    })
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(actions.len(), action_matrix::EXPECTED_ACTIONS);
    let routes = route_matrix::route_cases()
        .expect("route matrix")
        .into_iter()
        .map(|case| RouteRow {
            key: case.key(),
            classification: format!("{:?}", case.class),
            handler: case.descriptor.handler_identity,
            runtime_condition: case.descriptor.runtime_condition,
            evidence_shards: vec!["live-http-cli-api".to_owned()],
        })
        .collect::<Vec<_>>();
    assert!(!routes.is_empty());
    let Some(output) = std::env::var_os("LABBY_E2E_REPORT") else {
        return;
    };
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
    let declared = required("LABBY_E2E_DECLARED_SHARDS")
        .split(',')
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
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
