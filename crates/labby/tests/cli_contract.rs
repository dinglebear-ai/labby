//! Public CLI grammar and process-boundary contracts. No live gateway is required.

use clap::{CommandFactory, Parser};
use labby::cli::Cli;
use std::process::{Command, Output};

#[test]
fn live_repository_text_does_not_teach_retired_cli_prefixes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root");
    let tracked = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .expect("list tracked repository files");
    assert!(tracked.status.success());

    let mut stale = Vec::new();
    for raw in tracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|p| !p.is_empty())
    {
        let relative = String::from_utf8_lossy(raw);
        if relative == "CHANGELOG.md"
            || relative == "crates/labby/src/cli/migration.rs"
            || relative == "plugins/scripts/health-check"
            || relative == "docs/generated/cli-migration.md"
            || relative.starts_with("crates/labby/tests/")
            || relative.starts_with("docs/archive/")
            || relative.starts_with("docs/plans/")
            || relative.starts_with("docs/sessions/")
            || relative.starts_with("docs/superpowers/")
        {
            continue;
        }
        let path = root.join(relative.as_ref());
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (retired, replacement) in labby::cli::migration::MOVED {
            let needle = format!("labby {retired}");
            if contents.contains(&needle) {
                stale.push(format!("{relative}: {needle} -> labby {replacement}"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "live files contain retired CLI spellings:\n{}",
        stale.join("\n")
    );
}

fn public_commands(command: &clap::Command, prefix: &str, paths: &mut Vec<String>) {
    for child in command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
    {
        let path = format!("{prefix} {}", child.get_name());
        assert!(
            !child.get_name().contains('-'),
            "hyphenated public command: {path}"
        );
        for alias in child.get_all_aliases() {
            assert!(
                !alias.contains('-'),
                "hyphenated public alias: {path} -> {alias}"
            );
        }
        paths.push(path.clone());
        public_commands(child, &path, paths);
    }
}

fn invoke(args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    // Help must be usable even when installation configuration is malformed.
    std::fs::create_dir(home.path().join(".labby")).unwrap();
    std::fs::write(home.path().join(".labby/config.toml"), "[broken").unwrap();
    Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(args)
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("NO_COLOR", "1")
        .current_dir(home.path())
        .output()
        .unwrap()
}

#[test]
fn public_commands_and_aliases_never_contain_hyphens() {
    let mut command = Cli::command();
    command.build();
    let mut paths = Vec::new();
    public_commands(&command, "labby", &mut paths);
    assert!(!paths.is_empty());
}

#[test]
fn canonical_commands_use_resources_and_consistent_operands() {
    for args in [
        vec!["auth", "login", "--server", "https://example.invalid"],
        vec!["host", "service", "status"],
        vec!["host", "incus", "backup", "validate"],
        vec!["config", "draft", "discard", "--dry-run"],
        vec!["state", "access", "migrate"],
        vec!["help", "--all"],
        vec!["help", "host", "--all"],
        vec!["help", "--search", "oauth"],
    ] {
        let argv = [vec!["labby"], args.clone()].concat();
        assert!(
            Cli::try_parse_from(argv).is_ok(),
            "failed to parse {args:?}"
        );
    }
}

#[test]
#[cfg(feature = "gateway")]
fn server_workflow_is_shallow_and_names_are_positional() {
    for args in [
        vec!["server", "list"],
        vec!["server", "get", "linear-notification-worker"],
        vec![
            "server",
            "add",
            "axon",
            "--url",
            "https://example.invalid/mcp",
        ],
        vec![
            "server",
            "set",
            "axon",
            "--url",
            "https://example.invalid/mcp",
        ],
        vec!["server", "test", "axon"],
        vec!["server", "restart", "axon"],
        vec!["server", "auth", "login", "axon"],
        vec!["route", "list"],
        vec![
            "loadout",
            "set",
            "personal",
            "--description",
            "Personal tools",
        ],
        vec!["code", "run", "--file", "example.js"],
        vec!["snippet", "list"],
    ] {
        assert!(
            Cli::try_parse_from([vec!["labby"], args.clone()].concat()).is_ok(),
            "failed to parse {args:?}"
        );
    }
    assert!(Cli::try_parse_from(["labby", "server", "test"]).is_err());
}

#[test]
fn root_help_describes_cli_commands_without_loading_configuration() {
    for args in [
        vec!["--help"],
        vec!["--team-id", "audit", "--help"],
        vec!["--help", "--team-id", "audit"],
    ] {
        let output = invoke(&args);
        assert!(
            output.status.success(),
            "help failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains("host") && text.contains("auth"),
            "not CLI help: {text}"
        );
        assert!(
            !text.contains("(+"),
            "truncated action catalog leaked into CLI help"
        );
    }
}

#[test]
fn complete_json_help_matches_every_public_command() {
    let output = invoke(&["help", "--all", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let listed = document["commands"].as_array().expect("command inventory");
    let mut command = Cli::command();
    command.build();
    let mut paths = Vec::new();
    public_commands(&command, "labby", &mut paths);
    let expected = paths.into_iter().collect::<std::collections::BTreeSet<_>>();
    let actual = listed
        .iter()
        .map(|entry| {
            entry["command"]
                .as_str()
                .expect("qualified command path")
                .to_owned()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        listed.len(),
        actual.len(),
        "duplicate commands in JSON help"
    );
    assert_eq!(
        actual, expected,
        "JSON help must have no missing or phantom commands"
    );
    for required in ["labby help", "labby auth", "labby host", "labby config"] {
        assert!(
            actual.contains(required),
            "required public group missing: {required}"
        );
    }
}

#[test]
fn parser_errors_do_not_mistake_global_values_for_commands() {
    let output = invoke(&["--team-id", "audit", "--json", "host", "nonsense"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_ne!(error["command"], "audit");
    assert_eq!(error["error"]["origin"], "validation");
    assert_eq!(error["error"]["side_effects"], "none_expected");
    assert!(
        error["error"]["recovery"]["guidance"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
}

#[test]
fn retired_hyphenated_commands_are_rejected_instead_of_hidden_aliases() {
    assert!(Cli::try_parse_from(["labby", "setup", "host-service", "status"]).is_err());
    assert!(Cli::try_parse_from(["labby", "setup", "incusbackup", "validate"]).is_err());
}

fn runtime_command(home: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("NO_COLOR", "1")
        .env("LABBY_HOME", home.join(".labby"))
        .stdin(std::process::Stdio::null())
        .current_dir(home);
    command
}

#[test]
fn every_public_help_path_runs_offline_and_has_qualified_usage() {
    let mut root = Cli::command();
    root.build();
    let mut paths = Vec::new();
    public_commands(&root, "labby", &mut paths);
    for path in paths {
        let mut args = path.split_whitespace().skip(1).collect::<Vec<_>>();
        args.push("--help");
        let output = invoke(&args);
        assert!(
            output.status.success(),
            "{path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&format!("Usage: {path}")),
            "unqualified usage at {path}"
        );
    }
}

#[test]
fn retired_commands_explain_the_replacement_without_running_it() {
    let output = invoke(&["--json", "setup", "host-service", "restart"]);
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["cause"]
            .as_str()
            .unwrap()
            .contains("labby host service")
    );
    assert_eq!(error["error"]["side_effects"], "none_expected");
}

#[test]
#[cfg(feature = "gateway")]
fn runtime_json_errors_and_file_logs_preserve_context_without_raw_arguments() {
    let home = tempfile::tempdir().unwrap();
    let output = runtime_command(
        home.path(),
        &[
            "-vv",
            "--json",
            "code",
            "run",
            "--file",
            "private-source-name.js",
        ],
    )
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr)
        .expect("exactly one JSON error, not mixed console logs");
    assert_eq!(error["command"], "code run");
    assert_eq!(error["error"]["kind"], "invalid_param");
    assert_eq!(error["error"]["side_effects"], "none_expected");
    let request_id = error["request_id"].as_str().expect("correlation id");
    let log_dir = home.path().join(".local/share/labby/logs");
    let logs = std::fs::read_dir(log_dir)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(logs.contains(request_id));
    assert!(logs.contains("elapsed_ms"));
    assert!(logs.contains("code run"));
    assert!(
        !logs.contains("private-source-name.js"),
        "raw arguments must not enter operation logs"
    );
    for line in logs.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }
}

#[test]
#[cfg(feature = "gateway")]
fn human_errors_remain_visible_with_logging_disabled() {
    let home = tempfile::tempdir().unwrap();
    let output = runtime_command(
        home.path(),
        &["--quiet", "code", "run", "--file", "missing.js"],
    )
    .env("LABBY_LOG", "off")
    .output()
    .unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("Error [invalid_param]"));
    assert!(text.contains("Command: labby code run"));
    assert!(text.contains("Effects:"));
    assert!(text.contains("Next:"));
    assert!(text.contains("missing.js"));
}

#[test]
#[cfg(feature = "gateway")]
fn unavailable_file_logging_does_not_panic_or_corrupt_json_errors() {
    let home = tempfile::tempdir().unwrap();
    let blocked = home.path().join("not-a-directory");
    std::fs::write(&blocked, "occupied").unwrap();
    let output = runtime_command(
        home.path(),
        &["--json", "code", "run", "--file", "missing.js"],
    )
    .env("LABBY_LOG_DIR", &blocked)
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["error"]["kind"], "invalid_param");
}

#[test]
#[cfg(feature = "gateway")]
fn dry_run_is_json_and_does_not_dispatch_or_open_a_gateway() {
    let home = tempfile::tempdir().unwrap();
    let output = runtime_command(
        home.path(),
        &["--json", "snippet", "remove", "not-present", "--dry-run"],
    )
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["dry_run"], true);
    assert_eq!(value["executed"], false);
    assert_eq!(value["action"], "snippets.remove");
}

#[test]
#[cfg(feature = "gateway")]
fn code_catalog_commands_have_bounded_typed_inputs() {
    assert!(Cli::try_parse_from(["labby", "code", "search", "oauth", "--limit", "10"]).is_ok());
    assert!(Cli::try_parse_from(["labby", "code", "describe", "example.tool"]).is_ok());
    for limit in ["0", "101", "not-a-number"] {
        assert!(
            Cli::try_parse_from(["labby", "code", "search", "oauth", "--limit", limit]).is_err()
        );
    }
    assert!(Cli::try_parse_from(["labby", "code", "run"]).is_err());
}

#[test]
fn logs_are_bounded_structured_and_redacted_without_a_journal_dependency() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join("process-logs");
    std::fs::create_dir(&dir).unwrap();
    let event = serde_json::json!({"timestamp":"2026-09-19T00:00:00Z","level":"ERROR","target":"labby::cli::audit","fields":{"message":"command failed","request_id":"query-request","token":"unique-log-secret"}});
    std::fs::write(dir.join("lab.test.log"), format!("{event}\n")).unwrap();
    let output = runtime_command(
        home.path(),
        &[
            "--json",
            "logs",
            "--query",
            "query-request",
            "--level",
            "error",
            "--lines",
            "1",
        ],
    )
    .env("LABBY_LOG_DIR", &dir)
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["entries"].as_array().unwrap().len(), 1);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("unique-log-secret"));
    assert_eq!(result["filters"]["limit"], 1);
    assert!(output.stderr.is_empty());
}

#[test]
fn leaf_json_help_includes_the_selected_commands_usage_and_options() {
    let output = invoke(&["help", "host", "service", "restart", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["command"], "labby host service restart");
    assert!(
        document["usage"]
            .as_str()
            .is_some_and(|s| s.contains("labby host service restart")),
        "leaf help lost its usage: {document}"
    );
    assert!(
        document["description"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        document["help"]
            .as_str()
            .is_some_and(|s| s.contains("--json"))
    );
    assert!(document["commands"].as_array().unwrap().is_empty());
}

#[test]
fn downstream_json_argument_does_not_change_parser_error_format() {
    let output = invoke(&["host", "unknown", "--", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(serde_json::from_slice::<serde_json::Value>(&output.stderr).is_err());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));

    let output = invoke(&["--json", "host", "unknown", "--", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["error"]["kind"], "invalid_param");
}

#[test]
fn downstream_color_argument_does_not_override_offline_help_style() {
    let output = invoke(&["--color", "plain", "--help", "--", "--color", "color"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.stdout.contains(&0x1b),
        "forwarded argument enabled ANSI styling"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn public_name_contract_detects_injected_bad_commands_and_aliases() {
    for mut command in [
        clap::Command::new("labby").subcommand(clap::Command::new("bad-command")),
        clap::Command::new("labby").subcommand(clap::Command::new("valid").alias("bad-alias")),
    ] {
        command.build();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            public_commands(&command, "labby", &mut Vec::new());
        }))
        .expect_err("negative control: invalid command tree escaped validation");
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            message.contains("hyphenated public"),
            "unrelated panic is not a passing negative control: {message}"
        );
    }
    let mut valid = clap::Command::new("labby")
        .disable_help_subcommand(true)
        .subcommand(clap::Command::new("list").alias("ls"));
    valid.build();
    let mut paths = Vec::new();
    public_commands(&valid, "labby", &mut paths);
    assert_eq!(paths, ["labby list"]);
}

#[test]
fn setup_validation_recipe_uses_the_executable_cli_grammar() {
    let recipe = include_str!("../../../Justfile")
        .split_once("\nvalidate-plugin:\n")
        .expect("plugin validation recipe")
        .1
        .split("\n\n")
        .next()
        .unwrap();
    let command = recipe
        .lines()
        .find(|line| line.contains("cargo run"))
        .expect("CLI invocation");
    let arguments = command
        .split_once(" -- ")
        .expect("cargo argument boundary")
        .1;
    let argv = std::iter::once("labby").chain(arguments.split_whitespace());
    Cli::try_parse_from(argv).expect("setup-validation recipe must use valid Labby arguments");
}

fn snapshot_tree(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        entries: &mut std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            assert!(
                !kind.is_symlink(),
                "test fixture must not escape its sandbox"
            );
            entries.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                if kind.is_dir() {
                    None
                } else {
                    Some(std::fs::read(&path).unwrap())
                },
            );
            if kind.is_dir() {
                visit(root, &path, entries);
            }
        }
    }
    let mut entries = std::collections::BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

#[test]
fn offline_help_and_completion_do_not_write_installation_state() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join(".labby")).unwrap();
    std::fs::write(home.path().join(".labby/config.toml"), "[broken").unwrap();
    let before = snapshot_tree(home.path());
    assert!(!before.is_empty(), "nonempty fixture required");
    for args in [vec!["help", "--all", "--json"], vec!["completions", "bash"]] {
        let output = runtime_command(home.path(), &args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
        assert_eq!(
            snapshot_tree(home.path()),
            before,
            "offline discovery wrote state: {args:?}"
        );
    }
}

#[cfg(feature = "gateway")]
#[tokio::test]
async fn snippet_preview_and_confirmation_preserve_state_until_explicit_removal() {
    let home = tempfile::tempdir().unwrap();
    let gateway = wiremock::MockServer::start().await;
    async fn run(home: &std::path::Path, gateway: &wiremock::MockServer, args: &[&str]) -> Output {
        let mut command = tokio::process::Command::from(runtime_command(home, args));
        command
            .env("LABBY_SERVER_URL", gateway.uri())
            .kill_on_drop(true);
        tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
            .await
            .expect("CLI hung or unexpectedly prompted")
            .unwrap()
    }
    let create = run(
        home.path(),
        &gateway,
        &[
            "--json",
            "snippet",
            "add",
            "preserved",
            "--code",
            "async () => ({ preserved: true })",
            "--description",
            "Lifecycle regression fixture",
        ],
    )
    .await;
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&create.stdout).unwrap();
    let state = home.path().join(".labby");
    let before = snapshot_tree(&state);
    assert!(
        before.values().any(Option::is_some),
        "creation must produce actual saved content"
    );

    let preview = run(
        home.path(),
        &gateway,
        &["--json", "snippet", "remove", "preserved", "--dry-run"],
    )
    .await;
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(value["executed"], false);
    assert_eq!(snapshot_tree(&state), before, "dry-run changed saved state");

    let blocked = run(
        home.path(),
        &gateway,
        &["--json", "snippet", "remove", "preserved"],
    )
    .await;
    assert_eq!(blocked.status.code(), Some(1));
    assert!(blocked.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&blocked.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "confirmation_required");
    assert_eq!(error["error"]["side_effects"], "none_expected");
    assert_eq!(
        snapshot_tree(&state),
        before,
        "refused removal changed saved state"
    );

    // Positive control: the same resource really can be removed when confirmed.
    let removed = run(
        home.path(),
        &gateway,
        &["--json", "snippet", "remove", "preserved", "--yes"],
    )
    .await;
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_ne!(
        snapshot_tree(&state),
        before,
        "confirmed removal did not change state"
    );
    let missing = run(
        home.path(),
        &gateway,
        &["--json", "snippet", "get", "preserved"],
    )
    .await;
    assert!(!missing.status.success(), "removed snippet still exists");
    assert!(
        gateway.received_requests().await.unwrap().is_empty(),
        "local snippet administration contacted a gateway"
    );
}

#[test]
fn raw_journal_mode_rejects_json_before_starting_any_external_process() {
    let home = tempfile::tempdir().unwrap();
    let output = runtime_command(home.path(), &["--json", "logs", "journal", "--follow"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_param");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("No journal process was started")
    );
}
