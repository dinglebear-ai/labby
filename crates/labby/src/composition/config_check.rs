//! Offline validation of the configuration `labby serve` would start with.
//!
//! Runs the same config loader and the same startup validations that can make
//! `labby serve` exit or start with degraded subsystems, without starting
//! listeners or performing network I/O. Shared by `setup check` and
//! `doctor system.checks` so both report the failure class that stops startup,
//! and every Artifact source that startup would disable on all paths.

use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::config::LabConfig;
use crate::runtime_health::error_chain;

/// One failed validation, with the full error chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfigProblem {
    /// `true` when `labby serve` exits on this failure; `false` when it starts
    /// with a subsystem unavailable.
    pub(crate) fatal: bool,
    pub(crate) message: String,
}

/// Load `config.toml` from `candidates` through the serve loader and validate it.
///
/// Returns every problem found; an empty list means the config would start
/// cleanly. A load failure stops validation (there is nothing to validate).
pub(crate) fn check_config_candidates(candidates: &[PathBuf]) -> Vec<ConfigProblem> {
    match crate::config::load_toml(candidates).context("load config.toml") {
        Ok(config) => validate_startup_config(&config),
        Err(error) => vec![ConfigProblem {
            fatal: true,
            message: error_chain(error.as_ref()),
        }],
    }
}

/// Resolve the installation's `config.toml` candidates and validate them.
pub(crate) fn check_installed_config() -> (Option<PathBuf>, Vec<ConfigProblem>) {
    match crate::config::toml_candidates().context("resolve config.toml location") {
        Ok(candidates) => {
            let problems = check_config_candidates(&candidates);
            (candidates.first().cloned(), problems)
        }
        Err(error) => (
            None,
            vec![ConfigProblem {
                fatal: true,
                message: error_chain(error.as_ref()),
            }],
        ),
    }
}

/// Run the startup validations `labby serve` performs on an already-loaded config.
pub(crate) fn validate_startup_config(config: &LabConfig) -> Vec<ConfigProblem> {
    let mut problems = Vec::new();
    // Fatal at startup, in the order `labby serve` runs them. Once a fatal
    // validation fails the process exits before any degraded subsystem is
    // constructed, so later non-fatal checks would only duplicate or invent
    // diagnostics for startup work that never runs.
    let fatal = if let Err(error) = fatal_depot_checks(config) {
        problems.push(ConfigProblem {
            fatal: true,
            message: error_chain(error.as_ref()),
        });
        true
    } else {
        false
    };
    // Non-fatal: a rejected source starts serve with that source disabled on
    // every Artifact path; a construction failure starts serve with Artifact
    // services unavailable.
    #[cfg(feature = "skills")]
    if !fatal {
        problems.extend(skill_library_checks(config));
    }
    problems
}

fn fatal_depot_checks(config: &LabConfig) -> Result<()> {
    config
        .depot
        .validate_public_acquisition(&config.artifacts)
        .map_err(anyhow::Error::msg)
        .context("validate Public Depot acquisition")?;
    crate::dispatch::depot::manager::SecretSnapshot::capture(&config.depot)
        .validate_local_credentials(&config.depot)
        .map_err(anyhow::Error::msg)
        .context("validate local Depot credentials")?;
    crate::dispatch::depot::manager::host_policy(&config.depot)
        .map_err(anyhow::Error::msg)
        .context("validate Depot host policy")?;
    Ok(())
}

/// Admit `[[artifacts.sources]]` exactly as `labby serve` does and report every
/// source it would disable, then prove both Artifact adapters construct.
#[cfg(feature = "skills")]
fn skill_library_checks(config: &LabConfig) -> Vec<ConfigProblem> {
    let sources =
        crate::dispatch::artifact_sources::admit_host_sources(&config.artifacts, &config.depot);
    let mut problems = sources
        .rejected
        .iter()
        .map(|rejected| ConfigProblem {
            fatal: false,
            message: format!(
                "Artifact source `{}` would be disabled: {}",
                rejected.id, rejected.reason
            ),
        })
        .collect::<Vec<_>>();
    if let Err(error) = skill_library_adapters(&sources, config) {
        problems.push(ConfigProblem {
            fatal: false,
            message: format!(
                "Artifact services would be unavailable: {}",
                error_chain(error.as_ref())
            ),
        });
    }
    problems
}

#[cfg(feature = "skills")]
fn skill_library_adapters(
    sources: &crate::dispatch::artifact_sources::HostArtifactSources<'_>,
    config: &LabConfig,
) -> Result<()> {
    crate::dispatch::artifact_control::ArtifactControlPlane::from_admitted_sources(
        sources,
        &config.depot,
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
    .context("configure Artifact control plane")?;
    // Adapter construction creates per-source staging directories. Point it at
    // a throwaway directory so the check never writes into LABBY_HOME.
    let staging = tempfile::tempdir().context("create temporary staging directory")?;
    crate::dispatch::skill_library::import::ImportCoordinator::from_admitted_sources(
        sources,
        config,
        &staging.path().join("acquisition"),
        &|name| std::env::var_os(name),
    )
    .map(drop)
    .context("configure Skill Library exact-source adapters")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_passes() {
        assert!(validate_startup_config(&LabConfig::default()).is_empty());
    }

    #[test]
    fn missing_config_file_passes_with_defaults() {
        let directory = tempfile::tempdir().unwrap();
        assert!(check_config_candidates(&[directory.path().join("config.toml")]).is_empty());
    }

    #[test]
    fn unparseable_config_is_fatal_with_the_parse_cause() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[mcp\nport = 1").unwrap();
        let problems = check_config_candidates(&[path]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].fatal);
        assert!(
            problems[0]
                .message
                .starts_with("load config.toml: failed to parse"),
            "{}",
            problems[0].message
        );
    }

    #[test]
    fn invalid_depot_host_policy_is_fatal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[depot.private_hosts]\n\"depot.internal\" = []\n").unwrap();
        let problems = check_config_candidates(&[path]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].fatal);
        assert_eq!(
            problems[0].message,
            "validate Depot host policy: invalid Depot host policy"
        );
    }

    /// One unauthorized private pin disables that source on every path when
    /// `labby serve` starts. `setup check` and `doctor` must say so, as a
    /// non-fatal problem naming the source and the parameter, instead of
    /// reporting a clean start.
    #[cfg(feature = "skills")]
    #[test]
    fn disabled_artifact_source_is_reported_non_fatal() {
        use crate::config::{ArtifactPreferences, ArtifactSourceConfig, ArtifactSourceKind};

        let config = LabConfig {
            artifacts: ArtifactPreferences {
                sources: vec![ArtifactSourceConfig {
                    id: "depot".to_owned(),
                    kind: ArtifactSourceKind::Depot,
                    endpoint: "https://depot.example.com/api/artifacts/exact".to_owned(),
                    control_plane_url: Some("https://depot.example.com/".to_owned()),
                    pinned_addresses: vec!["10.0.0.5".parse().unwrap()],
                    bearer_token_env: None,
                }],
            },
            ..LabConfig::default()
        };
        let problems = validate_startup_config(&config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(!problems[0].fatal);
        assert!(
            problems[0].message.contains("depot"),
            "{}",
            problems[0].message
        );
        assert!(
            problems[0].message.contains("pinned_addresses"),
            "{}",
            problems[0].message
        );
    }
}
