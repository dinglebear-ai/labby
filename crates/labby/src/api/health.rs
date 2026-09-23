//! Liveness and readiness probes.
//!
//! `GET /health` — process is up. Always returns 200.
//! `GET /ready`  — process is ready to serve traffic. Returns 503 until all
//!                  readiness predicates are met.
//!
//! ## Readiness predicates
//!
//! 1. **Registry non-empty** — at least one service is registered in the tool
//!    registry. This passes immediately after `build_default_registry()` runs
//!    during `AppState` construction; a zero-service registry indicates a build
//!    misconfiguration rather than a transient boot condition.
//!
//! 2. **Gateway pool present** — when a gateway manager is wired into
//!    `AppState`, the upstream pool must have completed at least one successful
//!    load (i.e. `current_pool()` is `Some`). When no manager is wired this
//!    predicate is skipped (not every deployment uses the gateway).
//!
//! 3. **Core provider reachable** — trusted-host mode is not ready unless its
//!    configured private Core provider answers the negotiated protocol health.
//!
//! ## Degraded state
//!
//! When every predicate passes but an optional subsystem is unavailable (a
//! recorded startup degradation or a non-ready access store), `/ready` returns
//! 200 with `status: "degraded"` and a `degraded` list of stable codes. It
//! never returns plain `ready` in that state. Detail is not exposed on these
//! public probes; `doctor system.checks` reports it.
//!
//! **FLAG for AUTH agent:** `AppState` was not modified. Readiness is derived
//! from *existing* fields (`registry`, `gateway_manager`). If AUTH needs an
//! explicit `ready: AtomicBool` flag set at a precise moment during serve
//! start-up, that can replace predicate 1 without a breaking layout change.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

use super::state::AppState;

/// Response body for health/readiness probes.
#[cfg_attr(feature = "api-docs", derive(utoipa::ToSchema))]
#[derive(Debug, serde::Serialize)]
pub struct HealthResponse {
    /// Status string: `"ok"` for liveness; `"ready"`, `"degraded"`, or
    /// `"not_ready"` for readiness.
    pub status: String,
    /// Process role: `"master"` or `"node"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// OS process ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Seconds since the server started accepting requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_s: Option<u64>,
    /// Human-readable list of predicates not yet satisfied.
    /// Present only on 503 responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Vec<String>>,
    /// Stable codes of subsystems that are degraded while the process still
    /// serves traffic (HTTP 200, `status: "degraded"`). Codes only; detail is
    /// served by the authenticated `doctor system.checks` action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<Vec<String>>,
    /// Sealed runtime capability profile. Integrated mode reports a distinct
    /// value so Core can reject a standalone/all-features artifact at
    /// readiness time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_profile: Option<&'static str>,
    /// Private provider protocol accepted by the integrated profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_protocol: Option<&'static str>,
    /// Whether the managed Depot authority projection is ready. Present only
    /// when managed mode is configured. These probes are public, so the
    /// structured readiness detail (lag, gap, watermark, key generation,
    /// pending reason) is served on the authenticated `GET /v1/depot/status`
    /// route instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority_projection_ready: Option<bool>,
}

fn authority_projection_ready() -> Option<bool> {
    crate::dispatch::depot::authority_projection::projection_readiness()
        .map(|readiness| readiness.ready)
}

/// Liveness probe. Returns 200 as long as the process is running.
pub async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let uptime_s = state.server_start.elapsed().as_secs();
    let integrated = state.trusted_host_verifier.is_some();
    Json(HealthResponse {
        status: "ok".to_string(),
        mode: Some(if integrated {
            "integrated-gateway".to_string()
        } else {
            "gateway-host".to_string()
        }),
        pid: Some(std::process::id()),
        uptime_s: Some(uptime_s),
        pending: None,
        degraded: None,
        capability_profile: Some(if integrated {
            "unraid-core-integrated-v1"
        } else {
            "standalone-gateway-v1"
        }),
        provider_protocol: integrated.then_some("1.0"),
        authority_projection_ready: authority_projection_ready(),
    })
}

/// Stable codes for subsystems that are degraded but do not block serving.
///
/// - recorded startup degradations (for example `artifacts_unavailable`);
/// - `access_setup_pending`: the access store exists but is uninitialized or
///   has a prepared owner bootstrap/migration that has not completed;
/// - `access_blocked`: the access store is insecure, corrupt, newer-schema,
///   locked, read-only, or unavailable.
///
/// A missing access store is not reported: it is the normal state of an
/// installation that has not enabled access control.
async fn degraded_subsystems(state: &AppState) -> Vec<String> {
    use crate::access::{AccessRuntimeStatus, AccessSetupReason};

    let mut degraded: Vec<String> = state
        .subsystem_health
        .degraded_codes()
        .into_iter()
        .map(str::to_owned)
        .collect();
    match state.access_runtime.status().await {
        AccessRuntimeStatus::Ready
        | AccessRuntimeStatus::SetupRequired(AccessSetupReason::Missing) => {}
        AccessRuntimeStatus::SetupRequired(
            AccessSetupReason::Uninitialized | AccessSetupReason::ProofPending,
        ) => {
            degraded.push("access_setup_pending".to_owned());
        }
        AccessRuntimeStatus::Blocked(_) => degraded.push("access_blocked".to_owned()),
    }
    degraded
}

/// Readiness probe. Returns 503 until all predicates are satisfied. Once they
/// are, returns 200 with `status: "ready"`, or `status: "degraded"` plus a
/// `degraded` code list when an optional subsystem is unavailable. Degraded
/// stays 200 so rolling deploy gates keep a serving process; gates that must
/// refuse degraded deploys check `status == "ready"`.
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let mut pending: Vec<String> = Vec::new();

    // Predicate 1: registry must have at least one service registered.
    //
    // `build_default_registry()` always populates the registry before
    // `AppState::from_registry` completes, so this predicate passes in all
    // normal deployments.  A zero-service registry indicates a build or
    // feature-flag misconfiguration.
    if state.registry.services().is_empty() {
        pending.push("no services registered in tool registry".to_string());
    }

    // File Stash recovers persisted state asynchronously. Do not advertise
    // readiness while its enabled routes still refuse requests. An omitted
    // service (including unsupported platforms) must not gate the process.
    if state.registry.service("stash").is_some()
        && state.file_stash_runtime.status().await != crate::file_stash::FileStashStatus::Ready
    {
        pending.push("File Stash is not ready".to_string());
    }

    if let Some(reason) =
        crate::dispatch::depot::authority_projection::managed_projection_readiness_pending()
    {
        pending.push(reason);
    }

    // Predicate 2: when a gateway manager is wired, the pool must be present.
    //
    // The pool is `None` until `gateway.reload` completes its first successful
    // upstream discovery pass. Orchestrators (Kubernetes, Compose health-checks)
    // should wait for this before routing traffic so that MCP tool listings are
    // non-empty on first request.
    #[cfg(feature = "gateway")]
    {
        if let Some(manager) = &state.gateway_manager {
            if manager.current_pool().await.is_none() {
                pending.push("gateway pool not yet initialised".to_string());
            }

            if state.trusted_host_verifier.is_some() {
                let core_provider_health = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    manager.core_provider_health(),
                )
                .await;
                if !matches!(core_provider_health, Ok(Ok(()))) {
                    pending.push("Core provider unavailable or incompatible".to_string());
                }
            }
        } else if state.trusted_host_verifier.is_some() {
            pending.push("integrated gateway manager is unavailable".to_string());
        }
    }

    if pending.is_empty() {
        let degraded = degraded_subsystems(&state).await;
        (
            StatusCode::OK,
            Json(HealthResponse {
                status: if degraded.is_empty() {
                    "ready"
                } else {
                    "degraded"
                }
                .to_string(),
                mode: None,
                pid: None,
                uptime_s: None,
                pending: None,
                degraded: (!degraded.is_empty()).then_some(degraded),
                capability_profile: None,
                provider_protocol: None,
                authority_projection_ready: authority_projection_ready(),
            }),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "not_ready".to_string(),
                mode: None,
                pid: None,
                uptime_s: None,
                pending: Some(pending),
                degraded: None,
                capability_profile: None,
                provider_protocol: None,
                authority_projection_ready: authority_projection_ready(),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn integrated_health_self_reports_the_sealed_profile() {
        let verifier = Arc::new(labby_auth::trusted_host::TrustedHostVerifier::new(1, []));
        let response = health(State(AppState::new().with_trusted_host_verifier(verifier))).await;

        assert_eq!(response.0.mode.as_deref(), Some("integrated-gateway"));
        assert_eq!(
            response.0.capability_profile,
            Some("unraid-core-integrated-v1")
        );
        assert_eq!(response.0.provider_protocol, Some("1.0"));
    }

    /// The public probes never carry projection detail; a non-managed process
    /// omits the field entirely rather than advertising internal state.
    #[tokio::test]
    async fn public_probes_expose_only_a_readiness_boolean() {
        let response = health(State(AppState::new())).await;
        let body = serde_json::to_value(&response.0).unwrap();
        assert!(body.get("authority_projection").is_none());
        assert!(
            body.get("authority_projection_ready")
                .is_none_or(serde_json::Value::is_boolean)
        );
        for key in ["watermark", "key_generation", "lag", "gap", "pending"] {
            assert!(body.get(key).is_none(), "{key} leaked on /health");
        }
    }

    /// Disabled Stash must not gate readiness even though its runtime is blocked.
    #[tokio::test]
    async fn ready_returns_200_when_no_gateway_manager() {
        let mut registry = crate::registry::ToolRegistry::new();
        for service in AppState::new().registry.services() {
            if service.name != "stash" {
                registry.register(service.clone());
            }
        }
        let state = AppState::from_registry(registry);
        // Sanity-check our predicate: registry must be non-empty with --all-features.
        assert!(
            !state.registry.services().is_empty(),
            "AppState::new() must populate the registry; got 0 services"
        );
        let resp = ready(State(state)).await.into_response();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "/ready must return 200 when no gateway manager is wired"
        );
    }

    fn ready_state_without_stash() -> AppState {
        let mut registry = crate::registry::ToolRegistry::new();
        for service in AppState::new().registry.services() {
            if service.name != "stash" {
                registry.register(service.clone());
            }
        }
        AppState::from_registry(registry)
            .with_subsystem_health(Arc::new(crate::runtime_health::SubsystemHealth::default()))
    }

    async fn ready_body(state: AppState) -> (StatusCode, serde_json::Value) {
        let response = ready(State(state)).await.into_response();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn ready_reports_plain_ready_only_when_nothing_is_degraded() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        // A missing access store is the normal pre-access-control state.
        let path = directory.path().canonicalize().unwrap().join("access.db");
        let runtime = Arc::new(crate::access::AccessRuntime::initialize(path).await);
        let state = ready_state_without_stash().with_access_runtime(runtime);

        let (status, body) = ready_body(state).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ready");
        assert!(body.get("degraded").is_none());
    }

    #[tokio::test]
    async fn ready_reports_degraded_startup_subsystems_without_detail() {
        let health = Arc::new(crate::runtime_health::SubsystemHealth::default());
        health.record_degraded(
            crate::runtime_health::ARTIFACTS_UNAVAILABLE,
            "configure Skill Library exact-source adapters: secret-free cause".into(),
        );
        let state = ready_state_without_stash().with_subsystem_health(health);

        let (status, body) = ready_body(state).await;
        assert_eq!(status, StatusCode::OK, "degraded keeps serving");
        assert_eq!(body["status"], "degraded");
        let degraded = body["degraded"].as_array().unwrap();
        assert!(degraded.iter().any(|code| code == "artifacts_unavailable"));
        assert!(!body.to_string().contains("exact-source"), "detail leaked");
    }

    #[tokio::test]
    async fn ready_reports_a_blocked_access_store_as_degraded() {
        // `AppState::new()` wires the conservative blocked-unavailable runtime.
        let (status, body) = ready_body(ready_state_without_stash()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "degraded");
        assert_eq!(body["degraded"], serde_json::json!(["access_blocked"]));
    }

    #[tokio::test]
    async fn ready_refuses_enabled_blocked_stash() {
        // The static registry lets every host exercise the enabled-service
        // contract; production omits Stash on unsupported platforms.
        let state = AppState::from_registry(crate::registry::build_docs_registry());
        assert!(state.registry.service("stash").is_some());
        let response = ready(State(state)).await.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn integrated_ready_fails_closed_without_the_gateway_manager() {
        let verifier = Arc::new(labby_auth::trusted_host::TrustedHostVerifier::new(1, []));
        let state = AppState::new().with_trusted_host_verifier(verifier);

        let response = ready(State(state)).await.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// When a gateway manager is wired but the pool has not yet loaded, `/ready`
    /// must return 503 with `pending` naming the unsatisfied predicate.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn ready_returns_503_when_gateway_pool_absent() {
        use std::sync::Arc;

        use crate::dispatch::gateway::config_store::test_gateway_manager;
        use crate::dispatch::gateway::manager::GatewayRuntimeHandle;

        let runtime = GatewayRuntimeHandle::default();
        let directory = tempfile::tempdir().expect("tempdir");
        // Pool starts as None — manager is wired but pool not yet loaded.
        let manager = Arc::new(test_gateway_manager(
            directory.path().join("config.toml"),
            runtime,
        ));
        let state = AppState::new().with_gateway_manager(manager);

        let resp = ready(State(state)).await.into_response();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "/ready must return 503 when gateway pool is not yet initialised"
        );
    }
}
