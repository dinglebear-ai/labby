//! Automatic artifacts follow auxiliary-first execution truncation.
#![cfg(all(feature = "gateway", feature = "proxy-testkit"))]
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn auxiliary_omission_preserves_inline_results_without_creating_final_artifacts() {
    let home = tempfile::tempdir().unwrap();
    let fixture = env!("CARGO_BIN_EXE_stdio-mcp-fixture");
    std::fs::create_dir_all(home.path().join("tmp")).unwrap();
    std::fs::write(home.path().join("config.toml"), format!(
        "[code_mode]\nenabled=true\ntimeout_ms=30000\nmax_response_bytes=4096\nmax_response_tokens=100000\nresult_shape_policy=\"off\"\n[gateway]\nextra_stdio_commands=[{}]\n[[upstream]]\nname=\"fixture\"\ntransport=\"stdio\"\ncommand={}\nargs=[\"--forge\"]\n", json!(fixture), json!(fixture)
    )).unwrap();
    // The positive case uses the same writable owner, storage and enabled policy.
    for size in [2000, 10000] {
        let code = format!(
            "await callTool('fixture::forge.safe', {{query:'x'.repeat(5000)}}); return 'r'.repeat({size});"
        );
        let output = tokio::time::timeout(
            Duration::from_mins(1),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
                .args(["--json", "code", "run", "--code", &code])
                .env_clear()
                .env("HOME", home.path())
                .env("LABBY_HOME", home.path())
                .env("TMPDIR", home.path().join("tmp"))
                .env("LABBY_CODE_MODE_RUNNER_BACKEND", "process")
                .env("LABBY_CODE_MODE_AUTO_ARTIFACT_THRESHOLD_BYTES", "24576")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(4096)])
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["calls"][0]["ok"], true);
        assert_eq!(
            response["calls"][0]["upstream_content"]["omitted_blocks"],
            1
        );
        if size == 2000 {
            assert_eq!(response["result"], json!("r".repeat(size)));
            assert!(
                response.get("artifacts").is_none(),
                "empty artifact lists are omitted from the execution wire response"
            );
            let store = home.path().join("code-mode-artifacts");
            if store.exists() {
                for entry in std::fs::read_dir(store).unwrap() {
                    assert!(
                        !entry
                            .unwrap()
                            .path()
                            .join("automatic/final-result.json")
                            .exists()
                    );
                }
            }
        } else {
            let artifacts = response["artifacts"]
                .as_array()
                .expect("positive control artifact list");
            let receipt = artifacts
                .iter()
                .find(|value| value["path"] == "automatic/final-result.json")
                .expect("true preservation must write an artifact");
            assert!(receipt["artifact_id"].is_string());
            let absolute = receipt["absolute_path"].as_str().unwrap();
            let path = absolute.strip_prefix("~/").map_or_else(
                || std::path::PathBuf::from(absolute),
                |relative| home.path().join(relative),
            );
            let stored: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            assert_eq!(stored, json!("r".repeat(size)));
        }
    }
}
