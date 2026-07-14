use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use tokio::net::TcpStream;

use crate::dispatch::doctor::{Finding, Report, Severity};
use crate::oauth::public_relay::{
    MAX_REGISTRY_BACKUPS, MachineId, PublicRelayRegistryManager, PublicRelayRegistryStore,
    PublicRelaySnapshot, RelayTarget,
};

const TARGET_PROBE_CONCURRENCY: usize = 8;
const SERVICE: &str = "oauth_relay";
/// Aggregate bound on the whole target-probe pass. Each individual probe is
/// already capped at 1s, but with `TARGET_PROBE_CONCURRENCY` concurrency the
/// *pass* has no bound of its own — wall time otherwise grows linearly with
/// registry size (`machine_count / TARGET_PROBE_CONCURRENCY` seconds). This
/// keeps `oauth.relay.check --probe-targets` bounded for larger Tailscale
/// deployments instead of hanging proportional to fleet size.
const TARGET_PROBE_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

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
            let live_snapshot = manager.snapshot().await;
            match manager.store().load_snapshot().await {
                Ok(snapshot) if snapshot == live_snapshot => findings.push(Finding {
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
                        "live registry has {machines} machine(s) ({}); persisted registry has {} ({})",
                        describe_machine_ids(&live_snapshot),
                        snapshot.entries.len(),
                        describe_machine_ids(&snapshot),
                    ),
                }),
                Err(error) => findings.push(Finding {
                    service: SERVICE.into(),
                    check: "registry:corrupt".into(),
                    severity: Severity::Fail,
                    message: error.to_string(),
                }),
            }
            push_backup_accumulation_finding(&mut findings, manager.store());
            if probe_targets {
                let probe_pass = stream::iter(manager.probe_targets().await)
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
                    .collect::<Vec<_>>();
                match tokio::time::timeout(TARGET_PROBE_TOTAL_TIMEOUT, probe_pass).await {
                    Ok(target_findings) => findings.extend(target_findings),
                    Err(_) => findings.push(Finding {
                        service: SERVICE.into(),
                        check: "targets:probe".into(),
                        severity: Severity::Warn,
                        message: format!(
                            "target probing aborted after exceeding the {}s aggregate bound; registry may be too large for a single probe pass",
                            TARGET_PROBE_TOTAL_TIMEOUT.as_secs()
                        ),
                    }),
                }
            }
        }
        None => {
            let store = PublicRelayRegistryStore::new(PublicRelayRegistryStore::default_path());
            push_backup_accumulation_finding(&mut findings, &store);
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

/// Warn if `<registry>.bak.*` sidecars have accumulated beyond
/// `MAX_REGISTRY_BACKUPS`. `store.rs::prune_old_backups` runs after every
/// save and logs (but does not fail the save on) listing/removal errors, so
/// a persistently-failing prune is otherwise invisible to an operator —
/// this is the one surface that would catch unbounded accumulation from
/// that failure mode, since the cap is enforced by design and can only be
/// exceeded if pruning is broken.
fn push_backup_accumulation_finding(findings: &mut Vec<Finding>, store: &PublicRelayRegistryStore) {
    let count = store.count_backups();
    if count <= MAX_REGISTRY_BACKUPS {
        return;
    }
    findings.push(Finding {
        service: SERVICE.into(),
        check: "registry:backup_accumulation".into(),
        severity: Severity::Warn,
        message: format!(
            "found {count} relay registry backup file(s), expected at most {MAX_REGISTRY_BACKUPS}; backup pruning may be failing (see logs for `action = \"backup.prune\"` warnings)"
        ),
    });
}

/// Sorted, comma-joined machine id digest for a snapshot — used to make
/// `registry:stale` findings actionable when live and persisted registries
/// have the same entry *count* but different *content* (e.g. one machine
/// swapped for another). `PublicRelaySnapshot::entries` is a `BTreeMap<MachineId, _>`
/// so iteration order is already machine-id sorted.
fn describe_machine_ids(snapshot: &PublicRelaySnapshot) -> String {
    let ids: Vec<&str> = snapshot
        .entries
        .keys()
        .map(MachineId::as_str)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
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
            .save_entries(vec![crate::oauth::public_relay::PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                true,
            )])
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
            .save_entries(vec![crate::oauth::public_relay::PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            )])
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

    #[tokio::test]
    async fn relay_health_detects_equal_count_machine_swap_as_stale() {
        // lab-s1wtg: comparing `entries.len()` alone can't see a same-count
        // swap (one machine removed, a different one added). The persisted
        // and live registries must be compared by content, not just count.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let store = PublicRelayRegistryStore::new(path.clone());
        store
            .save_entries(vec![crate::oauth::public_relay::PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            )])
            .await
            .unwrap();
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();
        // Same entry count (1), different machine id entirely.
        std::fs::write(
            &path,
            r#"{
                "version": 1,
                "entries": [
                    {
                        "machine_id": "tootie",
                        "target_url": "http://100.120.242.29:38935/callback/tootie"
                    }
                ]
            }"#,
        )
        .unwrap();

        let report = check_public_relay(Some(Arc::new(manager)), false).await;

        let loaded = report
            .findings
            .iter()
            .find(|finding| finding.check == "registry:loaded")
            .unwrap();
        assert!(matches!(loaded.severity, Severity::Ok));
        let stale = report
            .findings
            .iter()
            .find(|finding| finding.check == "registry:stale")
            .unwrap_or_else(|| panic!("expected registry:stale finding, got {report:?}"));
        assert!(matches!(stale.severity, Severity::Warn));
        assert!(stale.message.contains("dookie"));
        assert!(stale.message.contains("tootie"));
    }

    #[tokio::test]
    async fn relay_health_warns_when_backup_files_exceed_the_cap() {
        // Backups are capped at `MAX_REGISTRY_BACKUPS` by design (pruned
        // after every save), so more than that on disk can only mean
        // pruning has been persistently failing -- this should be visible
        // in `labby doctor oauth-relay`, not just in swallowed prune logs.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let store = PublicRelayRegistryStore::new(path.clone());
        store
            .save_entries(vec![crate::oauth::public_relay::PublicRelayEntry::new(
                MachineId::parse("dookie").unwrap(),
                "http://100.88.16.79:38935/callback/dookie",
                None,
                false,
            )])
            .await
            .unwrap();
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        // Synthesize more backup sidecars than the cap allows.
        for i in 0..(MAX_REGISTRY_BACKUPS + 3) {
            std::fs::write(
                dir.path().join(format!("registry.json.bak.synthetic-{i}")),
                b"{}",
            )
            .unwrap();
        }

        let report = check_public_relay(Some(Arc::new(manager)), false).await;

        let finding = report
            .findings
            .iter()
            .find(|finding| finding.check == "registry:backup_accumulation")
            .unwrap_or_else(|| {
                panic!("expected registry:backup_accumulation finding, got {report:?}")
            });
        assert!(matches!(finding.severity, Severity::Warn));
        assert!(
            finding
                .message
                .contains(&(MAX_REGISTRY_BACKUPS + 3).to_string())
        );
    }

    #[tokio::test]
    async fn relay_health_does_not_warn_when_backup_count_is_within_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = PublicRelayRegistryStore::new(dir.path().join("registry.json"));
        let manager = PublicRelayRegistryManager::load(store).await.unwrap();

        let report = check_public_relay(Some(Arc::new(manager)), false).await;

        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.check != "registry:backup_accumulation"),
            "unexpected backup_accumulation finding: {report:?}"
        );
    }
}
