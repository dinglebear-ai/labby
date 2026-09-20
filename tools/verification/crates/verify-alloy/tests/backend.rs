//! Alloy adapter contract and bounded-process regressions.

use std::{collections::BTreeMap, fs, num::NonZeroU64, os::unix::fs::PermissionsExt};
use verify_alloy::{AlloyBackend, AlloyHarness};
use verify_core::{Availability, Backend, CheckPlan, InvariantId, Verdict};

fn fixture(script: &str) -> (tempfile::TempDir, AlloyBackend) {
    let dir = tempfile::tempdir().unwrap();
    let java = dir.path().join("java");
    fs::write(&java, script).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
    let jar = dir.path().join("alloy.jar");
    fs::write(&jar, b"fixture").unwrap();
    let mut backend = AlloyBackend::new(java, jar)
        .with_expected_sha256("f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d");
    backend
        .register(
            "browser",
            "safe",
            AlloyHarness::new("browser_request.als", "SingleAuthoritativeTerminal").unwrap(),
        )
        .unwrap();
    (dir, backend)
}
fn plan(timeout: u64) -> CheckPlan {
    CheckPlan {
        invariant: InvariantId::try_from("LABBY-REQ-001".to_owned()).unwrap(),
        model: "browser".into(),
        handle: "safe".into(),
        bounds: BTreeMap::from([("repeat".into(), 1.into())]),
        seed: None,
        timeout_ms: NonZeroU64::new(timeout).unwrap(),
    }
}

// Receipt-validation tests exercise parser and filesystem hardening, not deadline behavior.
// Leave enough room for the integrity/version probe when the CI test binary runs in parallel.
const RECEIPT_VALIDATION_TIMEOUT_MS: u64 = 5_000;

#[test]
fn metadata_is_honest_and_missing_skips() {
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
fn unsat_is_bounded_and_sat_is_falsified() {
    let (_dir, backend) = fixture(&alloy_script(false));
    assert!(matches!(
        backend.run(&plan(3000)).verdict,
        Verdict::Bounded { .. }
    ));
    let (_dir, backend) = fixture(&alloy_script(true));
    let report = backend.run(&plan(3000));
    assert!(matches!(report.verdict, Verdict::Falsified { .. }));
    assert!(
        report.scenarios.is_empty(),
        "Alloy instances need a project parser"
    );
}

#[test]
fn deadline_and_output_are_incomplete() {
    let (_dir, backend) =
        fixture("#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0;; *) sleep 1;; esac\n");
    assert!(matches!(
        backend.run(&plan(20)).verdict,
        Verdict::Incomplete { .. }
    ));
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0;; *) yes x | head -c 9000000;; esac\n",
    );
    assert!(matches!(
        backend.run(&plan(2000)).verdict,
        Verdict::Incomplete { .. }
    ));
}

#[test]
fn malformed_receipt_is_an_error() {
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0; exit;; esac\nwhile [ \"$#\" -gt 0 ]; do [ \"$1\" = -o ] && { shift; out=\"$1\"; }; shift; done\nmkdir -p \"$out\"; echo '{}' > \"$out/receipt.json\"\n",
    );
    let report = backend.run(&plan(RECEIPT_VALIDATION_TIMEOUT_MS));
    assert!(
        matches!(report.verdict, Verdict::Error { .. }),
        "malformed receipt returned {:?}",
        report.verdict,
    );
}

#[test]
fn malformed_solution_and_symlink_receipts_are_errors() {
    for body in [
        r#"{"commands":{"SingleAuthoritativeTerminal":{"name":"SingleAuthoritativeTerminal","type":"check","source":"check x","overall":1,"solution":false}}}"#,
        r#"{"commands":{"SingleAuthoritativeTerminal":{"solution":false}}}"#,
    ] {
        let script = format!(
            "#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0; exit;; esac\nwhile [ \"$#\" -gt 0 ]; do [ \"$1\" = -o ] && {{ shift; out=\"$1\"; }}; shift; done\nmkdir -p \"$out\"; printf '%s' '{}' > \"$out/receipt.json\"\n",
            body
        );
        let (_dir, backend) = fixture(&script);
        let report = backend.run(&plan(RECEIPT_VALIDATION_TIMEOUT_MS));
        assert!(
            matches!(report.verdict, Verdict::Error { .. }),
            "malformed receipt body {body:?} returned {:?}",
            report.verdict,
        );
    }
    let (_dir, backend) = fixture(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0; exit;; esac\nwhile [ \"$#\" -gt 0 ]; do [ \"$1\" = -o ] && { shift; out=\"$1\"; }; shift; done\nmkdir -p \"$out\"; ln -s /etc/passwd \"$out/receipt.json\"\n",
    );
    let report = backend.run(&plan(RECEIPT_VALIDATION_TIMEOUT_MS));
    assert!(
        matches!(report.verdict, Verdict::Error { .. }),
        "symlink receipt returned {:?}",
        report.verdict,
    );
}

#[test]
fn timeout_reaps_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("descendant-survived");
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0;; *) (sleep .2; touch '{}') & sleep 2;; esac\n",
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
        "#!/bin/sh\ncase \"$*\" in *version*) echo 6.2.0;; *) (sleep .2; touch '{}') & exit 0;; esac\n",
        marker.display()
    );
    let (_fixture, backend) = fixture(&script);
    assert!(!matches!(
        backend.run(&plan(1000)).verdict,
        Verdict::Bounded { .. } | Verdict::Falsified { .. }
    ));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!marker.exists());
}

#[test]
fn digest_mismatch_never_executes_java() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let java = dir.path().join("java");
    fs::write(&java, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o755)).unwrap();
    let mut backend = AlloyBackend::new(java, dir.path().join("missing.jar"));
    backend
        .register("browser", "safe", AlloyHarness::new("a", "b").unwrap())
        .unwrap();
    assert!(matches!(
        backend.run(&plan(100)).verdict,
        Verdict::Skipped { .. }
    ));
    assert!(!marker.exists());
}

#[test]
fn release_pin_is_exact() {
    assert_eq!(verify_alloy::ALLOY_VERSION, "6.2.0");
    assert_eq!(
        verify_alloy::ALLOY_SHA256,
        "6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d"
    );
}

#[test]
fn duplicate_registration_preserves_the_original_harness() {
    let (_dir, mut backend) = fixture(&alloy_script(false));
    assert!(
        backend
            .register(
                "browser",
                "safe",
                AlloyHarness::new("replacement.als", "Replacement").unwrap(),
            )
            .is_err()
    );
    assert!(matches!(
        backend.run(&plan(3000)).verdict,
        Verdict::Bounded { .. }
    ));
}

fn alloy_script(sat: bool) -> String {
    let solution = if sat { r#","solution":[{}]"# } else { "" };
    format!(
        r#"#!/bin/sh
case "$*" in *version*) echo 6.2.0; exit 0;; esac
while [ "$#" -gt 0 ]; do
  case "$1" in -o) shift; out="$1";; -c) shift; command="$1";; esac
  shift
done
mkdir -p "$out"
printf '{{"commands":{{"%s":{{"name":"%s","type":"check","source":"check fixture","overall":1%s}}}}}}' "$command" "$command" '{}' > "$out/receipt.json"
"#,
        solution
    )
}
