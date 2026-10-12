//! Configuration-only onboarding must not install a daemon or mutate the caller's home.
use std::process::Command;

#[test]
fn default_configuration_preserves_existing_oauth() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    let state = root.join("state");
    let initial = setup(&home, &state, false);
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    let before = std::fs::read(state.join(".env")).unwrap();
    let repeat = Command::new(env!("CARGO_BIN_EXE_labby"))
        .env_clear()
        .env("HOME", home)
        .env("LABBY_HOME", &state)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args(["setup", "--role", "server", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(
        repeat.status.success(),
        "{}",
        String::from_utf8_lossy(&repeat.stderr)
    );
    assert_eq!(std::fs::read(state.join(".env")).unwrap(), before);
}

#[cfg(all(unix, feature = "gateway"))]
fn chatgpt_setup(
    directory: &std::path::Path,
    dry_run: bool,
    occupied: bool,
) -> std::process::Output {
    chatgpt_setup_with_credentials(directory, dry_run, occupied, true)
}

#[cfg(all(unix, feature = "gateway"))]
fn chatgpt_setup_with_credentials(
    directory: &std::path::Path,
    dry_run: bool,
    occupied: bool,
    credentials: bool,
) -> std::process::Output {
    use std::os::unix::fs::PermissionsExt as _;
    let bin = directory.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let tailscale = bin.join("tailscale");
    std::fs::write(&tailscale, format!(r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
case "$*" in
  version) printf '1.102.4\n' ;;
  'status --json') printf '%s\n' '{{"BackendState":"Running","Self":{{"Online":true,"DNSName":"test.example.ts.net."}}}}' ;;
  'serve status --json') printf '%s\n' '{}' ;;
  *) exit 91 ;;
esac
"#, directory.join("calls").display(), if occupied { r#"{"TCP":{"443":{"HTTPS":true}}}"# } else { "{}" })).unwrap();
    std::fs::set_permissions(&tailscale, std::fs::Permissions::from_mode(0o700)).unwrap();
    for (name, body) in [
        ("node", "#!/bin/sh\nprintf 'v22.18.0\\n'\n"),
        ("npx", "#!/bin/sh\nexit 92\n"),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let home = directory.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env_clear()
        .env("PATH", &bin)
        .env("HOME", &home)
        .env("LABBY_HOME", directory.join("state"))
        .args(["setup", "--chatgpt", "--yes", "--no-browser", "--json"]);
    if credentials {
        command
            .env("LABBY_GOOGLE_CLIENT_ID", "test-client")
            .env("LABBY_GOOGLE_CLIENT_SECRET", "test-secret-never-print")
            .env("LABBY_AUTH_ADMIN_EMAIL", "owner@example.test");
    }
    if dry_run {
        command.arg("--dry-run");
    }
    command.output().unwrap()
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn chatgpt_preview_derives_callback_without_installing_or_publishing() {
    let temp = tempfile::Builder::new()
        .prefix("labby-setup-")
        .tempdir_in("/private/tmp")
        .or_else(|_| tempfile::tempdir())
        .unwrap();
    let output = chatgpt_setup(temp.path(), true, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["service_installed"], false);
    assert_eq!(json["public_url"], "https://test.example.ts.net");
    assert!(!stdout.contains("test-secret-never-print"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("test-secret-never-print"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("https://test.example.ts.net/auth/google/callback")
    );
    assert!(!temp.path().join("state").exists());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("calls")).unwrap(),
        "version\nstatus --json\nserve status --json\n"
    );
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn chatgpt_preview_reports_missing_credentials_without_writing_state() {
    let temp = tempfile::tempdir().unwrap();
    let output = chatgpt_setup_with_credentials(temp.path(), true, false, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "preview");
    assert_eq!(json["ready"], false);
    assert_eq!(
        json["missing_requirements"],
        serde_json::json!([
            "LABBY_GOOGLE_CLIENT_ID",
            "LABBY_GOOGLE_CLIENT_SECRET",
            "LABBY_AUTH_ADMIN_EMAIL"
        ])
    );
    assert!(!temp.path().join("state").exists());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("calls")).unwrap(),
        "version\nstatus --json\nserve status --json\n"
    );
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn google_configuration_still_requires_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir(&home).unwrap();
    // Exercise the real validator used by ChatGPT setup without requiring KVM
    // or executing the platform dependency-installation preflight.
    let output = Command::new(env!("CARGO_BIN_EXE_labby"))
        .env_clear()
        .env("HOME", &home)
        .env("LABBY_HOME", temp.path().join("state"))
        .args([
            "setup",
            "--role",
            "server",
            "--config-only",
            "--auth",
            "oauth",
            "--oauth",
            "google",
            "--public-url",
            "https://test.example.ts.net",
            "--yes",
            "--no-browser",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Google client ID is required"));
    assert!(!temp.path().join("state").exists());
}

#[cfg(all(unix, feature = "gateway"))]
#[test]
fn chatgpt_preview_preserves_occupied_public_port_and_stops() {
    let temp = tempfile::tempdir().unwrap();
    let output = chatgpt_setup(temp.path(), true, true);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already configured"));
    assert!(!temp.path().join("state").exists());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("calls")).unwrap(),
        "version\nstatus --json\nserve status --json\n"
    );
}

#[cfg(all(feature = "gateway", target_os = "macos", target_arch = "aarch64"))]
#[test]
fn chatgpt_setup_persists_oauth_and_runtime_without_publishing() {
    let temp = tempfile::Builder::new()
        .prefix("labby-setup-")
        .tempdir_in("/private/tmp")
        .unwrap();
    let output = chatgpt_setup(temp.path(), false, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state = temp.path().join("state");
    let config = std::fs::read_to_string(state.join("config.toml")).unwrap();
    assert!(config.contains("funnel"));
    assert!(config.contains("443"));
    let mcp: serde_json::Value =
        serde_json::from_slice(&std::fs::read(state.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(mcp["mcpServers"]["microsandbox"]["command"], "npx");
    let outcome: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(outcome["funnel_port"], 443);
    assert_eq!(outcome["mcp_url"], "https://test.example.ts.net/mcp");
    let env = std::fs::read_to_string(state.join(".env")).unwrap();
    assert!(env.contains("LABBY_AUTH_MODE=oauth"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("test-secret-never-print"));
    assert!(!temp.path().join("home/Library/LaunchAgents").exists());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("calls")).unwrap(),
        "version\nstatus --json\nserve status --json\n"
    );
}

#[cfg(all(feature = "gateway", target_os = "macos", target_arch = "aarch64"))]
#[test]
fn failed_runtime_configuration_does_not_print_success_or_publish() {
    let temp = tempfile::Builder::new()
        .prefix("labby-setup-")
        .tempdir_in("/private/tmp")
        .unwrap();
    let state = temp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    std::fs::write(state.join(".mcp.json"), "malformed-private-config").unwrap();
    let output = chatgpt_setup(temp.path(), false, false);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "setup printed success before the runtime configuration failed"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("malformed-private-config"));
    assert_eq!(
        std::fs::read_to_string(state.join(".mcp.json")).unwrap(),
        "malformed-private-config"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("calls")).unwrap(),
        "version\nstatus --json\nserve status --json\n"
    );
}

fn setup_command(home: &std::path::Path, state: &std::path::Path, dry_run: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env_clear()
        .env("HOME", home)
        .env("LABBY_HOME", state)
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(
            "LABBY_GOOGLE_CLIENT_ID",
            "test-client.apps.googleusercontent.com",
        )
        .env("LABBY_GOOGLE_CLIENT_SECRET", "test-client-secret")
        .env("LABBY_AUTH_ADMIN_EMAIL", "operator@example.com")
        .args([
            "setup",
            "--role",
            "server",
            "--auth",
            "oauth",
            "--oauth",
            "google",
            "--public-url",
            "https://node.example.ts.net:8443",
            "--no-desktop",
            "--yes",
            "--json",
        ]);
    #[cfg(windows)]
    for key in ["SystemRoot", "WINDIR"] {
        // The native preview checks a loopback port, so Winsock must be able
        // to resolve its provider DLLs without inheriting user configuration.
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if dry_run {
        command.arg("--dry-run");
    }
    command
}

fn setup(home: &std::path::Path, state: &std::path::Path, dry_run: bool) -> std::process::Output {
    setup_command(home, state, dry_run).output().unwrap()
}

fn setup_failure(output: &std::process::Output) -> String {
    // Both callers stop before credential generation. Redact the owned
    // fixture secrets before truncation so no partial secret can escape.
    let diagnostic = String::from_utf8_lossy(&output.stderr)
        .replace("test-client-secret", "[REDACTED]")
        .replace("test-secret-never-print", "[REDACTED]")
        .chars()
        .take(4_096)
        .collect::<String>();
    format!("setup exited with {}: {diagnostic}", output.status)
}

#[test]
fn server_setup_defaults_to_configuration_only() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("proxy-state");
    let preview = setup_command(directory.path(), &state, true)
        .output()
        .unwrap();
    assert!(preview.status.success());
    let output: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(output["config_only"], true);
    assert_eq!(output["service_installed"], false);
    assert!(!state.exists());
}

#[test]
fn proxy_oauth_configuration_is_isolated_idempotent_and_service_free() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let home = root.join("caller-home");
    std::fs::create_dir(&home).unwrap();
    let state = root.join("proxy-state");
    let preview = setup(&home, &state, true);
    assert!(
        preview.status.success(),
        "configuration preview failed: {}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(!state.exists(), "dry run created durable state");
    assert!(!home.join(".labby").exists());
    let first = setup(&home, &state, false);
    assert!(
        first.status.success(),
        "configuration setup failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let env = state.join(".env");
    let before = std::fs::read(&env).unwrap();
    let vars: std::collections::HashMap<_, _> = dotenvy::from_path_iter(&env)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        vars.get("LABBY_AUTH_MODE").map(String::as_str),
        Some("oauth")
    );
    assert_eq!(
        vars.get("LABBY_GOOGLE_CLIENT_SECRET").map(String::as_str),
        Some("test-client-secret")
    );
    assert_eq!(
        vars.get("LABBY_PUBLIC_URL").map(String::as_str),
        Some("https://node.example.ts.net:8443")
    );
    assert_eq!(vars.get("LABBY_TOKEN_ENCRYPTION_KEY").unwrap().len(), 64);
    assert!(!home.join(".labby").exists(), "setup escaped LABBY_HOME");
    assert!(
        !home.join("Library").exists(),
        "setup installed a LaunchAgent"
    );
    assert!(!String::from_utf8_lossy(&first.stdout).contains("test-client-secret"));
    let second = setup(&home, &state, false);
    assert!(second.status.success());
    assert_eq!(
        std::fs::read(&env).unwrap(),
        before,
        "repeat setup changed durable OAuth state"
    );
}

#[test]
fn proxy_configuration_rejects_relative_state_root() {
    let directory = tempfile::tempdir().unwrap();
    let result = setup(
        directory.path(),
        std::path::Path::new("relative-state"),
        false,
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("absolute"));
    assert!(!directory.path().join(".labby").exists());
}

#[test]
fn explicit_server_deployment_retains_managed_service_plan() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("proxy-state");
    let preview = setup_command(directory.path(), &state, true)
        .args(["--deployment", "native"])
        .output()
        .unwrap();
    assert!(preview.status.success(), "{}", setup_failure(&preview));
    let output: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(output["deployment"], "native");
    assert!(output["config_only"].is_null());
    assert!(!state.exists());
}

#[cfg(windows)]
#[test]
fn explicit_native_server_installation_rejects_windows_without_persisting_state() {
    let directory = tempfile::tempdir().unwrap();
    let state = directory.path().join("proxy-state");
    let output = setup_command(directory.path(), &state, false)
        .args(["--deployment", "native"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", setup_failure(&output));
    assert!(
        output.stdout.is_empty(),
        "unsupported installation printed an outcome"
    );
    let error: serde_json::Value = serde_json::from_slice(&output.stderr)
        .unwrap_or_else(|_| panic!("{}", setup_failure(&output)));
    assert!(
        error["error"]["message"].as_str().is_some_and(|message| {
            message.contains("native persistent server installation is not yet supported")
        }),
        "{}",
        setup_failure(&output)
    );
    // Applying a Windows native plan only reads existing defaults before the
    // platform rejection; it never reaches configuration or service writes.
    assert!(
        !state.exists(),
        "unsupported installation created durable state"
    );
    assert!(!directory.path().join(".labby").exists());
}
