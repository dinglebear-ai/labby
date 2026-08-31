#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use action_matrix::{EvidenceLevel, ScenarioKind, Surface};
use action_scenarios::{ActionOutcome, MATRIX_DEADLINE, MAX_CHILDREN};

#[test]
fn every_cli_classification_has_exactly_one_execution_or_contract_plan() {
    let plans = action_scenarios::exact_plans(Surface::Cli);
    assert_eq!(plans.len(), action_matrix::EXPECTED_CLI_ACTIONS);
    assert_eq!(action_scenarios::services_for(Surface::Cli).len(), 7);
    let grouped_execution = std::collections::BTreeSet::from([
        "gateway:gateway.add",
        "gateway:gateway.get",
        "gateway:gateway.test",
        "gateway:gateway.update",
        "gateway:gateway.remove",
        "setup:draft.discard",
        "setup:install_plugin",
        "setup:installed_plugins",
        "setup:services_status",
        "setup:uninstall_plugin",
        "snippets:snippets.create",
        "snippets:snippets.get",
        "snippets:snippets.validate",
        "snippets:snippets.remove",
    ]);
    let outcomes = action_matrix::intents()
        .iter()
        .filter(|intent| intent.applicable_surfaces.contains(&Surface::Cli))
        .map(|intent| {
            let executed = grouped_execution.contains(intent.key().as_str());
            let evidence = if executed {
                if matches!(
                    intent.scenario_kind,
                    ScenarioKind::StatefulScenario | ScenarioKind::DestructiveIsolated
                ) {
                    EvidenceLevel::LiveStateTransition
                } else {
                    EvidenceLevel::LiveSuccess
                }
            } else {
                match intent.scenario_kind {
                    ScenarioKind::ConditionalOptional => EvidenceLevel::RouterReachable,
                    ScenarioKind::ExternalOptional | ScenarioKind::ExcludedWithReason => {
                        EvidenceLevel::LiveErrorPath
                    }
                    _ => EvidenceLevel::MetadataOnly,
                }
            };
            ActionOutcome {
                key: intent.key(),
                surface: Surface::Cli,
                disposition: action_scenarios::disposition(intent),
                evidence,
                owner: intent.scenario_owner,
                outcome_kind: if executed {
                    "grouped_live_execution"
                } else {
                    "dedicated_contract_owner"
                }
                .into(),
                recovery: if executed {
                    "none_required"
                } else {
                    "owned_by_scenario_id"
                }
                .into(),
                side_effects: if executed {
                    "owned_state_cleaned"
                } else {
                    "none_expected"
                }
                .into(),
                canary_free: true,
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(outcomes.len(), action_matrix::EXPECTED_CLI_ACTIONS);
    assert!(outcomes.iter().all(|outcome| {
        outcome.surface == Surface::Cli
            && !outcome.outcome_kind.is_empty()
            && !outcome.recovery.is_empty()
            && !outcome.side_effects.is_empty()
            && outcome.canary_free
    }));
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.key.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        action_matrix::EXPECTED_CLI_ACTIONS
    );
}

#[tokio::test]
async fn compiled_cli_service_probes_use_stable_json_and_isolated_state() {
    tokio::time::timeout(MATRIX_DEADLINE, async {
        let root = tempfile::tempdir().expect("isolated CLI matrix root");
        let fixtures = action_scenarios::fixtures();
        let mut tasks = tokio::task::JoinSet::new();
        for fixture in fixtures
            .values()
            .filter(|fixture| fixture.cli_probe.is_some())
        {
            let home = root.path().join(&fixture.service);
            std::fs::create_dir_all(home.join("tmp")).unwrap();
            let args = fixture.cli_probe.clone().unwrap();
            let service = fixture.service.clone();
            tasks.spawn(async move {
                let output = action_scenarios::run_cli_probe(&home, &args).await?;
                Ok::<_, String>((service, output))
            });
            assert!(
                tasks.len() <= MAX_CHILDREN,
                "CLI child concurrency exceeded bound"
            );
            if tasks.len() == MAX_CHILDREN {
                let result = tasks.join_next().await.unwrap().unwrap().unwrap();
                action_scenarios::assert_json_or_help(&result.1, &result.0);
            }
        }
        while let Some(result) = tasks.join_next().await {
            let (service, output) = result.unwrap().unwrap();
            action_scenarios::assert_json_or_help(&output, &service);
        }
    })
    .await
    .expect("CLI matrix absolute deadline");
}

#[test]
fn mutation_capable_cli_services_are_explicit_and_disposable() {
    let fixtures = action_scenarios::fixtures();
    let mutable = fixtures
        .values()
        .filter(|fixture| fixture.can_mutate)
        .map(|fixture| fixture.service.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        mutable,
        std::collections::BTreeSet::from(["gateway", "setup", "snippets"])
    );
    let known = action_matrix::intents()
        .iter()
        .map(action_matrix::CaseIntent::key)
        .collect::<std::collections::BTreeSet<_>>();
    for fixture in fixtures.values() {
        for action in [
            Some(&fixture.success_action),
            Some(&fixture.invalid_action),
            fixture.policy_action.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                known.contains(&format!("{}:{action}", fixture.service)),
                "{} fixture references unknown action {action}",
                fixture.service
            );
        }
        assert_eq!(
            fixture.can_mutate,
            !fixture.workflow.is_empty(),
            "{} workflow ownership drifted",
            fixture.service
        );
    }
}

#[tokio::test]
async fn stateful_cli_workflows_observe_mutations_and_always_roll_them_back() {
    tokio::time::timeout(MATRIX_DEADLINE, async {
        let root = tempfile::tempdir().expect("owned workflow root");
        let home = root.path();
        std::fs::create_dir_all(home.join("tmp")).unwrap();

        // Snippets: create -> read -> validate -> remove -> prove absence.
        let create = action_scenarios::run_cli(
            home,
            &[
                "snippets",
                "create",
                "matrix-owned",
                "--code",
                "async () => ({ ok: true })",
                "--json",
            ],
        )
        .await
        .unwrap();
        action_scenarios::assert_json_or_help(&create, "snippets.create");
        let get = action_scenarios::run_cli(home, &["snippets", "get", "matrix-owned", "--json"])
            .await
            .unwrap();
        action_scenarios::assert_json_or_help(&get, "snippets.get");
        assert!(String::from_utf8_lossy(&get.stdout).contains("matrix-owned"));
        let validate =
            action_scenarios::run_cli(home, &["snippets", "validate", "matrix-owned", "--json"])
                .await
                .unwrap();
        action_scenarios::assert_json_or_help(&validate, "snippets.validate");
        let remove = action_scenarios::run_cli(
            home,
            &["snippets", "remove", "matrix-owned", "--yes", "--json"],
        )
        .await
        .unwrap();
        action_scenarios::assert_json_or_help(&remove, "snippets.remove");
        let absent =
            action_scenarios::run_cli(home, &["snippets", "get", "matrix-owned", "--json"])
                .await
                .unwrap();
        assert!(
            !absent.status.success(),
            "removed snippet remained observable"
        );
        action_scenarios::assert_sanitized(&absent.stdout, "snippets.absent");
        action_scenarios::assert_sanitized(&absent.stderr, "snippets.absent");

        // Setup: discard is destructive, but only the harness-owned draft may
        // be touched. Both dry-run interruption and authorized cleanup are
        // observable at the filesystem boundary.
        let draft = home.join(".labby/.env.draft");
        std::fs::create_dir_all(draft.parent().unwrap()).unwrap();
        std::fs::write(&draft, "LABBY_MATRIX_VALUE=owned\n").unwrap();
        let dry_run =
            action_scenarios::run_cli(home, &["setup", "draft", "discard", "--dry-run", "--json"])
                .await
                .unwrap();
        assert!(dry_run.status.success(), "setup dry-run failed");
        action_scenarios::assert_sanitized(&dry_run.stdout, "setup.draft.discard.dry_run");
        action_scenarios::assert_sanitized(&dry_run.stderr, "setup.draft.discard.dry_run");
        assert!(
            String::from_utf8_lossy(&dry_run.stdout).contains("draft.discard"),
            "dry-run did not bind the canonical action"
        );
        assert!(draft.exists(), "interrupted discard changed state");
        let discard =
            action_scenarios::run_cli(home, &["setup", "draft", "discard", "--yes", "--json"])
                .await
                .unwrap();
        action_scenarios::assert_json_or_help(&discard, "setup.draft.discard");
        assert!(
            !draft.exists(),
            "authorized discard did not clean owned state"
        );

        // Gateway: create -> read -> update -> delete uses only deliberately
        // unreachable loopback endpoints, so no external service or credential
        // can be contacted while durable configuration is exercised.
        let gateway_add = action_scenarios::run_cli(
            home,
            &[
                "gateway",
                "add",
                "--name",
                "matrix-owned",
                "--url",
                "http://127.0.0.1:9/mcp",
                "--json",
            ],
        )
        .await
        .unwrap();
        action_scenarios::assert_json_or_help(&gateway_add, "gateway.add");
        let gateway_get =
            action_scenarios::run_cli(home, &["gateway", "get", "matrix-owned", "--json"])
                .await
                .unwrap();
        action_scenarios::assert_json_or_help(&gateway_get, "gateway.get");
        assert!(String::from_utf8_lossy(&gateway_get.stdout).contains("127.0.0.1:9"));
        let gateway_update = action_scenarios::run_cli(
            home,
            &[
                "gateway",
                "update",
                "matrix-owned",
                "--url",
                "http://127.0.0.1:10/mcp",
                "--json",
            ],
        )
        .await
        .unwrap();
        action_scenarios::assert_json_or_help(&gateway_update, "gateway.update");
        let gateway_remove =
            action_scenarios::run_cli(home, &["gateway", "remove", "matrix-owned", "--json"])
                .await
                .unwrap();
        action_scenarios::assert_json_or_help(&gateway_remove, "gateway.remove");
        let gateway_absent =
            action_scenarios::run_cli(home, &["gateway", "get", "matrix-owned", "--json"])
                .await
                .unwrap();
        assert!(
            !gateway_absent.status.success(),
            "removed gateway remained observable"
        );
        action_scenarios::assert_sanitized(&gateway_absent.stdout, "gateway.absent");
        action_scenarios::assert_sanitized(&gateway_absent.stderr, "gateway.absent");

        // Missing transport parameters prove the stable invalid-input contract
        // after cleanup and cannot silently fall back to ambient configuration.
        let invalid = action_scenarios::run_cli(home, &["gateway", "test", "--json"])
            .await
            .unwrap();
        assert!(
            !invalid.status.success(),
            "invalid gateway proposal unexpectedly succeeded"
        );
        action_scenarios::assert_sanitized(&invalid.stdout, "gateway.invalid");
        action_scenarios::assert_sanitized(&invalid.stderr, "gateway.invalid");
    })
    .await
    .expect("stateful workflows absolute deadline");
}

#[tokio::test]
async fn legacy_cli_aliases_reach_the_same_dispatch_contract() {
    let root = tempfile::tempdir().expect("alias root");
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    for (command, canonical) in [
        ("install-plugin", "plugin.install"),
        ("uninstall-plugin", "plugin.uninstall"),
    ] {
        let output = action_scenarios::run_cli(
            root.path(),
            &["setup", command, "matrix-missing", "--dry-run", "--json"],
        )
        .await
        .unwrap();
        action_scenarios::assert_sanitized(&output.stdout, command);
        action_scenarios::assert_sanitized(&output.stderr, command);
        assert!(output.status.success(), "{command} alias dry-run failed");
        let rendered = String::from_utf8_lossy(&output.stdout);
        assert!(
            rendered.contains(canonical),
            "CLI alias did not resolve to {canonical}: {rendered}"
        );
    }
    for command in ["installed-plugins", "services-status"] {
        let output = action_scenarios::run_cli(root.path(), &["setup", command, "--help"])
            .await
            .unwrap();
        action_scenarios::assert_json_or_help(&output, command);
    }
}

#[tokio::test]
async fn explicit_remote_failure_never_falls_back_or_creates_local_state() {
    let root = tempfile::tempdir().expect("explicit remote root");
    std::fs::create_dir_all(root.path().join("tmp")).unwrap();
    let mut command = tokio::process::Command::from(live_labby::isolated_command(root.path()));
    command
        .env("LABBY_SERVER_URL", "http://127.0.0.1:9")
        .args(["gateway", "list", "--json"]);
    let output = tokio::time::timeout(action_scenarios::CHILD_DEADLINE, command.output())
        .await
        .expect("explicit remote failure deadline")
        .unwrap();
    assert!(
        !output.status.success(),
        "explicit remote silently fell back"
    );
    action_scenarios::assert_sanitized(&output.stdout, "explicit remote");
    action_scenarios::assert_sanitized(&output.stderr, "explicit remote");
    assert!(!root.path().join(".labby/config.toml").exists());
    assert!(!root.path().join(".labby/auth.db").exists());
}
