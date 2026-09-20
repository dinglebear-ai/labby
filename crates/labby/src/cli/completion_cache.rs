//! Bounded, authority-keyed completion snapshots. Queries perform local reads only.

use crate::{config::LabConfig, dispatch::error::ToolError};
use clap::CommandFactory;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_BYTES: usize = 256 * 1024;
const MAX_NAMES: usize = 2048;
const TTL_SECONDS: u64 = 15 * 60;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    authority: String,
    created_at: u64,
    names: BTreeMap<String, Vec<String>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/@".contains(&byte))
}
fn cache_path(authority: &str) -> anyhow::Result<PathBuf> {
    if authority.len() != 64 || !authority.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(crate::config::cli::invalid("Invalid completion-cache authority key.").into());
    }
    Ok(crate::installation::InstallationPaths::resolve()?
        .root()
        .join("completions")
        .join(format!("{authority}.json")))
}

fn read_at(path: &Path, authority: &str, clock: u64) -> Option<Snapshot> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return None;
    }
    let raw = crate::config::host_write::read_config_snapshot(path).ok()?;
    if raw.len() > MAX_BYTES {
        return None;
    }
    let snapshot: Snapshot = serde_json::from_str(&raw).ok()?;
    if snapshot.version != 1
        || snapshot.authority != authority
        || snapshot.created_at > clock
        || clock.saturating_sub(snapshot.created_at) > TTL_SECONDS
        || snapshot.names.len() > 4
        || snapshot
            .names
            .values()
            .any(|names| names.len() > MAX_NAMES || names.iter().any(|name| !safe_name(name)))
    {
        return None;
    }
    Some(snapshot)
}

#[cfg(feature = "gateway")]
fn names_from(value: &Value, resource: &str) -> Result<Vec<String>, ToolError> {
    let value = if resource == "snippet" {
        &value["snippets"]
    } else {
        value
    };
    let rows=value.as_array().ok_or_else(||ToolError::Sdk{sdk_kind:"decode_error".into(),message:format!("The {resource} catalog did not return a list. Its previous completion entries will not be reused.")})?;
    if rows.len() > MAX_NAMES {
        return Err(ToolError::Sdk {
            sdk_kind: "content_too_large".into(),
            message: format!("The {resource} catalog exceeds the 2048-name completion limit."),
        });
    }
    let mut names = BTreeSet::new();
    for row in rows {
        let name = if resource == "server" {
            row["config"]["name"].as_str()
        } else {
            row["name"].as_str()
        }
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "decode_error".into(),
            message: format!("The {resource} catalog contains an entry without a name."),
        })?;
        if safe_name(name) {
            names.insert(name.to_owned());
        }
    }
    Ok(names.into_iter().collect())
}

/// A refresh is explicit. A shell Tab key must never call this function.
#[cfg(feature = "gateway")]
pub async fn refresh(config: &LabConfig, team: Option<&str>) -> anyhow::Result<(Value, bool)> {
    let (server,authority)=crate::live_gateway::completion_authority(config,team)?
        .ok_or_else(||crate::config::cli::invalid("Cached remote completions require an explicit --server, --context, selected context, or server URL environment setting. No discovery was started."))?;
    let live = crate::live_gateway::detect(config, "cli")
        .await?
        .ok_or_else(|| {
            crate::config::cli::invalid("The selected completion gateway is unavailable.")
        })?
        .with_team_id(team.map(str::to_owned));
    if live.server_url() != server.as_str() {
        return Err(crate::config::cli::invalid(
            "Completion authority changed during discovery; no cache was written.",
        )
        .into());
    }
    let mut names = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for (resource, action) in [
        ("server", "gateway.list"),
        ("route", "gateway.protected_route.list_state"),
        ("loadout", "gateway.loadout.list"),
    ] {
        let result = live
            .dispatch_action(action, serde_json::json!({}))
            .await
            .and_then(|value| names_from(&value, resource));
        match result {
            Ok(values) => {
                names.insert(resource.to_owned(), values);
            }
            Err(error) => {
                errors.insert(resource.to_owned(),serde_json::json!({"kind":error.kind(),"message":labby_runtime::agent_error::sanitize_error_text(error.user_message(),1024)}));
            }
        }
    }
    // Snippet names belong to this local installation, never the remote gateway.
    match crate::dispatch::snippets::dispatch("snippets.list", serde_json::json!({}))
        .await
        .and_then(|value| names_from(&value, "snippet"))
    {
        Ok(values) => {
            names.insert("snippet".into(), values);
        }
        Err(error) => {
            errors.insert("snippet".into(),serde_json::json!({"kind":error.kind(),"message":"Local snippet names could not be loaded; old entries were discarded."}));
        }
    }
    let current = crate::live_gateway::completion_authority(config, team)?;
    if current.as_ref().map(|(_, key)| key) != Some(&authority) {
        return Err(crate::config::cli::invalid("Credential identity changed while refreshing completions. No cache was published; run refresh again with the intended identity.").into());
    }
    let counts = names
        .iter()
        .map(|(resource, values)| (resource.clone(), values.len()))
        .collect::<BTreeMap<_, _>>();
    let snapshot = Snapshot {
        version: 1,
        authority: authority.clone(),
        created_at: now(),
        names,
    };
    let body = serde_json::to_string(&snapshot)?;
    if body.len() > MAX_BYTES {
        return Err(crate::config::cli::invalid(
            "Completion snapshot exceeds 256 KiB. No cache was published.",
        )
        .into());
    }
    let path = cache_path(&authority)?;
    tokio::task::spawn_blocking(move || {
        let lock = crate::config::host_write::HostConfigLock::acquire(&path)?;
        lock.write(&body)
    })
    .await??;
    let success = errors.is_empty();
    Ok((
        serde_json::json!({"ok":success,"server":server.as_str(),"team_id":team,"expires_in_seconds":TTL_SECONDS,"counts":counts,"errors":errors}),
        success,
    ))
}

#[cfg(feature = "gateway")]
pub fn clear(config: &LabConfig, team: Option<&str>) -> anyhow::Result<Value> {
    let (_, authority) =
        crate::live_gateway::completion_authority(config, team)?.ok_or_else(|| {
            crate::config::cli::invalid(
                "Select the authority whose completion cache should be cleared.",
            )
        })?;
    let path = cache_path(&authority)?;
    if !path.try_exists()? {
        return Ok(serde_json::json!({"changed":false}));
    }
    let _lock = crate::config::host_write::HostConfigLock::acquire(&path)?;
    let changed = match std::fs::remove_file(&path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    Ok(serde_json::json!({"changed":changed}))
}

fn metadata_config(matches: &clap::ArgMatches) -> anyhow::Result<(LabConfig, Option<String>)> {
    let path = crate::installation::InstallationPaths::resolve()?.config_toml();
    let prefs = crate::config::cli::read(&path)?;
    // Reads the same local environment files as normal CLI dispatch. This does
    // not write files or contact services, and no value enters completion output.
    crate::config::load_dotenv()?;
    let environment_selects_target = ["CLAUDE_PLUGIN_OPTION_SERVER_URL", "LABBY_SERVER_URL"]
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()));
    let selected = crate::config::cli::select(
        &prefs,
        matches.get_one::<String>("server").map(String::as_str),
        matches.get_one::<String>("context").map(String::as_str),
        environment_selects_target,
    )?;
    let team = matches
        .get_one::<String>("team_id")
        .cloned()
        .or_else(|| selected.as_ref().and_then(|target| target.team_id.clone()));
    Ok((
        LabConfig {
            cli: prefs,
            cli_target: selected,
            ..Default::default()
        },
        team,
    ))
}

fn resource_for_path(path: &[String]) -> Option<&'static str> {
    let action = path.last()?.as_str();
    if ![
        "get", "set", "replace", "test", "restart", "enable", "disable", "remove", "run",
        "validate", "login", "status", "logout",
    ]
    .contains(&action)
    {
        return None;
    }
    match path.first()?.as_str() {
        "server" => Some("server"),
        "route" => Some("route"),
        "loadout" => Some("loadout"),
        "snippet" => Some("snippet"),
        _ => None,
    }
}

/// Derive static suggestions from Clap, optionally extending with a valid local snapshot.
/// Malformed/missing/stale caches yield no resource names, never a network request.
pub fn query(words: &[String]) -> Vec<String> {
    if words.len() > 128 || words.iter().map(String::len).sum::<usize>() > 32 * 1024 {
        return vec![];
    }
    let (prefix, before) = words
        .split_last()
        .map_or(("", &[][..]), |(last, before)| (last.as_str(), before));
    if prefix.bytes().any(|byte| byte.is_ascii_control()) || before.iter().any(|word| word == "--")
    {
        return vec![];
    }
    // Set partial parsing before building: Clap propagates global settings to
    // descendants during build. Setting it on an already-built root loses
    // nested commands when their required resource operand is not typed yet.
    let mut root = super::Cli::command()
        .color(clap::ColorChoice::Never)
        .ignore_errors(true);
    root.build();
    let Ok(matches) = root
        .clone()
        .try_get_matches_from(std::iter::once("labby").chain(before.iter().map(String::as_str)))
    else {
        return vec![];
    };
    let mut command = &root;
    let mut node = &matches;
    let mut path = Vec::new();
    while let Some((name, child)) = node.subcommand() {
        let Some(next) = command
            .get_subcommands()
            .find(|candidate| candidate.get_name() == name)
        else {
            break;
        };
        if next.is_hide_set() {
            return vec![];
        }
        path.push(name.to_owned());
        command = next;
        node = child;
    }
    let previous_arg = before
        .last()
        .and_then(|previous| {
            command.get_arguments().find(|arg| {
                arg.get_long()
                    .is_some_and(|name| previous == &format!("--{name}"))
                    || arg
                        .get_short()
                        .is_some_and(|name| previous == &format!("-{name}"))
            })
        })
        .filter(|arg| arg.get_action().takes_values());
    let mut candidates = BTreeSet::new();
    if let Some(arg) = previous_arg {
        if arg.get_id().as_str() == "context" {
            if let Ok((config, _)) = metadata_config(&matches) {
                candidates.extend(config.cli.contexts.into_keys());
            }
        } else if let Some(values) = arg.get_value_parser().possible_values() {
            candidates.extend(
                values
                    .filter(|value| !value.is_hide_set())
                    .map(|value| value.get_name().to_owned()),
            );
        }
    } else if prefix.starts_with('-') {
        for arg in command.get_arguments().filter(|arg| !arg.is_hide_set()) {
            if let Some(long) = arg.get_long() {
                candidates.insert(format!("--{long}"));
            }
            if let Some(short) = arg.get_short() {
                candidates.insert(format!("-{short}"));
            }
        }
    } else {
        candidates.extend(
            command
                .get_subcommands()
                .filter(|command| !command.is_hide_set())
                .map(|command| command.get_name().to_owned()),
        );
        if path.first().is_some_and(|part| part == "context") {
            if let Ok((config, _)) = metadata_config(&matches) {
                candidates.extend(config.cli.contexts.into_keys());
            }
        }
        #[cfg(feature = "gateway")]
        if let Some(resource) = resource_for_path(&path)
            && command.get_positionals().any(|arg| {
                matches!(arg.get_id().as_str(), "name" | "names")
                    && (!node.contains_id(arg.get_id().as_str())
                        || arg
                            .get_num_args()
                            .is_some_and(|arity| arity.max_values() > 1))
            })
        {
            let names = (|| -> Option<Vec<String>> {
                let (config, team) = metadata_config(&matches).ok()?;
                let (_, authority) =
                    crate::live_gateway::completion_authority(&config, team.as_deref()).ok()??;
                let snapshot = read_at(&cache_path(&authority).ok()?, &authority, now())?;
                snapshot.names.get(resource).cloned()
            })()
            .unwrap_or_default();
            candidates.extend(names.into_iter().filter(|name| !before.contains(name)));
        }
    }
    candidates
        .into_iter()
        .filter(|candidate| candidate.starts_with(prefix))
        .take(100)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_expired_wrong_identity_and_future_caches_are_not_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let mut snapshot = Snapshot {
            version: 1,
            authority: "a".repeat(64),
            created_at: 100,
            names: BTreeMap::from([("server".into(), vec!["alpha".into()])]),
        };
        let write = |snapshot: &Snapshot| {
            std::fs::write(&path, serde_json::to_vec(snapshot).unwrap()).unwrap()
        };
        write(&snapshot);
        assert_eq!(
            read_at(&path, &snapshot.authority, 101).unwrap().names["server"],
            vec!["alpha"]
        );
        assert!(read_at(&path, &"b".repeat(64), 101).is_none());
        assert!(read_at(&path, &snapshot.authority, 99).is_none());
        assert!(read_at(&path, &snapshot.authority, 100 + TTL_SECONDS + 1).is_none());
        snapshot
            .names
            .get_mut("server")
            .unwrap()
            .push("$(execute)".into());
        write(&snapshot);
        assert!(read_at(&path, &snapshot.authority, 101).is_none());
        std::fs::write(&path, "broken json").unwrap();
        assert!(read_at(&path, &snapshot.authority, 101).is_none());
    }
    #[test]
    fn static_completion_uses_command_graph_and_never_guesses_past_argument_boundaries() {
        assert!(query(&["ho".into()]).contains(&"host".into()));
        assert!(query(&["host".into(), "ser".into()]).contains(&"service".into()));
        assert!(query(&["--".into(), "ho".into()]).is_empty());
        assert!(query(&["host".into(), "--color".into(), "p".into()]).contains(&"plain".into()));
    }
}
