use std::process::Command;

fn write_invalid_config(root: &std::path::Path, command: &str) {
    std::fs::create_dir_all(root).expect("create config root");
    std::fs::write(
        root.join("config.toml"),
        format!(
            r#"
[[upstream]]
name = "invalid-fixture"
command = "{command}"
"#
        ),
    )
    .expect("write invalid gateway config");
}

fn run_serve(labby_home: &std::path::Path, user_home: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["serve", "--host", "127.0.0.1", "--port", "0"])
        .env("LABBY_HOME", labby_home)
        .env("HOME", user_home)
        .env("LABBY_AUTH_MODE", "bearer")
        .env("LABBY_MCP_HTTP_TOKEN", "invalid-config-test-token")
        .output()
        .expect("run labby serve")
}

fn run_setup(labby_home: &std::path::Path, user_home: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "setup", "proxy", "--yes", "--auth", "bearer"])
        .env("LABBY_HOME", labby_home)
        .env("HOME", user_home)
        .output()
        .expect("run labby setup proxy")
}

#[test]
fn labby_home_controls_config_secrets_and_durable_stores() {
    let labby_home = tempfile::tempdir().expect("labby home");
    let user_home = tempfile::tempdir().expect("user home");
    let legacy_home = user_home.path().join(".labby");

    let setup = run_setup(labby_home.path(), user_home.path());
    assert!(
        setup.status.success(),
        "setup must succeed: {}",
        String::from_utf8_lossy(&setup.stderr)
    );
    for name in ["config.toml", ".env"] {
        assert!(
            labby_home.path().join(name).is_file(),
            "setup must create {name} under LABBY_HOME"
        );
        assert!(
            !legacy_home.join(name).exists(),
            "setup must not create {name} under HOME when LABBY_HOME is explicit"
        );
    }

    write_invalid_config(labby_home.path(), "/labby-home-command");
    write_invalid_config(&legacy_home, "/legacy-home-command");

    let output = run_serve(labby_home.path(), user_home.path());
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "invalid config must fail startup");
    assert!(
        stderr.contains("/labby-home-command"),
        "LABBY_HOME config must win over HOME: {stderr}"
    );
    assert!(!stderr.contains("/legacy-home-command"));
    for name in ["usage.db", "codemode_journal.db"] {
        assert!(
            labby_home.path().join(name).is_file(),
            "{name} must be created under LABBY_HOME"
        );
        assert!(
            !legacy_home.join(name).exists(),
            "{name} must not be created under HOME when LABBY_HOME is explicit"
        );
    }
}
