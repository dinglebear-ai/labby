use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;

use crate::dispatch::doctor::{Finding, Report, Severity};
use crate::oauth::public_relay::{
    PublicRelayRegistryManager, PublicRelayRegistryStore, RelayTarget,
};

pub async fn check_public_relay(
    manager: Option<Arc<PublicRelayRegistryManager>>,
    probe_targets: bool,
) -> Report {
    let mut findings = Vec::new();
    match manager {
        Some(manager) => {
            let machines = manager.count().await;
            findings.push(Finding {
                service: "oauth_relay".into(),
                check: "registry:loaded".into(),
                severity: if machines == 0 {
                    Severity::Warn
                } else {
                    Severity::Ok
                },
                message: if machines == 0 {
                    "public relay registry loaded but empty".into()
                } else {
                    format!("public relay registry loaded with {machines} machine(s)")
                },
            });
            if probe_targets {
                for view in manager.list().await {
                    let machine =
                        match crate::oauth::public_relay::MachineId::parse(&view.machine_id) {
                            Ok(machine) => machine,
                            Err(error) => {
                                findings.push(Finding {
                                    service: "oauth_relay".into(),
                                    check: format!("target:{}", view.machine_id),
                                    severity: Severity::Fail,
                                    message: error.to_string(),
                                });
                                continue;
                            }
                        };
                    match manager.resolve(&machine).await {
                        Ok(target) => findings.push(probe_target(&target).await),
                        Err(error) => findings.push(Finding {
                            service: "oauth_relay".into(),
                            check: format!("target:{machine}"),
                            severity: Severity::Warn,
                            message: error.to_string(),
                        }),
                    }
                }
            }
        }
        None => {
            let store = PublicRelayRegistryStore::new(PublicRelayRegistryStore::default_path());
            if !store.path().exists() {
                findings.push(Finding {
                    service: "oauth_relay".into(),
                    check: "registry:missing".into(),
                    severity: Severity::Warn,
                    message: format!("{} not found", store.path().display()),
                });
            } else {
                match store.load_snapshot().await {
                    Ok(snapshot) => findings.push(Finding {
                        service: "oauth_relay".into(),
                        check: "registry:loadable".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "registry is loadable with {} machine(s), but public relay manager is not wired",
                            snapshot.entries.len()
                        ),
                    }),
                    Err(error) => findings.push(Finding {
                        service: "oauth_relay".into(),
                        check: "registry:corrupt".into(),
                        severity: Severity::Fail,
                        message: error.to_string(),
                    }),
                }
            }
        }
    }
    Report { findings }
}

async fn probe_target(target: &RelayTarget) -> Finding {
    let check = format!("target:{}", target.machine_id);
    let Some(host) = target.url.host_str() else {
        return Finding {
            service: "oauth_relay".into(),
            check,
            severity: Severity::Fail,
            message: "target host missing".into(),
        };
    };
    let Some(port) = target.url.port_or_known_default() else {
        return Finding {
            service: "oauth_relay".into(),
            check,
            severity: Severity::Fail,
            message: "target port missing".into(),
        };
    };
    let addr = format!("{host}:{port}");
    match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => Finding {
            service: "oauth_relay".into(),
            check,
            severity: Severity::Ok,
            message: format!("{} reachable", target.redacted_label()),
        },
        Ok(Err(error)) => Finding {
            service: "oauth_relay".into(),
            check,
            severity: Severity::Fail,
            message: format!("{} unreachable: {error}", target.redacted_label()),
        },
        Err(_) => Finding {
            service: "oauth_relay".into(),
            check,
            severity: Severity::Fail,
            message: format!("{} probe timed out", target.redacted_label()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn relay_health_reports_loaded_empty_registry_without_target_probe() {
        let dir = tempfile::tempdir().unwrap();
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        let report = check_public_relay(Some(Arc::new(manager)), false).await;

        assert_eq!(report.findings[0].check, "registry:loaded");
        assert!(matches!(report.findings[0].severity, Severity::Warn));
    }
}
