//! Contract, qualification, lane-separation, escaping, and renderer snapshots.

use std::collections::BTreeMap;

use insta::assert_snapshot;
use verify_core::InvariantId;
use verify_report::*;

fn invariant(value: &str) -> InvariantId {
    value.to_owned().try_into().unwrap()
}
fn artifact(id: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        id: id.into(),
        revision: Some("rev-1".into()),
        unavailable_reason: None,
    }
}
fn report() -> Report {
    let ids = vec![invariant("LABBY-SAFE-001"), invariant("LABBY-SAFE-002")];
    let observations = vec![
        EvidenceObservation {
            id: "replay-1".into(),
            lane: EvidenceLane::ModelReplay,
            requirement: Requirement::Required,
            verdict: EvidenceVerdict::Passed,
            invariant_ids: vec![ids[0].clone()],
            scenario_ids: vec!["b3:scenario".into()],
            sources: vec![SourceIdentity {
                source: "labby".into(),
                revision: Some("abc123".into()),
                dirty: Some(false),
                dirty_unavailable_reason: None,
                unavailable_reason: None,
            }],
            binary: None,
            fixtures: vec![],
            seed: Some(7),
            bounds: BTreeMap::from([("steps".into(), 4.into())]),
            deadline_ms: Some(1000),
            cleanup: None,
            diagnostic: Some("literal result".into()),
        },
        EvidenceObservation {
            id: "browser|fake".into(),
            lane: EvidenceLane::BrowserEmulation,
            requirement: Requirement::Optional,
            verdict: EvidenceVerdict::Passed,
            invariant_ids: vec![ids[0].clone()],
            scenario_ids: vec![],
            sources: vec![SourceIdentity {
                source: "browser fixture".into(),
                revision: Some("fixture-source-1".into()),
                dirty: None,
                dirty_unavailable_reason: Some("generated fixture".into()),
                unavailable_reason: None,
            }],
            binary: None,
            fixtures: vec![artifact("fake-dom")],
            seed: None,
            bounds: BTreeMap::from([("tabs".into(), 1.into())]),
            deadline_ms: Some(2000),
            cleanup: Some(CleanupEvidence {
                completed: true,
                detail: "closed <tab>".into(),
            }),
            diagnostic: Some("untrusted | <script>alert(1)</script>\nnext".into()),
        },
    ];
    Report {
        schema: 1,
        project: "labby <dev>".into(),
        catalog: artifact("formal/invariants.toml"),
        coverage: CatalogCoverage {
            invariant_ids: ids.clone(),
            replay_uncovered_ids: vec![ids[1].clone()],
            unconfigured_backend_ids: vec![ids[1].clone()],
            backend_uncovered_ids: ids.clone(),
        },
        scenarios: ScenarioCounts {
            total: 2,
            by_expectation: ExpectationCounts {
                invariant_holds: 1,
                invariant_violated: 1,
            },
            by_status: StatusCounts {
                active: 1,
                quarantined: 1,
                unreproduced: 0,
            },
        },
        observations,
    }
}

#[test]
fn snapshots_all_formats() {
    let report = report().validate().unwrap();
    assert_snapshot!("json", render_json(&report));
    assert_snapshot!("text", render_text(&report));
    assert_snapshot!("markdown", render_markdown(&report));
    assert_snapshot!("html", render_html(&report));
}

#[test]
fn rejects_bad_accounting() {
    let mut report = report();
    report.scenarios.total = 9;
    assert_eq!(
        report.validate().unwrap_err(),
        ReportError::Invalid("scenario expectation counts do not equal total")
    );
}
#[test]
fn required_skip_blocks_qualification() {
    let mut report = report();
    report.observations[0].verdict = EvidenceVerdict::Skipped;
    report.observations[0].diagnostic = Some("tool missing".into());
    assert_eq!(
        report.validate().unwrap().qualification(),
        Qualification::Blocked
    );
}
#[test]
fn bounded_never_qualifies_as_verified() {
    let mut report = report();
    report.observations[0].lane = EvidenceLane::ModelChecking;
    report.observations[0].verdict = EvidenceVerdict::Bounded;
    report.coverage.replay_uncovered_ids = report.coverage.invariant_ids.clone();
    report.coverage.backend_uncovered_ids = vec![invariant("LABBY-SAFE-002")];
    assert_eq!(
        report.validate().unwrap().qualification(),
        Qualification::Blocked
    );
}
#[test]
fn bounded_is_rejected_outside_model_checking() {
    let mut report = report();
    report.observations[0].verdict = EvidenceVerdict::Bounded;
    assert_eq!(
        report.validate().unwrap_err(),
        ReportError::Invalid("verified and bounded verdicts are restricted to model checking")
    );
}
#[test]
fn lanes_remain_separate() {
    let report = report().validate().unwrap();
    assert!(render_json(&report).contains("model_replay"));
    assert!(render_json(&report).contains("browser_emulation"));
}
#[test]
fn renderers_escape_untrusted_text() {
    let report = report().validate().unwrap();
    assert!(render_html(&report).contains("&lt;script&gt;"));
    assert!(!render_html(&report).contains("<script>"));
    let markdown = render_markdown(&report);
    assert!(markdown.contains("\\|"));
    assert!(markdown.contains("&lt;script&gt;"));
    assert!(!markdown.contains("<script>"));
}
#[test]
fn incomplete_requires_bounds_and_reason() {
    let mut report = report();
    report.observations[0].verdict = EvidenceVerdict::Incomplete;
    report.observations[0].bounds.clear();
    report.observations[0].diagnostic = None;
    assert!(report.validate().is_err());
}

#[test]
fn count_overflow_is_rejected() {
    let mut expectation_report = report();
    expectation_report.scenarios.total = 0;
    expectation_report.scenarios.by_expectation.invariant_holds = u64::MAX;
    expectation_report
        .scenarios
        .by_expectation
        .invariant_violated = 1;
    assert_eq!(
        expectation_report.validate().unwrap_err(),
        ReportError::Invalid("scenario expectation counts do not equal total")
    );

    let mut status_report = report();
    status_report.scenarios.total = 0;
    status_report.scenarios.by_expectation.invariant_holds = 0;
    status_report.scenarios.by_expectation.invariant_violated = 0;
    status_report.scenarios.by_status.active = u64::MAX;
    status_report.scenarios.by_status.quarantined = 1;
    status_report.scenarios.by_status.unreproduced = 0;
    assert_eq!(
        status_report.validate().unwrap_err(),
        ReportError::Invalid("scenario status counts do not equal total")
    );
}

#[test]
fn failed_cleanup_blocks_required_pass() {
    let mut report = report();
    report.observations[1].requirement = Requirement::Required;
    report.observations[1].cleanup.as_mut().unwrap().completed = false;
    assert_eq!(
        report.validate().unwrap().qualification(),
        Qualification::Blocked
    );
}

#[test]
fn implementation_pass_requires_execution_provenance() {
    let mut report = report();
    report.observations[1].sources.clear();
    report.observations[1].deadline_ms = Some(0);
    report.observations[1].cleanup = None;
    assert_eq!(
        report.validate().unwrap_err(),
        ReportError::Invalid(
            "implementation execution requires source, nonzero deadline, and cleanup evidence"
        )
    );
}

#[test]
fn dirty_state_requires_value_or_reason() {
    let mut report = report();
    report.observations[0].sources[0].dirty = None;
    assert_eq!(
        report.validate().unwrap_err(),
        ReportError::Invalid("dirty state requires a value or unavailable reason")
    );
}

#[test]
fn empty_catalog_and_skeletal_pass_are_rejected() {
    let mut empty = report();
    empty.coverage.invariant_ids.clear();
    empty.coverage.replay_uncovered_ids.clear();
    empty.coverage.unconfigured_backend_ids.clear();
    empty.coverage.backend_uncovered_ids.clear();
    empty.observations.clear();
    assert_eq!(
        empty.validate().unwrap_err(),
        ReportError::Invalid("catalog must contain unique invariant ids")
    );

    let mut skeletal = report();
    let observation = &mut skeletal.observations[0];
    observation.invariant_ids.clear();
    observation.scenario_ids.clear();
    observation.sources.clear();
    observation.bounds.clear();
    observation.deadline_ms = None;
    skeletal.coverage.replay_uncovered_ids = skeletal.coverage.invariant_ids.clone();
    assert_eq!(
        skeletal.validate().unwrap_err(),
        ReportError::Invalid("passing evidence requires an invariant id")
    );
}

#[test]
fn unavailable_metadata_blocks_nominal_pass() {
    let mut report = report();
    let source = &mut report.observations[0].sources[0];
    source.revision = None;
    source.unavailable_reason = Some("revision unavailable".into());
    source.dirty = None;
    source.dirty_unavailable_reason = Some("dirty state unavailable".into());
    assert_eq!(
        report.validate().unwrap().qualification(),
        Qualification::Blocked
    );
}

#[test]
fn controls_are_rejected_and_terminal_newlines_are_escaped() {
    for diagnostic in ["bad\u{1b}[31m", "bad\0"] {
        let mut report = report();
        report.observations[0].diagnostic = Some(diagnostic.into());
        assert_eq!(
            report.validate().unwrap_err(),
            ReportError::Invalid("text contains a disallowed control character")
        );
    }
    let report = report().validate().unwrap();
    assert!(render_text(&report).contains("untrusted | <script>alert(1)</script>\\nnext"));
}

#[test]
fn terminal_headers_escape_allowed_controls() {
    let mut report = report();
    report.project = "labby\nQualification: Passed".into();
    report.catalog.id = "catalog\tspoof".into();
    let output = render_text(&report.validate().unwrap());
    assert!(
        output.starts_with("Project: labby\\nQualification: Passed  catalog: catalog\\tspoof\n")
    );
    assert_eq!(output.matches("\nQualification:").count(), 1);
}

#[test]
fn backend_coverage_is_independent() {
    let mut report = report();
    report.coverage.backend_uncovered_ids.clear();
    assert_eq!(
        report.validate().unwrap_err(),
        ReportError::Invalid("backend uncovered invariant ids do not match observations")
    );
}

#[test]
fn backend_configuration_gaps_are_independent_and_validated() {
    let mut configured = report();
    configured.coverage.unconfigured_backend_ids.clear();
    configured.validate().unwrap();

    let mut duplicate = report();
    duplicate
        .coverage
        .unconfigured_backend_ids
        .push(invariant("LABBY-SAFE-002"));
    assert_eq!(
        duplicate.validate().unwrap_err(),
        ReportError::Invalid("invalid uncovered invariant ids")
    );
}

#[test]
fn encoded_input_is_bounded() {
    let input = " ".repeat(MAX_REPORT_BYTES + 1);
    assert_eq!(
        Report::from_json(&input).unwrap_err(),
        ReportError::Invalid("report exceeds encoded size limit")
    );
}
