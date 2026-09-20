//! Opt-in qualification against the exact Kani 0.67.0 bundle.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::PathBuf;

use verify_core::{Backend, CheckPlan, InvariantId, Verdict};
use verify_kani::{KaniBackend, KaniHarness};

fn plan(handle: &str, unwind: u32) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("LABBY-REQ-001".to_owned()).unwrap(),
        model: "browser_request".into(),
        handle: handle.into(),
        bounds: BTreeMap::from([
            ("max_output_bytes".into(), serde_json::json!(1_048_576)),
            ("unwind".into(), serde_json::json!(unwind)),
        ]),
        seed: None,
        timeout_ms: NonZeroU64::new(120_000).unwrap(),
    }
}

fn source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../formal/kani/browser_request.rs")
}

#[test]
#[ignore = "requires the exact Kani 0.67.0 bundle; set LABBY_KANI_DRIVER"]
fn exact_kani_bundle_proves_positive_and_falsifies_negative_control() {
    let executable = std::env::var_os("LABBY_KANI_DRIVER")
        .expect("LABBY_KANI_DRIVER must name the exact Kani 0.67.0 driver");
    let mut backend = KaniBackend::new(executable);
    backend
        .register(
            "browser_request",
            "request_single_terminal",
            KaniHarness::new(source(), "request_single_terminal").unwrap(),
        )
        .unwrap();
    backend
        .register(
            "browser_request",
            "request_single_terminal_negative",
            KaniHarness::new(source(), "request_single_terminal_negative").unwrap(),
        )
        .unwrap();

    let positive = backend.run(&plan("request_single_terminal", 8));
    assert!(
        matches!(positive.verdict, Verdict::Bounded { .. }),
        "positive harness was not bounded: {:?}",
        positive.verdict
    );
    assert_eq!(positive.tool_version.as_deref(), Some("0.67.0"));

    let incomplete = backend.run(&plan("request_single_terminal", 2));
    assert!(
        matches!(incomplete.verdict, Verdict::Incomplete { .. }),
        "under-unwound harness was not incomplete: {:?}",
        incomplete.verdict
    );

    let negative = backend.run(&plan("request_single_terminal_negative", 8));
    assert!(
        matches!(negative.verdict, Verdict::Falsified { .. }),
        "negative control was not falsified: {:?}",
        negative.verdict
    );
    assert!(negative.scenarios.is_empty());
}
