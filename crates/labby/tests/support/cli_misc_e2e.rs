use std::path::{Path, PathBuf};
use std::process::Output;

use crate::action_matrix::{EvidenceLevel, Surface};
use crate::action_scenarios::{self, ActionOutcome};

pub(crate) async fn run() {
    let root = tempfile::tempdir().expect("miscellaneous CLI E2E root");

    run_observation_cases(root.path()).await;
    run_setup_state(root.path()).await;
    run_plugin_lifecycle(root.path()).await;
    run_setup_mutations(root.path()).await;
    run_snippet_workflow(root.path()).await;

    let owned = root.path().join("owned");
    if owned.exists() {
        std::fs::remove_dir_all(&owned).expect("remove miscellaneous CLI owned state");
    }
    assert!(
        !owned.exists(),
        "miscellaneous CLI owned state survived cleanup"
    );
}

async fn run_observation_cases(root: &Path) {
    let doctor_home = home(root, "doctor-proxy");
    let doctor = execute_with_deadline(
        &doctor_home,
        &[
            "doctor",
            "proxy",
            "--app-url",
            "http://127.0.0.1:9",
            "--mcp-url",
            "http://127.0.0.1:9/mcp",
            "--route",
            "/matrix-owned",
            "--json",
        ],
        &[],
        std::time::Duration::from_secs(30),
    )
    .await;
    record_output("doctor:proxy.check", &doctor, EvidenceLevel::LiveSuccess);

    let logs_home = home(root, "server-logs");
    let logs = execute(
        &logs_home,
        &[
            "logs",
            "--no-follow",
            "--container",
            "matrix-owned-missing",
            "--json",
        ],
        &[],
    )
    .await;
    record_output(
        "server_logs:server_logs.query",
        &logs,
        EvidenceLevel::LiveSuccess,
    );
}

async fn run_setup_state(root: &Path) {
    let state_home = home(root, "setup-state");
    let state = execute(&state_home, &["setup", "--smoke", "--json"], &[]).await;
    record_success("setup:state", &state, EvidenceLevel::LiveSuccess);
}

async fn run_plugin_lifecycle(root: &Path) {
    let plugin_home = home(root, "setup-plugin-lifecycle");
    let install = execute(
        &plugin_home,
        &[
            "setup",
            "install-plugin",
            "matrix-owned-missing",
            "--yes",
            "--json",
        ],
        &[],
    )
    .await;
    record_output(
        "setup:plugin.install",
        &install,
        EvidenceLevel::LiveStateTransition,
    );

    let uninstall = execute(
        &plugin_home,
        &[
            "setup",
            "uninstall-plugin",
            "matrix-owned-missing",
            "--yes",
            "--json",
        ],
        &[],
    )
    .await;
    record_output(
        "setup:plugin.uninstall",
        &uninstall,
        EvidenceLevel::LiveStateTransition,
    );
}

async fn run_setup_mutations(root: &Path) {
    let sync_home = home(root, "setup-plugin-sync");
    let sync = execute(
        &sync_home,
        &["setup", "plugin-sync", "--yes", "--json"],
        &[("CLAUDE_PLUGIN_OPTION_SERVER_URL", "http://localhost:40100")],
    )
    .await;
    let synced_env = sync_home.join(".labby/.env");
    assert!(
        std::fs::read_to_string(&synced_env)
            .expect("read synchronized plugin env")
            .contains("LABBY_SERVER_URL"),
        "plugin sync did not publish its owned setting"
    );
    record_success(
        "setup:plugin_sync",
        &sync,
        EvidenceLevel::LiveStateTransition,
    );

    let proxy_home = home(root, "setup-proxy");
    let proxy = execute(
        &proxy_home,
        &[
            "setup",
            "proxy",
            "--exposure",
            "local",
            "--auth",
            "none",
            "--path",
            "/matrix-owned",
            "--port",
            "45123",
            "--yes",
            "--json",
        ],
        &[],
    )
    .await;
    let proxy_config = proxy_home.join(".labby/config.toml");
    let proxy_text = std::fs::read_to_string(&proxy_config).expect("read owned proxy config");
    assert!(proxy_text.contains("/matrix-owned"));
    assert!(proxy_text.contains("45123"));
    record_success(
        "setup:proxy.configure",
        &proxy,
        EvidenceLevel::LiveStateTransition,
    );

    for (directory, command, key) in [
        ("setup-plugin-hook", "plugin-hook", "setup:plugin_hook"),
        ("setup-repair", "repair", "setup:repair"),
    ] {
        let repair_home = home(root, directory);
        let before = tree_fingerprint(&repair_home);
        let output = execute(&repair_home, &["setup", command, "--json"], &[]).await;
        let after = tree_fingerprint(&repair_home);
        assert_ne!(before, after, "{key} did not mutate its owned fixture");
        record_success(key, &output, EvidenceLevel::LiveStateTransition);
    }
}

async fn run_snippet_workflow(root: &Path) {
    let snippet_home = home(root, "snippets");
    let name = "matrix-misc-owned";
    let create = execute(
        &snippet_home,
        &[
            "snippets",
            "create",
            name,
            "--code",
            "async () => ({ ok: true })",
            "--json",
        ],
        &[],
    )
    .await;
    record_success(
        "snippets:snippets.create",
        &create,
        EvidenceLevel::LiveStateTransition,
    );

    for (action, argv) in [
        (
            "snippets:snippets.get",
            vec!["snippets", "get", name, "--json"],
        ),
        (
            "snippets:snippets.validate",
            vec!["snippets", "validate", name, "--json"],
        ),
        (
            "snippets:snippets.exec",
            vec!["snippets", "exec", name, "--json"],
        ),
        (
            "snippets:snippets.test",
            vec!["snippets", "test", name, "--json"],
        ),
    ] {
        let output = execute(&snippet_home, &argv, &[]).await;
        record_success(action, &output, EvidenceLevel::LiveSuccess);
    }

    let remove = execute(
        &snippet_home,
        &["snippets", "remove", name, "--yes", "--json"],
        &[],
    )
    .await;
    record_success(
        "snippets:snippets.remove",
        &remove,
        EvidenceLevel::LiveStateTransition,
    );
    let absent = execute(&snippet_home, &["snippets", "get", name, "--json"], &[]).await;
    assert!(
        !absent.status.success(),
        "removed owned snippet remained readable"
    );
    action_scenarios::assert_sanitized(&absent.stdout, "removed snippet stdout");
    action_scenarios::assert_sanitized(&absent.stderr, "removed snippet stderr");
}

async fn execute(home: &Path, args: &[&str], environment: &[(&str, &str)]) -> Output {
    execute_with_deadline(home, args, environment, action_scenarios::CHILD_DEADLINE).await
}

async fn execute_with_deadline(
    home: &Path,
    args: &[&str],
    environment: &[(&str, &str)],
    deadline: std::time::Duration,
) -> Output {
    let mut command = tokio::process::Command::from(crate::live_labby::isolated_command(home));
    command
        .args(args)
        .env("LABBY_MATRIX_CANARY", action_scenarios::SECRET_CANARY);
    for (name, value) in environment {
        command.env(name, value);
    }
    tokio::time::timeout(deadline, command.output())
        .await
        .unwrap_or_else(|_| panic!("CLI child exceeded {:?}: {args:?}", deadline))
        .unwrap_or_else(|error| panic!("CLI child failed to start for {args:?}: {error}"))
}

fn record_success(key: &str, output: &Output, evidence: EvidenceLevel) {
    action_scenarios::assert_success_json(output, key);
    record(key, evidence, "owned_cli_evidence");
}

fn record_output(key: &str, output: &Output, success_evidence: EvidenceLevel) {
    action_scenarios::assert_sanitized(&output.stdout, key);
    action_scenarios::assert_sanitized(&output.stderr, key);
    let body = machine_json(output);
    if output.status.success() {
        record(key, success_evidence, "owned_cli_result");
        return;
    }
    let body = body.unwrap_or_else(|| panic!("{key} error was not a stable JSON envelope"));
    assert_eq!(body["ok"], false, "{key} error envelope");
    let error_kind = body
        .pointer("/error/kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("{key} error omitted error.kind"));
    let outcome = action_scenarios::dedicated_contract_reason(key)
        .filter(|_| action_scenarios::dedicated_contract_accepts(key, error_kind))
        .map_or_else(
            || format!("compiled_cli_error:{error_kind}"),
            |reason| format!("dedicated_contract:{reason}:{error_kind}"),
        );
    record(key, EvidenceLevel::LiveErrorPath, &outcome);
}

fn record(key: &str, evidence: EvidenceLevel, outcome_kind: &str) {
    let intent = crate::action_matrix::intents()
        .iter()
        .find(|intent| intent.key() == key)
        .unwrap_or_else(|| panic!("missing authoritative intent for {key}"));
    let exact_dedicated_contract = outcome_kind.starts_with("dedicated_contract:");
    assert!(
        evidence >= intent.minimum_evidence || exact_dedicated_contract,
        "{key} produced {evidence:?}, below {:?}",
        intent.minimum_evidence
    );
    ActionOutcome {
        key: key.to_owned(),
        surface: Surface::Cli,
        disposition: action_scenarios::disposition(intent),
        evidence,
        owner: intent.scenario_owner,
        outcome_kind: outcome_kind.to_owned(),
        recovery: "owned_fixture_cleanup_verified".into(),
        side_effects: "isolated_disposable_home".into(),
        canary_free: true,
    }
    .record();
}

fn machine_json(output: &Output) -> Option<serde_json::Value> {
    [&output.stdout, &output.stderr]
        .into_iter()
        .find_map(|bytes| serde_json::from_slice(bytes).ok())
        .or_else(|| {
            [&output.stdout, &output.stderr]
                .into_iter()
                .flat_map(|bytes| {
                    String::from_utf8_lossy(bytes)
                        .lines()
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .rev()
                .find_map(|line| serde_json::from_str(&line).ok())
        })
}

fn home(root: &Path, name: &str) -> PathBuf {
    let home = root.join("owned").join(name);
    std::fs::create_dir_all(home.join("tmp")).expect("create owned CLI home");
    home
}

fn tree_fingerprint(root: &Path) -> Vec<PathBuf> {
    fn collect(path: &Path, paths: &mut Vec<PathBuf>) {
        paths.push(path.to_path_buf());
        if !std::fs::symlink_metadata(path)
            .expect("inspect owned CLI home entry")
            .is_dir()
        {
            return;
        }
        for entry in std::fs::read_dir(path).expect("walk owned CLI home") {
            collect(&entry.expect("read owned CLI home entry").path(), paths);
        }
    }

    let mut paths = Vec::new();
    collect(root, &mut paths);
    paths.sort();
    paths
}
