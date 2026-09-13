#![allow(clippy::panic)]

#[path = "support/fault_control.rs"]
mod fault_control;
#[allow(dead_code)]
#[path = "support/route_matrix.rs"]
mod route_matrix;
use fault_control::{Fault, FaultControl};
use route_matrix::{RequestClass, invariant_for, route_cases};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
struct QualificationReport {
    schema_version: u32,
    qualification: &'static str,
    all_detectors_qualified: bool,
    results: Vec<QualificationResult>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Detected,
    Missed,
    BaselineFailed,
    Unverified,
}

#[derive(Debug, Serialize)]
struct QualificationResult {
    fault: &'static str,
    detector: &'static str,
    status: Status,
    evidence: String,
}

/// Qualification requires the same real detector to accept a healthy fixture
/// and reject its mutation. A missing detector or broken baseline is no proof.
fn qualify<T>(
    fault: Fault,
    detector: &'static str,
    healthy: &T,
    mutated: &T,
    run: impl Fn(&T) -> Result<(), String>,
) -> QualificationResult {
    let (status, evidence) = match run(healthy) {
        Err(error) => (Status::BaselineFailed, error),
        Ok(()) => match run(mutated) {
            Ok(()) => (Status::Missed, "mutated fixture was accepted".into()),
            Err(error) => (Status::Detected, error),
        },
    };
    QualificationResult {
        fault: fault.name(),
        detector,
        status,
        evidence,
    }
}

fn report_from_results(results: Vec<QualificationResult>) -> QualificationReport {
    let all_detectors_qualified = results.len() == Fault::ALL.len()
        && Fault::ALL.iter().all(|fault| {
            let mut matching = results.iter().filter(|result| result.fault == fault.name());
            matching
                .next()
                .is_some_and(|result| result.status == Status::Detected)
                && matching.next().is_none()
        });
    QualificationReport {
        schema_version: 1,
        qualification: "shared-detector-fixtures",
        all_detectors_qualified,
        results,
    }
}

/// Run independently of test ordering/filtering. This target qualifies detector
/// behavior against fixtures, not live server auth or process teardown itself.
fn run_qualification() -> Result<QualificationReport, String> {
    let route = route_cases()?
        .into_iter()
        .find(|case| case.class == RequestClass::Mcp)
        .ok_or("missing protected MCP route qualification fixture")?;
    let invariant = invariant_for(route.class);
    let mut malformed = route.descriptor.clone();
    malformed.auth_required = false;
    let mut results = vec![
        qualify(
            Fault::AuthBypass,
            "security-oracle",
            &reqwest::StatusCode::UNAUTHORIZED,
            &reqwest::StatusCode::OK,
            |status| invariant.validate_invalid_outcome(&route.descriptor, *status),
        ),
        qualify(
            Fault::IncorrectPolicyMetadata,
            "metadata-policy",
            &route.descriptor,
            &malformed,
            |descriptor| invariant.validate_descriptor(descriptor),
        ),
    ];
    // Keep coverage gaps visible. Listing a planned detector is not an executed
    // qualification, and local boolean replicas cannot stand in for it.
    for (fault, detector, reason) in [
        (
            Fault::MissingRoute,
            "matched-route",
            "live router attribution has no mutation fixture in this target",
        ),
        (
            Fault::LeakedDescendant,
            "owned-process-cleanup",
            "owned process teardown has no mutation fixture in this target",
        ),
        (
            Fault::SecretTraceRetention,
            "secret-artifact-scan",
            "retained evidence scanner has no mutation fixture in this target",
        ),
        (
            Fault::WrongSurfaceFlag,
            "surface-manifest",
            "surface manifest comparison has no mutation fixture in this target",
        ),
        (
            Fault::RemoteFallback,
            "remote-authority",
            "remote authority has no mutation fixture in this target",
        ),
        (
            Fault::DroppedRecoveryMetadata,
            "agent-error-contract",
            "recovery serialization has no mutation fixture in this target",
        ),
        (
            Fault::HiddenUpstreamLeak,
            "hidden-state-isolation",
            "scoped upstream isolation has no mutation fixture in this target",
        ),
        (
            Fault::StaleCatalogOverwrite,
            "catalog-generation",
            "catalog generation ordering has no mutation fixture in this target",
        ),
    ] {
        results.push(QualificationResult {
            fault: fault.name(),
            detector,
            status: Status::Unverified,
            evidence: reason.into(),
        });
    }
    Ok(report_from_results(results))
}

fn assert_executed_detectors_pass(report: &QualificationReport) {
    for result in &report.results {
        assert!(
            matches!(result.status, Status::Detected | Status::Unverified),
            "qualification fault={} detector={} status={:?}: {}",
            result.fault,
            result.detector,
            result.status,
            result.evidence,
        );
    }
}

#[test]
fn shared_detectors_detect_mutations_and_report_unverified_coverage() {
    let report = run_qualification().expect("qualification fixtures");
    assert_executed_detectors_pass(&report);
    assert_eq!(
        report
            .results
            .iter()
            .filter(|r| r.status == Status::Detected)
            .count(),
        2
    );
    assert_eq!(
        report
            .results
            .iter()
            .filter(|r| r.status == Status::Unverified)
            .count(),
        8
    );
    assert!(!report.all_detectors_qualified);
}

#[test]
fn bypassed_or_broken_detectors_cannot_claim_detection() {
    let bypassed = qualify(Fault::AuthBypass, "security-oracle", &401, &200, |_| Ok(()));
    assert_eq!(bypassed.status, Status::Missed);
    let broken = qualify(Fault::AuthBypass, "security-oracle", &401, &200, |_| {
        Err("always rejects".into())
    });
    assert_eq!(broken.status, Status::BaselineFailed);
    let empty = report_from_results(vec![]);
    assert!(!empty.all_detectors_qualified);
    assert!(empty.results.is_empty());
}

#[test]
fn invalid_conflicting_and_foreign_fault_handles_are_rejected() {
    let mut first = FaultControl::new();
    let second = FaultControl::new();
    assert!(first.activate(&["unknown-fault"]).is_err());
    assert!(first.activate(&["missing-route", "auth-bypass"]).is_err());
    let handle = first.activate(&["missing-route"]).expect("fault");
    assert!(second.inject(&handle, ()).is_err());
    first.release(handle).expect("release");
    assert!(first.activate(&["missing-route"]).is_ok());
}

#[test]
fn emit_separate_fault_qualification_report() {
    // Run the qualification here too: selecting only this test must never
    // manufacture results by assuming the other tests were executed.
    let report = run_qualification().expect("qualification fixtures");
    if let Some(path) = std::env::var_os("LABBY_E2E_FAULT_REPORT") {
        let path = PathBuf::from(path);
        std::fs::create_dir_all(path.parent().expect("report parent"))
            .expect("create report parent");
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).expect("serialize report"),
        )
        .expect("write qualification report");
    }
    assert_executed_detectors_pass(&report);
}
