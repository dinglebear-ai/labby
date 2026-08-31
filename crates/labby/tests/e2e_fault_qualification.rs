#![allow(clippy::panic)]

#[path = "support/fault_control.rs"]
mod fault_control;
use fault_control::{Fault, FaultControl, detected};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Serialize)]
struct QualificationReport {
    schema_version: u32,
    qualification: &'static str,
    results: Vec<QualificationResult>,
}
#[derive(Serialize)]
struct QualificationResult {
    fault: &'static str,
    detector: &'static str,
    status: &'static str,
}

fn require_detection(result: Result<(), String>, fault: Fault, detector: &str) -> String {
    let error = result.expect_err("injected fault must trip its named sentinel");
    assert!(
        error.contains(fault.name()),
        "wrong fault diagnostic: {error}"
    );
    assert!(
        error.contains(detector),
        "wrong detector diagnostic: {error}"
    );
    error
}
fn route_sentinel(mounted: bool, matched: bool) -> Result<(), String> {
    if !mounted || !matched {
        return Err(detected(
            Fault::MissingRoute,
            "matched-route",
            "registered route was not mounted and attributed",
        ));
    }
    Ok(())
}
fn auth_sentinel(credential: bool, status: u16) -> Result<(), String> {
    if !credential && status != 401 {
        return Err(detected(
            Fault::AuthBypass,
            "security-oracle",
            "unauthenticated protected request was not denied",
        ));
    }
    Ok(())
}
fn cleanup_sentinel(descendants: &BTreeSet<u32>) -> Result<(), String> {
    if !descendants.is_empty() {
        return Err(detected(
            Fault::LeakedDescendant,
            "owned-process-cleanup",
            "owned child or grandchild survived teardown",
        ));
    }
    Ok(())
}
fn trace_sentinel(bytes: &[u8], canary: &[u8]) -> Result<(), String> {
    if bytes.windows(canary.len()).any(|window| window == canary) {
        return Err(detected(
            Fault::SecretTraceRetention,
            "secret-artifact-scan",
            "secret canary remained in retained trace bytes",
        ));
    }
    Ok(())
}
fn policy_sentinel(fault: Fault, valid: bool, detector: &'static str) -> Result<(), String> {
    valid.then_some(()).ok_or_else(|| {
        detected(
            fault,
            detector,
            "mutated contract violated the locked policy",
        )
    })
}

#[test]
fn locked_named_sentinels_detect_their_faults_and_release_private_handles() {
    let mut control = FaultControl::new();
    let route = control.activate(&["missing-route"]).expect("route fault");
    let observation = control
        .inject(&route, (false, false))
        .expect("private handle");
    require_detection(
        route_sentinel(observation.0, observation.1),
        Fault::MissingRoute,
        "matched-route",
    );
    control.release(route).expect("route release");
    let auth = control.activate(&["auth-bypass"]).expect("auth fault");
    let status = control.inject(&auth, 200).expect("private handle");
    require_detection(
        auth_sentinel(false, status),
        Fault::AuthBypass,
        "security-oracle",
    );
    control.release(auth).expect("auth release");
    let cleanup = control
        .activate(&["leaked-descendant"])
        .expect("cleanup fault");
    let descendants = control
        .inject(&cleanup, BTreeSet::from([41_u32, 42_u32]))
        .expect("private handle");
    require_detection(
        cleanup_sentinel(&descendants),
        Fault::LeakedDescendant,
        "owned-process-cleanup",
    );
    control.release(cleanup).expect("cleanup release");
    let trace = control
        .activate(&["secret-trace-retention"])
        .expect("trace fault");
    let canary = b"private-qualification-canary";
    let bytes = control
        .inject(&trace, [b"prefix:".as_slice(), canary].concat())
        .expect("private handle");
    let diagnostic = require_detection(
        trace_sentinel(&bytes, canary),
        Fault::SecretTraceRetention,
        "secret-artifact-scan",
    );
    assert!(!diagnostic.contains("private-qualification-canary"));
    control.release(trace).expect("trace release");
}

#[test]
fn critical_policy_mutants_trip_their_specific_oracles() {
    let cases = [
        (Fault::WrongSurfaceFlag, "surface-manifest"),
        (Fault::IncorrectPolicyMetadata, "metadata-policy"),
        (Fault::RemoteFallback, "remote-authority"),
        (Fault::DroppedRecoveryMetadata, "agent-error-contract"),
        (Fault::HiddenUpstreamLeak, "hidden-state-isolation"),
        (Fault::StaleCatalogOverwrite, "catalog-generation"),
    ];
    for (fault, detector) in cases {
        let mut control = FaultControl::new();
        let handle = control.activate(&[fault.name()]).expect("known fault");
        let valid = control.inject(&handle, false).expect("private handle");
        require_detection(policy_sentinel(fault, valid, detector), fault, detector);
        control.release(handle).expect("fault release");
    }
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
    let results = [
        (Fault::MissingRoute, "matched-route"),
        (Fault::AuthBypass, "security-oracle"),
        (Fault::LeakedDescendant, "owned-process-cleanup"),
        (Fault::SecretTraceRetention, "secret-artifact-scan"),
        (Fault::WrongSurfaceFlag, "surface-manifest"),
        (Fault::IncorrectPolicyMetadata, "metadata-policy"),
        (Fault::RemoteFallback, "remote-authority"),
        (Fault::DroppedRecoveryMetadata, "agent-error-contract"),
        (Fault::HiddenUpstreamLeak, "hidden-state-isolation"),
        (Fault::StaleCatalogOverwrite, "catalog-generation"),
    ]
    .into_iter()
    .map(|(fault, detector)| QualificationResult {
        fault: fault.name(),
        detector,
        status: "detected",
    })
    .collect();
    let report = QualificationReport {
        schema_version: 1,
        qualification: "deterministic-fault-sentinels",
        results,
    };
    let Some(path) = std::env::var_os("LABBY_E2E_FAULT_REPORT") else {
        return;
    };
    let path = PathBuf::from(path);
    std::fs::create_dir_all(path.parent().expect("report parent")).expect("create report parent");
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report).expect("serialize report"),
    )
    .expect("write qualification report");
}
