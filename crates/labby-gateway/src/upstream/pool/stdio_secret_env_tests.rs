//! Secret stdio environment values reach the child and nothing else.

use std::os::unix::fs::PermissionsExt as _;

use super::connect_stdio::connect_stdio_upstream_with_dotenv;
use super::stdio_secret_env::secret_environment;
use super::testsupport::test_upstream_config;
use crate::test_support::{SharedBuf, TRACING_TEST_LOCK, captured_logs};

const FAKE_FIGMA: &str = "fake-figma-value-7c1d";
const FAKE_OTHER: &str = "fake-other-value-93ab";

fn stdio_config(dir: &std::path::Path) -> labby_runtime::gateway_config::UpstreamConfig {
    let script = dir.join("dump-env.sh");
    std::fs::write(&script, "#!/bin/sh\nenv > \"$1\"\nexit 1\n").expect("write fixture");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let mut config = test_upstream_config();
    config.name = "figma".to_string();
    config.url = None;
    config.command = Some(script.to_string_lossy().into_owned());
    config.args = vec![dir.join("child.env").to_string_lossy().into_owned()];
    config
        .env
        .insert("FIGMA_MODE".to_string(), "plain".to_string());
    config.secret_env.insert(
        "FIGMA_API_KEY".to_string(),
        "LABBY_GW_FIGMA_ENV_FIGMA_API_KEY".to_string(),
    );
    config.secret_env.insert(
        "OTHER_TOKEN".to_string(),
        "LABBY_GW_FIGMA_ENV_OTHER_TOKEN".to_string(),
    );
    config
}

fn write_dotenv(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let dotenv = dir.join(".env");
    std::fs::write(&dotenv, body).expect("write dotenv");
    dotenv
}

#[test]
fn secret_values_reach_the_spawned_child_and_never_the_logs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = stdio_config(dir.path());
    let dotenv = write_dotenv(
        dir.path(),
        &format!(
            "LABBY_GW_FIGMA_ENV_FIGMA_API_KEY={FAKE_FIGMA}\nLABBY_GW_FIGMA_ENV_OTHER_TOKEN={FAKE_OTHER}\n"
        ),
    );
    let logs = SharedBuf::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let error = {
        let _lock = TRACING_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = tracing::subscriber::set_default(subscriber);
        let command = config.command.as_deref().expect("command");
        // A current-thread runtime keeps the thread-local subscriber in scope
        // and the sync lock out of any await point.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(connect_stdio_upstream_with_dotenv(
                command,
                &config.args,
                &config,
                Some(&dotenv),
                None,
                None,
                (),
                None,
            ))
            .expect_err("the fixture exits before the handshake")
    };

    let child_env = std::fs::read_to_string(dir.path().join("child.env")).expect("child env");
    assert!(
        child_env
            .lines()
            .any(|l| l == format!("FIGMA_API_KEY={FAKE_FIGMA}"))
    );
    assert!(
        child_env
            .lines()
            .any(|l| l == format!("OTHER_TOKEN={FAKE_OTHER}"))
    );
    assert!(child_env.lines().any(|l| l == "FIGMA_MODE=plain"));
    assert!(
        !child_env.contains("LABBY_GW_FIGMA_ENV_"),
        "credential variable names stay in the gateway"
    );

    let logs = captured_logs(&logs);
    assert!(logs.contains("FIGMA_API_KEY"), "spawn log names the keys");
    for fake in [FAKE_FIGMA, FAKE_OTHER] {
        assert!(!logs.contains(fake), "secret value logged");
        assert!(!format!("{error:#}").contains(fake), "secret in error");
    }
}

#[test]
fn missing_secret_value_refuses_spawn_without_naming_other_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = stdio_config(dir.path());
    let dotenv = write_dotenv(
        dir.path(),
        &format!("LABBY_GW_FIGMA_ENV_FIGMA_API_KEY={FAKE_FIGMA}\n"),
    );

    let error = secret_environment(&config, Some(&dotenv)).expect_err("missing value");

    assert_eq!(error.kind(), "upstream_credential_missing");
    let message = error.to_string();
    assert!(message.contains("OTHER_TOKEN"), "{message}");
    assert!(!message.contains(FAKE_FIGMA));
}

#[test]
fn secret_values_are_passed_verbatim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = stdio_config(dir.path());
    config.secret_env.remove("OTHER_TOKEN");
    let dotenv = write_dotenv(
        dir.path(),
        // The host credential writer's quoting of `Bearer fake value $HOME"q`.
        "LABBY_GW_FIGMA_ENV_FIGMA_API_KEY=\"Bearer fake value \\$HOME\\\"q\"\n",
    );

    let env = secret_environment(&config, Some(&dotenv)).expect("resolved");

    assert_eq!(env.len(), 1);
    assert_eq!(env[0].0, "FIGMA_API_KEY");
    assert_eq!(env[0].1, "Bearer fake value $HOME\"q");
}

#[test]
fn hand_edited_mapping_outside_the_namespace_refuses_spawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = stdio_config(dir.path());
    config.secret_env.insert(
        "OTHER_TOKEN".to_string(),
        "LABBY_OAUTH_ENCRYPTION_KEY".to_string(),
    );
    let dotenv = write_dotenv(
        dir.path(),
        &format!(
            "LABBY_GW_FIGMA_ENV_FIGMA_API_KEY={FAKE_FIGMA}\nLABBY_OAUTH_ENCRYPTION_KEY={FAKE_OTHER}\n"
        ),
    );

    let error = secret_environment(&config, Some(&dotenv)).expect_err("outside namespace");

    assert_eq!(error.kind(), "invalid_param");
    assert!(!error.to_string().contains(FAKE_OTHER));
}
