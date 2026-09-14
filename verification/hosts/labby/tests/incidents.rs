//! Incident CLI redaction and prefix exploration acceptance tests.

use std::process::Command;

#[test]
fn actual_cli_reduces_and_explores_without_echoing_log_payloads() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("incident.json");
    std::fs::write(&input,r#"{"schema":1,"events":[{"step":{"action":"connect","generation":"credential-canary"},"authorization":"credential-canary"},{"step":{"action":"admit","request":"subject-canary"}},{"step":{"action":"cancel","request":"subject-canary"}}]}"#).unwrap();
    for command in ["incident", "incident-explore"] {
        let output = Command::new(env!("CARGO_BIN_EXE_labby-verify"))
            .arg(command)
            .arg(&input)
            .arg("LABBY-REQ-005")
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("credential-canary"));
        assert!(!text.contains("subject-canary"));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let reduced = if command == "incident" {
            &value
        } else {
            &value["incident"]
        };
        assert_eq!(reduced["scenario"]["status"], "unreproduced");
        assert_eq!(reduced["scenario"]["origin"]["kind"], "incident");
        assert_eq!(reduced["replay"]["gate_failure"], false);
        if command == "incident-explore" {
            assert_eq!(value["exploration"]["verdict"]["verdict"], "bounded");
        }
    }
    std::fs::write(
        &input,
        r#"{"schema":1,"events":[{"step":{"action":"credential-canary"}}]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_labby-verify"))
        .arg("incident")
        .arg(&input)
        .arg("LABBY-REQ-001")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("credential-canary"));
}

#[test]
fn actual_cli_rejects_oversized_and_non_regular_inputs() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("oversized.json");
    std::fs::write(&input, vec![b'x'; 1_048_577]).unwrap();
    for path in [input.as_path(), root.path()] {
        let output = Command::new(env!("CARGO_BIN_EXE_labby-verify"))
            .arg("incident-explore")
            .arg(path)
            .arg("LABBY-REQ-001")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "incident reduction failed: cannot read bounded incident input\n"
        );
    }
}
