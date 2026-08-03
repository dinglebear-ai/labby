//! Read-only preflight for persisted ephemeral stdio-proxy preferences.

use std::ffi::OsStr;
#[cfg(feature = "gateway")]
use std::path::PathBuf;

use crate::proxy::config::{ProxyAuthMode, ProxyExposure, ProxyPreferences};
#[cfg(feature = "gateway")]
use crate::proxy::tailscale::{ServeStatus, TailscaleStatus};

use super::types::{Finding, Report, Severity};

fn finding(check: &str, severity: Severity, message: impl Into<String>) -> Finding {
    Finding {
        service: "proxy".to_string(),
        check: check.to_string(),
        severity,
        message: message.into(),
    }
}

#[cfg(feature = "gateway")]
fn dependency_failure(check: &str, dependency: &str) -> Finding {
    finding(
        check,
        Severity::Fail,
        format!("check unavailable because {dependency} failed"),
    )
}

#[cfg(not(feature = "gateway"))]
fn gateway_feature_unavailable(check: &str) -> Finding {
    finding(
        check,
        Severity::Warn,
        "gateway feature is not compiled into this labby build",
    )
}

pub async fn check_proxy_preflight() -> Report {
    let config = match crate::config::load() {
        Ok(config) => config,
        Err(error) => {
            return Report {
                findings: vec![finding(
                    "proxy:config",
                    Severity::Fail,
                    format!("persisted proxy configuration is invalid: {error:#}"),
                )],
            };
        }
    };
    let preferences = &config.proxy;
    let mut findings = vec![config_finding(preferences)];
    findings.extend(launcher_findings());
    findings.extend(auth_findings(&config, preferences).await);
    if matches!(preferences.exposure, ProxyExposure::Local) {
        findings.push(finding(
            "proxy:tailscale-skipped",
            Severity::Ok,
            "local exposure does not require Tailscale",
        ));
    } else {
        #[cfg(feature = "gateway")]
        findings.extend(tailscale_findings().await);
        #[cfg(not(feature = "gateway"))]
        findings.push(gateway_feature_unavailable("proxy:tailscale-version"));
    }
    Report { findings }
}

fn config_finding(preferences: &ProxyPreferences) -> Finding {
    match preferences.validate() {
        Ok(()) => finding(
            "proxy:config",
            Severity::Ok,
            format!(
                "persisted proxy path and port selection are valid ({}, {}..={})",
                preferences.path, preferences.port_range_start, preferences.port_range_end
            ),
        ),
        Err(error) => finding(
            "proxy:config",
            Severity::Fail,
            format!("persisted proxy configuration is invalid: {error}"),
        ),
    }
}

fn launcher_findings() -> Vec<Finding> {
    let path = std::env::var_os("PATH");
    [
        ("proxy:launcher-node", "node"),
        ("proxy:launcher-python3", "python3"),
    ]
    .into_iter()
    .map(|(check, launcher)| {
        let available =
            crate::proxy::command::executable_on_path(OsStr::new(launcher), path.as_deref())
                .is_some();
        finding(
            check,
            if available {
                Severity::Ok
            } else {
                Severity::Fail
            },
            if available {
                format!("`{launcher}` launcher is available")
            } else {
                format!("`{launcher}` launcher is not available on PATH")
            },
        )
    })
    .collect()
}

async fn auth_findings(
    _config: &crate::config::LabConfig,
    preferences: &ProxyPreferences,
) -> Vec<Finding> {
    match preferences.auth {
        ProxyAuthMode::None => vec![finding(
            "proxy:auth-none",
            Severity::Warn,
            "proxy authentication is disabled",
        )],
        ProxyAuthMode::Bearer => {
            let present = std::env::var_os(&preferences.bearer_token_env)
                .is_some_and(|value| !value.is_empty());
            vec![finding(
                "proxy:bearer-secret",
                if present {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                if present {
                    format!(
                        "bearer secret is present in `{}`",
                        preferences.bearer_token_env
                    )
                } else {
                    format!(
                        "bearer secret is missing from `{}`",
                        preferences.bearer_token_env
                    )
                },
            )]
        }
        ProxyAuthMode::Tailnet => vec![finding(
            "proxy:auth-tailnet",
            Severity::Ok,
            "tailnet identity authentication is selected",
        )],
        ProxyAuthMode::Oauth => {
            #[cfg(feature = "gateway")]
            {
                oauth_findings(_config).await
            }
            #[cfg(not(feature = "gateway"))]
            {
                vec![gateway_feature_unavailable("proxy:oauth-daemon")]
            }
        }
    }
}

#[cfg(feature = "gateway")]
async fn oauth_findings(config: &crate::config::LabConfig) -> Vec<Finding> {
    let mut findings = Vec::new();
    let issuer = match crate::config::resolve_auth_for_config(config) {
        Ok(auth) if matches!(auth.mode, labby_auth::config::AuthMode::OAuth) => auth.public_url,
        Ok(_) => None,
        Err(error) => {
            findings.push(finding(
                "proxy:oauth-stable-issuer",
                Severity::Fail,
                format!("OAuth configuration is invalid: {error}"),
            ));
            None
        }
    };
    let issuer = match issuer {
        Some(issuer) => {
            findings.push(finding(
                "proxy:oauth-stable-issuer",
                Severity::Ok,
                format!("stable OAuth issuer is configured at {issuer}"),
            ));
            Some(issuer)
        }
        None => {
            if !findings
                .iter()
                .any(|item| item.check == "proxy:oauth-stable-issuer")
            {
                findings.push(finding(
                    "proxy:oauth-stable-issuer",
                    Severity::Fail,
                    "proxy OAuth requires auth mode oauth and a stable public issuer",
                ));
            }
            None
        }
    };

    let Some(gateway) = crate::live_gateway::detect(config).await else {
        findings.push(finding(
            "proxy:oauth-daemon",
            Severity::Fail,
            "no live Labby daemon is reachable",
        ));
        for check in [
            "proxy:oauth-lease-create",
            "proxy:oauth-lease-renew",
            "proxy:oauth-lease-release",
            "proxy:oauth-issuer-metadata",
            "proxy:oauth-jwks",
        ] {
            findings.push(dependency_failure(check, "live daemon discovery"));
        }
        return findings;
    };
    findings.push(finding(
        "proxy:oauth-daemon",
        Severity::Ok,
        "live Labby daemon is reachable",
    ));

    for (check, action) in [
        (
            "proxy:oauth-lease-create",
            "gateway.oauth.resource_lease.create",
        ),
        (
            "proxy:oauth-lease-renew",
            "gateway.oauth.resource_lease.renew",
        ),
        (
            "proxy:oauth-lease-release",
            "gateway.oauth.resource_lease.release",
        ),
    ] {
        match gateway.supports_action(action).await {
            Ok(true) => findings.push(finding(
                check,
                Severity::Ok,
                format!("live daemon supports `{action}`"),
            )),
            Ok(false) => findings.push(finding(
                check,
                Severity::Fail,
                format!("live daemon does not support `{action}`"),
            )),
            Err(error) => findings.push(finding(
                check,
                Severity::Fail,
                format!("could not inspect live daemon action catalog: {error}"),
            )),
        }
    }

    let Some(issuer) = issuer else {
        findings.push(dependency_failure(
            "proxy:oauth-issuer-metadata",
            "stable issuer configuration",
        ));
        findings.push(dependency_failure(
            "proxy:oauth-jwks",
            "stable issuer configuration",
        ));
        return findings;
    };
    match gateway.verify_oauth_issuer(&issuer).await {
        Ok(_) => {
            findings.push(finding(
                "proxy:oauth-issuer-metadata",
                Severity::Ok,
                "authorization-server metadata exactly matches the stable issuer",
            ));
            findings.push(finding(
                "proxy:oauth-jwks",
                Severity::Ok,
                "issuer JWKS is reachable and valid",
            ));
        }
        Err(error) => {
            let message = error.to_string();
            let jwks_failure = message.contains("JWKS");
            findings.push(finding(
                "proxy:oauth-issuer-metadata",
                if jwks_failure {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                if jwks_failure {
                    "authorization-server metadata exactly matches the stable issuer".to_string()
                } else {
                    format!("issuer metadata verification failed: {message}")
                },
            ));
            findings.push(finding(
                "proxy:oauth-jwks",
                Severity::Fail,
                if jwks_failure {
                    format!("issuer JWKS verification failed: {message}")
                } else {
                    "check unavailable because issuer metadata verification failed".to_string()
                },
            ));
        }
    }
    findings
}

#[cfg(feature = "gateway")]
async fn tailscale_findings() -> Vec<Finding> {
    let executable = std::env::var_os("LABBY_TAILSCALE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tailscale"));
    let mut findings = Vec::new();
    match crate::proxy::tailscale::run_checked(&executable, ["version"]).await {
        Ok(version) if !version.trim().is_empty() => findings.push(finding(
            "proxy:tailscale-version",
            Severity::Ok,
            format!("Tailscale CLI version {} is available", version.trim()),
        )),
        Ok(_) => findings.push(finding(
            "proxy:tailscale-version",
            Severity::Fail,
            "Tailscale CLI returned an empty version",
        )),
        Err(error) => {
            findings.push(finding(
                "proxy:tailscale-version",
                Severity::Fail,
                format!("Tailscale CLI is unavailable: {error:#}"),
            ));
            for check in [
                "proxy:tailscale-running",
                "proxy:tailscale-online",
                "proxy:tailscale-dns",
                "proxy:tailscale-https-serve",
            ] {
                findings.push(dependency_failure(check, "Tailscale executable/version"));
            }
            return findings;
        }
    }

    let status = match crate::proxy::tailscale::run_checked(&executable, ["status", "--json"])
        .await
        .and_then(|raw| TailscaleStatus::parse(&raw))
    {
        Ok(status) => status,
        Err(error) => {
            findings.push(finding(
                "proxy:tailscale-running",
                Severity::Fail,
                format!("Tailscale status is unavailable or invalid: {error:#}"),
            ));
            for check in [
                "proxy:tailscale-online",
                "proxy:tailscale-dns",
                "proxy:tailscale-https-serve",
            ] {
                findings.push(dependency_failure(check, "Tailscale status"));
            }
            return findings;
        }
    };
    findings.push(finding(
        "proxy:tailscale-running",
        if status.backend_running() {
            Severity::Ok
        } else {
            Severity::Fail
        },
        if status.backend_running() {
            "Tailscale backend state is Running"
        } else {
            "Tailscale backend state is not Running"
        },
    ));
    findings.push(finding(
        "proxy:tailscale-online",
        if status.online() {
            Severity::Ok
        } else {
            Severity::Fail
        },
        if status.online() {
            "local Tailscale node is online"
        } else {
            "local Tailscale node is offline"
        },
    ));
    findings.push(finding(
        "proxy:tailscale-dns",
        if status.dns_name().is_empty() {
            Severity::Fail
        } else {
            Severity::Ok
        },
        if status.dns_name().is_empty() {
            "local Tailscale node has no DNS name".to_string()
        } else {
            format!("local Tailscale DNS name is {}", status.dns_name())
        },
    ));

    match crate::proxy::tailscale::run_checked(&executable, ["serve", "status", "--json"])
        .await
        .and_then(|raw| ServeStatus::parse(&raw))
    {
        Ok(_) => findings.push(finding(
            "proxy:tailscale-https-serve",
            Severity::Ok,
            "Tailscale HTTPS Serve status is readable without mutation",
        )),
        Err(error) => findings.push(finding(
            "proxy:tailscale-https-serve",
            Severity::Fail,
            format!("Tailscale HTTPS Serve capability is unavailable: {error:#}"),
        )),
    }
    findings
}

#[cfg(all(test, not(feature = "gateway")))]
mod feature_boundary_tests {
    use super::*;

    #[test]
    fn gateway_feature_fallback_is_a_structured_warning() {
        let finding = gateway_feature_unavailable("proxy:gateway-feature");

        assert_eq!(finding.service, "proxy");
        assert_eq!(finding.check, "proxy:gateway-feature");
        assert!(matches!(finding.severity, Severity::Warn));
        assert_eq!(
            finding.message,
            "gateway feature is not compiled into this labby build"
        );
    }
}
