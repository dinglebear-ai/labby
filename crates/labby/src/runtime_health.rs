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
}
