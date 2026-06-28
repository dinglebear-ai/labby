use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::process::Command;

use crate::error::ToolError;
use crate::state::workspace::StateWorkspace;

use super::command::GitCommandSpec;

pub(crate) async fn dispatch_git_method(
    workspace: &StateWorkspace,
    method: &str,
    params: Value,
) -> Result<Value, ToolError> {
    let spec = GitCommandSpec::for_method(method, params)?;
    let stdout = run_git(workspace.root_path(), &spec.args).await?;
    Ok(json!({ "ok": true, "stdout": stdout }))
}

pub(crate) async fn run_git(workspace_root: &Path, args: &[String]) -> Result<String, ToolError> {
    let git = git_binary();
    let mut command = Command::new(git);
    command
        .args(args)
        .current_dir(workspace_root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_device())
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "timeout".to_string(),
            message: "git command timed out".to_string(),
        })?
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to run git: {err}"),
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout)
        .chars()
        .take(64 * 1024)
        .collect::<String>();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(16 * 1024)
        .collect::<String>();
    if !output.status.success() {
        return Err(ToolError::Sdk {
            sdk_kind: "git_failed".to_string(),
            message: format!("git failed: {}", redact_git_output(&stderr)),
        });
    }
    Ok(redact_git_output(&stdout))
}

fn git_binary() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from("git.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/bin/git")
    }
}

fn null_device() -> &'static str {
    #[cfg(windows)]
    {
        "NUL"
    }
    #[cfg(not(windows))]
    {
        "/dev/null"
    }
}

fn redact_git_output(value: &str) -> String {
    let value = value.replace("https://", "https://[REDACTED]@");
    let tokenish = regex::Regex::new(r"(ghp_|github_pat_|glpat-)[A-Za-z0-9_]+")
        .expect("static git token redaction regex");
    tokenish.replace_all(&value, "[REDACTED]").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::quota::StateWorkspaceLimits;

    #[tokio::test]
    async fn git_provider_initializes_and_commits_workspace_file() {
        let temp = tempfile::tempdir().unwrap();
        let workspace =
            StateWorkspace::new(temp.path().to_path_buf(), StateWorkspaceLimits::default())
                .unwrap();
        workspace
            .write_file(
                &crate::state::path::VirtualPath::parse("src/app.rs").unwrap(),
                "fn main() {}\n",
            )
            .await
            .unwrap();

        dispatch_git_method(&workspace, "init", json!({}))
            .await
            .unwrap();
        dispatch_git_method(&workspace, "add", json!({"path": "src/app.rs"}))
            .await
            .unwrap();
        dispatch_git_method(
            &workspace,
            "commit",
            json!({
                "message": "initial state",
                "authorName": "Lab",
                "authorEmail": "lab@example.invalid"
            }),
        )
        .await
        .unwrap();
        let log = dispatch_git_method(&workspace, "log", json!({"limit": 1}))
            .await
            .unwrap();
        assert!(log["stdout"].as_str().unwrap().contains("initial state"));
    }
}
