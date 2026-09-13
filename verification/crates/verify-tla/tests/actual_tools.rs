//! Exact-release TLC qualification. CI T3 supplies the pinned artifacts.

use std::{collections::BTreeMap, num::NonZeroU64, path::PathBuf};
use verify_core::{Backend, CheckPlan, InvariantId, Verdict};
use verify_tla::{TlaBackend, TlaHarness};

fn plan(handle: &str) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("LABBY-REQ-001".to_owned()).unwrap(),
        model: "browser_request".into(),
        handle: handle.into(),
        bounds: BTreeMap::from([("workers".into(), 1.into()), ("depth".into(), 20.into())]),
        seed: None,
        timeout_ms: NonZeroU64::new(60_000).unwrap(),
    }
}

#[test]
#[ignore = "T3 only: requires LABBY_JAVA and the exact LABBY_TLA_TOOLS_JAR release"]
fn actual_tlc_positive_and_negative_controls() {
    let java = std::env::var_os("LABBY_JAVA").expect("LABBY_JAVA must be set by T3");
    let jar =
        std::env::var_os("LABBY_TLA_TOOLS_JAR").expect("LABBY_TLA_TOOLS_JAR must be set by T3");
    let formal = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../formal/tla");
    let mut backend = TlaBackend::new(java, jar);
    backend
        .register(
            "browser_request",
            "positive",
            TlaHarness::new(
                formal.join("BrowserRequest.tla"),
                formal.join("BrowserRequest.cfg"),
            )
            .unwrap(),
        )
        .unwrap();
    backend
        .register(
            "browser_request",
            "negative",
            TlaHarness::new(
                formal.join("BrokenBrowserRequest.tla"),
                formal.join("BrokenBrowserRequest.cfg"),
            )
            .unwrap(),
        )
        .unwrap();

    let positive = backend.run(&plan("positive"));
    assert!(
        matches!(positive.verdict, Verdict::Bounded { .. }),
        "unexpected positive-control report: {positive:?}"
    );
    let negative = backend.run(&plan("negative"));
    assert!(
        matches!(negative.verdict, Verdict::Falsified { .. }),
        "unexpected negative-control report: {negative:?}"
    );
}
