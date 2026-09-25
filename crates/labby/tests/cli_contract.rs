//! Public CLI grammar and process-boundary contracts. No live gateway is required.

use clap::{CommandFactory, Parser};
use labby::cli::Cli;
use std::process::{Command, Output};

const ANSI_ESCAPE: u8 = 0x1b;

#[derive(Debug, Clone)]
struct PublicCommand {
    canonical: Vec<String>,
    aliases: Vec<Vec<String>>,
    leaf: bool,
}

fn command_inventory() -> Vec<PublicCommand> {
    fn visit(command: &clap::Command, parent: &[String], inventory: &mut Vec<PublicCommand>) {
        for child in command
            .get_subcommands()
            .filter(|child| !child.is_hide_set())
        {
            let mut canonical = parent.to_vec();
            canonical.push(child.get_name().to_owned());
            let aliases = child
                .get_all_aliases()
                .map(|alias| {
                    let mut path = parent.to_vec();
                    path.push(alias.to_owned());
                    path
                })
                .collect();
            let leaf = child.get_subcommands().all(clap::Command::is_hide_set);
            inventory.push(PublicCommand {
                canonical: canonical.clone(),
                aliases,
                leaf,
            });
            visit(child, &canonical, inventory);
        }
    }

    let mut root = Cli::command();
    root.build();
    let mut inventory = Vec::new();
    visit(&root, &[], &mut inventory);
    inventory
}

fn assert_plain_output(output: &Output, description: &str) {
    assert!(
        !output.stdout.contains(&ANSI_ESCAPE) && !output.stderr.contains(&ANSI_ESCAPE),
        "ANSI escape leaked into {description}"
    );
}

fn find_command<'a>(root: &'a clap::Command, path: &[String]) -> &'a clap::Command {
    path.iter().fold(root, |command, name| {
        let child = command
            .get_subcommands()
            .find(|child| child.get_name() == name);
        assert!(
            child.is_some(),
            "missing Clap command path: {}",
            path.join(" ")
        );
        child.unwrap()
    })
}

fn representative_value(arg: &clap::Arg) -> String {
    if let Some(value) = arg
        .get_value_parser()
        .possible_values()
        .and_then(|mut values| values.next())
    {
        return value.get_name().to_owned();
    }
    let hint = arg
        .get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.as_str().to_ascii_uppercase())
        .unwrap_or_else(|| arg.get_id().as_str().to_ascii_uppercase());
    if hint.contains("URL") || hint.contains("ENDPOINT") || hint.contains("SERVER") {
        "https://example.invalid".to_owned()
    } else if hint.contains("DURATION")
        || ((hint.contains("TIMEOUT") || hint.contains("GRACE"))
            && !hint.contains("SECONDS")
            && !hint.contains("MINUTES"))
    {
        "1s".to_owned()
    } else if hint.contains("PORT") {
        "40100".to_owned()
    } else if hint.contains("LIMIT")
        || hint.contains("LINES")
        || hint.contains("COUNT")
        || hint.contains("MAX")
        || hint.contains("SECONDS")
        || hint.contains("TTL")
        || hint.contains("UNIX")
        || hint.contains("MINUTES")
        || hint.contains("SIZE")
        || hint.contains("DEPTH")
        || hint.contains("RETRIES")
        || hint.contains("PAGE")
        || hint.contains("OFFSET")
    {
        "1".to_owned()
    } else if hint.contains("PATH")
        || hint.contains("FILE")
        || hint.contains("DIR")
        || hint.contains("KEY")
    {
        "/private/tmp/labby-cli-contract".to_owned()
    } else if hint.contains("BOOL") {
        "true".to_owned()
    } else {
        "contract-value".to_owned()
    }
}

fn option_spellings(arg: &clap::Arg) -> Vec<String> {
    let mut spellings = Vec::new();
    if let Some(long) = arg.get_long() {
        spellings.push(format!("--{long}"));
    }
    if let Some(aliases) = arg.get_all_aliases() {
        spellings.extend(aliases.into_iter().map(|alias| format!("--{alias}")));
    }
    if let Some(short) = arg.get_short() {
        spellings.push(format!("-{short}"));
    }
    if let Some(aliases) = arg.get_all_short_aliases() {
        spellings.extend(aliases.into_iter().map(|alias| format!("-{alias}")));
    }
    spellings
}

fn parser_probe(path: &[String], spelling: &str, arg: &clap::Arg) -> Vec<String> {
    let mut argv = vec!["labby".to_owned()];
    argv.extend(path.iter().cloned());
    argv.push(spelling.to_owned());
    if arg.get_action().takes_values() {
        let count = arg.get_num_args().map_or(1, |range| range.min_values());
        argv.extend(std::iter::repeat_with(|| representative_value(arg)).take(count));
    }
    argv.push("--help".to_owned());
    argv
}

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
            let needles = [
                format!("labby {retired}"),
                format!("labby --json {retired}"),
                format!("-- --json {retired}"),
            ];
            for needle in needles {
                if contents.contains(&needle) {
                    stale.push(format!("{relative}: {needle} -> labby {replacement}"));
                }
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
    if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile_file);
    }
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
fn every_public_command_and_alias_has_plain_offline_help() {
    for entry in command_inventory() {
        for path in std::iter::once(&entry.canonical).chain(entry.aliases.iter()) {
            let mut args = path.iter().map(String::as_str).collect::<Vec<_>>();
            args.push("--help");
            let label = format!("labby {}", path.join(" "));
            let output = invoke(&args);
            assert!(
                output.status.success(),
                "{label}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!output.stdout.is_empty(), "empty help for {label}");
            assert!(output.stderr.is_empty(), "stderr from help for {label}");
            assert_plain_output(&output, &format!("help for {label}"));
        }
    }
}

#[test]
fn every_public_command_has_structured_json_help() {
    for entry in command_inventory() {
        let mut args = vec!["help"];
        args.extend(entry.canonical.iter().map(String::as_str));
        args.push("--json");
        let qualified = format!("labby {}", entry.canonical.join(" "));
        let output = invoke(&args);
        assert!(
            output.status.success(),
            "{qualified}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "stderr from JSON help for {qualified}"
        );
        assert_plain_output(&output, &format!("JSON help for {qualified}"));
        let parsed = serde_json::from_slice(&output.stdout);
        assert!(
            parsed.is_ok(),
            "invalid JSON help for {qualified}: {parsed:?}"
        );
        let document: serde_json::Value = parsed.unwrap();
        assert_eq!(document["command"], qualified);
        assert!(
            document["usage"]
                .as_str()
                .is_some_and(|usage| !usage.is_empty()),
            "missing usage for {qualified}"
        );
    }
}

#[test]
fn every_public_option_and_short_flag_accepts_a_representative_value() {
    let mut root = Cli::command();
    root.build();
    let inventory = command_inventory();
    let paths =
        std::iter::once(Vec::new()).chain(inventory.iter().map(|entry| entry.canonical.clone()));
    let mut covered = std::collections::BTreeSet::new();

    for path in paths {
        let command = find_command(&root, &path);
        for arg in command.get_arguments().filter(|arg| !arg.is_hide_set()) {
            for spelling in option_spellings(arg) {
                if matches!(spelling.as_str(), "--help" | "-h" | "--version" | "-V") {
                    continue;
                }
                let argv = parser_probe(&path, &spelling, arg);
                let error = Cli::command()
                    .try_get_matches_from(argv.clone())
                    .expect_err("--help parser probe must stop before command execution");
                assert_eq!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp,
                    "option parser probe failed for {argv:?}: {error}"
                );
                covered.insert(format!("labby {} {spelling}", path.join(" ")));
            }
        }
    }
    assert!(!covered.is_empty(), "Clap graph exposed no options");
}

#[test]
fn feature_specific_command_inventory_matches_the_compiled_slice() {
    let roots = command_inventory()
        .into_iter()
        .filter(|entry| entry.canonical.len() == 1)
        .map(|entry| entry.canonical[0].clone())
        .collect::<std::collections::BTreeSet<_>>();
    for gateway_command in ["gateway", "server", "route", "loadout", "code", "snippet"] {
        assert_eq!(
            roots.contains(gateway_command),
            cfg!(feature = "gateway"),
            "{gateway_command} did not follow the gateway feature"
        );
    }
    assert_eq!(
        roots.contains("skill"),
        cfg!(any(feature = "skills", feature = "gateway")),
        "skill did not follow its documented feature contract"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeafPlan {
    Execute {
        args: &'static [&'static str],
        output: ScenarioOutput,
    },
    Exempt(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScenarioOutput {
    Json,
    Plain,
}

fn leaf_plan(path: &str) -> Option<LeafPlan> {
    Some(match path {
        "help" => LeafPlan::Execute {
            args: &["help", "--all", "--json"],
            output: ScenarioOutput::Json,
        },
        "context list" => LeafPlan::Execute {
            args: &["context", "list", "--json"],
            output: ScenarioOutput::Json,
        },
        "config show" => LeafPlan::Execute {
            args: &["config", "show", "--json"],
            output: ScenarioOutput::Json,
        },
        "config check" => LeafPlan::Execute {
            args: &["config", "check", "--json"],
            output: ScenarioOutput::Json,
        },
        "completions query" => LeafPlan::Execute {
            args: &["completions", "query", "--"],
            output: ScenarioOutput::Plain,
        },

        // Each exemption names a concrete leaf. Do not collapse these arms to a
        // command-prefix wildcard: the exact list is what makes a new Clap leaf
        // fail review instead of silently inheriting an exemption.
        "context get" | "context add" | "context set" | "context use" | "context remove"
        | "context clear" => LeafPlan::Exempt(
            "requires a named context fixture or intentionally mutates durable context selection",
        ),
        "auth login"
        | "auth status"
        | "auth logout"
        | "auth bootstrap prepare"
        | "auth bootstrap consume"
        | "auth bootstrap status"
        | "auth bootstrap recover"
        | "auth bootstrap cleanup"
        | "auth owner link"
        | "auth relay local"
        | "auth relay registry list"
        | "auth relay registry import"
        | "auth relay registry register"
        | "auth relay registry remove"
        | "auth relay registry disable"
        | "auth relay registry enable"
        | "auth provider google revoke" => LeafPlan::Exempt(
            "requires an authenticated identity, bootstrap secret, provider, or relay fixture",
        ),
        "gateway reload"
        | "gateway status"
        | "gateway sessions list"
        | "gateway urls"
        | "gateway usage metrics"
        | "gateway usage calls"
        | "server list"
        | "server get"
        | "server add"
        | "server set"
        | "server remove"
        | "server test"
        | "server status"
        | "server enable"
        | "server disable"
        | "server restart"
        | "server cleanup"
        | "server auth login"
        | "server auth status"
        | "server auth logout"
        | "server discover"
        | "server import"
        | "server pending list"
        | "server pending approve"
        | "server pending reject"
        | "server quarantine list"
        | "server quarantine restore"
        | "route list"
        | "route get"
        | "route add"
        | "route replace"
        | "route remove"
        | "route test"
        | "loadout list"
        | "loadout get"
        | "loadout add"
        | "loadout set"
        | "loadout remove"
        | "code search"
        | "code describe"
        | "code status"
        | "code enable"
        | "code disable"
        | "code ui status"
        | "code ui enable"
        | "code ui disable"
        | "code run"
        | "code hints preview"
        | "code hints apply" => {
            LeafPlan::Exempt("requires a live gateway/provider or a purpose-built protocol fixture")
        }
        "snippet list"
        | "snippet get"
        | "snippet run"
        | "snippet add"
        | "snippet validate"
        | "snippet remove"
        | "snippet test"
        | "skill list"
        | "skill search"
        | "skill get"
        | "skill read"
        | "skill source list"
        | "skill source trust"
        | "skill source untrust"
        | "skill source exposure set"
        | "skill source exposure clear" => LeafPlan::Exempt(
            "requires repository/plugin artifact fixtures and may create usage or trust state",
        ),
        "doctor auth" | "doctor relay" | "doctor proxy" | "doctor system" | "logs journal" => {
            LeafPlan::Exempt("inspects host services, credentials, network routes, or system logs")
        }
        "setup wizard"
        | "setup check"
        | "setup repair"
        | "host install"
        | "host update auto enable"
        | "host update auto disable"
        | "host update auto status"
        | "host service unit"
        | "host service install"
        | "host service status"
        | "host service restart"
        | "host service rollback"
        | "host service uninstall"
        | "host incus setup"
        | "host incus sync"
        | "host incus backup validate"
        | "host incus backup apply"
        | "host incus ssh bootstrap"
        | "host incus ssh verify" => {
            LeafPlan::Exempt("inspects or mutates the host installation, service manager, or Incus")
        }
        "config draft discard"
        | "config proxy set"
        | "state access migrate"
        | "state export"
        | "state verify"
        | "state restore" => LeafPlan::Exempt(
            "requires a configuration/state fixture or intentionally mutates durable state",
        ),
        "serve mcp" | "mcp" | "proxy" => {
            LeafPlan::Exempt("starts a long-running transport and requires lifecycle orchestration")
        }
        "completions refresh" | "completions clear" => {
            LeafPlan::Exempt("intentionally mutates the persistent shell completion cache")
        }
        _ => return None,
    })
}

#[test]
fn every_leaf_has_an_executable_scenario_or_a_reviewed_environment_exemption() {
    let leaves = command_inventory()
        .into_iter()
        .filter(|entry| entry.leaf)
        .collect::<Vec<_>>();
    assert!(!leaves.is_empty());
    let inventory = leaves
        .iter()
        .map(|entry| entry.canonical.join(" "))
        .collect::<std::collections::BTreeSet<_>>();
    let leaf_count = inventory.len();
    let plans = inventory
        .iter()
        .map(|path| {
            let plan = leaf_plan(path);
            assert!(
                plan.is_some(),
                "public CLI leaf has no explicit reviewed plan: {path}"
            );
            (path.clone(), plan.unwrap())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        plans
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        inventory,
        "the explicit leaf plan must exactly equal the Clap-derived leaf inventory"
    );

    let mut executed = 0;
    let mut exempt = 0;
    for (path, plan) in plans {
        let LeafPlan::Execute {
            args,
            output: expected_output,
        } = plan
        else {
            let LeafPlan::Exempt(reason) = plan else {
                unreachable!()
            };
            assert!(!reason.trim().is_empty(), "empty exemption for {path}");
            exempt += 1;
            continue;
        };
        let home = tempfile::tempdir().unwrap();
        let before = snapshot_tree(home.path());
        let output = runtime_command(home.path(), args).output().unwrap();
        assert!(
            output.status.success(),
            "offline scenario for {} failed: {}",
            path,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "offline scenario wrote stderr: {path}"
        );
        if expected_output == ScenarioOutput::Json {
            let parsed = serde_json::from_slice::<serde_json::Value>(&output.stdout);
            assert!(
                parsed.is_ok(),
                "invalid JSON from {path}: {:?}",
                parsed.err()
            );
        } else {
            assert!(!output.stdout.is_empty(), "empty output from {path}");
        }
        assert_plain_output(&output, &format!("offline leaf scenario for {path}"));
        assert_eq!(
            snapshot_tree(home.path()),
            before,
            "offline scenario for {path} wrote state"
        );
        executed += 1;
    }
    assert_eq!(
        executed, 5,
        "review executable plans when this count changes"
    );
    assert_eq!(
        exempt,
        leaf_count - executed,
        "every non-executed leaf needs a reviewed exemption"
    );
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
