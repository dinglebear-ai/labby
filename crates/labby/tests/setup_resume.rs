use std::path::{Path, PathBuf};
use std::process::Command;

fn labby_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_labby"))
}

fn setup_state(root: &Path) -> serde_json::Value {
    let output = Command::new(labby_bin())
        .arg("--json")
        .arg("setup")
        .arg("resume")
        .env("LABBY_HOME", root)
        .output()
        .expect("run setup resume");
    assert!(
        output.status.success(),
        "setup resume failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse setup state")
}

fn google_env(secret: &str) -> String {
    format!(
        "LABBY_MCP_TRANSPORT=http
         LABBY_MCP_HTTP_HOST=127.0.0.1
         LABBY_MCP_HTTP_PORT=8765
         LABBY_AUTH_MODE=oauth
         LABBY_AUTH_PROVIDER=google
         LABBY_PUBLIC_URL=https://labby.example.com
         LABBY_GOOGLE_CLIENT_ID=client.apps.googleusercontent.com
         LABBY_GOOGLE_CLIENT_SECRET={secret}
         LABBY_AUTH_ADMIN_EMAIL=operator@example.com
"
    )
}

#[test]
fn staged_personal_oauth_resumes_at_commit_without_exposing_secret() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".env.draft"),
        google_env("RESUME_CANARY_SECRET"),
    )
    .unwrap();

    let value = setup_state(root.path());
    assert_eq!(value["last_completed_step"], 2);
    assert_eq!(value["resume_from"], "commit_configuration");
    assert_eq!(value["personal_oauth_configured"], true);
    assert_eq!(value["claude_code_configured"], false);
    assert!(value["has_draft"].as_bool().unwrap());
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("RESUME_CANARY_SECRET")
    );
}

#[test]
fn committed_oauth_resumes_at_claude_and_claude_config_advances_to_readiness() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".env"),
        google_env("COMMITTED_CANARY_SECRET"),
    )
    .unwrap();

    let value = setup_state(root.path());
    assert_eq!(value["last_completed_step"], 3);
    assert_eq!(value["resume_from"], "connect_claude_code");
    assert_eq!(value["personal_oauth_configured"], true);
    assert_eq!(value["claude_code_configured"], false);

    std::fs::write(
        root.path().join("config.toml"),
        r#"[[upstream]]
name = "claude-local"
enabled = true
priority = 1.0
command = "/usr/local/bin/claude"
args = ["mcp", "serve"]
proxy_resources = true
proxy_prompts = true
proxy_skills = false
"#,
    )
    .unwrap();

    let value = setup_state(root.path());
    assert_eq!(value["last_completed_step"], 4);
    assert_eq!(value["resume_from"], "verify_readiness");
    assert_eq!(value["claude_code_configured"], true);
    assert!(
        !serde_json::to_string(&value)
            .unwrap()
            .contains("COMMITTED_CANARY_SECRET")
    );
}
