#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Command;

fn private_write(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("restrict fixture");
}

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command.env("LABBY_HOME", home);
    command
}

#[test]
fn public_cli_requires_and_authenticates_external_recovery_key() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    fs::create_dir(&home).expect("create home");
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).expect("restrict home");
    private_write(&home.join("state"), b"durable");
    let key = temp.path().join("recovery.key");
    private_write(&key, &[7_u8; 32]);
    let bundle = temp.path().join("bundle");

    let readable_key = temp.path().join("readable-recovery.key");
    private_write(&readable_key, &[9_u8; 32]);
    fs::set_permissions(&readable_key, fs::Permissions::from_mode(0o644))
        .expect("make recovery key group/world readable");
    let readable = command(&home)
        .env("LABBY_RECOVERY_KEY_PATH", &readable_key)
        .args(["state", "export", "--output"])
        .arg(temp.path().join("readable-key-bundle"))
        .output()
        .expect("run readable-key export");
    assert!(!readable.status.success());
    assert!(String::from_utf8_lossy(&readable.stderr).contains("owner-only"));

    let exported = command(&home)
        .env("LABBY_RECOVERY_KEY_PATH", &key)
        .args(["state", "export", "--output"])
        .arg(&bundle)
        .output()
        .expect("run export");
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    assert!(!bundle.join("recovery.key").exists());

    let json = command(&home)
        .env("LABBY_RECOVERY_KEY_PATH", &key)
        .args(["--json", "state", "verify", "--bundle"])
        .arg(&bundle)
        .output()
        .expect("run JSON verify");
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let outcome: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("state --json emits one JSON outcome");
    assert_eq!(outcome["operation"], "verify");
    assert_eq!(outcome["committed"], false);
    assert!(outcome["entries_verified"].as_u64().is_some());
    assert!(outcome["maintenance_warning"].is_null());

    let missing = command(&home)
        .args(["state", "verify", "--bundle"])
        .arg(&bundle)
        .output()
        .expect("run missing-key verify");
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("LABBY_RECOVERY_KEY_PATH"));

    let wrong_key = temp.path().join("wrong.key");
    private_write(&wrong_key, &[8_u8; 32]);
    let wrong = command(&home)
        .env("LABBY_RECOVERY_KEY_PATH", &wrong_key)
        .args(["state", "verify", "--bundle"])
        .arg(&bundle)
        .output()
        .expect("run wrong-key verify");
    assert!(!wrong.status.success());
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("recovery authentication failed"));

    let manifest_path = bundle.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read manifest")).expect("parse");
    manifest["installation_root"] = serde_json::Value::String("/replayed/root".into());
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize"),
    )
    .expect("tamper manifest");
    let replayed = command(&home)
        .env("LABBY_RECOVERY_KEY_PATH", &key)
        .args(["state", "verify", "--bundle"])
        .arg(&bundle)
        .output()
        .expect("run replay verify");
    assert!(!replayed.status.success());
    assert!(String::from_utf8_lossy(&replayed.stderr).contains("recovery authentication failed"));
}
