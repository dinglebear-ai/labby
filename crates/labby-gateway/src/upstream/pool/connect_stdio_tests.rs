//! Regression coverage for the stdio (child-process) connect path.

use std::os::unix::fs::PermissionsExt as _;

use super::connect_stdio::{
    connect_stdio_upstream, prefers_legacy_stdio_lifecycle, stdio_lifecycle_key,
};
use super::testsupport::test_upstream_config;

/// A child that exits before answering the handshake proves nothing about its
/// MCP lifecycle: it must be spawned exactly once, and its command must not be
/// remembered as a legacy-lifecycle server.
#[tokio::test]
async fn stdio_child_exiting_before_the_handshake_is_spawned_exactly_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spawn_log = dir.path().join("spawns.log");
    let script = dir.path().join("exit-immediately.sh");
    std::fs::write(&script, "#!/bin/sh\necho spawned >> \"$1\"\nexit 1\n").expect("write fixture");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut config = test_upstream_config();
    config.name = "exits-before-handshake".to_string();
    config.command = Some(script.to_string_lossy().into_owned());
    config.args = vec![spawn_log.to_string_lossy().into_owned()];
    let command = config.command.as_deref().expect("command");

    let error = connect_stdio_upstream(command, &config.args, &config, None, None, (), None)
        .await
        .expect_err("a child that exits before answering the handshake must fail to connect");

    let spawns = std::fs::read_to_string(&spawn_log).unwrap_or_default();
    assert_eq!(
        spawns.lines().count(),
        1,
        "child must be spawned exactly once; connect error: {error:#}"
    );
    let key = stdio_lifecycle_key(&config.name, command, &config.args);
    assert!(
        !prefers_legacy_stdio_lifecycle(&key),
        "a child that died before the handshake must not be remembered as a legacy lifecycle"
    );
}

/// Stderr from a dying child is not a lifecycle rejection. Even when the tail
/// happens to contain text the compatibility classifier looks for, a child
/// that closed its stdout before answering must not be respawned.
///
/// This covers the child-exit gate. The `_alive_` test below covers the same
/// hazard for a child that is still running, where that gate cannot help.
#[tokio::test]
async fn stdio_child_exiting_with_lifecycle_looking_stderr_is_not_retried() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spawn_log = dir.path().join("spawns.log");
    let script = dir.path().join("crash-with-noise.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\necho spawned >> \"$1\"\necho 'Error: Method not found' >&2\nexit 1\n",
    )
    .expect("write fixture");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut config = test_upstream_config();
    config.name = "crashes-with-lifecycle-noise".to_string();
    config.command = Some(script.to_string_lossy().into_owned());
    config.args = vec![spawn_log.to_string_lossy().into_owned()];
    let command = config.command.as_deref().expect("command");

    let error = connect_stdio_upstream(command, &config.args, &config, None, None, (), None)
        .await
        .expect_err("a crashing child must fail to connect");

    let spawns = std::fs::read_to_string(&spawn_log).unwrap_or_default();
    assert_eq!(
        spawns.lines().count(),
        1,
        "stderr noise must not trigger a lifecycle respawn; connect error: {error:#}"
    );
    let key = stdio_lifecycle_key(&config.name, command, &config.args);
    assert!(
        !prefers_legacy_stdio_lifecycle(&key),
        "a crashing child must not be remembered as a legacy lifecycle"
    );
}

/// A live child's stderr must never drive lifecycle classification.
///
/// The child-exit gate cannot help here. That gate records stdout EOF, so it
/// only fires once the child stops talking; this fixture answers on stdout and
/// keeps the pipe open, which is exactly the shape of a healthy server that
/// merely logged something unfortunate. The only thing then standing between an
/// ordinary log line and a spurious respawn is that classification reads the
/// MCP error alone.
///
/// The fixture logs "Method not found" (everyday MCP server log vocabulary) and
/// fails the handshake for an unrelated protocol reason: it answers the probe
/// with a well-formed JSON-RPC result of the wrong shape. Feeding stderr to the
/// classifier turns that into a legacy downgrade and a second spawn.
#[tokio::test]
async fn stdio_live_child_stderr_never_drives_lifecycle_classification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let spawn_log = dir.path().join("spawns.log");
    let script = dir.path().join("noisy-but-alive.sh");
    std::fs::write(
        &script,
        r#"#!/bin/sh
echo spawned >> "$1"
echo 'Error: Method not found' >&2
read request
id=`printf '%s' "$request" | sed -n 's/.*"id":\([0-9]*\).*/\1/p'`
[ -z "$id" ] && id=0
printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
sleep 10
"#,
    )
    .expect("write fixture");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut config = test_upstream_config();
    config.name = "alive-with-lifecycle-noise".to_string();
    config.command = Some(script.to_string_lossy().into_owned());
    config.args = vec![spawn_log.to_string_lossy().into_owned()];
    let command = config.command.as_deref().expect("command");

    let error = connect_stdio_upstream(command, &config.args, &config, None, None, (), None)
        .await
        .expect_err("a wrong-shaped handshake result must fail to connect");

    let spawns = std::fs::read_to_string(&spawn_log).unwrap_or_default();
    assert_eq!(
        spawns.lines().count(),
        1,
        "a live child's stderr must not trigger a lifecycle respawn; connect error: {error:#}"
    );
    let key = stdio_lifecycle_key(&config.name, command, &config.args);
    assert!(
        !prefers_legacy_stdio_lifecycle(&key),
        "log noise must not be remembered as a legacy lifecycle"
    );
}
