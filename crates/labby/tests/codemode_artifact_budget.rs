//! Execution-wide quotas include both runner writes and final preservation.
#![cfg(feature = "gateway")]
use serde_json::Value;
use std::time::Duration;

async fn execute(home: &std::path::Path, code: &str) -> Value {
    std::fs::create_dir_all(home.join("tmp")).unwrap();
    std::fs::write(home.join("config.toml"), "[code_mode]\nenabled=true\ntimeout_ms=30000\nmax_response_bytes=1024\nartifact_max_mib=16\nartifact_max_store_mib=512\nartifact_retention_runs=0\n").unwrap();
    let output = tokio::time::timeout(
        Duration::from_mins(1),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
            .args(["--json", "code", "run", "--code", code])
            .env_clear()
            .env("HOME", home)
            .env("LABBY_HOME", home)
            .env("TMPDIR", home.join("tmp"))
            .env("LABBY_CODE_MODE_RUNNER_BACKEND", "process")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(4096)]),
        String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn quota_case(files: usize, bytes: usize, preserve: bool) {
    let home = tempfile::tempdir().unwrap();
    let code = format!(
        "for (let i=0;i<{files};i++) await writeArtifact('file-'+i,'x'.repeat({bytes})); return 'z'.repeat(524286);"
    );
    let output = execute(home.path(), &code).await;
    let artifacts = output["artifacts"].as_array().unwrap();
    let final_artifact = artifacts
        .iter()
        .find(|receipt| receipt["path"] == "automatic/final-result.json");
    assert_eq!(
        final_artifact.is_some(),
        preserve,
        "files={files}, explicit bytes={bytes}, receipts={}",
        artifacts.len()
    );
    assert!(artifacts.len() <= 256);
    assert!(
        artifacts
            .iter()
            .map(|receipt| receipt["bytes"].as_u64().unwrap())
            .sum::<u64>()
            <= 64 * 1024 * 1024
    );
    if let Some(receipt) = final_artifact {
        assert!(
            receipt["artifact_id"].as_str().is_some(),
            "final preservation remains owner-bound"
        );
        let absolute = receipt["absolute_path"].as_str().unwrap();
        let path = absolute.strip_prefix("~/").map_or_else(
            || std::path::PathBuf::from(absolute),
            |relative| home.path().join(relative),
        );
        let run_id = path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let stored = labby_codemode::read_receipted_artifact(
            home.path(),
            run_id,
            receipt["path"].as_str().unwrap(),
            receipt["sha256"].as_str().unwrap(),
            receipt["bytes"].as_u64().unwrap() as usize,
        )
        .await
        .unwrap();
        assert!(
            serde_json::from_slice::<Value>(&stored).unwrap() == Value::String("z".repeat(524_286)),
            "final artifact content differs; stored bytes={}",
            stored.len(),
        );
    }
}

#[tokio::test]
async fn final_artifact_cannot_exceed_execution_file_quota() {
    quota_case(256, 0, false).await;
}

#[tokio::test]
async fn final_artifact_fills_last_available_execution_file() {
    quota_case(255, 0, true).await;
}

#[tokio::test]
async fn final_artifact_cannot_exceed_execution_byte_quota() {
    quota_case(8, 8 * 1024 * 1024, false).await;
}

#[tokio::test]
async fn final_artifact_fits_below_execution_byte_quota() {
    quota_case(8, (64 * 1024 * 1024 - 524_288) / 8, true).await;
}

#[tokio::test]
async fn explicit_final_result_path_cannot_advertise_unrelated_returned_json() {
    let home = tempfile::tempdir().unwrap();
    let output = execute(
        home.path(),
        r"await writeArtifact('automatic/final-result.json', JSON.stringify(Object.fromEntries([['explicit', true]]))); return 'z'.repeat(524286);",
    )
    .await;
    let artifacts = output["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1, "explicit artifact must not be replaced");
    let receipt = &artifacts[0];
    assert_eq!(receipt["path"], "automatic/final-result.json");
    let stored_path = receipt["absolute_path"].as_str().unwrap();
    let path = stored_path.strip_prefix("~/").map_or_else(
        || std::path::PathBuf::from(stored_path),
        |relative| home.path().join(relative),
    );
    let run_id = path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    let stored = labby_codemode::read_receipted_artifact(
        home.path(),
        run_id,
        receipt["path"].as_str().unwrap(),
        receipt["sha256"].as_str().unwrap(),
        receipt["bytes"].as_u64().unwrap() as usize,
    )
    .await
    .unwrap();
    assert_eq!(stored, br#"{"explicit":true}"#);
    assert_eq!(receipt["bytes"], stored.len());
    assert_eq!(output["result"]["truncated"], true);
    assert!(
        output["result"]
            .get("preserved_result_artifact_id")
            .is_none(),
        "an unrelated explicit artifact is not the complete returned JSON"
    );
    assert!(
        !output["result"]["next_action"]
            .as_str()
            .unwrap()
            .contains("Complete returned JSON saved")
    );
}
