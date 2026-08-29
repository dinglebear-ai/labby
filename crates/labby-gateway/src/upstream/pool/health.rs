//! Circuit-breaker and health accounting for the upstream pool.
//!
//! These methods record per-capability success/failure, drive the
//! consecutive-failure circuit breaker, expose last-error and reprobe-due
//! queries, and surface upstream counts/status. They are an `impl UpstreamPool`
//! block on the struct defined in `pool.rs`, so the private `catalog` and
//! `connections` fields are visible without annotation.

use std::time::Instant;

use super::super::types;
use super::super::types::{UpstreamCapability, UpstreamEntry, UpstreamHealth};
use super::UpstreamPool;

pub(super) fn record_failure_on_entry(
    upstream_name: &str,
    entry: &mut UpstreamEntry,
    capability: UpstreamCapability,
    error: String,
) {
    let previous = entry.health_for(capability);
    let was_open = previous.is_open();
    let new_count = match previous {
        UpstreamHealth::Healthy => 1,
        UpstreamHealth::Unhealthy {
            consecutive_failures,
        } => consecutive_failures.saturating_add(1),
    };
    entry.set_health_for(
        capability,
        UpstreamHealth::Unhealthy {
            consecutive_failures: new_count,
        },
    );
    // Reset the quarantine clock on every failed reprobe so an open circuit
    // cannot immediately become due again after a slow failing request.
    entry.set_unhealthy_since_for(capability, Some(Instant::now()));
    entry.set_last_error_for(capability, Some(error.clone()));
    if !was_open && new_count >= types::CIRCUIT_BREAKER_THRESHOLD {
        let retry_after = types::reprobe_interval_for_failures(new_count);
        tracing::warn!(
            upstream = %upstream_name,
            capability = ?capability,
            consecutive_failures = new_count,
            retry_after_ms = retry_after.as_millis(),
            error = %error,
            "circuit breaker open — upstream quarantined"
        );
    } else if new_count >= types::CIRCUIT_BREAKER_THRESHOLD {
        let retry_after = types::reprobe_interval_for_failures(new_count);
        tracing::debug!(
            upstream = %upstream_name,
            capability = ?capability,
            consecutive_failures = new_count,
            retry_after_ms = retry_after.as_millis(),
            error = %error,
            "open circuit reprobe failed; quarantine extended"
        );
    }
}

pub(super) fn record_success_on_entry(
    upstream_name: &str,
    entry: &mut UpstreamEntry,
    capability: UpstreamCapability,
) {
    if !entry.health_for(capability).is_routable() {
        tracing::info!(
            upstream = %upstream_name,
            capability = ?capability,
            "circuit breaker reset — upstream healthy"
        );
    }
    entry.set_health_for(capability, UpstreamHealth::Healthy);
    entry.set_unhealthy_since_for(capability, None);
    entry.set_last_error_for(capability, None);
}

impl UpstreamPool {
    pub async fn record_failure(&self, upstream_name: &str, error: impl Into<String>) {
        self.record_failure_for(upstream_name, UpstreamCapability::Tools, error)
            .await;
    }

    /// Record a failure for a specific upstream capability, potentially marking it unhealthy.
    ///
    /// After `CIRCUIT_BREAKER_THRESHOLD` consecutive failures, the upstream
    /// is excluded from the matching capability listing until a successful re-probe.
    pub async fn record_failure_for(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
        error: impl Into<String>,
    ) {
        let mut catalog = self.catalog_write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            record_failure_on_entry(upstream_name, entry, capability, error.into());
        }
    }

    /// Record a success for an upstream capability, resetting the circuit breaker.
    pub async fn record_success(&self, upstream_name: &str) {
        self.record_success_for(upstream_name, UpstreamCapability::Tools)
            .await;
    }

    /// Record a success for a specific upstream capability, resetting the circuit breaker.
    pub async fn record_success_for(&self, upstream_name: &str, capability: UpstreamCapability) {
        let mut catalog = self.catalog_write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            record_success_on_entry(upstream_name, entry, capability);
        }
    }

    /// Return the most relevant last error for an upstream, if any capability has one.
    pub async fn upstream_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Tools)
            .or_else(|| entry.last_error_for(UpstreamCapability::Resources))
            .or_else(|| entry.last_error_for(UpstreamCapability::Prompts))
            .map(ToOwned::to_owned)
    }

    /// Clear one capability's circuit breaker after a fresh connect.
    ///
    /// Circuit state counts consecutive failures against a *session*. Once an
    /// upstream reconnects, that count describes a peer that no longer exists,
    /// so carrying it forward is already wrong — and for the optional
    /// capabilities it is unrecoverable rather than merely stale.
    ///
    /// The latch: `routable_upstream_peers` filters open circuits out of the
    /// prompt/resource listing fan-out, and that fan-out is the only place a
    /// success for those capabilities is ever recorded. So three failed
    /// listings open the circuit, and the listing that could close it is
    /// precisely the one that is now skipped — a permanent silent no-op. There
    /// is no time-based escape: `UpstreamHealth::is_open` is a bare threshold
    /// comparison with no quarantine expiry, `REPROBE_INTERVAL` reprobing
    /// drives tool health only, and `replace_catalog_tools` leaves the health
    /// fields untouched. Only a full `discover_all` rebuild clears it today.
    ///
    /// Resetting here is what makes the optional-capability circuits
    /// recoverable at all (bead lab-zfyxk).
    pub async fn reset_capability_circuit(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
    ) {
        let mut catalog = self.catalog_write().await;
        if let Some(entry) = catalog.get_mut(upstream_name) {
            entry.set_health_for(capability, UpstreamHealth::Healthy);
            entry.set_unhealthy_since_for(capability, None);
            entry.set_last_error_for(capability, None);
        }
    }

    /// Return the health of one specific capability, if the upstream is known.
    pub async fn upstream_capability_health(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
    ) -> Option<UpstreamHealth> {
        let catalog = self.catalog.read().await;
        Some(catalog.get(upstream_name)?.health_for(capability))
    }

    /// Return the last error recorded for one specific capability, if any.
    ///
    /// `upstream_last_error` deliberately collapses every capability into a
    /// single "worst" error because callers use it to decide whether the
    /// upstream is connected at all. That collapse is why an optional
    /// capability failing has to be filtered back out downstream — a failed
    /// `prompts/list` must not render a server with perfectly good tools as
    /// disconnected.
    ///
    /// This accessor keeps the capabilities separable so operator surfaces can
    /// report an optional-capability failure as a *warning* instead, rather
    /// than choosing between "mark the whole upstream down" and "say nothing"
    /// (bead lab-zfyxk).
    pub async fn upstream_capability_error(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
    ) -> Option<String> {
        let catalog = self.catalog.read().await;
        catalog
            .get(upstream_name)?
            .last_error_for(capability)
            .map(ToOwned::to_owned)
    }

    /// Return the last tools-capability error for an upstream, if any.
    pub async fn upstream_tool_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Tools)
            .map(ToOwned::to_owned)
    }

    /// Return the last Agent Skills capability error for an upstream, if any.
    pub async fn upstream_skills_last_error(&self, upstream_name: &str) -> Option<String> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        entry
            .last_error_for(UpstreamCapability::Skills)
            .map(ToOwned::to_owned)
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn insert_entry_for_tests(&self, name: &str, entry: UpstreamEntry) {
        self.catalog_write().await.insert(name.to_string(), entry);
    }

    /// Test-only: insert a fully-formed `UpstreamEntry` into the catalog.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn insert_entry_for_test(&self, name: &str, entry: UpstreamEntry) {
        self.catalog_write().await.insert(name.to_string(), entry);
    }

    /// Check if an upstream capability is due for a re-probe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn should_reprobe(&self, upstream_name: &str) -> bool {
        self.should_reprobe_for(upstream_name, UpstreamCapability::Tools)
            .await
    }

    /// Check if a specific upstream capability is due for a re-probe.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn should_reprobe_for(
        &self,
        upstream_name: &str,
        capability: UpstreamCapability,
    ) -> bool {
        let catalog = self.catalog.read().await;
        if let Some(entry) = catalog.get(upstream_name)
            && let UpstreamHealth::Unhealthy {
                consecutive_failures,
            } = entry.health_for(capability)
            && consecutive_failures >= types::CIRCUIT_BREAKER_THRESHOLD
            && let Some(since) = entry.unhealthy_since_for(capability)
        {
            return since.elapsed() >= types::reprobe_interval_for_failures(consecutive_failures);
        }
        false
    }

    /// Filter out upstream tools whose names collide with built-in service tools.
    ///
    /// Built-in lab services permanently take precedence. Upstream tools with
    /// colliding names are dropped with a warning.
    pub async fn filter_collisions(&self, builtin_names: &[&str]) {
        let mut catalog = self.catalog_write().await;
        for entry in catalog.values_mut() {
            let collisions: Vec<String> = entry
                .tools
                .keys()
                .filter(|name| builtin_names.contains(&name.as_str()))
                .cloned()
                .collect();
            for name in &collisions {
                tracing::warn!(
                    upstream = %entry.name,
                    tool = %name,
                    "upstream tool name collides with built-in service — rejecting upstream tool"
                );
                entry.tools.remove(name);
            }
        }
    }

    /// Get the number of connected upstreams.
    pub async fn upstream_count(&self) -> usize {
        self.catalog.read().await.len()
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn connection_count_for_tests(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Observe the exact OAuth connection identity without influencing routing.
    #[cfg(any(test, feature = "testkit"))]
    pub async fn subject_connection_identity_for_tests(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Option<(u64, u64)> {
        let connections = self.subject_connections.read().await;
        let connection = connections.get(&(upstream.to_string(), subject.to_string()))?;
        Some((
            connection._connection.incarnation?.get(),
            connection._connection.runtime.oauth_credential_generation?,
        ))
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn oauth_runtime_counts_for_tests(&self) -> (usize, usize) {
        (
            self.subject_connections.read().await.len(),
            self.task_routes.read().await.len(),
        )
    }

    /// Get names of all registered upstreams with their tool health status.
    pub async fn upstream_status(&self) -> Vec<(String, UpstreamHealth)> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .map(|e| (e.name.to_string(), e.tool_health))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::super::entries::healthy_in_process_entry;
    use super::*;

    #[tokio::test]
    async fn upstream_last_error_tracks_capability_failure_details() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Resources,
            "resource listing returned 401 unauthorized",
        )
        .await;

        assert_eq!(
            pool.upstream_last_error("github").await.as_deref(),
            Some("resource listing returned 401 unauthorized")
        );

        pool.record_success_for("github", UpstreamCapability::Resources)
            .await;
        assert_eq!(pool.upstream_last_error("github").await, None);
    }

    #[tokio::test]
    async fn upstream_tool_last_error_ignores_non_tool_failures() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Resources,
            "resource listing returned 401 unauthorized",
        )
        .await;
        pool.record_failure_for(
            "github",
            UpstreamCapability::Prompts,
            "prompt listing returned 501 unsupported",
        )
        .await;

        assert_eq!(pool.upstream_tool_last_error("github").await, None);

        pool.record_failure_for(
            "github",
            UpstreamCapability::Tools,
            "tool listing returned 500 internal error",
        )
        .await;

        assert_eq!(
            pool.upstream_tool_last_error("github").await.as_deref(),
            Some("tool listing returned 500 internal error")
        );
    }

    /// Helper: read the current tool-capability health for an upstream.
    async fn tool_health(pool: &UpstreamPool, name: &str) -> UpstreamHealth {
        let catalog = pool.catalog.read().await;
        catalog
            .get(name)
            .expect("entry present")
            .health_for(UpstreamCapability::Tools)
    }

    #[tokio::test]
    async fn circuit_breaker_opens_after_threshold_then_closes_on_success() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());
        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        // Starts healthy/routable.
        assert!(tool_health(&pool, "github").await.is_routable());
        assert!(!tool_health(&pool, "github").await.is_open());

        // Record CIRCUIT_BREAKER_THRESHOLD consecutive failures. The breaker
        // should only open on the final one.
        for i in 1..types::CIRCUIT_BREAKER_THRESHOLD {
            pool.record_failure_for(
                "github",
                UpstreamCapability::Tools,
                format!("tool listing failed (attempt {i})"),
            )
            .await;
            assert!(
                tool_health(&pool, "github").await.is_routable(),
                "breaker must stay closed before reaching the threshold (after {i} failures)"
            );
            assert!(!tool_health(&pool, "github").await.is_open());
        }

        // The threshold-th consecutive failure opens the breaker.
        pool.record_failure_for(
            "github",
            UpstreamCapability::Tools,
            "tool listing failed (threshold hit)",
        )
        .await;

        let opened = tool_health(&pool, "github").await;
        assert!(
            opened.is_open(),
            "breaker must be open after CIRCUIT_BREAKER_THRESHOLD failures"
        );
        assert!(!opened.is_routable(), "open breaker must not be routable");
        assert!(matches!(
            opened,
            UpstreamHealth::Unhealthy {
                consecutive_failures
            } if consecutive_failures == types::CIRCUIT_BREAKER_THRESHOLD
        ));

        // A single success closes/recovers the breaker.
        pool.record_success_for("github", UpstreamCapability::Tools)
            .await;

        let recovered = tool_health(&pool, "github").await;
        assert!(
            matches!(recovered, UpstreamHealth::Healthy),
            "success must reset breaker to Healthy"
        );
        assert!(recovered.is_routable());
        assert!(!recovered.is_open());
        // Last-error and unhealthy-since are cleared on recovery.
        assert_eq!(pool.upstream_tool_last_error("github").await, None);
    }

    #[test]
    fn reprobe_quarantine_backs_off_exponentially_and_caps() {
        assert_eq!(
            types::reprobe_interval_for_failures(types::CIRCUIT_BREAKER_THRESHOLD),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            types::reprobe_interval_for_failures(types::CIRCUIT_BREAKER_THRESHOLD + 1),
            std::time::Duration::from_mins(1)
        );
        assert_eq!(
            types::reprobe_interval_for_failures(types::CIRCUIT_BREAKER_THRESHOLD + 2),
            std::time::Duration::from_mins(2)
        );
        assert_eq!(
            types::reprobe_interval_for_failures(u32::MAX),
            types::MAX_REPROBE_INTERVAL
        );
    }

    #[tokio::test]
    async fn failed_reprobe_resets_and_extends_quarantine_clock() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("broken");
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), HashMap::new());
        pool.catalog
            .write()
            .await
            .insert("broken".to_string(), entry);

        for _ in 0..types::CIRCUIT_BREAKER_THRESHOLD {
            pool.record_failure_for("broken", UpstreamCapability::Tools, "down")
                .await;
        }
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("broken").unwrap();
            entry.tool_unhealthy_since = Instant::now().checked_sub(
                types::reprobe_interval_for_failures(types::CIRCUIT_BREAKER_THRESHOLD),
            );
        }
        assert!(pool.should_reprobe("broken").await);

        pool.record_failure_for("broken", UpstreamCapability::Tools, "still down")
            .await;

        assert!(
            !pool.should_reprobe("broken").await,
            "a failed reprobe must start a fresh, longer quarantine window"
        );
    }

    #[tokio::test]
    async fn per_upstream_bulkhead_is_independent_and_bounded() {
        let pool = UpstreamPool::new().with_upstream_call_concurrency(1);
        let alpha = pool.acquire_upstream_call_permit("alpha").await.unwrap();

        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                pool.acquire_upstream_call_permit("alpha")
            )
            .await
            .is_err(),
            "a second call to the same upstream must wait for its permit"
        );

        let beta = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            pool.acquire_upstream_call_permit("beta"),
        )
        .await
        .expect("another upstream has an independent bulkhead")
        .unwrap();
        drop(beta);
        drop(alpha);

        let _permit = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            pool.acquire_upstream_call_permit("alpha"),
        )
        .await
        .expect("released permit becomes available")
        .unwrap();
    }
}
