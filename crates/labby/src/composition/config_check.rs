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
    /// Stable operator-facing code shared with Doctor capability findings.
    pub(crate) code: &'static str,
    /// `true` when `labby serve` exits on this failure; `false` when it starts
    /// with a subsystem unavailable or a security guard silently narrows access.
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
            code: "config_invalid",
            fatal: true,
            message: error_chain(error.as_ref()),
        }],
    }
}

/// Resolve the installation's `config.toml` candidates and validate them.
pub(crate) fn check_installed_config() -> (Option<PathBuf>, Vec<ConfigProblem>) {
    match crate::config::toml_candidates().context("resolve config.toml location") {
        Ok(candidates) => {
            let mut problems = check_config_candidates(&candidates);
            problems.extend(check_runtime_guard_candidates(&candidates, None));
            (candidates.first().cloned(), problems)
        }
        Err(error) => (
            None,
            vec![ConfigProblem {
                code: "config_invalid",
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
            code: "config_invalid",
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

/// Validate runtime security/exposure guards that can make a healthy-looking
/// daemon reject requests or ignore configured capabilities while it keeps
/// serving.
pub(crate) fn check_runtime_guard_candidates(
    candidates: &[PathBuf],
    bind_host_override: Option<&str>,
) -> Vec<ConfigProblem> {
    let Ok(config) = crate::config::load_toml(candidates) else {
        // The ordinary config check owns load/parse failures so callers do not
        // receive the same problem twice.
        return Vec::new();
    };
    runtime_guard_problems(&config, bind_host_override)
}

/// Validate process-resolved guard inputs for an already-loaded config.
pub(crate) fn runtime_guard_problems(
    config: &LabConfig,
    bind_host_override: Option<&str>,
) -> Vec<ConfigProblem> {
    let bind_host = bind_host_override
        .map(str::to_owned)
        .or_else(|| std::env::var("LABBY_MCP_HTTP_HOST").ok())
        .or_else(|| config.mcp.host.clone())
        .unwrap_or_else(|| "127.0.0.1".to_owned());
    let env_allowed_hosts = std::env::var("LABBY_MCP_ALLOWED_HOSTS").ok();
    let public_urls = config.public_urls();
    let host_validation_disabled =
        std::env::var("LABBY_HOST_VALIDATION_DISABLED").as_deref() == Ok("1");
    validate_runtime_guards(
        config,
        &bind_host,
        env_allowed_hosts.as_deref(),
        public_urls.app.as_deref(),
        public_urls.mcp_gateway.as_deref(),
        host_validation_disabled,
        cfg!(feature = "gateway"),
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_runtime_guards(
    config: &LabConfig,
    bind_host: &str,
    env_allowed_hosts: Option<&str>,
    public_app_url: Option<&str>,
    public_mcp_url: Option<&str>,
    host_validation_disabled: bool,
    gateway_enabled: bool,
) -> Vec<ConfigProblem> {
    let mut problems = Vec::new();
    let config_hosts = config.mcp.allowed_hosts.as_deref().unwrap_or(&[]);
    let env_hosts = env_allowed_hosts
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    if config_hosts.iter().any(|host| host.trim() == "*")
        || env_hosts.iter().any(|host| *host == "*")
    {
        problems.push(ConfigProblem {
            code: "host_allowlist_wildcard_ignored",
            fatal: false,
            message: "MCP allowed hosts contains `*`, but Labby intentionally ignores wildcards to preserve DNS-rebinding protection. Requests relying on that wildcard will be rejected. Replace `*` with the exact public hostname/IP in mcp.allowed_hosts or LABBY_MCP_ALLOWED_HOSTS.".into(),
        });
    }

    if host_validation_disabled {
        problems.push(ConfigProblem {
            code: "host_validation_disabled",
            fatal: false,
            message: "LABBY_HOST_VALIDATION_DISABLED=1 disables DNS-rebinding protection. This is a test-only escape hatch; unset it for a normal deployment.".into(),
        });
    }

    let has_explicit_remote_host = config_hosts
        .iter()
        .map(String::as_str)
        .chain(env_hosts.iter().copied())
        .any(|host| host != "*" && host_is_non_loopback(host))
        || [public_app_url, public_mcp_url]
            .into_iter()
            .flatten()
            .filter_map(|value| url::Url::parse(value).ok())
            .filter_map(|url| url.host_str().map(str::to_owned))
            .any(|host| host_is_non_loopback(&host));
    if host_is_non_loopback(bind_host) && !has_explicit_remote_host {
        problems.push(ConfigProblem {
            code: "remote_host_not_allowed",
            fatal: false,
            message: format!(
                "Labby binds HTTP on `{bind_host}`, but no non-loopback public/allowed Host is configured. DNS-rebinding protection will reject requests made through a LAN, Tailscale, or reverse-proxy hostname even though the listener is reachable. Configure LABBY_PUBLIC_URL, LABBY_MCP_GATEWAY_URL, mcp.allowed_hosts, or LABBY_MCP_ALLOWED_HOSTS with the exact authority clients use."
            ),
        });
    }

    if !gateway_enabled && !config.protected_mcp_routes.is_empty() {
        problems.push(ConfigProblem {
            code: "gateway_feature_required",
            fatal: true,
            message: "protected MCP routes are configured, but this Labby build does not include the gateway feature. Install a gateway-enabled build before starting this configuration.".into(),
        });
    }
    if !gateway_enabled && !config.upstream.is_empty() {
        problems.push(ConfigProblem {
            code: "gateway_feature_unavailable",
            fatal: false,
            message: format!(
                "{} gateway upstream(s) are configured, but this Labby build does not include the gateway feature. Those upstreams will be ignored. Install a gateway-enabled build or remove the unused upstream configuration.",
                config.upstream.len()
            ),
        });
    }

    problems
}

fn host_is_non_loopback(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return false;
    }
    let host = if let Some(rest) = trimmed.strip_prefix('[') {
        rest.split_once(']').map(|(host, _)| host).unwrap_or(rest)
    } else if let Some((host, port)) = trimmed.rsplit_once(':') {
        if !host.contains(':') && port.parse::<u16>().is_ok() {
            host
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    !matches!(
        host.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "127.0.0.1" | "::1" | "localhost"
    )
}
fn fatal_depot_checks(config: &LabConfig) -> Result<()> {
    config
        .depot
        .validate_public_acquisition_with_env(&config.artifacts, &|name| std::env::var_os(name))
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
    let sources = crate::dispatch::artifact_sources::admit_host_sources(
        &config.artifacts,
        &config.depot,
        &|name| std::env::var_os(name),
    );
    let mut problems = sources
        .rejected
        .iter()
        .map(|rejected| ConfigProblem {
            code: "artifact_source_disabled",
            fatal: false,
            message: format!(
                "Artifact source `{}` would be disabled: {}. Fix or remove that source configuration before relying on it.",
                rejected.id, rejected.reason
            ),
        })
        .collect::<Vec<_>>();
    if let Err(error) = skill_library_adapters(&sources, config) {
        problems.push(ConfigProblem {
            code: "artifacts_unavailable",
            fatal: false,
            message: format!(
                "Artifact services would be unavailable: {}. Fix the Artifact/Depot source configuration, then restart Labby.",
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
    fn runtime_guard_reports_ignored_wildcard_and_unusable_remote_bind() {
        let config = LabConfig::default();
        let problems =
            validate_runtime_guards(&config, "0.0.0.0", Some("*"), None, None, false, true);
        let codes = problems
            .iter()
            .map(|problem| problem.code)
            .collect::<Vec<_>>();
        assert!(codes.contains(&"host_allowlist_wildcard_ignored"));
        assert!(codes.contains(&"remote_host_not_allowed"));
        assert!(problems.iter().all(|problem| !problem.fatal));
    }

    #[test]
    fn runtime_guard_accepts_an_exact_remote_host() {
        let config = LabConfig::default();
        let problems = validate_runtime_guards(
            &config,
            "0.0.0.0",
            Some("labby.example.test"),
            None,
            None,
            false,
            true,
        );
        assert!(
            problems
                .iter()
                .all(|problem| problem.code != "remote_host_not_allowed"),
            "{problems:?}"
        );
    }

    #[test]
    fn runtime_guard_surfaces_test_only_host_validation_bypass() {
        let config = LabConfig::default();
        let problems = validate_runtime_guards(&config, "127.0.0.1", None, None, None, true, true);
        assert!(
            problems
                .iter()
                .any(|problem| problem.code == "host_validation_disabled")
        );
    }

    #[test]
    fn runtime_guard_reports_upstreams_ignored_by_a_no_gateway_build() {
        let config = toml::from_str::<LabConfig>(
            r#"
[[upstream]]
name = "demo"
url = "http://127.0.0.1:9000/mcp"
"#,
        )
        .expect("upstream parses");
        let problems =
            validate_runtime_guards(&config, "127.0.0.1", None, None, None, false, false);
        let problem = problems
            .iter()
            .find(|problem| problem.code == "gateway_feature_unavailable")
            .expect("missing gateway capability must be surfaced");
        assert!(!problem.fatal);
        assert!(problem.message.contains("1 gateway upstream"));
    }

    #[test]
    fn runtime_guard_fails_closed_for_protected_routes_without_gateway_support() {
        let config = toml::from_str::<LabConfig>(
            r#"
[[protected_mcp_routes]]
name = "tools"
enabled = true
public_host = "mcp.example.com"
public_path = "/tools"
backend_url = "http://127.0.0.1:3100/mcp"
"#,
        )
        .expect("protected route parses");
        let problems =
            validate_runtime_guards(&config, "127.0.0.1", None, None, None, false, false);
        let problem = problems
            .iter()
            .find(|problem| problem.code == "gateway_feature_required")
            .expect("unsupported protected route must be surfaced");
        assert!(problem.fatal);
    }

    #[test]
    fn host_classification_handles_ports_and_ipv6_loopback() {
        for host in [
            "localhost",
            "localhost:8765",
            "127.0.0.1:8765",
            "[::1]:8765",
        ] {
            assert!(!host_is_non_loopback(host), "{host}");
        }
        for host in ["0.0.0.0", "::", "100.64.0.10", "labby.example.test:443"] {
            assert!(host_is_non_loopback(host), "{host}");
        }
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
