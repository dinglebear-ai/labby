//! Secret-safe `.env` credential and raw-pair writers.

use std::path::Path;

use anyhow::Result;

use super::env_merge;

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
    let entries = pairs
        .iter()
        .map(|(key, value)| env_merge::EnvEntry::new(key.clone(), value.clone()))
        .collect();
    Ok(env_merge::merge(
        path,
        env_merge::MergeRequest {
            entries,
            force,
            expected_mtime: None,
        },
    )?
    .skipped)
}
