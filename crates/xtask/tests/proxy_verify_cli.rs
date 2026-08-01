use std::process::Command;

#[test]
fn proxy_verify_help_advertises_the_stable_cli() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["proxy-verify", "--help"])
        .output()
        .expect("run proxy-verify help");

    assert!(output.status.success(), "help failed: {output:?}");
    let stdout = String::from_utf8(output.stdout).expect("help is UTF-8");
    for required in [
        "--binary",
        "--output",
        "--keep-temp",
        "--json",
        "LABBY_PROXY_LIVE=1",
    ] {
        assert!(
            stdout.contains(required),
            "missing {required} in:\n{stdout}"
        );
    }
}

#[test]
fn proxy_verify_early_failure_still_writes_a_sanitized_manifest() {
    let output_dir =
        std::env::temp_dir().join(format!("labby-proxy-proof-cli-test-{}", std::process::id()));
    drop(std::fs::remove_dir_all(&output_dir));
    let secret = "proxy-verifier-secret-canary";
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "proxy-verify",
            "--binary",
            "/definitely/missing/labby",
            "--output",
        ])
        .arg(&output_dir)
        .env("LABBY_PROXY_BEARER_TOKEN", secret)
        .output()
        .expect("run failing proxy verifier");

    assert!(
        !output.status.success(),
        "missing binary unexpectedly passed"
    );
    let manifest =
        std::fs::read(output_dir.join("manifest.json")).expect("failure must still write manifest");
    let parsed: serde_json::Value =
        serde_json::from_slice(&manifest).expect("manifest is valid JSON");
    assert_eq!(parsed["schema"], "labby.proxy-proof");
    assert_eq!(parsed["version"], 1);
    assert_eq!(parsed["result"], "failed");
    assert!(!String::from_utf8_lossy(&manifest).contains(secret));
    std::fs::remove_dir_all(output_dir).expect("remove proof output");
}

#[test]
fn independent_target_and_output_roots_produce_identical_normalized_manifests() {
    let root = std::env::temp_dir().join(format!(
        "labby-proxy-proof-determinism-test-{}",
        std::process::id()
    ));
    drop(std::fs::remove_dir_all(&root));
    std::fs::create_dir(&root).expect("create determinism root");
    let mut manifests = Vec::new();

    for suffix in ["a", "b"] {
        let output_dir = root.join(format!("proof-{suffix}"));
        let target_dir = root.join(format!("target-{suffix}"));
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args([
                "proxy-verify",
                "--binary",
                "/definitely/missing/labby",
                "--output",
            ])
            .arg(&output_dir)
            .env("CARGO_TARGET_DIR", target_dir)
            .env("LABBY_PROXY_BEARER_TOKEN", "determinism-secret-canary")
            .output()
            .expect("run verifier determinism case");
        assert!(!output.status.success());
        let manifest = std::fs::read(output_dir.join("manifest.json")).expect("read manifest");
        assert!(!String::from_utf8_lossy(&manifest).contains("canary"));
        for artifact in std::fs::read_dir(&output_dir).expect("list proof artifacts") {
            let bytes = std::fs::read(artifact.expect("artifact entry").path())
                .expect("read proof artifact");
            assert!(!String::from_utf8_lossy(&bytes).contains("canary"));
        }
        manifests.push(manifest);
    }

    assert_eq!(manifests[0], manifests[1]);
    std::fs::remove_dir_all(root).expect("remove determinism root");
}
