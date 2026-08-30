//! Native per-upstream OAuth orchestration for the stdio transport.
//!
//! Stdio has no inbound HTTP listener for an authorization-server redirect.
//! This module supplies the missing host-side piece around the reusable
//! `labby-auth` OAuth manager: a loopback-only callback listener, a browser
//! launcher, and single-flight reauthorization for each `(upstream, subject)`.
//!
//! The listener is deliberately bound to `127.0.0.1`, never to a configured
//! network interface. OAuth state and authorization codes are accepted only
//! when the encrypted SQLite state store identifies the configured upstream,
//! the shared stdio subject, and an in-process pending flow.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};
use axum::{Router, http::StatusCode, routing::get};
use dashmap::DashMap;
use labby_auth::config::AuthConfig;
use labby_auth::sqlite::SqliteStore;
use labby_auth::upstream::cache::{OauthReauthFuture, OauthReauthHandler};
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_auth::upstream::runtime::{
    UpstreamOauthRuntime, build_upstream_oauth_runtime_with_redirect,
};
use labby_auth::upstream::types::OauthError;
use labby_runtime::gateway_config::UpstreamConfig;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{Mutex, Notify};
use tokio::time::{Instant, timeout_at};
use url::Url;

const CALLBACK_PATH: &str = "/auth/upstream/callback";
const DEFAULT_CALLBACK_PORT: u16 = 0;
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_mins(5);

/// Build a native OAuth runtime for `labby mcp`.
///
/// The callback listener is created before the OAuth managers so the exact
/// loopback redirect URI is fixed into dynamic registration and every manager.
/// A port can be pinned with `LABBY_STDIO_OAUTH_CALLBACK_PORT`; zero (the
/// default) asks the OS for an ephemeral loopback port.
pub async fn build_stdio_upstream_oauth_runtime(
    upstreams: &[UpstreamConfig],
    auth_config: &AuthConfig,
    encryption_key_raw: Option<&str>,
) -> Result<Option<UpstreamOauthRuntime>> {
    if !upstreams.iter().any(|upstream| upstream.oauth.is_some()) {
        return Ok(None);
    }
    anyhow::ensure!(
        encryption_key_raw.is_some_and(|value| !value.trim().is_empty()),
        "LABBY_OAUTH_ENCRYPTION_KEY is required when native stdio upstream OAuth is configured"
    );

    let port = match std::env::var("LABBY_STDIO_OAUTH_CALLBACK_PORT") {
        Ok(raw) => raw.parse::<u16>().with_context(
            || "LABBY_STDIO_OAUTH_CALLBACK_PORT must be a valid TCP port (0 means ephemeral)",
        )?,
        Err(_) => DEFAULT_CALLBACK_PORT,
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .with_context(|| format!("bind stdio OAuth callback listener on 127.0.0.1:{port}"))?;
    let address = listener
        .local_addr()
        .context("read stdio OAuth callback listener address")?;
    let redirect_uri = format!("http://127.0.0.1:{}{CALLBACK_PATH}", address.port());

    let Some(mut runtime) = build_upstream_oauth_runtime_with_redirect(
        upstreams,
        auth_config,
        encryption_key_raw,
        redirect_uri,
    )
    .await?
    else {
        return Ok(None);
    };

    let coordinator = StdioOauthCoordinator::new(runtime.managers.clone(), runtime.sqlite.clone());
    coordinator.spawn_callback_server(listener);
    runtime.cache = runtime
        .cache
        .with_reauth_handler(coordinator.reauth_handler());

    tracing::info!(
        subsystem = "gateway_client",
        phase = "oauth.stdio.ready",
        bind_host = "127.0.0.1",
        bind_port = address.port(),
        oauth_upstream_count = runtime.managers.len(),
        "native stdio upstream OAuth callback ready"
    );
    Ok(Some(runtime))
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
}

struct PendingFlow {
    upstream: String,
    notify: Arc<Notify>,
}

struct FlowOutcome {
    success: bool,
    message: String,
}

/// Removes every in-memory and durable artifact when an authorization future
/// is dropped, including Tokio task cancellation while it is awaiting the
/// browser callback.
struct PendingFlowGuard<'a> {
    coordinator: &'a StdioOauthCoordinator,
    state: String,
    armed: bool,
}

impl<'a> PendingFlowGuard<'a> {
    fn new(coordinator: &'a StdioOauthCoordinator, state: String) -> Self {
        Self {
            coordinator,
            state,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingFlowGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let pending_removed = self.coordinator.pending.remove(&self.state).is_some();
        self.coordinator.outcomes.remove(&self.state);
        let sqlite = self.coordinator.sqlite.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            match sqlite.delete_upstream_oauth_state(&state).await {
                Ok(()) => tracing::info!(
                    subsystem = "gateway_client",
                    phase = "oauth.stdio.pending.cleanup",
                    pending_removed,
                    "cleaned abandoned native stdio OAuth flow"
                ),
                Err(error) => tracing::warn!(
                    subsystem = "gateway_client",
                    phase = "oauth.stdio.pending.cleanup",
                    kind = error.kind(),
                    pending_removed,
                    "failed to clean abandoned native stdio OAuth flow"
                ),
            }
        });
    }
}

/// Host-owned interactive OAuth coordinator for one stdio process.
struct StdioOauthCoordinator {
    managers: Arc<DashMap<String, UpstreamOauthManager>>,
    sqlite: SqliteStore,
    pending: DashMap<String, PendingFlow>,
    outcomes: DashMap<String, FlowOutcome>,
    locks: DashMap<(String, String), Arc<Mutex<()>>>,
}

impl StdioOauthCoordinator {
    fn new(managers: Arc<DashMap<String, UpstreamOauthManager>>, sqlite: SqliteStore) -> Arc<Self> {
        Arc::new(Self {
            managers,
            sqlite,
            pending: DashMap::new(),
            outcomes: DashMap::new(),
            locks: DashMap::new(),
        })
    }

    fn reauth_handler(self: &Arc<Self>) -> OauthReauthHandler {
        let coordinator = Arc::clone(self);
        Arc::new(move |upstream, subject| {
            let coordinator = Arc::clone(&coordinator);
            let future: OauthReauthFuture =
                Box::pin(async move { coordinator.reauthorize(&upstream, &subject).await });
            future
        })
    }

    fn spawn_callback_server(self: &Arc<Self>, listener: TcpListener) {
        let router = Router::new()
            .route(CALLBACK_PATH, get(stdio_oauth_callback))
            .with_state(Arc::clone(self));
        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router).await {
                tracing::warn!(
                    subsystem = "gateway_client",
                    phase = "oauth.stdio.callback.stop",
                    error = %error,
                    "native stdio OAuth callback listener stopped"
                );
            }
        });
    }

    async fn reauthorize(&self, upstream: &str, subject: &str) -> Result<(), OauthError> {
        const SHARED_SUBJECT: &str = "gateway";
        if subject != SHARED_SUBJECT {
            return Err(OauthError::Internal(
                "native stdio OAuth only accepts the shared gateway subject".to_string(),
            ));
        }
        let manager = self
            .managers
            .get(upstream)
            .map(|entry| entry.clone())
            .ok_or_else(|| {
                OauthError::Internal(format!(
                    "no OAuth manager registered for upstream '{upstream}'"
                ))
            })?;

        let lock_key = (upstream.to_string(), subject.to_string());
        let lock = self
            .locks
            .entry(lock_key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        let authorization = manager.begin_authorization(subject).await?;
        let state = authorization_state(&authorization.authorization_url)?;
        let notify = Arc::new(Notify::new());
        self.pending.insert(
            state.clone(),
            PendingFlow {
                upstream: upstream.to_string(),
                notify: Arc::clone(&notify),
            },
        );
        let mut pending_guard = PendingFlowGuard::new(self, state.clone());

        if let Err(error) = open_in_browser(&authorization.authorization_url).await {
            return Err(OauthError::Internal(format!(
                "open OAuth authorization in browser: {error}"
            )));
        }
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.stdio.browser.open",
            upstream,
            "opened upstream OAuth authorization in the default browser"
        );

        let deadline = Instant::now() + AUTHORIZATION_TIMEOUT;
        loop {
            if let Some((_, outcome)) = self.outcomes.remove(&state) {
                pending_guard.disarm();
                return if outcome.success {
                    Ok(())
                } else {
                    Err(OauthError::NeedsReauth(outcome.message))
                };
            }
            if timeout_at(deadline, notify.notified()).await.is_err() {
                return Err(OauthError::NeedsReauth(
                    "stdio OAuth authorization timed out".to_string(),
                ));
            }
        }
    }

    async fn handle_callback(&self, query: CallbackQuery) -> Response {
        let Some(state) = query.state.as_deref().filter(|state| !state.is_empty()) else {
            return callback_response(StatusCode::BAD_REQUEST, "OAuth callback is missing state");
        };
        let now = unix_now();
        let owner = match self
            .sqlite
            .find_upstream_oauth_state_owner(state, now)
            .await
        {
            Ok(Some(owner)) => owner,
            Ok(None) => {
                return callback_response(
                    StatusCode::BAD_REQUEST,
                    "OAuth state is invalid or expired",
                );
            }
            Err(error) => {
                tracing::warn!(
                    subsystem = "gateway_client",
                    phase = "oauth.stdio.callback.lookup",
                    error = %error,
                    "native stdio OAuth callback state lookup failed"
                );
                return callback_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "OAuth callback failed",
                );
            }
        };
        let (upstream, subject) = owner;
        if subject != "gateway" {
            return callback_response(StatusCode::BAD_REQUEST, "OAuth state subject is invalid");
        }
        let Some(pending) = self
            .pending
            .get(state)
            .map(|entry| (entry.upstream.clone(), Arc::clone(&entry.notify)))
        else {
            return callback_response(StatusCode::BAD_REQUEST, "OAuth flow is no longer pending");
        };
        if pending.0 != upstream {
            return callback_response(StatusCode::BAD_REQUEST, "OAuth state upstream is invalid");
        }
        let Some(manager) = self.managers.get(&upstream).map(|entry| entry.clone()) else {
            return callback_response(
                StatusCode::BAD_REQUEST,
                "OAuth upstream is no longer configured",
            );
        };

        let result = if query.error.is_some() {
            drop(
                self.sqlite
                    .delete_upstream_oauth_state_by_csrf(state, now)
                    .await,
            );
            tracing::warn!(
                subsystem = "gateway_client",
                phase = "oauth.stdio.callback.denied",
                upstream = %upstream,
                "upstream OAuth authorization was denied"
            );
            Err("authorization server denied access".to_string())
        } else {
            let Some(code) = query.code.as_deref().filter(|code| !code.is_empty()) else {
                drop(
                    self.sqlite
                        .delete_upstream_oauth_state_by_csrf(state, now)
                        .await,
                );
                return callback_response(
                    StatusCode::BAD_REQUEST,
                    "OAuth callback is missing code",
                );
            };
            manager
                .complete_authorization_callback_with_issuer(
                    &subject,
                    code,
                    state,
                    query.iss.as_deref(),
                )
                .await
                .map(|_| ())
                .map_err(|error| {
                    tracing::warn!(
                        subsystem = "gateway_client",
                        phase = "oauth.stdio.callback.exchange",
                        upstream = %upstream,
                        kind = error.kind(),
                        "native stdio OAuth callback exchange failed"
                    );
                    drop(error);
                    "authorization code exchange failed".to_string()
                })
        };

        if result.is_err() {
            drop(
                self.sqlite
                    .delete_upstream_oauth_state_by_csrf(state, now)
                    .await,
            );
        }
        if let Some((_, pending)) = self.pending.remove(state) {
            self.outcomes.insert(
                state.to_string(),
                FlowOutcome {
                    success: result.is_ok(),
                    message: result.as_ref().err().cloned().unwrap_or_default(),
                },
            );
            // Retain a permit when the waiter has not reached `notified()` yet;
            // `notify_waiters()` can lose the wakeup in that race and make a
            // completed browser flow appear to time out.
            pending.notify.notify_one();
        }
        if result.is_ok() {
            callback_response(
                StatusCode::OK,
                "Authorization completed. You may close this tab.",
            )
        } else {
            callback_response(
                StatusCode::BAD_GATEWAY,
                "Authorization failed. You may close this tab.",
            )
        }
    }
}

async fn stdio_oauth_callback(
    State(coordinator): State<Arc<StdioOauthCoordinator>>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    coordinator.handle_callback(query).await
}

fn callback_response(status: StatusCode, message: &str) -> Response {
    (
        status,
        Html(format!("<html><body><p>{message}</p></body></html>")),
    )
        .into_response()
}

fn authorization_state(authorization_url: &str) -> Result<String, OauthError> {
    Url::parse(authorization_url)
        .map_err(|error| OauthError::Internal(format!("parse authorization URL: {error}")))?
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        .filter(|state| !state.is_empty())
        .ok_or_else(|| OauthError::Internal("authorization URL has no state".to_string()))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    anyhow::bail!("automatic browser launch is not supported on this platform");

    command.arg(url);
    let status = command.status().await.context("launch browser command")?;
    anyhow::ensure!(status.success(), "browser command exited with {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use dashmap::DashMap;
    use labby_auth::at_rest::TokenEncryptionKey;
    use labby_auth::sqlite::SqliteStore;
    use labby_auth::types::UpstreamOauthStateRow;

    use super::{
        CALLBACK_PATH, PendingFlow, PendingFlowGuard, StdioOauthCoordinator, authorization_state,
        unix_now,
    };

    #[test]
    fn callback_path_is_loopback_only_surface() {
        assert_eq!(CALLBACK_PATH, "/auth/upstream/callback");
    }

    #[test]
    fn authorization_state_is_extracted_without_logging_url() {
        let state = authorization_state(
            "https://issuer.example/authorize?client_id=client&state=csrf-value&code_challenge=x",
        )
        .expect("state");
        assert_eq!(state, "csrf-value");
    }

    #[test]
    fn authorization_state_is_required() {
        assert!(authorization_state("https://issuer.example/authorize?client_id=client").is_err());
    }

    #[tokio::test]
    async fn aborting_native_flow_removes_pending_memory_and_durable_state() {
        let directory = tempfile::tempdir().unwrap();
        let sqlite = SqliteStore::open_with_key(
            directory.path().join("oauth.db"),
            Some(
                TokenEncryptionKey::from_encoded(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        let now = unix_now();
        sqlite
            .save_upstream_oauth_state(UpstreamOauthStateRow {
                upstream_name: "cancelled-upstream".to_string(),
                subject: "gateway".to_string(),
                csrf_token: "cancelled-state".to_string(),
                pkce_verifier: "cancelled-verifier".to_string(),
                expected_issuer: None,
                require_issuer: false,
                requested_scopes: Vec::new(),
                created_at: now,
                expires_at: now + 300,
            })
            .await
            .unwrap();
        let coordinator = StdioOauthCoordinator::new(Arc::new(DashMap::new()), sqlite.clone());
        let task_coordinator = Arc::clone(&coordinator);
        let task = tokio::spawn(async move {
            task_coordinator.pending.insert(
                "cancelled-state".to_string(),
                PendingFlow {
                    upstream: "cancelled-upstream".to_string(),
                    notify: Arc::new(tokio::sync::Notify::new()),
                },
            );
            let _guard = PendingFlowGuard::new(&task_coordinator, "cancelled-state".to_string());
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        assert!(coordinator.pending.contains_key("cancelled-state"));

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if sqlite
                    .find_upstream_oauth_state_owner("cancelled-state", now)
                    .await
                    .unwrap()
                    .is_none()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancellation cleanup");

        assert!(!coordinator.pending.contains_key("cancelled-state"));
        assert!(!coordinator.outcomes.contains_key("cancelled-state"));
    }
}
#[test]
fn callback_query_preserves_rfc9207_issuer_verbatim() {
    let query: CallbackQuery = serde_json::from_value(serde_json::json!({
        "code": "c", "state": "s", "iss": "https://issuer.example/tenant"
    }))
    .unwrap();
    assert_eq!(query.iss.as_deref(), Some("https://issuer.example/tenant"));
    let missing: CallbackQuery =
        serde_json::from_value(serde_json::json!({"code": "c", "state": "s"})).unwrap();
    assert!(missing.iss.is_none());
}
