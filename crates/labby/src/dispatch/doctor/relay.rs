use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use tokio::net::TcpStream;

use crate::dispatch::doctor::{Finding, Report, Severity};
use crate::oauth::public_relay::{
    PublicRelayRegistryManager, PublicRelayRegistryStore, RelayTarget,
};

const TARGET_PROBE_CONCURRENCY: usize = 8;
const SERVICE: &str = "oauth_relay";

pub async fn check_public_relay(
    manager: Option<Arc<PublicRelayRegistryManager>>,
    probe_targets: bool,
) -> Report {
    let mut findings = Vec::new();
    match manager {
        Some(manager) => {
            let machines = manager.count().await;
            findings.push(Finding {
                service: SERVICE.into(),
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
            match manager.store().load_snapshot().await {
                Ok(snapshot) if snapshot.entries.len() == machines => findings.push(Finding {
                    service: SERVICE.into(),
                    check: "registry:persisted".into(),
                    severity: Severity::Ok,
                    message: format!(
                        "persisted registry is loadable with {} machine(s)",
                        snapshot.entries.len()
                    ),
                }),
                Ok(snapshot) => findings.push(Finding {
                    service: SERVICE.into(),
                    check: "registry:stale".into(),
                    severity: Severity::Warn,
                    message: format!(
                        "live registry has {machines} machine(s), persisted registry has {}",
                        snapshot.entries.len()
                    ),
                }),
                Err(error) => findings.push(Finding {
                    service: SERVICE.into(),
                    check: "registry:corrupt".into(),
                    severity: Severity::Fail,
                    message: error.to_string(),
                }),
            }
            if probe_targets {
                let target_findings = stream::iter(manager.probe_targets().await)
                    .map(|(machine, target)| async move {
                        match target {
                            Ok(target) => probe_target(&target).await,
                            Err(error) => Finding {
                                service: SERVICE.into(),
                                check: format!("target:{machine}"),
                                severity: Severity::Warn,
                                message: error.to_string(),
                            },
                        }
                    })
                    .buffer_unordered(TARGET_PROBE_CONCURRENCY)
                    .collect::<Vec<_>>()
                    .await;
                findings.extend(target_findings);
            }
        }
        None => {
            let store = PublicRelayRegistryStore::new(PublicRelayRegistryStore::default_path());
            if !store.path().exists() {
                findings.push(Finding {
                    service: SERVICE.into(),
                    check: "registry:missing".into(),
                    severity: Severity::Warn,
                    message: format!("{} not found", store.path().display()),
                });
            } else {
                match store.load_snapshot().await {
                    Ok(snapshot) => findings.push(Finding {
                        service: SERVICE.into(),
                        check: "registry:loadable".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "registry is loadable with {} machine(s), but public relay manager is not wired",
                            snapshot.entries.len()
                        ),
                    }),
                    Err(error) => findings.push(Finding {
                        service: SERVICE.into(),
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
    let check = format!("target:{}", target.machine_id());
    let Some(host) = target.host_str() else {
        return Finding {
            service: SERVICE.into(),
            check,
            severity: Severity::Fail,
            message: "target host missing".into(),
        };
    };
    let Some(port) = target.port_or_known_default() else {
        return Finding {
            service: SERVICE.into(),
            check,
            severity: Severity::Fail,
            message: "target port missing".into(),
        };
    };
    let addr = format!("{host}:{port}");
    match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => Finding {
            service: SERVICE.into(),
            check,
            severity: Severity::Ok,
            message: format!("{} reachable", target.redacted_label()),
        },
        Ok(Err(error)) => Finding {
            service: SERVICE.into(),
            check,
            severity: Severity::Fail,
            message: format!("{} unreachable: {error}", target.redacted_label()),
        },
        Err(_) => Finding {
            service: SERVICE.into(),
            check,
            severity: Severity::Fail,
            message: format!("{} probe timed out", target.redacted_label()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex};

    static LAB_HOME_TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    #[tokio::test]
    async fn relay_health_reports_loaded_empty_registry_without_target_probe() {
        let dir = tempfile::tempdir().unwrap();
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        let report = check_public_relay(Some(Arc::new(manager)), false).await;

        assert_eq!(report.findings[0].check, "registry:loaded");
        assert!(matches!(report.findings[0].severity, Severity::Warn));
    }

    #[tokio::test]
    async fn relay_health_reports_disabled_targets_when_probe_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        store
            .save_entries(vec![crate::oauth::public_relay::PublicRelayEntry {
                machine_id: crate::oauth::public_relay::MachineId::parse("dookie").unwrap(),
                target_url: "http://100.88.16.79:38935/callback/dookie".into(),
                description: None,
                disabled: true,
            }])
            .await
            .unwrap();
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        let report = check_public_relay(Some(Arc::new(manager)), true).await;

        let target = report
            .findings
            .iter()
            .find(|finding| finding.check == "target:dookie")
            .unwrap();
        assert!(matches!(target.severity, Severity::Warn));
        assert!(target.message.contains("machine is disabled"));
    }

    #[tokio::test]
    async fn relay_health_reports_corrupt_registry_when_manager_missing() {
        let _guard = LAB_HOME_TEST_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        crate::dispatch::helpers::set_test_lab_home(Some(dir.path().to_path_buf()));
        let path = PublicRelayRegistryStore::default_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not valid json").unwrap();

        let report = check_public_relay(None, false).await;
        crate::dispatch::helpers::set_test_lab_home(None);

        let finding = report
            .findings
            .iter()
            .find(|finding| finding.check == "registry:corrupt")
            .unwrap();
        assert!(matches!(finding.severity, Severity::Fail));
    }

    #[tokio::test]
    async fn relay_health_reports_corrupt_registry_when_live_manager_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let store = PublicRelayRegistryStore::new(path.clone());
        store
            .save_entries(vec![crate::oauth::public_relay::PublicRelayEntry {
                machine_id: crate::oauth::public_relay::MachineId::parse("dookie").unwrap(),
                target_url: "http://100.88.16.79:38935/callback/dookie".into(),
                description: None,
                disabled: false,
            }])
            .await
            .unwrap();
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();
        std::fs::write(&path, "{not valid json").unwrap();

        let report = check_public_relay(Some(Arc::new(manager)), false).await;

        let loaded = report
            .findings
            .iter()
            .find(|finding| finding.check == "registry:loaded")
            .unwrap();
        assert!(matches!(loaded.severity, Severity::Ok));
        let corrupt = report
            .findings
            .iter()
            .find(|finding| finding.check == "registry:corrupt")
            .unwrap();
        assert!(matches!(corrupt.severity, Severity::Fail));
    }
}
