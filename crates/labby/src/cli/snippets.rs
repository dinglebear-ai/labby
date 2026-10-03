use std::cell::Cell;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::cli::helpers::{print_dry_run, run_action_command, run_confirmable_action_command};
use crate::config::LabConfig;
use crate::output::OutputFormat;

#[derive(Debug, Args)]
pub struct SnippetsArgs {
    #[command(subcommand)]
    pub command: SnippetsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SnippetsCommand {
    /// List built-in and user snippets.
    List,
    /// Show one snippet body and metadata.
    Get(SnippetNameArgs),
    /// Execute a snippet through gateway Code Mode.
    #[command(name = "run")]
    Exec(SnippetExecArgs),
    /// Add a user snippet. Existing snippets require explicit --force replacement.
    #[command(name = "add")]
    Create(SnippetCreateArgs),
    /// Validate a snippet without saving or executing it.
    Validate(SnippetValidateArgs),
    /// Remove a user snippet.
    Remove(SnippetRemoveArgs),
    /// Test with deterministic fixtures; use --live to contact upstreams.
    Test(SnippetTestArgs),
    /// Generate an editable fixture from tool schemas without executing tools.
    Fixture(SnippetFixtureArgs),
}

#[derive(Debug, Args)]
pub struct SnippetFixtureArgs {
    pub name: String,
    /// Exact tool IDs for legacy snippets without declarations.
    #[arg(long = "tool")]
    pub tools: Vec<String>,
    /// Compare current contracts with a saved fixture; never execute tools.
    #[arg(long, conflicts_with_all = ["output", "results"])]
    pub check: Option<PathBuf>,
    /// Saved schema map keyed by upstream::tool; uses no gateway when supplied.
    #[arg(long)]
    pub schemas: Option<PathBuf>,
    /// Synthetic result overrides keyed by upstream::tool.
    #[arg(long)]
    pub results: Option<PathBuf>,
    /// Populate optional fields and one array item, or generate minimal values.
    #[arg(long, default_value = "populated", value_parser = ["populated", "minimal"])]
    pub variant: String,
    /// Save the generated fixture to a new file; existing files are never overwritten.
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SnippetNameArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct SnippetExecArgs {
    pub name: String,
    /// Input values passed to the snippet as key=value pairs.
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SnippetTestArgs {
    pub name: Option<String>,
    /// Test every listed snippet using its sibling .test.json fixture.
    #[arg(long, conflicts_with = "name", default_value_t = false)]
    pub all: bool,
    /// Contact real upstreams instead of using fixtures.
    #[arg(long, conflicts_with = "fixture", default_value_t = false)]
    pub live: bool,
    /// Read a deterministic JSON fixture instead of the sibling .test.json file.
    #[arg(long, conflicts_with_all = ["live", "all"])]
    pub fixture: Option<PathBuf>,
    /// Input values passed to the snippet as key=value pairs.
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SnippetCreateArgs {
    pub name: String,
    /// Read snippet body from a file.
    #[arg(long, conflicts_with = "code")]
    pub file: Option<PathBuf>,
    /// Inline snippet body.
    #[arg(long, conflicts_with = "file")]
    pub code: Option<String>,
    /// Human-readable snippet description for generated frontmatter.
    #[arg(long)]
    pub description: Option<String>,
    /// Overwrite an existing user snippet.
    #[arg(long, short = 'f', default_value_t = false)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct SnippetValidateArgs {
    /// Existing snippet name or filename stem for --file/--code validation.
    pub name: String,
    /// Validate snippet body from a file instead of an existing snippet.
    #[arg(long, conflicts_with = "code")]
    pub file: Option<PathBuf>,
    /// Validate inline snippet body instead of an existing snippet.
    #[arg(long, conflicts_with = "file")]
    pub code: Option<String>,
}

#[derive(Debug, Args)]
pub struct SnippetRemoveArgs {
    pub name: String,
    /// Confirm removal without prompting.
    #[arg(short = 'y', long, default_value_t = false)]
    pub yes: bool,
    /// Alias for --yes.
    #[arg(long = "no-confirm", default_value_t = false)]
    pub no_confirm: bool,
    /// Show what would be removed without deleting it.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
}

pub async fn run(
    args: SnippetsArgs,
    format: OutputFormat,
    config: &LabConfig,
    team_id: Option<&str>,
) -> Result<ExitCode> {
    if let SnippetsCommand::Fixture(args) = args.command {
        return generate_fixture(args, format, config, team_id).await;
    }
    let needs_upstreams = matches!(&args.command, SnippetsCommand::Exec(_))
        || matches!(&args.command, SnippetsCommand::Test(test) if test.live);
    if needs_upstreams {
        if let Some(live) = crate::live_gateway::detect(config, "cli").await? {
            validate_remote_team_selection(team_id)?;
            return run_on_selected_daemon(args.command, live, format).await;
        }
        crate::cli::gateway::build_manager(config, true).await?;
    }

    let (action, params, yes, dry_run) = match args.command {
        SnippetsCommand::Fixture(_) => unreachable!("fixture generation handled above"),
        SnippetsCommand::List => ("snippets.list".to_string(), json!({}), true, false),
        SnippetsCommand::Get(args) => (
            "snippets.get".to_string(),
            json!({ "name": args.name }),
            true,
            false,
        ),
        SnippetsCommand::Exec(args) => (
            "snippets.exec".to_string(),
            json!({
                "name": args.name,
                "params": crate::cli::params::parse_kv_params(args.params)?,
            }),
            true,
            false,
        ),
        SnippetsCommand::Create(args) => (
            "snippets.create".to_string(),
            json!({
                "name": args.name,
                "body": read_snippet_body(args.code, args.file)?,
                "description": args.description,
                "force": args.force,
            }),
            true,
            false,
        ),
        SnippetsCommand::Validate(args) => {
            let body = match (args.code, args.file) {
                (Some(code), None) => Some(code),
                (None, Some(path)) => Some(std::fs::read_to_string(path)?),
                (None, None) => None,
                (Some(_), Some(_)) => unreachable!("clap enforces conflicts_with"),
            };
            (
                "snippets.validate".to_string(),
                json!({
                    "name": args.name,
                    "body": body,
                }),
                true,
                false,
            )
        }
        SnippetsCommand::Remove(args) => (
            "snippets.remove".to_string(),
            json!({ "name": args.name }),
            args.yes || args.no_confirm,
            args.dry_run,
        ),
        SnippetsCommand::Test(args) => (
            "snippets.test".to_string(),
            json!({
                "name": args.name,
                "all": args.all,
                "live": args.live,
                "fixture": read_fixture(args.fixture)?,
                "params": crate::cli::params::parse_kv_params(args.params)?,
            }),
            true,
            false,
        ),
    };

    if dry_run {
        print_dry_run("snippets", &action, &params, format)?;
        return Ok(ExitCode::SUCCESS);
    }

    if action == "snippets.remove" {
        return run_confirmable_action_command(
            "snippets",
            crate::dispatch::snippets::ACTIONS,
            action,
            params,
            yes,
            format,
            |action, params| async move {
                crate::dispatch::snippets::dispatch(&action, params).await
            },
        )
        .await;
    }

    let test_failed = Cell::new(false);
    let failed_flag = &test_failed;
    let source_limit = config.code_mode.max_source_bytes;
    let exit = run_action_command(
        "snippets",
        action,
        params,
        format,
        |action, params| async move {
            let report = if action == "snippets.test" {
                crate::dispatch::snippets::dispatch::dispatch_with_source_limit(
                    &action,
                    params,
                    source_limit,
                )
                .await?
            } else {
                crate::dispatch::snippets::dispatch(&action, params).await?
            };
            if action == "snippets.test" {
                failed_flag.set(report["passed"] != true);
            }
            Ok(report)
        },
    )
    .await?;
    Ok(if test_failed.get() {
        ExitCode::FAILURE
    } else {
        exit
    })
}

async fn generate_fixture(
    args: SnippetFixtureArgs,
    format: OutputFormat,
    config: &LabConfig,
    team_id: Option<&str>,
) -> Result<ExitCode> {
    let saved = args.schemas.is_some();
    let schemas = read_fixture(args.schemas)?;
    let results = read_fixture(args.results)?.unwrap_or_else(|| json!({}));
    let params = json!({"name":args.name,"params":crate::cli::params::parse_kv_params(args.params)?,
        "schemas":schemas,"results":results,"variant":args.variant,
        "tools":if args.tools.is_empty() {None} else {Some(args.tools)}, "check":read_fixture(args.check)?});
    let report = if !saved {
        if let Some(live) = crate::live_gateway::detect(config, "cli").await? {
            validate_remote_team_selection(team_id)?;
            crate::dispatch::snippets::generate_remote_fixture(&live, params).await?
        } else {
            crate::cli::gateway::build_manager(config, true).await?;
            crate::dispatch::snippets::dispatch("snippets.fixture", params).await?
        }
    } else {
        crate::dispatch::snippets::dispatch("snippets.fixture", params).await?
    };
    if report["ready"] == true
        && let Some(path) = args.output
    {
        crate::dispatch::snippets::write_fixture_output(&path, &report["fixture"])?;
    }
    crate::output::print(&report, format)?;
    Ok(remote_test_exit_code(report["ready"] == true))
}

fn read_fixture(path: Option<PathBuf>) -> Result<Option<Value>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let cap = labby_codemode::snippet::harness::MAX_FIXTURE_BYTES;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take((cap + 1) as u64)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= cap, "fixture exceeds 512 KiB");
    Ok(Some(serde_json::from_slice(&bytes)?))
}

async fn run_on_selected_daemon(
    command: SnippetsCommand,
    live: crate::live_gateway::LiveGateway,
    format: OutputFormat,
) -> Result<ExitCode> {
    match command {
        SnippetsCommand::Exec(args) => {
            let params = crate::cli::params::parse_kv_params(args.params)?;
            let response = execute_remote_snippet(&live, &args.name, params).await?;
            crate::output::print(&response, format)?;
            Ok(ExitCode::SUCCESS)
        }
        SnippetsCommand::Test(args) => {
            let params = crate::cli::params::parse_kv_params(args.params)?;
            if args.all {
                let names = remote_bulk_snippet_names(
                    &crate::dispatch::helpers::lab_home(),
                    &crate::dispatch::snippets::store::builtin_snippet_dir(),
                )?;
                let mut results = Vec::with_capacity(names.len());
                for name in names {
                    let result = test_remote_snippet(&live, &name, params.clone()).await;
                    let mut report = match result {
                        Ok(value) => value,
                        Err(error) => {
                            json!({"name": name, "passed": false, "error": error.to_string()})
                        }
                    };
                    compact_bulk_report(&mut report);
                    results.push(report);
                }
                let passed =
                    !results.is_empty() && results.iter().all(|value| value["passed"] == true);
                crate::output::print(&json!({"passed": passed, "results": results}), format)?;
                Ok(remote_test_exit_code(passed))
            } else {
                let name = args
                    .name
                    .ok_or_else(|| anyhow::anyhow!("provide a snippet name or --all"))?;
                let result = test_remote_snippet(&live, &name, params).await?;
                crate::output::print(&result, format)?;
                Ok(remote_test_exit_code(result["passed"] == true))
            }
        }
        _ => unreachable!("only executable snippet commands select a daemon"),
    }
}

fn remote_bulk_snippet_names(
    lab_home: &std::path::Path,
    builtin_dir: &std::path::Path,
) -> Result<std::collections::BTreeSet<String>> {
    let names: std::collections::BTreeSet<_> =
        crate::dispatch::snippets::store::list_snippets(lab_home, builtin_dir)?
            .into_iter()
            .map(|snippet| snippet.name)
            .collect();
    anyhow::ensure!(
        names.len() <= 100,
        "test --all is bounded to 100 snippets; test named subsets instead"
    );
    Ok(names)
}

async fn test_remote_snippet(
    live: &crate::live_gateway::LiveGateway,
    name: &str,
    params: Value,
) -> Result<Value> {
    let response = execute_remote_snippet(live, name, params).await?;
    let passed = remote_snippet_passed(&response);
    Ok(json!({"name": name, "passed": passed, "response": response}))
}

fn remote_snippet_passed(response: &Value) -> bool {
    let calls_passed = response["calls"]
        .as_array()
        .is_some_and(|calls| calls.iter().all(|call| call["ok"] == true));
    let result_passed = response["result"]
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let shaped_or_truncated = response["result_shaping"]["changed"] == true
        || response["result_shaping"]["truncated"] == true
        || response["result"].get("truncated") == Some(&Value::Bool(true));
    calls_passed && result_passed && response.get("result").is_some() && !shaped_or_truncated
}

fn remote_test_exit_code(passed: bool) -> ExitCode {
    if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn compact_bulk_report(report: &mut Value) {
    let diagnostics = if report["passed"] == false {
        report.get("response").map(|response| {
            json!({
                "failed_calls": response["calls"].as_array().map_or(0, |calls| calls.iter().filter(|call| call["ok"] != true).count()),
                "missing_result": response.get("result").is_none(),
                "result_failed": response["result"]["ok"] == false,
                "result_shaped": response["result_shaping"]["changed"] == true
                    || response["result_shaping"]["truncated"] == true
                    || response["result"]["truncated"] == true,
            })
        })
    } else {
        None
    };
    if let Some(object) = report.as_object_mut() {
        object.remove("result");
        object.remove("response");
        object.remove("calls");
        if let Some(diagnostics) = diagnostics {
            object.insert("diagnostics".into(), diagnostics);
        }
    }
}

fn validate_remote_team_selection(team_id: Option<&str>) -> Result<()> {
    anyhow::ensure!(
        team_id.is_none(),
        "remote snippet execution cannot apply --team-id to the daemon's MCP Code Mode route; use a Team-bound gateway endpoint"
    );
    Ok(())
}

#[cfg(test)]
mod remote_snippet_tests {
    use super::*;

    #[test]
    fn bulk_tests_deduplicate_shadowed_names_and_bound_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let builtin = dir.path().join("builtin");
        let home = dir.path().join("home");
        let user = labby_codemode::snippet::store::user_snippet_dir(&home);
        std::fs::create_dir_all(&builtin).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        for index in 0..100 {
            let filename = format!("snippet-{index:03}.js");
            std::fs::write(builtin.join(&filename), "async () => 'builtin'").unwrap();
            std::fs::write(user.join(filename), "async () => 'override'").unwrap();
        }
        let names = remote_bulk_snippet_names(&home, &builtin).unwrap();
        assert_eq!(names.len(), 100);
        assert_eq!(
            names.iter().filter(|name| *name == "snippet-000").count(),
            1
        );
        let resolved =
            labby_codemode::snippet::store::resolve_snippet(&home, &builtin, "snippet-000")
                .unwrap();
        assert_eq!(resolved.path, user.join("snippet-000.js"));
        std::fs::write(user.join("extra.js"), "async () => true").unwrap();
        assert!(remote_bulk_snippet_names(&home, &builtin).is_err());
    }

    #[test]
    fn live_verdict_requires_a_result_successful_calls_and_complete_output() {
        assert!(remote_snippet_passed(&json!({"result": null, "calls": []})));
        for response in [
            json!({"calls": []}),
            json!({"result": {"ok": false}, "calls": []}),
            json!({"result": true, "calls": [{"ok": false}]}),
            json!({"result": true, "calls": [], "result_shaping": {"truncated": true}}),
            json!({"result": true, "calls": [], "result_shaping": {"changed": true}}),
            json!({"result": {"truncated": true}, "calls": []}),
        ] {
            assert!(!remote_snippet_passed(&response), "{response}");
        }
    }

    #[test]
    fn failed_live_verdict_sets_failure_exit_status() {
        assert_eq!(remote_test_exit_code(false), ExitCode::FAILURE);
        assert_eq!(remote_test_exit_code(true), ExitCode::SUCCESS);
    }

    #[test]
    fn selected_team_is_rejected_before_remote_execution() {
        assert!(validate_remote_team_selection(Some("team-alpha")).is_err());
        assert!(validate_remote_team_selection(None).is_ok());
    }

    #[test]
    fn bulk_reports_keep_verdicts_without_large_payloads() {
        let mut report = json!({
            "name": "demo", "passed": false,
            "response": {"result": [1, 2, 3], "calls": [{"ok": false}]},
            "error": "failed"
        });
        compact_bulk_report(&mut report);
        assert_eq!(
            report,
            json!({
                "name": "demo", "passed": false, "error": "failed",
                "diagnostics": {"failed_calls": 1, "missing_result": false,
                    "result_failed": false, "result_shaped": false}
            })
        );
    }
}

async fn execute_remote_snippet(
    live: &crate::live_gateway::LiveGateway,
    name: &str,
    params: Value,
) -> Result<Value> {
    use crate::dispatch::snippets::store::{
        builtin_snippet_dir, code_for_snippet, merge_snippet_input, resolve_snippet,
    };

    let snippet = resolve_snippet(
        &crate::dispatch::helpers::lab_home(),
        &builtin_snippet_dir(),
        name,
    )?;
    let input = merge_snippet_input(&snippet, params)?;
    let code = code_for_snippet(&snippet)?;
    let code = crate::dispatch::snippets::store::wrap_snippet_with_input_bounded(
        &code,
        &input,
        labby_codemode::MAX_SOURCE_BYTES,
    )?;
    if snippet
        .tools
        .as_ref()
        .is_some_and(|tools| tools.as_slice().is_empty())
    {
        anyhow::bail!(
            "snippet `{name}` declares no upstream tools; remote execution cannot preserve that restriction"
        );
    }
    Ok(live
        .call_codemode_tool_scoped(&code, snippet.tools.as_ref().map(|tools| tools.as_slice()))
        .await?)
}

fn read_snippet_body(code: Option<String>, file: Option<PathBuf>) -> Result<String> {
    match (code, file) {
        (Some(code), None) => Ok(code),
        (None, Some(path)) => Ok(std::fs::read_to_string(path)?),
        _ => anyhow::bail!("provide exactly one of --code or --file"),
    }
}
