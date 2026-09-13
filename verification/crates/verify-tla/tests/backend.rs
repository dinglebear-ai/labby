//! TLC adapter contract and bounded-process regressions.

use std::{collections::BTreeMap, fs, num::NonZeroU64, os::unix::fs::PermissionsExt, path::Path};
use tempfile::TempDir;
use verify_core::{Availability, Backend, CheckPlan, InvariantId, Verdict};
use verify_tla::{TlaBackend, TlaHarness};

fn fixture(script: &str) -> (TempDir, TlaBackend) {
    let dir = tempfile::tempdir().unwrap();
    let java = dir.path().join("java");
    fs::write(&java, script).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
    let jar = dir.path().join("tla2tools.jar");
    fs::write(&jar, b"fixture").unwrap();
    let mut backend = TlaBackend::new(java, jar)
        .with_expected_sha256("f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d");
    backend
        .register(
            "browser",
            "safe",
            TlaHarness::new("BrowserRequest.tla", "BrowserRequest.cfg").unwrap(),
        )
        .unwrap();
    (dir, backend)
}
fn plan(timeout: u64) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("LABBY-REQ-001".to_owned()).unwrap(),
        model: "browser".into(),
        handle: "safe".into(),
        bounds: BTreeMap::from([("workers".into(), 1.into())]),
        seed: None,
        timeout_ms: NonZeroU64::new(timeout).unwrap(),
    }
}

#[test]
fn metadata_is_honest_and_registration_does_not_probe() {
    let (_dir, backend) = fixture("#!/bin/sh\nexit 99\n");
    assert!(backend.has_handle("browser", "safe"));
    let capabilities = backend.capabilities();
    assert!(capabilities.bounded);
    assert!(!capabilities.fairness);
    assert!(!capabilities.concurrency);
    assert!(matches!(
        backend.availability(),
        Availability::Missing { .. }
    ));
    assert!(matches!(
        backend.run(&plan(100)).verdict,
        Verdict::Skipped { .. }
    ));
}

#[test]
fn exact_release_help_exit_one_is_available() {
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19'; exit 1;; *) echo 'Model checking completed. No error has been found';; esac\n",
    );
    assert!(matches!(backend.availability(), Availability::Ready { .. }));
    assert!(matches!(
        backend.run(&plan(1000)).verdict,
        Verdict::Bounded { .. }
    ));
}

#[test]
fn bounded_success_and_violation_are_distinct() {
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) echo 'Model checking completed. No error has been found';; esac\n",
    );
    assert!(matches!(
        backend.run(&plan(1000)).verdict,
        Verdict::Bounded { .. }
    ));
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) echo 'Error: Invariant SingleTerminal is violated.'; exit 12;; esac\n",
    );
    let report = backend.run(&plan(1000));
    assert!(
        matches!(report.verdict, Verdict::Falsified { .. }),
        "{report:?}"
    );
    assert!(
        report.scenarios.is_empty(),
        "TLC trace is not projectable without a project parser"
    );
}

#[test]
fn digest_mismatch_skips_before_executing() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let java = dir.path().join("java");
    fs::write(&java, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
    let mut backend = TlaBackend::new(java, dir.path().join("missing.jar"));
    backend
        .register("browser", "safe", TlaHarness::new("a", "b").unwrap())
        .unwrap();
    assert!(matches!(
        backend.run(&plan(100)).verdict,
        Verdict::Skipped { .. }
    ));
    assert!(!marker.exists());
}

#[test]
fn deadline_and_output_are_incomplete() {
    let (_dir, backend) =
        fixture("#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) sleep 1;; esac\n");
    assert!(matches!(
        backend.run(&plan(20)).verdict,
        Verdict::Incomplete { .. }
    ));
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) yes x | head -c 9000000;; esac\n",
    );
    assert!(matches!(
        backend.run(&plan(2000)).verdict,
        Verdict::Incomplete { .. }
    ));
}

#[test]
fn timeout_reaps_descendants_and_rejects_unframed_violation_text() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("descendant-survived");
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) (sleep .2; touch '{}') & sleep 2;; esac\n",
        marker.display()
    );
    let (_fixture, backend) = fixture(&script);
    assert!(matches!(
        backend.run(&plan(30)).verdict,
        Verdict::Incomplete { .. }
    ));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!marker.exists());

    let marker = dir.path().join("orphan-survived");
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) (sleep .2; touch '{}') & exit 0;; esac\n",
        marker.display()
    );
    let (_fixture, backend) = fixture(&script);
    assert!(!matches!(
        backend.run(&plan(1000)).verdict,
        Verdict::Bounded { .. } | Verdict::Falsified { .. }
    ));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!marker.exists());

    let (_fixture, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *) echo 'is violated'; exit 1;; esac\n",
    );
    let report = backend.run(&plan(1000));
    assert!(
        matches!(report.verdict, Verdict::Error { .. }),
        "{report:?}"
    );
}

#[test]
fn real_pins_are_documented() {
    assert_eq!(verify_tla::TLA_TOOLS_SHA256.len(), 64);
    assert!(verify_tla::APALACHE_IMAGE.contains("@sha256:"));
    assert!(!Path::new("missing").exists());
}

#[test]
fn duplicate_registration_preserves_the_original_harness() {
    let (_dir, mut backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-help*) echo 'TLC Version 2.19';; *BrowserRequest.tla*) echo 'Model checking completed. No error has been found';; *) exit 9;; esac\n",
    );
    assert!(
        backend
            .register(
                "browser",
                "safe",
                TlaHarness::new("replacement.tla", "replacement.cfg").unwrap(),
            )
            .is_err()
    );
    assert!(matches!(
        backend.run(&plan(1000)).verdict,
        Verdict::Bounded { .. }
    ));
}

#[test]
fn simulation_depth_is_rejected_before_probing() {
    let (dir, backend) = fixture("#!/bin/sh\nexit 99\n");
    // A nonexistent tool must not hide an invalid model-checking contract.
    fs::remove_file(dir.path().join("java")).unwrap();
    let mut request = plan(1000);
    request.bounds.insert("depth".into(), 1.into());
    let report = backend.run(&request);
    assert!(
        matches!(report.verdict, Verdict::Error { reason } if reason.contains("simulation-only"))
    );
    assert!(report.tool_version.is_none());
}

#[test]
fn successful_scope_records_deadline_without_simulation_depth() {
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *-depth*) exit 99;; *-help*) echo 'TLC Version 2.19'; exit 1;; *) echo 'Model checking completed. No error has been found';; esac\n",
    );
    let report = backend.run(&plan(1000));
    let Verdict::Bounded { bounds } = report.verdict else {
        panic!("expected successful finite model result");
    };
    assert_eq!(bounds["scope"], "registered_module_and_config");
    assert_eq!(bounds["timeout_ms"], 1000);
    assert_eq!(bounds["workers"], 1);
    assert!(!bounds.contains_key("depth"));
}
