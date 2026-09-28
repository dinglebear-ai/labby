#![cfg(feature = "gateway")]
//! Completion-cache isolation and offline behavior at the executable boundary.
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Output, Stdio},
    time::Duration,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path},
};

async fn daemon() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/gateway/actions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"name":"gateway.reload"}])))
        .mount(&server)
        .await;
    for (action, value) in [
        (
            "gateway.list",
            json!([{"config":{"name":"alpha"}},{"config":{"name":"$(unsafe)"}}]),
        ),
        (
            "gateway.protected_route.list_state",
            json!([{"name":"private-route"}]),
        ),
        ("gateway.loadout.list", json!([{"name":"personal"}])),
    ] {
        Mock::given(method("POST"))
            .and(path("/v1/gateway"))
            .and(body_partial_json(json!({"action":action})))
            .respond_with(ResponseTemplate::new(200).set_body_json(value))
            .mount(&server)
            .await;
    }
    server
}

async fn run(home: &Path, server: &MockServer, token: &str, team: &str, args: &[&str]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .args(["--team-id", team])
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("LABBY_HOME", home.join(".labby"))
        .env("LABBY_SERVER_URL", server.uri())
        .env("LABBY_MCP_HTTP_TOKEN", token)
        .env("NO_COLOR", "1")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .expect("completion command must finish")
        .unwrap()
}
fn success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn snapshots(home: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(home.join(".labby/completions"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[cfg(unix)]
#[tokio::test]
async fn generated_shell_wrappers_preserve_partial_words_and_query_only_the_local_cache() {
    let home = tempfile::tempdir().unwrap();
    let server = daemon().await;
    let token = "shell-completion-fixture";
    success(
        &run(
            home.path(),
            &server,
            token,
            "team-shell",
            &["--json", "completions", "refresh"],
        )
        .await,
    );
    let request_count = server.received_requests().await.unwrap().len();
    let before = snapshots(home.path());
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_labby"), bin.join("labby")).unwrap();
    #[cfg(not(target_os = "macos"))]
    let shells = [("bash", "/bin/bash")];
    #[cfg(target_os = "macos")]
    let shells = [("bash", "/bin/bash"), ("zsh", "/bin/zsh")];
    for (shell, executable) in shells {
        let generated = success(
            &run(
                home.path(),
                &server,
                token,
                "team-shell",
                &["--json", "completions", shell, "--resources"],
            )
            .await,
        );
        let script_path = home.path().join(format!("completion.{shell}"));
        std::fs::write(&script_path, generated["script"].as_str().unwrap()).unwrap();
        let code = if shell == "bash" {
            r#"source "$1"
COMP_WORDS=(labby --team-id team-shell server get al)
COMP_CWORD=6
_labby_with_resources labby al get
printf '%s\n' "${COMPREPLY[@]}"
"#
        } else {
            // Exercise the generated wrapper's real zsh expansion outside ZLE.
            // Static Clap completion is tested separately by its generator.
            r#"autoload -Uz compinit
compinit -D
source "$1"
_labby() { return 0; }
compadd() { shift; printf '%s\n' "$@"; }
words=(labby --team-id team-shell server get al)
CURRENT=7
_labby_with_resources
"#
        };
        let mut command = tokio::process::Command::new(executable);
        command
            .args(["-f", "-c", code, "labby-shell-test"])
            .arg(&script_path)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", home.path())
            .env("LABBY_HOME", home.path().join(".labby"))
            .env("LABBY_SERVER_URL", server.uri())
            .env("LABBY_MCP_HTTP_TOKEN", token)
            .env("NO_COLOR", "1")
            .current_dir(home.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .expect("generated shell completion hung")
            .unwrap();
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let candidates = String::from_utf8(output.stdout).unwrap();
        assert!(
            candidates.lines().any(|candidate| candidate == "alpha"),
            "{shell} dropped the resource prefix: {candidates:?}"
        );
    }
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        request_count,
        "shell completion made a network request"
    );
    assert_eq!(
        snapshots(home.path()),
        before,
        "shell completion mutated cached names"
    );
}

#[tokio::test]
async fn cached_queries_are_offline_and_bound_to_destination_team_and_credential() {
    let home = tempfile::tempdir().unwrap();
    let server = daemon().await;
    let other = daemon().await;
    let token = "distinct-completion-secret";
    let refreshed = success(
        &run(
            home.path(),
            &server,
            token,
            "team-a",
            &["--json", "completions", "refresh"],
        )
        .await,
    );
    assert_eq!(refreshed["counts"]["server"], 1);
    let before = snapshots(home.path());
    assert!(!before.is_empty());
    for body in before.values() {
        assert!(!String::from_utf8_lossy(body).contains(token));
        assert!(!String::from_utf8_lossy(body).contains("$(unsafe)"));
    }
    let count = server.received_requests().await.unwrap().len();
    for (resource, prefix, expected) in [
        ("server", "al", "alpha"),
        ("route", "pri", "private-route"),
        ("loadout", "per", "personal"),
    ] {
        let value = success(
            &run(
                home.path(),
                &server,
                token,
                "team-a",
                &[
                    "--json",
                    "completions",
                    "query",
                    "--",
                    resource,
                    "get",
                    prefix,
                ],
            )
            .await,
        );
        assert_eq!(value, json!([expected]));
    }
    for (words, expected) in [
        (vec!["server", "restart", "al"], json!(["alpha"])),
        (vec!["server", "auth", "status", "al"], json!(["alpha"])),
        (vec!["server", "get", "already-selected", "al"], json!([])),
    ] {
        let args = [vec!["--json", "completions", "query", "--"], words.clone()].concat();
        let value = success(&run(home.path(), &server, token, "team-a", &args).await);
        assert_eq!(value, expected, "incorrect operand position for {words:?}");
    }
    for (target, credential, team) in [
        (&server, token, "team-b"),
        (&server, "different-secret", "team-a"),
        (&other, token, "team-a"),
    ] {
        let value = success(
            &run(
                home.path(),
                target,
                credential,
                team,
                &[
                    "--json",
                    "completions",
                    "query",
                    "--",
                    "server",
                    "get",
                    "al",
                ],
            )
            .await,
        );
        assert_eq!(value, json!([]), "names leaked across authority selection");
    }
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        count,
        "Tab contacted the gateway"
    );
    assert!(other.received_requests().await.unwrap().is_empty());
    assert_eq!(
        snapshots(home.path()),
        before,
        "completion queries wrote cache state"
    );
}

#[tokio::test]
async fn expired_or_corrupt_cache_degrades_to_static_commands_without_network() {
    let home = tempfile::tempdir().unwrap();
    let server = daemon().await;
    success(
        &run(
            home.path(),
            &server,
            "token",
            "team",
            &["--json", "completions", "refresh"],
        )
        .await,
    );
    let directory = home.path().join(".labby/completions");
    let file = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|value| value == "json"))
        .unwrap();
    let count = server.received_requests().await.unwrap().len();
    let mut cache: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    cache["created_at"] = json!(1);
    for bytes in [serde_json::to_vec(&cache).unwrap(), b"{broken".to_vec()] {
        std::fs::write(&file, &bytes).unwrap();
        let names = success(
            &run(
                home.path(),
                &server,
                "token",
                "team",
                &[
                    "--json",
                    "completions",
                    "query",
                    "--",
                    "server",
                    "get",
                    "al",
                ],
            )
            .await,
        );
        assert_eq!(names, json!([]));
        let commands = success(
            &run(
                home.path(),
                &server,
                "token",
                "team",
                &["--json", "completions", "query", "--", "ho"],
            )
            .await,
        );
        assert!(commands.as_array().unwrap().contains(&json!("host")));
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
    assert_eq!(server.received_requests().await.unwrap().len(), count);
}

#[tokio::test]
async fn failed_refresh_discards_old_category_without_discarding_other_results() {
    let home = tempfile::tempdir().unwrap();
    let server = daemon().await;
    success(
        &run(
            home.path(),
            &server,
            "token",
            "team",
            &["--json", "completions", "refresh"],
        )
        .await,
    );
    Mock::given(method("POST"))
        .and(path("/v1/gateway"))
        .and(body_partial_json(json!({"action":"gateway.list"})))
        .respond_with(ResponseTemplate::new(503))
        .with_priority(1)
        .mount(&server)
        .await;
    let output = run(
        home.path(),
        &server,
        "token",
        "team",
        &["--json", "completions", "refresh"],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["errors"]["server"].is_object());
    assert_eq!(result["counts"]["route"], 1);
    let names = success(
        &run(
            home.path(),
            &server,
            "token",
            "team",
            &[
                "--json",
                "completions",
                "query",
                "--",
                "server",
                "get",
                "al",
            ],
        )
        .await,
    );
    assert_eq!(
        names,
        json!([]),
        "failed refresh must not resurrect stale names"
    );
}

#[tokio::test]
async fn clearing_cache_is_authority_scoped_and_does_not_contact_the_gateway() {
    let home = tempfile::tempdir().unwrap();
    let server = daemon().await;
    for team in ["a", "b"] {
        success(
            &run(
                home.path(),
                &server,
                "token",
                team,
                &["--json", "completions", "refresh"],
            )
            .await,
        );
    }
    let count = server.received_requests().await.unwrap().len();
    let cleared = success(
        &run(
            home.path(),
            &server,
            "token",
            "a",
            &["--json", "completions", "clear"],
        )
        .await,
    );
    assert_eq!(cleared["changed"], true);
    let a = success(
        &run(
            home.path(),
            &server,
            "token",
            "a",
            &[
                "--json",
                "completions",
                "query",
                "--",
                "server",
                "get",
                "al",
            ],
        )
        .await,
    );
    let b = success(
        &run(
            home.path(),
            &server,
            "token",
            "b",
            &[
                "--json",
                "completions",
                "query",
                "--",
                "server",
                "get",
                "al",
            ],
        )
        .await,
    );
    assert_eq!(a, json!([]));
    assert_eq!(b, json!(["alpha"]));
    assert_eq!(server.received_requests().await.unwrap().len(), count);
}
