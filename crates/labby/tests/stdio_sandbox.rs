#[cfg(target_os = "linux")]
#[test]
fn hostile_stdio_child_cannot_read_home_secrets_or_mutate_labby_state() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let allowed = root.path().join("upstream-state");
    let private_tmp = root.path().join("upstream-a-tmp");
    let other_tmp = root.path().join("upstream-b-tmp");
    let package = root.path().join("upstream-package");
    let package_cache = root.path().join("upstream-package-cache");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::create_dir_all(home.join(".labby")).unwrap();
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&private_tmp).unwrap();
    std::fs::create_dir_all(&other_tmp).unwrap();
    std::fs::create_dir_all(&package).unwrap();
    std::fs::create_dir_all(&package_cache).unwrap();
    std::fs::write(package.join("offline-package.txt"), "offline-package").unwrap();
    std::fs::write(package_cache.join("cached-package.txt"), "cached-package").unwrap();
    std::fs::write(home.join(".ssh/id_ed25519"), "host-secret").unwrap();
    std::fs::write(home.join(".labby/config.toml"), "protected=true").unwrap();
    std::fs::write(allowed.join("input.txt"), "explicit-input").unwrap();

    let script = r#"
#!/bin/sh
set -eu
! cat "$HOST_HOME/.ssh/id_ed25519" >/dev/null 2>&1
! printf compromised >"$HOST_HOME/.labby/config.toml" 2>/dev/null
! cat "/proc/$PARENT_PID/environ" >/dev/null 2>&1
! printf crossed >"$OTHER_UPSTREAM_TMP/crossed" 2>/dev/null
test "$(cat "$UPSTREAM_STATE/input.txt")" = explicit-input
test "$(cat "$PACKAGE_ROOT/offline-package.txt")" = offline-package
test "$(cat "$npm_config_cache/cached-package.txt")" = cached-package
printf cache-write >"$npm_config_cache/runtime-state.txt"
printf persisted >"$UPSTREAM_STATE/output.txt"
printf isolated >"$TMPDIR/output.txt"
"#;
    let executable = package.join("npx");
    std::fs::write(&executable, script).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args([
            "__stdio-sandbox",
            "--read-only",
            "/bin",
            "--read-only",
            "/usr",
            "--read-only",
            "/lib",
            "--read-only",
            "/etc",
            "--read-write",
            "/dev/null",
            "--read-only",
        ])
        .arg(&package)
        .args(["--read-only"])
        .arg(&executable)
        .args(["--read-write"])
        .arg(&allowed)
        .args(["--read-write"])
        .arg(&private_tmp)
        .args(["--read-write"])
        .arg(&package_cache)
        .args(["--"])
        .arg(&executable)
        .env("HOST_HOME", &home)
        .env("UPSTREAM_STATE", &allowed)
        .env("PACKAGE_ROOT", &package)
        .env("npm_config_cache", &package_cache)
        .env("TMPDIR", &private_tmp)
        .env("OTHER_UPSTREAM_TMP", &other_tmp)
        .env("PARENT_PID", std::process::id().to_string())
        .status()
        .unwrap();

    assert!(status.success(), "sandboxed hostile child failed: {status}");
    assert_eq!(
        std::fs::read_to_string(home.join(".labby/config.toml")).unwrap(),
        "protected=true"
    );
    assert_eq!(
        std::fs::read_to_string(allowed.join("output.txt")).unwrap(),
        "persisted"
    );
    assert_eq!(
        std::fs::read_to_string(private_tmp.join("output.txt")).unwrap(),
        "isolated"
    );
    assert_eq!(
        std::fs::read_to_string(package_cache.join("runtime-state.txt")).unwrap(),
        "cache-write"
    );
    assert!(!other_tmp.join("crossed").exists());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn required_stdio_sandbox_fails_closed_when_platform_backend_is_unavailable() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["__stdio-sandbox", "--", "echo", "must-not-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("stdio sandbox is unavailable on this platform")
    );
}
