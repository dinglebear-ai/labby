//! Probe helpers for `probe_upstream_oauth_for_upstream`.
//!
//! This module decomposes the probe flow into named, single-responsibility
//! helpers so the top-level orchestrator (`run`) stays under ~80 lines and the
//! two near-identical URL-conflict checks are deduplicated (Q-M4).

use std::sync::Arc;

use url::Url;

use crate::gateway::manager::GatewayManager;
use crate::gateway::oauth::ProbeResult;
use labby_auth::upstream::{
    http_client::authorization_manager_for_upstream, manager::UpstreamOauthManager,
};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
};
use labby_runtime::redact::redact_url;

use super::{OauthRuntime, should_use_dynamic_registration};

#[cfg(test)]
static TEST_PROBE_METADATA: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<String, rmcp::transport::auth::AuthorizationMetadata>,
    >,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(super) fn install_test_probe_metadata(
    url: &str,
    metadata: rmcp::transport::auth::AuthorizationMetadata,
) {
    TEST_PROBE_METADATA
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(url.to_string(), metadata);
}

// ── public validators (also used by tests in the parent module) ──────────────

pub(crate) fn validate_probe_url(raw: &str) -> Result<Url, ToolError> {
    let parsed = Url::parse(raw).map_err(|_| ToolError::InvalidParam {
        message: "invalid upstream URL".to_string(),
        param: "url".to_string(),
    })?;
    if parsed.scheme() != "https" {
        return Err(ToolError::InvalidParam {
            message: "upstream OAuth probe URL must use https".to_string(),
            param: "url".to_string(),
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ToolError::InvalidParam {
            message: "upstream OAuth probe URL must not include userinfo".to_string(),
            param: "url".to_string(),
        });
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ToolError::InvalidParam {
            message: "upstream OAuth probe URL must not include query strings or fragments"
                .to_string(),
            param: "url".to_string(),
        });
    }
    Ok(parsed)
}

pub(crate) fn validate_probe_upstream_name(raw: &str) -> Result<String, ToolError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ToolError::InvalidParam {
            message: "upstream name must not be empty".to_string(),
            param: "upstream".to_string(),
        });
    }
    if name.len() > 128
        || name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | '?' | '#'))
    {
        return Err(ToolError::InvalidParam {
            message: "upstream name contains unsupported characters".to_string(),
            param: "upstream".to_string(),
        });
    }
    Ok(name.to_string())
}

pub(crate) fn probe_manager_key(parsed: &Url) -> String {
    let host = parsed.host_str().unwrap_or("upstream");
    let mut key = match parsed.port() {
        Some(port) => format!("{host}-{port}"),
        None => host.to_string(),
    };
    let path = parsed.path().trim_matches('/');
    if !path.is_empty() {
        key.push('-');
        key.push_str(path);
    }
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        + &format!(
            "-{:016x}",
            xxhash_rust::xxh3::xxh3_64(parsed.as_str().as_bytes())
        )
}

// ── private helpers ───────────────────────────────────────────────────────────

/// Resolve the probe identity: canonical URL, display URL, upstream name, and
/// whether the name is already persisted in the gateway config.
///
/// Returns `Err` immediately if the named upstream exists under a different URL
/// (first conflict check, deduplicated from the manager-map check below).
async fn resolve_probe_identity(
    manager: &GatewayManager,
    url: &str,
    upstream_name: Option<&str>,
) -> Result<(String, String, String, bool), ToolError> {
    let parsed = validate_probe_url(url)?;
    let canonical_url = parsed.as_str().to_string();
    let redacted_url = redact_url(&canonical_url);

    let name = match upstream_name {
        Some(name) => validate_probe_upstream_name(name)?,
        None => {
            let cfg = manager.config.read().await;
            cfg.upstream
                .iter()
                .find(|u| u.url.as_deref() == Some(canonical_url.as_str()))
                .map(|u| u.name.clone())
                .unwrap_or_else(|| probe_manager_key(&parsed))
        }
    };

    let name_is_persisted = check_name_url_conflict(manager, &name, &canonical_url).await?;
    Ok((canonical_url, redacted_url, name, name_is_persisted))
}

/// Return `true` when `name` is already in the gateway config pointing to
/// `canonical_url`, `false` when the name is not present at all, or `Err` when
/// the name exists but points to a *different* URL (conflict).
async fn check_name_url_conflict(
    manager: &GatewayManager,
    name: &str,
    canonical_url: &str,
) -> Result<bool, ToolError> {
    let cfg = manager.config.read().await;
    match cfg.upstream.iter().find(|u| u.name == name) {
        Some(existing) if existing.url.as_deref() != Some(canonical_url) => {
            Err(ToolError::InvalidParam {
                message: format!("upstream `{name}` is already configured for a different URL"),
                param: "upstream".to_string(),
            })
        }
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

/// Validate that all required OAuth runtime resources are present, and check
/// or report missing env vars with a clear error message.
fn require_oauth_runtime_with_prereq_check<'a>(
    manager: &'a GatewayManager,
    name: &str,
    started: std::time::Instant,
) -> Result<OauthRuntime<'a>, ToolError> {
    // Check each prerequisite independently so the error names only what's missing.
    if manager.oauth_key.is_none()
        || manager.oauth_sqlite.is_none()
        || manager.oauth_redirect_uri.is_none()
    {
        let missing: Vec<&str> = [
            manager
                .oauth_key
                .is_none()
                .then_some("LABBY_OAUTH_ENCRYPTION_KEY"),
            manager
                .oauth_redirect_uri
                .is_none()
                .then_some("LABBY_PUBLIC_URL"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let message = format!(
            "upstream OAuth not configured — set {} to enable it",
            missing.join(" and ")
        );
        tracing::warn!(
            service = "upstream_oauth",
            action = "probe",
            upstream = %name,
            kind = "not_configured",
            elapsed_ms = started.elapsed().as_millis(),
            %message,
            "upstream oauth probe: oauth resources not configured"
        );
        return Err(ToolError::Sdk {
            sdk_kind: "not_configured".to_string(),
            message,
        });
    }

    manager.require_oauth_runtime()
}

/// Look up the `prefer_client_metadata_document` override from the persisted
/// upstream config, if any.
async fn resolve_prefer_cimd(manager: &GatewayManager, name: &str) -> Option<bool> {
    let cfg = manager.config.read().await;
    cfg.upstream
        .iter()
        .find(|u| u.name == name)
        .and_then(|u| u.oauth.as_ref())
        .and_then(|o| o.prefer_client_metadata_document)
}

/// Register a new transient `UpstreamOauthManager` for the given upstream, or
/// evict-and-replace a stale one that points to a different URL.
///
/// Deduplicates the URL-conflict check for the manager-map path (matches the
/// same guard already performed on the persisted config in `resolve_probe_identity`,
/// but applied to the in-memory manager map which may have a stale entry).
pub(super) const TRANSIENT_MANAGER_TTL: std::time::Duration = std::time::Duration::from_mins(15);
pub(super) const TRANSIENT_MANAGER_MAX: usize = 64;

pub(super) fn transient_manager_evictions(
    leases: &mut std::collections::HashMap<String, tokio::time::Instant>,
    now: tokio::time::Instant,
    incoming: &str,
) -> Vec<String> {
    let mut evicted: Vec<String> = leases
        .iter()
        .filter(|(_, created)| now.duration_since(**created) >= TRANSIENT_MANAGER_TTL)
        .map(|(name, _)| name.clone())
        .collect();
    for name in &evicted {
        leases.remove(name);
    }
    while leases.len() >= TRANSIENT_MANAGER_MAX && !leases.contains_key(incoming) {
        let Some(oldest) = leases
            .iter()
            .min_by_key(|(_, created)| **created)
            .map(|(name, _)| name.clone())
        else {
            break;
        };
        leases.remove(&oldest);
        evicted.push(oldest);
    }
    evicted
}

#[cfg(test)]
pub(super) fn schedule_transient_manager_expiry(
    leases: Arc<tokio::sync::Mutex<std::collections::HashMap<String, tokio::time::Instant>>>,
    managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    client_cache: Option<labby_auth::upstream::cache::OauthClientCache>,
    name: String,
    lease_started: tokio::time::Instant,
    ttl: std::time::Duration,
) {
    tokio::spawn(async move {
        tokio::time::sleep(ttl).await;
        let mut leases = leases.lock().await;
        if leases.get(&name) != Some(&lease_started) {
            return;
        }
        leases.remove(&name);
        managers.remove(&name);
        if let Some(cache) = client_cache {
            cache.evict_upstream(&name);
        }
    });
}

fn ensure_transient_manager_sweeper(gm: &GatewayManager) {
    use std::sync::atomic::Ordering;

    if gm
        .transient_oauth_sweeper_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let leases = gm.transient_oauth_managers.clone();
    let managers = gm
        .upstream_oauth_managers
        .as_ref()
        .expect("OAuth runtime manager map was required above")
        .clone();
    let client_cache = gm.oauth_client_cache.clone();
    let owner = Arc::downgrade(&gm.transient_oauth_sweeper_owner);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_mins(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if owner.upgrade().is_none() {
                break;
            }
            let now = tokio::time::Instant::now();
            let expired: Vec<String> = {
                let mut leases = leases.lock().await;
                let expired = leases
                    .iter()
                    .filter(|(_, started)| now.duration_since(**started) >= TRANSIENT_MANAGER_TTL)
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                for name in &expired {
                    leases.remove(name);
                }
                expired
            };
            for name in expired {
                managers.remove(&name);
                if let Some(cache) = &client_cache {
                    cache.evict_upstream(&name);
                }
            }
        }
    });
}

async fn register_transient_manager(
    gm: &GatewayManager,
    runtime: &OauthRuntime<'_>,
    name: &str,
    name_is_persisted: bool,
    canonical_url: &str,
    use_dynamic_registration: bool,
    prefer_cimd: Option<bool>,
    metadata: &rmcp::transport::auth::AuthorizationMetadata,
    strategy: &str,
    started: std::time::Instant,
) -> Result<(), ToolError> {
    // Expire abandoned probe leases and enforce a hard cardinality bound
    // before publishing another callback-visible manager. Managers already
    // reconciled from durable config are reused above and never enter this
    // lease table, so exploratory traffic cannot evict them.
    let mut leases = gm.transient_oauth_managers.lock().await;
    let now = tokio::time::Instant::now();
    for evicted in transient_manager_evictions(&mut leases, now, name) {
        runtime.managers.remove(&evicted);
        gm.evict_upstream_clients(&evicted);
    }

    if let Some(existing) = runtime.managers.get(name) {
        let existing_url = existing.upstream_config().url.clone();
        drop(existing);
        if existing_url.as_deref() != Some(canonical_url) {
            if name_is_persisted {
                return Err(ToolError::InvalidParam {
                    message: format!("upstream `{name}` is already configured for a different URL"),
                    param: "upstream".to_string(),
                });
            }
            runtime.managers.remove(name);
            leases.remove(name);
            gm.evict_upstream_clients(name);
            tracing::info!(
                service = "upstream_oauth",
                action = "probe",
                upstream = %name,
                "upstream oauth probe: replaced stale transient manager"
            );
        } else {
            if let Some(lease) = leases.get_mut(name) {
                *lease = now;
            }
            tracing::info!(
                service = "upstream_oauth",
                action = "probe",
                upstream = %name,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth probe: reusing existing manager"
            );
            return Ok(());
        }
    }

    // Build and insert a new transient manager.
    let registration = if use_dynamic_registration {
        UpstreamOauthRegistration::Dynamic
    } else {
        // No RFC 7591 dynamic registration — use the Client ID Metadata
        // Document (CIMD) approach: the lab's own metadata-document URL
        // acts as the client_id. Derive it from the redirect_uri origin.
        let metadata_doc_url = Url::parse(runtime.redirect_uri.as_str())
            .ok()
            .map(|mut u| {
                u.set_path("/.well-known/oauth-client");
                u.set_query(None);
                u.set_fragment(None);
                u.to_string()
            })
            .unwrap_or_default();
        UpstreamOauthRegistration::ClientMetadataDocument {
            url: metadata_doc_url,
        }
    };

    let config = UpstreamConfig {
        enabled: true,
        name: name.to_string(),
        url: Some(canonical_url.to_string()),
        transport: None,
        socket_path: None,
        headers: Default::default(),
        bearer_token_env: None,
        command: None,
        args: vec![],
        env: std::collections::BTreeMap::new(),
        proxy_resources: false,
        proxy_prompts: false,
        expose_tools: None,
        expose_resources: None,
        expose_prompts: None,
        proxy_skills: false,
        expose_skills: None,
        code_mode_hint: None,
        oauth: Some(UpstreamOauthConfig {
            mode: UpstreamOauthMode::AuthorizationCodePkce,
            registration,
            scopes: metadata.scopes_supported.clone(),
            credential: Default::default(),
            // Propagate the operator override so that if this transient
            // config is later persisted it retains the explicit setting.
            prefer_client_metadata_document: prefer_cimd,
        }),
        imported_from: None,
        priority: 1.0,
    };

    let new_manager = UpstreamOauthManager::new(
        runtime.sqlite.clone(),
        runtime.key.clone(),
        config,
        runtime.redirect_uri.as_ref().clone(),
    );
    runtime.managers.insert(name.to_string(), new_manager);
    // Every manager created by this path is transient, including a configured
    // upstream that does not acquire its durable OAuth config until callback.
    leases.insert(name.to_string(), now);
    ensure_transient_manager_sweeper(gm);
    tracing::info!(
        service = "upstream_oauth",
        action = "probe",
        upstream = %name,
        registration_strategy = strategy,
        elapsed_ms = started.elapsed().as_millis(),
        "upstream oauth probe: transient manager registered"
    );
    Ok(())
}

// ── orchestrator ─────────────────────────────────────────────────────────────

/// Top-level probe entry point. Delegates to named helpers and is intentionally
/// kept short so the overall flow is easy to follow.
pub(crate) async fn run(
    manager: &GatewayManager,
    url: &str,
    upstream_name: Option<&str>,
) -> Result<ProbeResult, ToolError> {
    let started = std::time::Instant::now();

    let (canonical_url, redacted_url, name, name_is_persisted) =
        resolve_probe_identity(manager, url, upstream_name).await?;

    tracing::info!(
        service = "upstream_oauth",
        action = "probe",
        upstream = %name,
        url = %redacted_url,
        "upstream oauth probe: connecting"
    );

    let auth_manager = authorization_manager_for_upstream(&canonical_url)
        .await
        .map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "probe",
                upstream = %name,
                url = %redacted_url,
                kind = e.kind(),
                error = %e,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth probe: connection failed"
            );
            ToolError::Sdk {
                sdk_kind: e.kind().to_string(),
                message: format!("failed to prepare upstream OAuth client: {e}"),
            }
        })?;

    #[cfg(test)]
    let injected_metadata = TEST_PROBE_METADATA
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&canonical_url)
        .cloned();
    #[cfg(not(test))]
    let injected_metadata: Option<rmcp::transport::auth::AuthorizationMetadata> = None;

    let metadata = if let Some(metadata) = injected_metadata {
        metadata
    } else {
        match auth_manager.resolve_metadata().await {
            Ok(resolution) if resolution.source.is_discovered() => {
                let m = resolution.metadata;
                tracing::info!(
                    service = "upstream_oauth",
                    action = "probe",
                    upstream = %name,
                    url = %redacted_url,
                    issuer = m.issuer.as_deref().unwrap_or("<none>"),
                    supports_dynamic_registration = m.registration_endpoint.is_some(),
                    scopes = ?m.scopes_supported,
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth probe: OAuth metadata discovered"
                );
                m
            }
            resolution => {
                let fallback =
                    labby_auth::upstream::manager::discover_published_metadata(&canonical_url)
                        .await
                        .map_err(|error| ToolError::Sdk {
                            sdk_kind: error.kind().to_string(),
                            message: format!("OAuth metadata discovery failed: {error}"),
                        })?;
                if let Some(metadata) = fallback {
                    tracing::info!(
                        service = "upstream_oauth",
                        action = "probe",
                        upstream = %name,
                        url = %redacted_url,
                        issuer = metadata.issuer.as_deref().unwrap_or("<none>"),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream oauth probe: OAuth metadata discovered with Labby issuer policy"
                    );
                    metadata
                } else {
                    let reason = resolution
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "no published OAuth metadata".to_string());
                    tracing::info!(
                        service = "upstream_oauth",
                        action = "probe",
                        upstream = %name,
                        url = %redacted_url,
                        reason,
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream oauth probe: no OAuth metadata found"
                    );
                    return Ok(ProbeResult {
                        upstream: name,
                        url: redacted_url.clone(),
                        transient: false,
                        durability: "not_registered_no_oauth_metadata".to_string(),
                        oauth_discovered: false,
                        issuer: None,
                        scopes: None,
                        registration_strategy: None,
                    });
                }
            }
        }
    };

    let prefer_cimd = resolve_prefer_cimd(manager, &name).await;
    let supports_dynamic = metadata.registration_endpoint.is_some();
    let use_dynamic_registration =
        should_use_dynamic_registration(&name, supports_dynamic, prefer_cimd);
    let strategy = if use_dynamic_registration {
        "dynamic"
    } else {
        "client_metadata_document"
    };

    let runtime = require_oauth_runtime_with_prereq_check(manager, &name, started)?;

    register_transient_manager(
        manager,
        &runtime,
        &name,
        name_is_persisted,
        &canonical_url,
        use_dynamic_registration,
        prefer_cimd,
        &metadata,
        strategy,
        started,
    )
    .await?;

    Ok(ProbeResult {
        upstream: name,
        url: redacted_url.clone(),
        transient: true,
        durability: "transient_until_oauth_callback_persists_gateway_config".to_string(),
        oauth_discovered: true,
        issuer: metadata.issuer,
        scopes: metadata.scopes_supported,
        registration_strategy: Some(strategy.to_string()),
    })
}
