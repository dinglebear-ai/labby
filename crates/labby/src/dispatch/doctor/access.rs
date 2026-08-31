//! Read-only projection of access-store health into doctor findings.

use std::path::PathBuf;

use crate::access::{AccessHealth, AccessHealthStatus};

use super::types::{Finding, Report, Severity};

pub(crate) async fn check_access_store() -> Report {
    let path = match crate::config::access_db_path() {
        Ok(path) => path,
        Err(_) => {
            return Report {
                findings: vec![finding(
                    Severity::Fail,
                    "Access store location is unavailable; configure an absolute Labby state directory.",
                )],
            };
        }
    };

    let health = tokio::task::spawn_blocking(move || inspect(path)).await;
    let finding = match health {
        Ok(health) => project(health),
        Err(_) => finding(
            Severity::Fail,
            "Access store check could not complete; retry the check and inspect Labby service logs.",
        ),
    };
    Report {
        findings: vec![finding],
    }
}

fn inspect(path: PathBuf) -> AccessHealth {
    crate::access::inspect_health(&path)
}

fn project(health: AccessHealth) -> Finding {
    match health.status {
        AccessHealthStatus::Ready => finding(Severity::Ok, "Access store is ready."),
        AccessHealthStatus::Missing => finding(
            Severity::Warn,
            "Access store is not initialized; run owner bootstrap before enabling access enforcement.",
        ),
        AccessHealthStatus::Uninitialized => finding(
            Severity::Warn,
            "Access store needs initialization or owner bootstrap before access enforcement.",
        ),
        AccessHealthStatus::Prepared => finding(
            Severity::Warn,
            "Access bootstrap is prepared; start Labby and consume the one-time proof.",
        ),
        AccessHealthStatus::Insecure => finding(
            Severity::Fail,
            "Access store path or permissions are insecure; secure the state directory and store files.",
        ),
        AccessHealthStatus::Corrupt => finding(
            Severity::Fail,
            "Access store failed integrity checks; restore or repair it before access enforcement.",
        ),
        AccessHealthStatus::NewerSchema => finding(
            Severity::Fail,
            "Access store schema is newer than this Labby version; upgrade Labby before using it.",
        ),
        AccessHealthStatus::Locked => finding(
            Severity::Fail,
            "Access store is locked; retry after the current database operation completes.",
        ),
        AccessHealthStatus::ReadOnly => finding(
            Severity::Fail,
            "Access store is read-only; restore writable owner-only permissions.",
        ),
        AccessHealthStatus::Unavailable => finding(
            Severity::Fail,
            "Access store is unavailable; verify its path and filesystem availability.",
        ),
    }
}

fn finding(severity: Severity, message: &str) -> Finding {
    Finding {
        service: "access".to_string(),
        check: "store".to_string(),
        severity,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    #[test]
    fn every_health_status_has_a_stable_safe_projection() {
        let cases = [
            (AccessHealthStatus::Missing, Severity::Warn),
            (AccessHealthStatus::Uninitialized, Severity::Warn),
            (AccessHealthStatus::Ready, Severity::Ok),
            (AccessHealthStatus::Insecure, Severity::Fail),
            (AccessHealthStatus::Corrupt, Severity::Fail),
            (AccessHealthStatus::NewerSchema, Severity::Fail),
            (AccessHealthStatus::Locked, Severity::Fail),
            (AccessHealthStatus::ReadOnly, Severity::Fail),
            (AccessHealthStatus::Unavailable, Severity::Fail),
        ];

        for (status, expected) in cases {
            let projected = project(AccessHealth {
                status,
                detail: "ignored",
            });
            assert_eq!(projected.service, "access");
            assert_eq!(projected.check, "store");
            assert!(matches!(
                (projected.severity, expected),
                (Severity::Ok, Severity::Ok)
                    | (Severity::Warn, Severity::Warn)
                    | (Severity::Fail, Severity::Fail)
            ));
            assert!(!projected.message.contains("ignored"));
            assert!(!projected.message.contains("SELECT"));
        }
    }

    #[test]
    fn catalog_marks_access_check_as_safe_and_unprivileged() {
        let spec = super::super::ACTIONS
            .iter()
            .find(|spec| spec.name == "access.check")
            .expect("access.check action");
        assert!(!spec.destructive);
        assert!(!spec.requires_admin);
    }

    #[tokio::test]
    async fn both_dispatch_layers_route_access_check() {
        let mcp = super::super::dispatch("access.check", json!({}))
            .await
            .expect("MCP dispatch");
        let clients = Arc::new(crate::dispatch::clients::ServiceClients::from_env());
        let api = super::super::dispatch_with_clients(&clients, "access.check", json!({}))
            .await
            .expect("API dispatch");

        for report in [mcp, api] {
            assert_eq!(report["findings"].as_array().map(Vec::len), Some(1));
            assert_eq!(report["findings"][0]["service"], "access");
            assert_eq!(report["findings"][0]["check"], "store");
        }
    }

    #[tokio::test]
    async fn local_full_audit_emits_exactly_one_access_finding_last() {
        let clients = Arc::new(crate::dispatch::clients::ServiceClients::from_env());
        let (tx, mut rx) = tokio::sync::mpsc::channel(512);
        super::super::service::stream_audit_full(clients, tx).await;

        let mut findings = Vec::new();
        while let Ok(finding) = rx.try_recv() {
            findings.push(finding);
        }
        let access_positions: Vec<_> = findings
            .iter()
            .enumerate()
            .filter_map(|(index, finding)| (finding.service == "access").then_some(index))
            .collect();
        assert_eq!(access_positions, vec![findings.len() - 1]);
        assert_eq!(
            findings.last().map(|finding| finding.check.as_str()),
            Some("store")
        );
    }
}
