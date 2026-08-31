use std::process::Command;

#[path = "support/lib.rs"]
mod support;

fn command(home: &std::path::Path) -> Command {
    // `isolated_command` deliberately redirects temporary files into the test
    // home. tempfile requires that TMPDIR already exist, just as a real login
    // environment's system temporary directory does.
    std::fs::create_dir_all(home.join("tmp")).expect("create isolated TMPDIR");
    support::isolated_command(home)
}

#[test]
fn setup_proxy_noninteractive_dry_run_is_supported() {
    let home = tempfile::tempdir().expect("temp home");
    let output = command(home.path())
        .args(["--json", "setup", "proxy", "--yes", "--dry-run"])
        .output()
        .expect("run labby setup proxy dry-run");

    assert!(
        output.status.success(),
        "setup proxy dry-run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("dry-run JSON output");
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["changed"], true);
    assert!(!home.path().join(".labby/config.toml").exists());
    assert!(!home.path().join(".labby/.env").exists());
}

#[test]
fn setup_proxy_non_tty_without_yes_fails_without_reading_stdin() {
    let home = tempfile::tempdir().expect("temp home");
    let output = command(home.path())
        .args(["setup", "proxy"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run noninteractive setup proxy");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("setup proxy requires --yes when stdin is not a TTY")
    );
}

#[test]
fn bearer_setup_preserves_comments_hardens_secret_and_is_byte_idempotent() {
    let home = tempfile::tempdir().expect("temp home");
    let lab_home = home.path().join(".labby");
    std::fs::create_dir_all(&lab_home).unwrap();
    let config = lab_home.join("config.toml");
    let env = lab_home.join(".env");
    std::fs::write(&config, "# operator config\n[foreign]\nkeep = true\n").unwrap();
    std::fs::write(&env, "# operator env\nUNRELATED=value\n").unwrap();

    let first = command(home.path())
        .args(["--json", "setup", "proxy", "--yes", "--auth", "bearer"])
        .output()
        .expect("first bearer setup");
    assert!(
        first.status.success(),
        "first setup failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["changed"], true);
    assert_eq!(first_json["secret_changed"], true);

    let config_once = std::fs::read(&config).unwrap();
    let env_once = std::fs::read(&env).unwrap();
    let config_text = String::from_utf8(config_once.clone()).unwrap();
    let env_text = String::from_utf8(env_once.clone()).unwrap();
    assert!(config_text.contains("# operator config"));
    assert!(config_text.contains("[foreign]"));
    assert!(config_text.contains("auth = \"bearer\""));
    assert!(!config_text.contains("LABBY_PROXY_BEARER_TOKEN="));
    assert!(env_text.contains("# operator env"));
    assert!(env_text.contains("UNRELATED=value"));
    let token = env_text
        .lines()
        .find_map(|line| line.strip_prefix("LABBY_PROXY_BEARER_TOKEN="))
        .expect("generated proxy bearer token");
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|character| character.is_ascii_hexdigit()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&env).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!String::from_utf8_lossy(&first.stdout).contains(token));
    assert!(!String::from_utf8_lossy(&first.stderr).contains(token));

    let second = command(home.path())
        .args(["--json", "setup", "proxy", "--yes", "--auth", "bearer"])
        .output()
        .expect("second bearer setup");
    assert!(second.status.success());
    let second_json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second_json["changed"], false);
    assert_eq!(second_json["config_changed"], false);
    assert_eq!(second_json["secret_changed"], false);
    assert_eq!(std::fs::read(&config).unwrap(), config_once);
    assert_eq!(std::fs::read(&env).unwrap(), env_once);
}

#[cfg(unix)]
#[test]
fn bearer_setup_repairs_existing_secret_permissions_without_rewriting_value() {
    use std::os::unix::fs::PermissionsExt as _;

    let home = tempfile::tempdir().unwrap();
    let labby_home = home.path().join(".labby");
    std::fs::create_dir_all(&labby_home).unwrap();
    std::fs::set_permissions(&labby_home, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        labby_home.join("config.toml"),
        r#"[proxy]
exposure = "local"
auth = "bearer"
bearer_token_env = "LABBY_PROXY_TOKEN"
"#,
    )
    .unwrap();
    std::fs::write(
        labby_home.join(".env"),
        r"LABBY_PROXY_TOKEN=existing-secret
",
    )
    .unwrap();
    std::fs::set_permissions(
        labby_home.join(".env"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let output = command(home.path())
        .args([
            "--json",
            "setup",
            "proxy",
            "--yes",
            "--exposure",
            "local",
            "--auth",
            "bearer",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["secret_changed"], true);
    assert_eq!(
        std::fs::metadata(&labby_home).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(labby_home.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(labby_home.join(".env")).unwrap(),
        "LABBY_PROXY_TOKEN=existing-secret
"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("existing-secret"));
}

#[test]
fn bearer_token_stdin_is_stored_but_never_printed() {
    use std::io::Write as _;

    let home = tempfile::tempdir().expect("temp home");
    let secret = "stdin-secret-with-spaces # not output";
    let mut child = command(home.path())
        .args(["--json", "setup", "proxy", "--yes", "--bearer-token-stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn stdin setup");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{secret}\n").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "stdin setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let env = std::fs::read_to_string(home.path().join(".labby/.env")).unwrap();
    assert!(env.contains("LABBY_PROXY_BEARER_TOKEN="));
    assert!(env.contains("stdin-secret-with-spaces"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    let config = std::fs::read_to_string(home.path().join(".labby/config.toml")).unwrap();
    assert!(!config.contains(secret));
}
