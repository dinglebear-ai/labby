#![cfg(all(unix, feature = "proxy-testkit"))]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn labby_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_labby"))
}

fn fixture_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stdio-mcp-fixture"))
}

fn make_fake_claude(root: &Path, working: bool) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("create fake bin dir");
    let claude = bin.join("claude");
    if working {
        std::fs::copy(fixture_bin(), &claude).expect("copy stdio fixture as claude");
    } else {
        std::fs::write(
            &claude,
            "#!/bin/sh
exit 7
",
        )
        .expect("write failing claude fixture");
    }
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755))
        .expect("make fake claude executable");
    claude
}

fn run_setup(root: &Path, args: &[&str]) -> Output {
    let labby_home = root.join("labby-home");
    let user_home = root.join("home");
    std::fs::create_dir_all(&labby_home).expect("create LABBY_HOME");
    std::fs::create_dir_all(&user_home).expect("create HOME");
    Command::new(labby_bin())
        .arg("--json")
        .arg("setup")
        .arg("claude-code")
        .args(args)
        .env("LABBY_HOME", &labby_home)
        .env("HOME", &user_home)
        .output()
        .expect("run setup claude-code")
}

fn write_previous_upstream(root: &Path, claude: &Path, extra: &str) {
    let labby_home = root.join("labby-home");
    std::fs::create_dir_all(&labby_home).expect("create LABBY_HOME");
    std::fs::write(
        labby_home.join("config.toml"),
        format!(
            r#"[[upstream]]
name = "claude-local"
enabled = true
priority = 1.0
command = {command:?}
args = ["mcp", "serve", "legacy"]
proxy_resources = true
proxy_prompts = true
proxy_skills = false
{extra}
"#,
            command = claude.to_string_lossy().as_ref(),
        ),
    )
    .expect("write previous config");
}

#[test]
fn claude_code_apply_and_rollback_restore_exact_previous_upstream() {
    let root = tempfile::tempdir().expect("test root");
    let claude = make_fake_claude(root.path(), true);
    write_previous_upstream(root.path(), &claude, "");

    let apply = run_setup(
        root.path(),
        &[
            "--name",
            "claude-local",
            "--claude-path",
            claude.to_str().unwrap(),
            "--apply",
            "--yes",
        ],
    );
    assert!(
        apply.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&apply.stderr)
    );
    let config = std::fs::read_to_string(root.path().join("labby-home/config.toml")).unwrap();
    assert!(config.contains(r#"args = ["mcp", "serve"]"#), "{config}");
    assert!(!config.contains("legacy"), "{config}");

    let rollback = root
        .path()
        .join("labby-home/setup-backups/claude-code-claude-local.json");
    let mode = std::fs::metadata(&rollback).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "rollback record must be owner-only");

    let undo = run_setup(root.path(), &["--name", "claude-local", "--rollback"]);
    assert!(
        undo.status.success(),
        "rollback failed: {}",
        String::from_utf8_lossy(&undo.stderr)
    );
    let restored = std::fs::read_to_string(root.path().join("labby-home/config.toml")).unwrap();
    assert!(restored.contains("legacy"), "{restored}");
    assert!(
        !rollback.exists(),
        "successful rollback must retire its record"
    );
}

#[test]
fn claude_code_failed_preflight_never_mutates_config() {
    let root = tempfile::tempdir().expect("test root");
    let claude = make_fake_claude(root.path(), false);
    let output = run_setup(
        root.path(),
        &[
            "--name",
            "claude-local",
            "--claude-path",
            claude.to_str().unwrap(),
            "--apply",
        ],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("preflight failed before any config change")
    );
    assert!(!root.path().join("labby-home/config.toml").exists());
    assert!(
        !root
            .path()
            .join("labby-home/setup-backups/claude-code-claude-local.json")
            .exists()
    );
}

#[test]
fn claude_code_refuses_to_replace_credential_bearing_existing_upstream() {
    let root = tempfile::tempdir().expect("test root");
    let claude = make_fake_claude(root.path(), true);
    write_previous_upstream(
        root.path(),
        &claude,
        r#"env = { SECRET = "must-not-snapshot" }"#,
    );
    let before = std::fs::read(root.path().join("labby-home/config.toml")).unwrap();

    let output = run_setup(
        root.path(),
        &[
            "--name",
            "claude-local",
            "--claude-path",
            claude.to_str().unwrap(),
            "--apply",
            "--yes",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to replace"));
    assert_eq!(
        std::fs::read(root.path().join("labby-home/config.toml")).unwrap(),
        before
    );
    assert!(
        !root
            .path()
            .join("labby-home/setup-backups/claude-code-claude-local.json")
            .exists()
    );
}

#[test]
fn claude_code_rollback_refuses_to_overwrite_later_user_changes() {
    let root = tempfile::tempdir().expect("test root");
    let claude = make_fake_claude(root.path(), true);
    let apply = run_setup(
        root.path(),
        &[
            "--name",
            "claude-local",
            "--claude-path",
            claude.to_str().unwrap(),
            "--apply",
        ],
    );
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stderr)
    );

    let config_path = root.path().join("labby-home/config.toml");
    let changed = std::fs::read_to_string(&config_path)
        .unwrap()
        .replace("proxy_resources = true", "proxy_resources = false");
    std::fs::write(&config_path, changed).unwrap();

    let rollback = run_setup(root.path(), &["--name", "claude-local", "--rollback"]);
    assert!(!rollback.status.success());
    assert!(String::from_utf8_lossy(&rollback.stderr).contains("changed after Labby applied it"));
    let current = std::fs::read_to_string(&config_path).unwrap();
    assert!(current.contains("proxy_resources = false"));
    assert!(
        root.path()
            .join("labby-home/setup-backups/claude-code-claude-local.json")
            .exists(),
        "rollback evidence must remain when safe restore is refused"
    );
}
