//! Exact-release Alloy qualification. CI T3 supplies the pinned artifacts.

use std::{collections::BTreeMap, num::NonZeroU64, path::PathBuf};
use verify_alloy::{AlloyBackend, AlloyHarness};
use verify_core::{Backend, CheckPlan, InvariantId, Verdict};

fn plan(handle: &str) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("LABBY-REQ-001".to_owned()).unwrap(),
        model: "browser_request".into(),
        handle: handle.into(),
        bounds: BTreeMap::from([("repeat".into(), 1.into())]),
        seed: None,
        timeout_ms: NonZeroU64::new(60_000).unwrap(),
    }
}

#[test]
#[ignore = "T3 only: requires LABBY_JAVA and the exact LABBY_ALLOY_JAR release"]
fn actual_alloy_positive_and_negative_controls() {
    let java = std::env::var_os("LABBY_JAVA").expect("LABBY_JAVA must be set by T3");
    let jar = std::env::var_os("LABBY_ALLOY_JAR").expect("LABBY_ALLOY_JAR must be set by T3");
    let module =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../formal/alloy/browser_request.als");
    let mut backend = AlloyBackend::new(java, jar);
    backend
        .register(
            "browser_request",
            "positive",
            AlloyHarness::new(&module, "SingleAuthoritativeTerminal").unwrap(),
        )
        .unwrap();
    backend
        .register(
            "browser_request",
            "negative",
            // Alloy names the final anonymous run command `run$3`; the predicate
            // name itself is not a command selector in the 6.2.0 CLI.
            AlloyHarness::new(&module, "run$3").unwrap(),
        )
        .unwrap();

    assert!(matches!(
        backend.run(&plan("positive")).verdict,
        Verdict::Bounded { .. }
    ));
    let negative = backend.run(&plan("negative"));
    assert!(
        matches!(negative.verdict, Verdict::Falsified { .. }),
        "unexpected negative-control report: {negative:?}"
    );
}
