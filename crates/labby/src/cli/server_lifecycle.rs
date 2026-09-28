//! Bounded lifecycle orchestration over a single authoritative gateway client.
//! Mutations and authorization remain in shared gateway dispatch.

use crate::{
    config::LabConfig,
    dispatch::error::ToolError,
    live_gateway::LiveGateway,
    output::{OutputFormat, print},
};
use clap::Args;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    process::ExitCode,
    time::{Duration, Instant},
};

const MAX_TARGETS: usize = 128;

#[derive(Debug, Args)]
pub struct Targets {
    /// Explicit upstream names. Omitting names never means all servers.
    #[arg(num_args = 1.., required_unless_present = "all", conflicts_with = "all")]
    pub names: Vec<String>,
    /// Select all visible servers; restart selects enabled servers only.
    #[arg(long)]
    pub all: bool,
    /// Inspect targets and show the proposed operation without changing them.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct ToggleArgs {
    #[command(flatten)]
    pub targets: Targets,
    /// Clean up old processes when disabling. Not valid when enabling.
    #[arg(long)]
    pub cleanup: bool,
    /// Use broader process cleanup matching; requires --cleanup.
    #[arg(long, requires = "cleanup")]
    pub aggressive: bool,
}

#[derive(Debug, Args)]
pub struct RestartArgs {
    #[command(flatten)]
    pub targets: Targets,
    /// Bound completion per server (1ms to 5m), for example 30s.
    #[arg(long, default_value = "30s", value_parser = completion_timeout)]
    pub timeout: u64,
    /// Return an accepted result without waiting for restart completion.
    #[arg(long)]
    pub no_wait: bool,
    /// Use broader host-wide matching when cleaning up the old runtime.
    #[arg(long)]
    pub aggressive: bool,
}

fn completion_timeout(raw: &str) -> Result<u64, String> {
    let millis = super::duration::milliseconds(raw)?;
    if millis > 300_000 {
        return Err("restart timeout must not exceed 5m".into());
    }
    Ok(millis)
}

#[derive(Debug)]
pub enum Request {
    Enable(ToggleArgs),
    Disable(ToggleArgs),
    Restart(RestartArgs),
}

impl Request {
    fn action(&self) -> &'static str {
        match self {
            Self::Enable(_) => "gateway.mcp.enable",
            Self::Disable(_) => "gateway.mcp.disable",
            Self::Restart(_) => "gateway.mcp.restart",
        }
    }
    fn targets(&self) -> &Targets {
        match self {
            Self::Enable(args) | Self::Disable(args) => &args.targets,
            Self::Restart(args) => &args.targets,
        }
    }
    fn params(&self, name: &str) -> Value {
        match self {
            Self::Enable(_) => json!({"name":name,"origin":"cli"}),
            Self::Disable(args) => {
                json!({"name":name,"origin":"cli","cleanup":args.cleanup,"aggressive":args.aggressive})
            }
            Self::Restart(args) => {
                json!({"name":name,"origin":"cli","aggressive":args.aggressive,"wait_ms":if args.no_wait { 0 } else { args.timeout }})
            }
        }
    }
    fn no_wait(&self) -> bool {
        matches!(self, Self::Restart(args) if args.no_wait)
    }
}

fn select_names(request: &Request, snapshot: &Value) -> Result<Vec<String>, ToolError> {
    let rows = snapshot.as_array().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "decode_error".into(),
        message:
            "Gateway returned an invalid server inventory. No lifecycle operation was dispatched."
                .into(),
    })?;
    let mut visible = BTreeMap::new();
    for row in rows {
        let name = row["name"].as_str().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "decode_error".into(),
            message:
                "Gateway server inventory is missing a name. No lifecycle operation was dispatched."
                    .into(),
        })?;
        let enabled = row["enabled"].as_bool().ok_or_else(|| ToolError::Sdk { sdk_kind:"decode_error".into(),message:"Gateway server inventory is missing enabled state. No lifecycle operation was dispatched.".into() })?;
        if visible.insert(name.to_owned(), enabled).is_some() {
            return Err(crate::config::cli::invalid(
                "Gateway returned duplicate server names. No lifecycle operation was dispatched.",
            ));
        }
    }
    let targets = request.targets();
    let restarting = matches!(request, Request::Restart(_));
    let names = if targets.all {
        visible
            .iter()
            .filter(|(_, enabled)| !restarting || **enabled)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    } else {
        let unique = targets.names.iter().collect::<BTreeSet<_>>();
        if unique.len() != targets.names.len() {
            return Err(crate::config::cli::invalid(
                "Duplicate lifecycle target names are not allowed. Each server is changed at most once.",
            ));
        }
        for name in &targets.names {
            let enabled = visible.get(name).ok_or_else(|| crate::config::cli::invalid(format!("Server `{name}` is not visible on the selected gateway. No lifecycle operation was dispatched.")))?;
            if restarting && !enabled {
                return Err(crate::config::cli::invalid(format!(
                    "Server `{name}` is disabled. Enable it explicitly before restarting. No lifecycle operation was dispatched."
                )));
            }
        }
        targets.names.clone()
    };
    if names.is_empty() {
        return Err(crate::config::cli::invalid(
            "No servers match the explicit lifecycle selection. No operation was dispatched.",
        ));
    }
    if names.len() > MAX_TARGETS {
        return Err(crate::config::cli::invalid(
            "At most 128 servers may be selected. Split the explicit target list into smaller batches.",
        ));
    }
    Ok(names)
}

fn completion(request: &Request, value: &Value) -> Result<&'static str, ToolError> {
    let (gateway, expected_enabled) = match request {
        Request::Enable(_) => (value, true),
        Request::Disable(_) => (&value["gateway"], false),
        Request::Restart(_) => {
            match value["completed"].as_bool() {
                Some(false) if request.no_wait() => return Ok("accepted"),
                Some(false) => return Err(ToolError::Sdk { sdk_kind:"timeout".into(), message:"The restart is still running on the selected gateway. It was not replayed. Inspect server status and gateway logs before retrying.".into() }),
                Some(true) => {},
                None => return Err(ToolError::Sdk { sdk_kind:"decode_error".into(),message:"Gateway restart response omitted its completion state. Effects may have occurred; inspect server status before retrying.".into() }),
            }
            if value["gateway"]["runtime"]["connected"].as_bool() != Some(true) {
                return Err(ToolError::Sdk { sdk_kind:"upstream_error".into(),message:"The restart completed but the replacement connection is not healthy. Inspect server get NAME and server test NAME; no second restart was attempted.".into() });
            }
            (&value["gateway"], true)
        }
    };
    if gateway["config"]["enabled"].as_bool() != Some(expected_enabled) {
        return Err(ToolError::Sdk { sdk_kind:"unexpected_response".into(),message:"Gateway did not confirm the requested enabled state. Inspect server status before retrying; no operation was replayed.".into() });
    }
    Ok("completed")
}

fn failure(error: &ToolError, action: &str) -> Value {
    let extra = super::helpers::diagnostic_value(&error.extra_fields(), 8192);
    labby_runtime::agent_error::build_agent_error_value(
        error.kind(),
        error.user_message(),
        Some(&extra),
        &labby_runtime::agent_error::AgentErrorContext {
            service: Some("gateway".into()),
            action: Some(action.to_owned()),
            ..Default::default()
        },
    )
}

/// Resolve the authority exactly once; never fail over or retry a mutation.
pub async fn run(
    request: Request,
    config: &LabConfig,
    team_id: Option<&str>,
    format: OutputFormat,
) -> anyhow::Result<ExitCode> {
    if matches!(&request, Request::Enable(args) if args.cleanup || args.aggressive) {
        return Err(crate::config::cli::invalid("Cleanup is a disable operation; server enable does not accept --cleanup or --aggressive.").into());
    }
    let live = crate::live_gateway::detect(config,"cli").await?.ok_or_else(|| ToolError::Sdk { sdk_kind:"daemon_unavailable".into(),message:"No authoritative gateway is reachable. No lifecycle operation was dispatched; local state was not used as a fallback.".into() })?
        .with_team_id(team_id.map(str::to_owned));
    let live = match &request {
        Request::Restart(args) => live.with_dispatch_timeout(
            Duration::from_millis(args.timeout).saturating_add(Duration::from_secs(5)),
        ),
        _ => live,
    };
    run_selected(request, &live, format).await
}

async fn run_selected(
    request: Request,
    live: &LiveGateway,
    format: OutputFormat,
) -> anyhow::Result<ExitCode> {
    let action = request.action();
    let inventory = live.dispatch_action("gateway.mcp.list", json!({})).await?;
    let names = select_names(&request, &inventory)?;
    if request.targets().dry_run {
        super::helpers::print_dry_run(
            "gateway",
            action,
            &json!({"server":live.server_url(),"team_id":live.team_id(),"targets":names}),
            format,
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    let request_id = super::helpers::REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| ulid::Ulid::new().to_string());
    let mut results = Vec::new();
    let mut failed = 0usize;
    for name in &names {
        let start = Instant::now();
        // Some shared mutations deliberately outlive their HTTP caller. Bound
        // the CLI wait independently and never interpret a lost response as a
        // safe reason to repeat a mutation.
        let deadline = match &request {
            Request::Restart(args) if args.no_wait => Duration::from_secs(5),
            Request::Restart(args) => {
                Duration::from_millis(args.timeout).saturating_add(Duration::from_secs(5))
            }
            _ => Duration::from_secs(30),
        };
        let response = tokio::time::timeout(deadline,live.dispatch_action(action, request.params(name))).await
            .unwrap_or_else(|_|Err(ToolError::Sdk { sdk_kind:"timeout".into(),message:"The lifecycle response deadline expired. The gateway may still be completing the operation. It was not replayed; inspect server status and correlated gateway logs before retrying.".into() }));
        let result = match response {
            Ok(value) => match completion(&request, &value) {
                Ok(status) => json!({"name":name,"status":status,"result":value}),
                Err(error) => {
                    failed += 1;
                    json!({"name":name,"status":"failed","error":failure(&error,action),"result":value})
                }
            },
            Err(error) => {
                failed += 1;
                json!({"name":name,"status":"failed","error":failure(&error,action)})
            }
        };
        let status = result["status"].as_str().unwrap_or("failed");
        // tracing serializes u128 as text; keep elapsed_ms numeric for log queries.
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        if status == "failed" {
            tracing::warn!(target: "labby::cli::audit", surface="cli",service="gateway",action,upstream=%name,request_id=%request_id,elapsed_ms,status,
                kind=result["error"]["kind"].as_str().unwrap_or("unknown"),
                origin=result["error"]["origin"].as_str().unwrap_or("unknown"),
                side_effects=result["error"]["side_effects"].as_str().unwrap_or("unknown"),
                "server lifecycle outcome");
        } else {
            tracing::info!(target: "labby::cli::audit", surface="cli",service="gateway",action,upstream=%name,request_id=%request_id,elapsed_ms,status,"server lifecycle outcome");
        }
        results.push(super::helpers::diagnostic_value(&result, 64 * 1024));
    }
    print(
        &json!({"ok":failed==0,"request_id":request_id,"server":live.server_url(),"team_id":live.team_id(),"action":action,"selected":names.len(),"failed":failed,"results":results}),
        format,
    )?;
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn restart(no_wait: bool) -> Request {
        Request::Restart(RestartArgs {
            targets: Targets {
                names: vec!["one".into()],
                all: false,
                dry_run: false,
            },
            timeout: 1000,
            no_wait,
            aggressive: false,
        })
    }
    #[test]
    fn accepted_is_not_completed_without_explicit_nonwaiting_mode() {
        assert!(completion(&restart(false), &json!({"completed":false})).is_err());
        assert_eq!(
            completion(&restart(true), &json!({"completed":false})).unwrap(),
            "accepted"
        );
        assert!(completion(&restart(true), &json!({})).is_err());
    }
    #[test]
    fn completed_but_disconnected_restart_is_failure() {
        let mut result = json!({"completed":true,"gateway":{"config":{"enabled":true},"runtime":{"connected":false}}});
        assert!(completion(&restart(false), &result).is_err());
        result["gateway"]["runtime"]["connected"] = json!(true);
        assert_eq!(completion(&restart(false), &result).unwrap(), "completed");
    }
    #[test]
    fn selection_rejects_missing_disabled_duplicate_and_excessive_targets() {
        let snapshot = json!([{"name":"one","enabled":true},{"name":"two","enabled":false}]);
        assert_eq!(
            select_names(&restart(false), &snapshot).unwrap(),
            vec!["one"]
        );
        let mut request = restart(false);
        if let Request::Restart(args) = &mut request {
            args.targets.names = vec!["two".into()];
        }
        assert!(select_names(&request, &snapshot).is_err());
        if let Request::Restart(args) = &mut request {
            args.targets.names = vec!["one".into(), "one".into()];
        }
        assert!(select_names(&request, &snapshot).is_err());
        if let Request::Restart(args) = &mut request {
            args.targets.names.clear();
            args.targets.all = true;
        }
        assert_eq!(select_names(&request, &snapshot).unwrap(), vec!["one"]);
        assert!(select_names(&request, &json!({})).is_err());
        let too_many = Value::Array(
            (0..129)
                .map(|i| json!({"name":format!("n{i}"),"enabled":true}))
                .collect(),
        );
        assert!(select_names(&request, &too_many).is_err());
    }
}
