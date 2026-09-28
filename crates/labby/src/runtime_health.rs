//! Process-scoped record of subsystems that started degraded.
//!
//! `labby serve` keeps running when an optional subsystem (for example the
//! Skill Library behind the Artifact services) fails to start. Without a
//! durable record, that state is visible only in one startup log line while
//! probes stay green. Startup records each degradation here once; `/ready`
//! projects the stable codes and `doctor system.checks` projects the detail.
//!
//! Codes are stable, public-safe identifiers. Details are operator-facing
//! error chains built from configuration and startup errors; callers must not
//! record secret-bearing values.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

/// Skill Library bootstrap failed; Artifact services answer `service_unavailable`.
pub(crate) const ARTIFACTS_UNAVAILABLE: &str = "artifacts_unavailable";
/// Access-control persistence is blocked, so authority-dependent operations are unavailable.
pub(crate) const ACCESS_UNAVAILABLE: &str = "access_unavailable";
/// File Stash was selected but its persistence runtime could not become usable.
pub(crate) const FILE_STASH_UNAVAILABLE: &str = "file_stash_unavailable";
/// Workspace filesystem browsing is configured but its root is invalid.
pub(crate) const WORKSPACE_UNAVAILABLE: &str = "workspace_unavailable";
/// Configured public OAuth callback relay state failed to load.
pub(crate) const OAUTH_RELAY_UNAVAILABLE: &str = "oauth_relay_unavailable";
/// Actor-key derivation failed, so privacy-preserving actor correlation is disabled.
pub(crate) const ACTOR_KEY_UNAVAILABLE: &str = "actor_key_unavailable";
/// Usage telemetry was enabled but its durable store failed to open.
pub(crate) const USAGE_TELEMETRY_UNAVAILABLE: &str = "usage_telemetry_unavailable";
/// Code Mode journaling was enabled but its durable store failed to open.
pub(crate) const CODEMODE_JOURNAL_UNAVAILABLE: &str = "codemode_journal_unavailable";
/// Configured gateway auto-import failed during startup discovery.
pub(crate) const GATEWAY_IMPORT_DEGRADED: &str = "gateway_import_degraded";
/// One or more configured OpenAPI providers were omitted or partially loaded.
pub(crate) const OPENAPI_PROVIDER_DEGRADED: &str = "openapi_provider_degraded";
/// Nested stdio recursion protection intentionally suppressed upstream spawning.
pub(crate) const STDIO_UPSTREAM_RUNTIME_SUPPRESSED: &str = "stdio_upstream_runtime_suppressed";
/// First-run bootstrap failed while the daemon continued serving.
pub(crate) const BOOTSTRAP_DEGRADED: &str = "bootstrap_degraded";
/// First-run bootstrap could not reload its generated environment file.
pub(crate) const BOOTSTRAP_ENV_RELOAD_DEGRADED: &str = "bootstrap_env_reload_degraded";
/// This build cannot honor configured gateway upstreams.
#[cfg(not(feature = "gateway"))]
pub(crate) const GATEWAY_FEATURE_UNAVAILABLE: &str = "gateway_feature_unavailable";

/// Recorded subsystem degradations for one process (or one test fixture).
#[derive(Debug, Default)]
pub(crate) struct SubsystemHealth {
    degraded: Mutex<BTreeMap<&'static str, String>>,
}

static PROCESS: LazyLock<Arc<SubsystemHealth>> = LazyLock::new(Arc::default);

impl SubsystemHealth {
    /// The process-wide instance written by `labby serve` startup.
    pub(crate) fn process() -> Arc<Self> {
        Arc::clone(&PROCESS)
    }

    /// Record (or replace) one degraded subsystem with operator-facing detail.
    pub(crate) fn record_degraded(&self, code: &'static str, detail: String) {
        let detail = if detail.trim().is_empty() {
            format!(
                "{code} is unavailable; inspect Doctor capability findings and Labby server logs for the root cause"
            )
        } else {
            detail
        };
        self.degraded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(code, detail);
    }

    /// Stable degraded codes, sorted. Safe for public probes.
    pub(crate) fn degraded_codes(&self) -> Vec<&'static str> {
        self.degraded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect()
    }

    /// Degraded codes with their detail, sorted by code.
    pub(crate) fn degraded_details(&self) -> Vec<(&'static str, String)> {
        self.degraded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(code, detail)| (*code, detail.clone()))
            .collect()
    }
}

/// Render an error and every `source()` cause as `outer: inner: root`.
///
/// `Display` on an `anyhow::Error` (and on most typed errors) prints only the
/// outermost context, which hides the actionable root cause in logs. Use
/// `error_chain(error.as_ref())` for `anyhow::Error` values.
pub(crate) fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        // Some error types repeat their source in their own Display; skip
        // exact repeats so the chain stays readable.
        if !rendered.ends_with(&cause_text) {
            rendered.push_str(": ");
            rendered.push_str(&cause_text);
        }
        source = cause.source();
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    #[test]
    fn error_chain_includes_inner_anyhow_causes() {
        let error = Err::<(), _>(anyhow::anyhow!("private pinned address rejected"))
            .context("configure Skill Library exact-source adapters")
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "configure Skill Library exact-source adapters"
        );
        assert_eq!(
            error_chain(error.as_ref()),
            "configure Skill Library exact-source adapters: private pinned address rejected"
        );
    }

    #[test]
    fn error_chain_walks_typed_sources() {
        let io = std::io::Error::other("disk gone");
        let error = anyhow::Error::new(io).context("open store");
        assert_eq!(error_chain(error.as_ref()), "open store: disk gone");
    }

    #[test]
    fn records_are_sorted_and_replaceable() {
        let health = SubsystemHealth::default();
        assert!(health.degraded_codes().is_empty());
        health.record_degraded("zeta", "first".into());
        health.record_degraded(ARTIFACTS_UNAVAILABLE, "cause".into());
        health.record_degraded("zeta", "second".into());
        assert_eq!(health.degraded_codes(), [ARTIFACTS_UNAVAILABLE, "zeta"]);
        assert_eq!(health.degraded_details()[1], ("zeta", "second".to_owned()));
    }

    #[test]
    fn every_degraded_code_has_nonempty_operator_detail() {
        let health = SubsystemHealth::default();
        health.record_degraded("blank_detail", "   ".into());
        let details = health.degraded_details();
        assert_eq!(health.degraded_codes(), ["blank_detail"]);
        assert_eq!(details.len(), 1);
        assert!(!details[0].1.trim().is_empty());
        assert!(details[0].1.contains("Doctor capability findings"));
    }
}
