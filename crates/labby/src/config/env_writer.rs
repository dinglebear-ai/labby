//! Secret-safe `.env` credential and raw-pair writers.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use super::env_merge;
use super::secret_files::{open_secret_file, restrict_secret_file_permissions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvCredential {
    pub service: String,
    pub url: Option<String>,
    pub secret: Option<String>,
    pub env_field: String,
}

pub fn write_service_creds(
    path: &Path,
    creds: &[EnvCredential],
    force: bool,
) -> Result<env_merge::MergeOutcome, env_merge::MergeError> {
    let mut entries = Vec::new();
    for credential in creds {
        let service = credential.service.to_uppercase();
        if let Some(url) = &credential.url {
            entries.push(env_merge::EnvEntry::new(
                format!("{service}_URL"),
                url.clone(),
            ));
        }
        if let Some(secret) = &credential.secret {
            entries.push(env_merge::EnvEntry::new(
                credential.env_field.clone(),
                secret.clone(),
            ));
        }
    }
    env_merge::merge(
        path,
        env_merge::MergeRequest {
            entries,
            force,
            expected_mtime: None,
        },
    )
}

pub fn write_env_pairs(
    path: &Path,
    pairs: &[(String, String)],
    force: bool,
) -> Result<Vec<String>> {
    let raw = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };
    let lines: Vec<&str> = raw.lines().collect();
    let existing: HashMap<String, String> = lines
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            line.split_once('=')
                .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    let mut conflicts = Vec::new();
    let mut overrides = HashMap::new();
    let mut additions = Vec::new();
    for (key, value) in pairs {
        match existing.get(key) {
            None => additions.push((key.clone(), value.clone())),
            Some(current) if current == value => {}
            Some(_) if force => {
                overrides.insert(key.clone(), value.clone());
            }
            Some(current) => conflicts.push(format!(
                "CONFLICT: {key} already set to {current:?}; skipping (use --force to overwrite)"
            )),
        }
    }
    let mut output = Vec::new();
    for line in lines {
        let trimmed = line.trim();
        let replacement = if trimmed.is_empty() || trimmed.starts_with('#') {
            None
        } else {
            trimmed.split_once('=').and_then(|(key, _)| {
                overrides
                    .get(key.trim())
                    .map(|value| format!("{}={}", key.trim(), quote_env_value(value)))
            })
        };
        output.push(replacement.unwrap_or_else(|| line.to_owned()));
    }
    if !additions.is_empty() {
        if !output.last().is_none_or(|line| line.trim().is_empty()) {
            output.push(String::new());
        }
        output.extend(
            additions
                .iter()
                .map(|(key, value)| format!("{key}={}", quote_env_value(value))),
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let mut file = open_secret_file(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        for line in output {
            writeln!(file, "{line}").with_context(|| format!("write {}", temporary.display()))?;
        }
        file.sync_all()
            .with_context(|| format!("sync {}", temporary.display()))?;
    }
    std::fs::rename(&temporary, path)
        .with_context(|| format!("rename {} -> {}", temporary.display(), path.display()))?;
    restrict_secret_file_permissions(path).with_context(|| format!("harden {}", path.display()))?;
    Ok(conflicts)
}

fn quote_env_value(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ' ' | '\t' | '#' | '$' | '\\' | '"' | '\'' | '`'))
    {
        format!("\"{}\"", value.replace('\\', r"\\").replace('"', r#"\""#))
    } else {
        value.to_owned()
    }
}
