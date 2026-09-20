//! Non-secret CLI connection preferences stored in the canonical host config.

use super::host_write::{HostConfigLock, HostWriteError};
use crate::dispatch::error::ToolError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};
use url::{Host, Url};

const MAX_CONTEXTS: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CliPreferences {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_context: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub contexts: BTreeMap<String, ConnectionContext>,
}

/// A destination and optional authority selector, never credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionContext {
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
}

/// Invocation-only selection. Never serialized back into configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedTarget {
    pub server: String,
    pub team_id: Option<String>,
    pub source: TargetSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetSource {
    Argument,
    Context,
}

pub fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".into(),
        message: message.into(),
    }
}

pub fn normalize_server(raw: &str) -> Result<String, ToolError> {
    if raw.len() > 4096 {
        return Err(invalid("Labby server URL exceeds 4096 bytes."));
    }
    let mut url = Url::parse(raw.trim()).map_err(|_| {
        invalid("Labby server URL is invalid. Use an HTTPS URL or loopback HTTP URL.")
    })?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "Labby server URLs must not contain credentials, query strings, or fragments. Use auth login for credentials.",
        ));
    }
    let loopback = match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.host().is_none() || !(url.scheme() == "https" || url.scheme() == "http" && loopback) {
        return Err(invalid(
            "Labby server URL must use HTTPS; plaintext HTTP is allowed only for loopback.",
        ));
    }
    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix("/mcp").unwrap_or(path);
    let path = if path.is_empty() {
        "/".to_owned()
    } else {
        format!("{path}/")
    };
    url.set_path(&path);
    Ok(url.to_string())
}

pub fn validate_name(name: &str) -> Result<(), ToolError> {
    if name.is_empty()
        || name.len() > 64
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return Err(invalid(
            "Context names must contain 1 to 64 ASCII letters, digits, dots, underscores, or hyphens; . and .. are not names.",
        ));
    }
    Ok(())
}

impl ConnectionContext {
    pub fn new(server: &str, team_id: Option<String>) -> Result<Self, ToolError> {
        if team_id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 512
                || !id.is_ascii()
                || id.bytes().any(|c| c.is_ascii_control())
        }) {
            return Err(invalid(
                "Context Team ID must contain 1 to 512 ASCII bytes without control characters.",
            ));
        }
        Ok(Self {
            server: normalize_server(server)?,
            team_id,
        })
    }
}

impl CliPreferences {
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.contexts.len() > MAX_CONTEXTS {
            return Err(invalid("At most 128 connection contexts are supported."));
        }
        for (name, entry) in &self.contexts {
            validate_name(name)?;
            ConnectionContext::new(&entry.server, entry.team_id.clone())?;
        }
        if let Some(name) = &self.current_context {
            validate_name(name)?;
            if !self.contexts.contains_key(name) {
                return Err(invalid(
                    "The selected context does not exist. Select an existing context with context use, or run context clear.",
                ));
            }
        }
        Ok(())
    }
}

/// Resolve without I/O or environment mutation. An invocation selection wins;
/// environment targets win over the persisted convenience selection.
pub fn select(
    preferences: &CliPreferences,
    server: Option<&str>,
    context: Option<&str>,
    environment_selects_target: bool,
) -> Result<Option<SelectedTarget>, ToolError> {
    if server.is_some() && context.is_some() {
        return Err(invalid("Use either --server or --context, not both."));
    }
    if let Some(server) = server {
        return Ok(Some(SelectedTarget {
            server: normalize_server(server)?,
            team_id: None,
            source: TargetSource::Argument,
        }));
    }
    let context = context.or_else(|| {
        (!environment_selects_target)
            .then_some(preferences.current_context.as_deref())
            .flatten()
    });
    let Some(name) = context else {
        return Ok(None);
    };
    validate_name(name)?;
    let entry = preferences.contexts.get(name).ok_or_else(|| invalid(format!("Context `{name}` does not exist. Use labby context list or labby context add NAME --server URL.")))?;
    let entry = ConnectionContext::new(&entry.server, entry.team_id.clone())?;
    Ok(Some(SelectedTarget {
        server: entry.server,
        team_id: entry.team_id,
        source: TargetSource::Context,
    }))
}

#[derive(Debug)]
pub enum Change {
    Add {
        name: String,
        entry: ConnectionContext,
        select: bool,
    },
    Set {
        name: String,
        server: Option<String>,
        team_id: Option<String>,
        clear_team: bool,
    },
    Use(String),
    Remove(String),
    Clear,
}

fn write_error(error: HostWriteError) -> ToolError {
    let kind = if matches!(error, HostWriteError::Busy) {
        "conflict"
    } else {
        "internal_error"
    };
    ToolError::Sdk {
        sdk_kind: kind.into(),
        message: format!(
            "Cannot update CLI connection preferences: {error}. No other configuration source was used; inspect the host config before retrying."
        ),
    }
}

fn preferences_from(raw: &str) -> Result<CliPreferences, ToolError> {
    #[derive(Deserialize, Default)]
    struct Envelope {
        #[serde(default)]
        cli: CliPreferences,
    }
    let parsed: Envelope = toml::from_str(raw).map_err(|_| invalid("Cannot read CLI preferences from config.toml. Repair the TOML document; no alternate configuration was used."))?;
    Ok(parsed.cli)
}

/// Metadata-only read. Does not create locks, directories, or migrate config.
pub fn read(path: &Path) -> Result<CliPreferences, ToolError> {
    let raw = super::host_write::read_config_snapshot(path).map_err(write_error)?;
    preferences_from(&raw)
}

/// Serialize read/modify/write with the existing host lock and atomic writer.
/// Only the [cli] table is changed; unrelated comments and tables are preserved.
pub fn change(path: &Path, change: Change) -> Result<(CliPreferences, bool), ToolError> {
    match &change {
        Change::Add { name, entry, .. } => {
            validate_name(name)?;
            ConnectionContext::new(&entry.server, entry.team_id.clone())?;
        }
        Change::Set {
            name,
            server,
            team_id,
            clear_team,
        } => {
            validate_name(name)?;
            if let Some(server) = server {
                normalize_server(server)?;
            }
            if *clear_team && team_id.is_some() {
                return Err(invalid("--clear-team conflicts with --team-id."));
            }
        }
        Change::Use(name) | Change::Remove(name) => validate_name(name)?,
        Change::Clear => {}
    }
    let lock = HostConfigLock::acquire(path).map_err(write_error)?;
    let raw = lock.read_raw().map_err(write_error)?;
    let mut preferences = preferences_from(&raw)?;
    let previous = preferences.clone();
    match change {
        Change::Add {
            name,
            entry,
            select,
        } => {
            if preferences.contexts.contains_key(&name) {
                return Err(ToolError::Sdk {
                    sdk_kind: "conflict".into(),
                    message: format!(
                        "Context `{name}` already exists. Use context set to change it explicitly."
                    ),
                });
            }
            if select {
                preferences.current_context = Some(name.clone());
            }
            preferences.contexts.insert(name, entry);
        }
        Change::Set {
            name,
            server,
            team_id,
            clear_team,
        } => {
            let entry = preferences
                .contexts
                .get_mut(&name)
                .ok_or_else(|| invalid("Context does not exist. Use context add first."))?;
            if let Some(server) = server {
                entry.server = normalize_server(&server)?;
            }
            if clear_team {
                entry.team_id = None;
            } else if team_id.is_some() {
                entry.team_id = team_id;
            }
        }
        Change::Use(name) => {
            if !preferences.contexts.contains_key(&name) {
                return Err(invalid(
                    "Context does not exist. Use context list to select an existing name.",
                ));
            }
            preferences.current_context = Some(name);
        }
        Change::Remove(name) => {
            if preferences.current_context.as_ref() == Some(&name) {
                return Err(invalid(
                    "This is the active context. Run context clear or select another context before removing it.",
                ));
            }
            if preferences.contexts.remove(&name).is_none() {
                return Err(invalid(
                    "Context does not exist. Use context list to inspect saved names.",
                ));
            }
        }
        Change::Clear => preferences.current_context = None,
    }
    preferences.validate()?;
    let changed = preferences != previous;
    if changed {
        let mut document: toml_edit::DocumentMut = raw
            .parse()
            .map_err(|_| invalid("Host config is not valid TOML; no changes were written."))?;
        let serialized = toml::to_string(&preferences)
            .map_err(|_| ToolError::internal_message("Cannot serialize CLI preferences"))?;
        let table: toml_edit::DocumentMut = serialized
            .parse()
            .map_err(|_| ToolError::internal_message("Cannot construct CLI preferences table"))?;
        document["cli"] = toml_edit::Item::Table(table.as_table().clone());
        lock.write(&document.to_string()).map_err(write_error)?;
    }
    Ok((preferences, changed))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_context_overrides_environment_but_default_context_does_not() {
        let mut prefs = CliPreferences::default();
        prefs.contexts.insert(
            "one".into(),
            ConnectionContext::new("https://one.example", Some("one-team".into())).unwrap(),
        );
        prefs.current_context = Some("one".into());
        assert!(select(&prefs, None, None, true).unwrap().is_none());
        assert_eq!(
            select(&prefs, None, Some("one"), true)
                .unwrap()
                .unwrap()
                .team_id
                .as_deref(),
            Some("one-team")
        );
        let direct = select(&prefs, Some("https://two.example"), None, false)
            .unwrap()
            .unwrap();
        assert_eq!(direct.server, "https://two.example/");
        assert!(direct.team_id.is_none());
        assert!(select(&prefs, Some("https://two.example"), Some("one"), false).is_err());
    }
    #[test]
    fn repeated_selection_does_not_rewrite_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        change(
            &path,
            Change::Add {
                name: "one".into(),
                entry: ConnectionContext::new("https://one.example", None).unwrap(),
                select: true,
            },
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let (_, changed) = change(&path, Change::Use("one".into())).unwrap();
        assert!(!changed);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[test]
    fn read_of_missing_configuration_is_nonmutating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent/config.toml");
        assert_eq!(read(&path).unwrap(), CliPreferences::default());
        assert!(!path.parent().unwrap().exists());
    }
}
