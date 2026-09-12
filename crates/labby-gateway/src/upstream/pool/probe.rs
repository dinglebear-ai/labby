//! Background reprobe scheduling and the reprobe/heartbeat engine.
//!
//! `ensure_probe_task` spawns a per-upstream background loop that periodically
//! calls `reprobe_upstream` (heartbeat existing connections, reconnect on
//! failure) with jittered backoff. Manager and discovery paths schedule tasks;
//! on-demand readiness also uses the reprobe engine.

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::transport::websocket::{jitter_delay, reprobe_backoff};
use super::super::types;
use super::super::types::UpstreamCapability;
use super::super::types::UpstreamRuntimeOwner;
use super::UpstreamPool;
use super::catalog_pagination;
use super::connect::connect_upstream_with_client;
use super::connect::stable_jitter_seed;
use super::helpers::{
    AUTH_FAILURE_REPROBE_ATTEMPT_FLOOR, DISCOVERY_TIMEOUT, auth_error_should_backoff_aggressively,
    classify_upstream_error, upstream_name_is_uri_safe, upstream_transport,
};
use super::skills_list::peer_declares_skills;
use super::tools::MAX_UPSTREAM_TOOLS;
use super::validate::validate_upstream_config;

#[cfg(any(test, feature = "testkit"))]
static PROBE_TASK_SCHEDULE_COUNTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

impl UpstreamPool {
    pub(crate) async fn ensure_probe_task(&self, config: UpstreamConfig) {
        if !config.enabled || config.oauth.is_some() {
            return;
        }

        if !upstream_name_is_uri_safe(&config.name) || validate_upstream_config(&config).is_err() {
            tracing::warn!(upstream = %config.name, "skipping recovery for invalid upstream configuration");
            return;
        }

        let mut tasks = self.probe_tasks.write().await;
        // Disable drains this map under the same lock. Check the flag only
        // after acquiring it so a waiter cannot resurrect a cancelled task.
        if !self
            .auto_reconnect
            .load(std::sync::atomic::Ordering::Relaxed)
            || tasks.contains_key(&config.name)
        {
            return;
        }
        let cancel = CancellationToken::new();
        tasks.insert(config.name.clone(), cancel.clone());
        drop(tasks);
        #[cfg(any(test, feature = "testkit"))]
        {
            let mut counts = PROBE_TASK_SCHEDULE_COUNTS
                .lock()
                .expect("probe task schedule counts lock");
            *counts.entry(config.name.clone()).or_default() += 1;
        }
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "scheduled",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(&config),
            "upstream reprobe scheduled"
        );

        let pool = self.clone();
        tokio::spawn(async move {
            let mut attempt = 0_u32;
            loop {
                let sleep_for = reprobe_sleep_for(&config.name, attempt);
                tracing::debug!(
                    surface = "dispatch",
                    service = "upstream.pool",
                    action = "upstream.reprobe",
                    event = "sleep",
                    operation = "health",
                    upstream = %config.name,
                    transport = upstream_transport(&config),
                    attempt,
                    sleep_ms = sleep_for.as_millis(),
                    "upstream reprobe sleep scheduled"
                );
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "cancelled",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            "upstream reprobe cancelled"
                        );
                        break;
                    },
                    _ = tokio::time::sleep(sleep_for) => {}
                }

                let permit = tokio::select! {
                    _ = cancel.cancelled() => break,
                    permit = pool.reprobe_semaphore.clone().acquire_owned() => {
                        match permit {
                            Ok(permit) => permit,
                            Err(_) => break,
                        }
                    }
                };
                let reprobe_started = Instant::now();
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    result = pool.reprobe_upstream(&config, None, None) => result,
                };
                match result {
                    Ok(true) => {
                        tracing::info!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "finish",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            changed = true,
                            "upstream reprobe succeeded"
                        );
                        attempt = 0;
                    }
                    Ok(false) => {
                        tracing::debug!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "finish",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            changed = false,
                            "upstream reprobe skipped"
                        );
                    }
                    Err(error) => {
                        let kind = classify_upstream_error(&error.to_string());
                        attempt = attempt.saturating_add(1);
                        if auth_error_should_backoff_aggressively(kind) {
                            attempt = attempt.max(AUTH_FAILURE_REPROBE_ATTEMPT_FLOOR);
                        }
                        tracing::warn!(
                            surface = "dispatch",
                            service = "upstream.pool",
                            action = "upstream.reprobe",
                            event = "error",
                            operation = "health",
                            upstream = %config.name,
                            transport = upstream_transport(&config),
                            attempt,
                            elapsed_ms = reprobe_started.elapsed().as_millis(),
                            kind,
                            error = %error,
                            "upstream reprobe failed"
                        );
                    }
                }
                drop(permit);
            }
        });
    }

    #[cfg(any(test, feature = "testkit"))]
    pub fn reset_probe_task_schedule_count_for_tests(upstream: &str) {
        PROBE_TASK_SCHEDULE_COUNTS
            .lock()
            .expect("probe task schedule counts lock")
            .remove(upstream);
    }

    #[cfg(any(test, feature = "testkit"))]
    pub fn probe_task_schedule_count_for_tests(upstream: &str) -> usize {
        PROBE_TASK_SCHEDULE_COUNTS
            .lock()
            .expect("probe task schedule counts lock")
            .get(upstream)
            .copied()
            .unwrap_or_default()
    }

    pub(super) async fn reprobe_upstream(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        runtime_owner: Option<&UpstreamRuntimeOwner>,
    ) -> anyhow::Result<bool> {
        let started = Instant::now();
        tracing::debug!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "start",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(config),
            "upstream reprobe start"
        );
        let existing_peer = {
            let connections = self.connections.read().await;
            connections
                .get(&config.name)
                .map(|connection| connection.peer.clone())
        };

        if let Some(peer) = existing_peer {
            match catalog_pagination::list_tools(&peer, DISCOVERY_TIMEOUT, MAX_UPSTREAM_TOOLS).await
            {
                Ok(tools) => {
                    self.replace_catalog_tools(config, tools, Some(peer_declares_skills(&peer)))
                        .await;
                    self.record_success_for(&config.name, UpstreamCapability::Tools)
                        .await;
                    tracing::info!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.finish",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream heartbeat succeeded"
                    );
                    return Ok(true);
                }
                Err(error) => {
                    self.record_failure_for(
                        &config.name,
                        UpstreamCapability::Tools,
                        format!("upstream heartbeat failed: {}", error.bounded_text()),
                    )
                    .await;
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "upstream.reprobe",
                        event = "heartbeat.error",
                        operation = "health",
                        upstream = %config.name,
                        transport = upstream_transport(config),
                        elapsed_ms = started.elapsed().as_millis(),
                        kind = error.kind(),
                        error = %error.bounded_text(),
                        "upstream heartbeat failed"
                    );
                }
            }
        } else {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "empty",
                operation = "health",
                upstream = %config.name,
                transport = upstream_transport(config),
                elapsed_ms = started.elapsed().as_millis(),
                kind = "upstream_not_connected",
                "upstream reprobe found no existing connection"
            );
        }

        let stale_connection = self.remove_connection_binding(&config.name).await;
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.reprobe.reconnect")
                .await;
        }
        // A skill catalog belongs to the connection that produced it. The peer
        // is being replaced, so anything cached against the old one must go —
        // otherwise a read could be routed against a manifest the reconnected
        // upstream never published.
        self.invalidate_upstream_skills(&config.name).await;

        let subject = config.oauth.as_ref().and(oauth_subject);
        let runtime_owner = runtime_owner.or(self.runtime_owner.as_ref());
        let (conn, tools) = connect_upstream_with_client(
            config,
            subject,
            self.oauth_client_cache.as_ref(),
            self.runtime_origin.as_deref(),
            runtime_owner,
            Some(&self.shared_http_client),
        )
        .await?;
        let supports_skills = peer_declares_skills(&conn.peer);
        self.install_connected_tools(config, conn, tools, Some(supports_skills))
            .await?;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.reprobe",
            event = "reconnect.finish",
            operation = "health",
            upstream = %config.name,
            transport = upstream_transport(config),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream reprobe reconnect succeeded"
        );
        Ok(true)
    }
}

fn reprobe_sleep_for(upstream: &str, attempt: u32) -> std::time::Duration {
    let base = if attempt == 0 {
        types::REPROBE_INTERVAL
    } else {
        reprobe_backoff(attempt)
    };
    // Healthy upstreams used to all wake on the exact same first interval.
    // Stable jitter on every interval spreads fleet work while remaining
    // deterministic for a given upstream/attempt pair.
    jitter_delay(base, stable_jitter_seed(upstream, attempt))
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::*;
    use super::*;

    #[tokio::test]
    async fn ensure_probe_task_registers_before_returning() {
        let pool = UpstreamPool::new().with_auto_reconnect(true);
        let config = named_test_upstream_config("probe-race");
        UpstreamPool::reset_probe_task_schedule_count_for_tests("probe-race");

        pool.ensure_probe_task(config).await;

        assert_eq!(
            UpstreamPool::probe_task_schedule_count_for_tests("probe-race"),
            1
        );
        assert_eq!(pool.probe_tasks.read().await.len(), 1);

        pool.drain_for_swap("probe.registration.test").await;
        assert!(pool.probe_tasks.read().await.is_empty());
    }

    #[tokio::test]
    async fn recovery_does_not_schedule_invalid_upstreams() {
        let pool = UpstreamPool::new().with_auto_reconnect(true);
        let mut bind_all = named_test_upstream_config("bind-all-recovery");
        bind_all.url = Some("http://0.0.0.0:8000/mcp".into());
        let unsafe_name = named_test_upstream_config("unsafe/name");
        let mut disabled = named_test_upstream_config("disabled-recovery");
        disabled.enabled = false;
        for config in [bind_all, unsafe_name, disabled] {
            pool.ensure_probe_task(config).await;
        }
        assert!(pool.probe_tasks.read().await.is_empty());
    }

    #[tokio::test]
    async fn disabling_recovery_prevents_waiting_registration() {
        let pool = UpstreamPool::new().with_auto_reconnect(true);
        let tasks = pool.probe_tasks.write().await;
        let config = named_test_upstream_config("disabled-during-registration");
        let registration = pool.ensure_probe_task(config);
        tokio::pin!(registration);
        assert!(futures::poll!(&mut registration).is_pending());
        pool.set_auto_reconnect(false);
        drop(tasks);
        registration.await;
        assert!(pool.probe_tasks.read().await.is_empty());
    }

    #[tokio::test]
    async fn recovery_rechecks_enabled_state_before_draining_waiting_tasks() {
        let pool = UpstreamPool::new();
        let mut tasks = pool.probe_tasks.write().await;
        let cancel = CancellationToken::new();
        let reconciliation = pool.ensure_recovery_tasks(&[]);
        tokio::pin!(reconciliation);
        assert!(futures::poll!(&mut reconciliation).is_pending());
        // A newer enable operation supersedes the waiting disable operation.
        pool.set_auto_reconnect(true);
        tasks.insert("newer-recovery".into(), cancel.clone());
        drop(tasks);
        reconciliation.await;
        assert!(!cancel.is_cancelled());
        assert_eq!(pool.probe_tasks.read().await.len(), 1);
        pool.drain_for_swap("test.recovery_enable_wins").await;
    }

    #[tokio::test(start_paused = true)]
    async fn disabling_recovery_cancels_an_active_probe() {
        let pool = UpstreamPool::new().with_auto_reconnect(true);
        let config = named_test_upstream_config("cancel-active-reprobe");
        // Hold the connection map so the heartbeat stalls after it obtains its
        // concurrency permit, before it can inspect or replace a transport.
        let connections = pool.connections.write().await;
        let permits = pool.reprobe_semaphore.available_permits();
        pool.ensure_probe_task(config).await;
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_mins(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(pool.reprobe_semaphore.available_permits(), permits - 1);
        pool.set_auto_reconnect(false);
        pool.ensure_recovery_tasks(&[]).await;
        tokio::task::yield_now().await;
        assert_eq!(pool.reprobe_semaphore.available_permits(), permits);
        assert!(pool.probe_tasks.read().await.is_empty());
        drop(connections);
    }

    #[tokio::test]
    async fn auto_reconnect_option_controls_recovery_task_schedule() {
        let upstream = "auto-reconnect-option";
        let config = named_test_upstream_config(upstream);
        UpstreamPool::reset_probe_task_schedule_count_for_tests(upstream);

        let disabled = UpstreamPool::new();
        disabled
            .ensure_recovery_tasks(std::slice::from_ref(&config))
            .await;
        assert_eq!(
            UpstreamPool::probe_task_schedule_count_for_tests(upstream),
            0
        );

        let enabled = UpstreamPool::new().with_auto_reconnect(true);
        enabled
            .ensure_recovery_tasks(std::slice::from_ref(&config))
            .await;
        assert_eq!(
            UpstreamPool::probe_task_schedule_count_for_tests(upstream),
            1
        );
        assert_eq!(enabled.probe_tasks.read().await.len(), 1);

        enabled.set_auto_reconnect(false);
        enabled
            .ensure_recovery_tasks(std::slice::from_ref(&config))
            .await;
        assert!(enabled.probe_tasks.read().await.is_empty());

        enabled.drain_for_swap("test.auto_reconnect").await;
    }

    #[tokio::test]
    async fn disabled_upstream_reprobe_is_inert() {
        let pool = UpstreamPool::new();
        let mut config = test_upstream_config();
        config.enabled = false;
        config.command = Some("definitely-not-spawned".to_string());

        let result = pool
            .reprobe_tools_for_upstream(&config)
            .await
            .expect("disabled reprobe should not error");

        assert!(!result);
        assert!(pool.find_tool("anything").await.is_none());
    }

    #[test]
    fn healthy_reprobe_intervals_are_stably_jittered() {
        let alpha = reprobe_sleep_for("alpha", 0);
        assert_eq!(alpha, reprobe_sleep_for("alpha", 0));
        assert_ne!(alpha, reprobe_sleep_for("bravo", 0));
        assert_ne!(alpha, types::REPROBE_INTERVAL);
    }

    #[tokio::test]
    async fn reprobe_gate_caps_peak_fleet_concurrency() {
        let pool = UpstreamPool::new();
        let limit = super::super::helpers::upstream_discovery_concurrency(None);
        let mut held = Vec::new();
        for _ in 0..limit {
            held.push(
                pool.reprobe_semaphore
                    .clone()
                    .try_acquire_owned()
                    .expect("permit below configured limit"),
            );
        }
        assert!(pool.reprobe_semaphore.try_acquire().is_err());
        drop(held.pop());
        assert!(pool.reprobe_semaphore.try_acquire().is_ok());
    }

    #[test]
    fn observability_source_covers_pool_acquire_reprobe_and_drain_events() {
        // The pool was split into `pool.rs` + the `pool/` child modules, so the
        // observability instrumentation now lives across several files. Scan the
        // whole upstream-pool source tree (pool.rs + every pool/*.rs) so this
        // guard stays robust as code relocates between modules. A missing string
        // here means a real dropped-instrumentation regression — never delete an
        // assertion to make this test pass; add the file the string moved into.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/upstream");
        let mut source =
            std::fs::read_to_string(format!("{dir}/pool.rs")).expect("read pool.rs source");
        let pool_dir = format!("{dir}/pool");
        if let Ok(entries) = std::fs::read_dir(&pool_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    source.push_str(
                        &std::fs::read_to_string(&path).expect("read pool child module source"),
                    );
                }
            }
        }
        for expected in [
            "action = \"upstream.acquire\"",
            "elapsed_ms",
            "pool_size",
            "connection_count",
            "action = \"upstream.reprobe\"",
            "operation = \"health\"",
            "action = \"upstream.pool.drain\"",
            "cancelled_probe_count",
            "kind = \"upstream_pool_empty\"",
            "kind = \"upstream_not_connected\"",
            "fn log_upstream_request_start",
            "fn log_upstream_request_finish",
            "fn log_upstream_request_error",
            "action = \"upstream.request\"",
        ] {
            assert!(
                source.contains(expected),
                "missing upstream pool observability field `{expected}`"
            );
        }
    }
}
