//! Credential-free bootstrap configuration for the desktop shell.

use std::{fs, io};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::DEFAULT_CONTROL_PLANE_URL;

const SETTINGS_FILE: &str = "settings.json";
const LEGACY_APP_IDENTIFIER: &str = "tv.tootie.lab.palette";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedBootstrapSettings {
    control_plane_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MigratedBootstrapSettings<'a> {
    control_plane_url: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BootstrapSettings {
    pub(crate) control_plane_url: String,
}

pub(crate) fn value_for(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub(crate) fn load_bootstrap_settings(app: &AppHandle) -> Result<BootstrapSettings, String> {
    let path = app
        .path()
        .app_config_dir()
        .map_err(|error| format!("failed to resolve app config directory: {error}"))?
        .join(SETTINGS_FILE);
    let persisted = read_bootstrap_settings(&path)?;
    let migrated = if persisted.is_none() {
        legacy_settings_path(&path)
            .and_then(|legacy_path| fs::read_to_string(legacy_path).ok())
            .and_then(|contents| legacy_origin(&contents))
    } else {
        None
    };
    if let Some(origin) = migrated.as_deref() {
        persist_migrated_origin(&path, origin)?;
    }
    let control_plane_url = persisted
        .or(migrated)
        .or_else(|| value_for("LABBY_CONTROL_PLANE_URL"))
        .or_else(|| value_for("LABBY_API_URL"))
        .unwrap_or_else(|| DEFAULT_CONTROL_PLANE_URL.to_owned());
    Ok(BootstrapSettings { control_plane_url })
}

fn legacy_origin(contents: &str) -> Option<String> {
    parse_bootstrap_settings(contents, "legacy settings")
        .ok()
        .flatten()
        .and_then(|origin| crate::validate_control_plane_origin(&origin).ok())
}

fn read_bootstrap_settings(path: &std::path::Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => parse_bootstrap_settings(&contents, &path.display().to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to read desktop settings at {}: {error}",
            path.display()
        )),
    }
}

fn legacy_settings_path(current: &std::path::Path) -> Option<std::path::PathBuf> {
    current
        .parent()?
        .parent()
        .map(|root| root.join(LEGACY_APP_IDENTIFIER).join(SETTINGS_FILE))
}

fn persist_migrated_origin(path: &std::path::Path, origin: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "desktop settings path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create desktop settings directory {}: {error}",
            parent.display()
        )
    })?;
    let contents = serde_json::to_vec_pretty(&MigratedBootstrapSettings {
        control_plane_url: origin,
    })
    .map_err(|error| format!("failed to encode migrated desktop settings: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents).map_err(|error| {
        format!(
            "failed to write migrated desktop settings at {}: {error}",
            temporary.display()
        )
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        format!(
            "failed to install migrated desktop settings at {}: {error}",
            path.display()
        )
    })
}

fn parse_bootstrap_settings(contents: &str, source: &str) -> Result<Option<String>, String> {
    let settings: PersistedBootstrapSettings = serde_json::from_str(contents)
        .map_err(|error| format!("failed to parse desktop settings at {source}: {error}"))?;
    Ok(settings
        .control_plane_url
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_file_migrates_only_the_control_plane_origin() {
        let json = r#"{
          "serverUrl": "https://api.example.com",
          "controlPlaneUrl": "https://control.example.com",
          "staticToken": "must-not-load",
          "projectId": "must-not-load",
          "shortcut": "Alt+Space",
          "theme": "dark"
        }"#;
        assert_eq!(
            parse_bootstrap_settings(json, "fixture")
                .unwrap()
                .as_deref(),
            Some("https://control.example.com")
        );
    }

    #[test]
    fn legacy_api_origin_is_not_a_control_plane_fallback() {
        let json = r#"{"serverUrl":"https://legacy.example","staticToken":"ignored"}"#;
        assert_eq!(
            parse_bootstrap_settings(json, "fixture")
                .unwrap()
                .as_deref(),
            None
        );
    }

    #[test]
    fn legacy_bundle_path_is_a_sibling_of_the_new_bundle() {
        let current = std::path::Path::new("/config/tv.tootie.labby.desktop/settings.json");
        assert_eq!(
            legacy_settings_path(current).unwrap(),
            std::path::Path::new("/config/tv.tootie.lab.palette/settings.json")
        );
    }

    #[test]
    fn invalid_legacy_settings_are_not_migrated() {
        for contents in [
            "invalid json",
            r#"{"controlPlaneUrl":false}"#,
            r#"{"controlPlaneUrl":""}"#,
            r#"{"controlPlaneUrl":"http://remote.example"}"#,
            r#"{"controlPlaneUrl":"file:///private/config"}"#,
            r#"{"serverUrl":"https://api.example"}"#,
        ] {
            assert_eq!(legacy_origin(contents), None);
        }
        assert_eq!(
            legacy_origin(
                r#"{"controlPlaneUrl":"https://control.example/","staticToken":"ignored"}"#
            ),
            Some("https://control.example".to_owned())
        );
    }

    #[test]
    fn migrated_file_contains_only_the_control_plane_origin() {
        let directory =
            std::env::temp_dir().join(format!("labby-desktop-migration-{}", std::process::id()));
        let path = directory.join("settings.json");
        persist_migrated_origin(&path, "https://control.example.com").unwrap();
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"controlPlaneUrl": "https://control.example.com"})
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
