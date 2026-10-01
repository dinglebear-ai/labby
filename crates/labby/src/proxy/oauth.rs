//! OAuth resource-lease ownership for an ephemeral proxy.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::live_gateway::LiveGateway;

pub struct ProxyOauthContext {
    pub gateway: LiveGateway,
    pub auth_state: std::sync::Arc<labby_auth::state::AuthState>,
    pub issuer: url::Url,
}

impl std::fmt::Debug for ProxyOauthContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyOauthContext")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl ProxyOauthContext {
    pub async fn prepare(config: &crate::config::LabConfig) -> Result<Self> {
        let mut auth_config = crate::config::resolve_auth_for_config(config)
            .context("proxy OAuth configuration is invalid")?;
        if !matches!(auth_config.mode, labby_auth::config::AuthMode::OAuth) {
            bail!("proxy OAuth requires the live Labby daemon to run in OAuth mode");
        }
        let issuer = auth_config
            .public_url
            .clone()
            .context("proxy OAuth requires a stable Labby public issuer")?;
        let gateway = crate::live_gateway::detect(config, "cli")
            .await
            .context("proxy OAuth live-daemon discovery failed")?
            .context("proxy OAuth requires a reachable live Labby daemon")?;
        gateway
            .verify_resource_lease_actions()
            .await
            .context("live Labby daemon does not support proxy OAuth leases")?;
        let daemon_jwks = gateway
            .verify_oauth_issuer(&issuer)
            .await
            .context("stable Labby OAuth issuer verification failed")?;

        // The proxy never accepts the daemon's static administrator token as an
        // OAuth fallback. It validates only signed access tokens.
        auth_config.disable_static_token_with_oauth = true;
        let auth_state = std::sync::Arc::new(
            labby_auth::state::AuthState::new(auth_config)
                .await
                .context("same-host OAuth auth state construction failed")?,
        );
        if daemon_jwks != *auth_state.signing_keys.jwks() {
            bail!("live daemon JWKS does not match the configured same-host signing keys");
        }
        Ok(Self {
            gateway,
            auth_state,
            issuer,
        })
    }
}

/// Build an issuer hosted by the proxy itself, using the planned Funnel origin.
pub async fn prepare_self_hosted(
    config: &crate::config::LabConfig,
    resource: &url::Url,
    scopes: &[String],
) -> Result<(std::sync::Arc<labby_auth::state::AuthState>, url::Url)> {
    let mut auth_config = resolve_self_hosted_auth_config(config, resource)?;
    let issuer = auth_config
        .public_url
        .clone()
        .context("Funnel issuer is missing")?;
    configure_scope_policy(&mut auth_config, scopes)?;
    auth_config.enable_dynamic_registration = true;
    auth_config.disable_static_token_with_oauth = true;
    let state = labby_auth::state::AuthState::new(auth_config)
        .await
        .context("self-hosted OAuth state construction failed")?;
    state
        .replace_configured_resource_scopes([(resource.to_string(), scopes.to_vec())])
        .context("register Funnel MCP OAuth resource")?;
    Ok((std::sync::Arc::new(state), issuer))
}

/// Resolve the Funnel issuer before OAuth configuration validation, preserving
/// explicit file and environment overrides and rejecting a mismatched origin.
pub fn resolve_self_hosted_auth_config(
    config: &crate::config::LabConfig,
    resource: &url::Url,
) -> Result<labby_auth::config::AuthConfig> {
    resolve_self_hosted_auth_config_with_env(config, resource, std::env::vars())
}

fn resolve_self_hosted_auth_config_with_env(
    config: &crate::config::LabConfig,
    resource: &url::Url,
    env: impl IntoIterator<Item = (String, String)>,
) -> Result<labby_auth::config::AuthConfig> {
    let mut issuer = resource.clone();
    issuer.set_path("/");
    issuer.set_query(None);
    issuer.set_fragment(None);
    let mut file_auth = config.auth.clone().unwrap_or_default();
    if file_auth.public_url.is_none() {
        file_auth.public_url = config
            .public_urls
            .as_ref()
            .and_then(|urls| urls.app.clone())
            .or_else(|| Some(issuer.to_string()));
    }
    let mut auth_config = crate::config::resolve_auth_with_env(Some(&file_auth), env)
        .context("proxy OAuth configuration is invalid")?;
    if !matches!(auth_config.mode, labby_auth::config::AuthMode::OAuth) {
        bail!("Funnel requires LABBY_AUTH_MODE=oauth and Google OAuth credentials");
    }
    if auth_config
        .public_url
        .as_ref()
        .is_some_and(|configured| configured.origin() != issuer.origin())
    {
        bail!("configured OAuth issuer differs from the Tailscale Funnel origin");
    }
    validate_google_callback(&auth_config, &issuer)?;
    auth_config.public_url = Some(issuer);
    auth_config.resource_path = resource.path().to_string();
    Ok(auth_config)
}

pub const DEFAULT_LEASE_TTL: Duration = Duration::from_mins(2);
pub const DEFAULT_RENEW_INTERVAL: Duration = Duration::from_secs(40);
pub const DEFAULT_RENEW_JITTER_MAX: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy)]
pub struct OAuthLeaseTiming {
    pub ttl: Duration,
    pub renew_interval: Duration,
    pub jitter_max: Duration,
}

impl Default for OAuthLeaseTiming {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_LEASE_TTL,
            renew_interval: DEFAULT_RENEW_INTERVAL,
            jitter_max: DEFAULT_RENEW_JITTER_MAX,
        }
    }
}

impl OAuthLeaseTiming {
    #[must_use]
    pub fn proxy_default() -> Self {
        let timing = Self::default();
        #[cfg(feature = "proxy-testkit")]
        if let Some(interval) = std::env::var("LABBY_PROXY_TEST_RENEW_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .map(Duration::from_millis)
        {
            return Self {
                renew_interval: interval,
                jitter_max: Duration::ZERO,
                ..timing
            };
        }
        timing
    }
}

pub struct OAuthLeaseGuard {
    gateway: LiveGateway,
    lease_id: String,
    cancellation: CancellationToken,
    renewal_task: Option<tokio::task::JoinHandle<Result<()>>>,
    active: bool,
}

impl std::fmt::Debug for OAuthLeaseGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthLeaseGuard")
            .field("lease_id", &"<redacted>")
            .field("active", &self.active)
            .field("renewal_task", &self.renewal_task.is_some())
            .finish_non_exhaustive()
    }
}

impl OAuthLeaseGuard {
    pub async fn create(
        gateway: LiveGateway,
        resource: &str,
        scopes: Vec<String>,
        owner: &str,
        timing: OAuthLeaseTiming,
    ) -> Result<Self> {
        if timing.ttl.is_zero() || timing.renew_interval.is_zero() {
            bail!("OAuth lease TTL and renewal interval must be non-zero");
        }
        let lease = gateway
            .create_resource_lease(resource, scopes, timing.ttl, owner)
            .await
            .context("OAuth resource lease creation failed")?;
        let lease_id = lease.id;
        let cancellation = CancellationToken::new();
        let renew_cancel = cancellation.clone();
        let renew_gateway = gateway.clone();
        let renew_id = lease_id.clone();
        let renewal_task = tokio::spawn(async move {
            loop {
                let delay = timing.renew_interval + bounded_jitter(timing.jitter_max);
                tokio::select! {
                    () = renew_cancel.cancelled() => return Ok(()),
                    () = tokio::time::sleep(delay) => {}
                }
                renew_gateway
                    .renew_resource_lease(&renew_id, timing.ttl)
                    .await
                    .context("OAuth resource lease renewal failed")?;
                tracing::debug!(
                    surface = "cli",
                    service = "proxy",
                    action = "proxy.oauth.lease.renew",
                    "OAuth proxy resource lease renewed"
                );
            }
        });
        tracing::debug!(
            surface = "cli",
            service = "proxy",
            action = "proxy.oauth.lease.create",
            "OAuth proxy resource lease created"
        );
        Ok(Self {
            gateway,
            lease_id,
            cancellation,
            renewal_task: Some(renewal_task),
            active: true,
        })
    }

    pub async fn wait_for_failure(&mut self) -> Result<()> {
        let task = self
            .renewal_task
            .take()
            .context("OAuth lease renewal supervisor is no longer running")?;
        task.await.context("OAuth lease renewal task panicked")?
    }

    pub async fn release(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.cancellation.cancel();
        if let Some(task) = self.renewal_task.take() {
            drop(task.await);
        }
        self.gateway
            .release_resource_lease(&self.lease_id)
            .await
            .context("OAuth resource lease release failed")?;
        self.active = false;
        tracing::debug!(
            surface = "cli",
            service = "proxy",
            action = "proxy.oauth.lease.release",
            "OAuth proxy resource lease released"
        );
        Ok(())
    }
}

pub async fn verify_protected_resource_metadata(
    local_url: &url::Url,
    public_resource: &url::Url,
) -> Result<()> {
    let metadata_url = local_url
        .join("/.well-known/oauth-protected-resource")
        .context("construct proxy protected-resource metadata URL")?;
    let host = match public_resource.port() {
        Some(port) => format!(
            "{}:{port}",
            public_resource
                .host_str()
                .context("proxy OAuth resource has no host")?
        ),
        None => public_resource
            .host_str()
            .context("proxy OAuth resource has no host")?
            .to_string(),
    };
    let response = reqwest::Client::new()
        .get(metadata_url)
        .header(reqwest::header::HOST, host)
        .send()
        .await
        .context("proxy protected-resource metadata is unreachable")?;
    if !response.status().is_success() {
        bail!(
            "proxy protected-resource metadata returned HTTP {}",
            response.status()
        );
    }
    let metadata: labby_auth::types::ProtectedResourceMetadata = response
        .json()
        .await
        .context("proxy protected-resource metadata is invalid")?;
    if metadata.resource != public_resource.as_str() {
        bail!("proxy protected-resource metadata advertises the wrong resource");
    }
    Ok(())
}

impl Drop for OAuthLeaseGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.renewal_task.take() {
            task.abort();
        }
        // An async release is deliberately impossible from Drop. The daemon TTL
        // is the crash/forced-termination recovery boundary.
    }
}

#[must_use]
pub fn owner_fingerprint() -> String {
    let mut nonce = [0_u8; 16];
    if getrandom::fill(&mut nonce).is_err() {
        nonce.copy_from_slice(&std::process::id().to_le_bytes().repeat(4));
    }
    let mut hash = Sha256::new();
    hash.update(std::process::id().to_le_bytes());
    hash.update(nonce);
    let digest = hash.finalize();
    digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bounded_jitter(max: Duration) -> Duration {
    if max.is_zero() {
        return Duration::ZERO;
    }
    let mut bytes = [0_u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return Duration::ZERO;
    }
    let max_nanos = max.as_nanos().min(u128::from(u64::MAX)) as u64;
    let nanos = u64::from_le_bytes(bytes) % max_nanos.saturating_add(1);
    Duration::from_nanos(nanos)
}

fn validate_google_callback(
    config: &labby_auth::config::AuthConfig,
    issuer: &url::Url,
) -> Result<()> {
    if matches!(
        config.inbound_provider,
        Some(labby_auth::config::InboundProviderKind::Authelia)
    ) {
        return Ok(());
    }
    let mut callback = issuer.clone();
    callback.set_path(labby_auth::config::GOOGLE_CALLBACK_PATH);
    if config.google.callback_path != labby_auth::config::GOOGLE_CALLBACK_PATH
        || config
            .google
            .callback_url
            .as_ref()
            .is_some_and(|configured| configured != &callback)
    {
        bail!("configured Google callback differs from the Tailscale Funnel callback {callback}");
    }
    Ok(())
}

fn configure_scope_policy(
    config: &mut labby_auth::config::AuthConfig,
    scopes: &[String],
) -> Result<()> {
    if scopes.is_empty() {
        bail!("Funnel OAuth requires at least one scope");
    }
    config.scopes_supported = scopes.to_vec();
    // The proxy requires all of these scopes for every MCP request. The
    // canonical resource's omitted-scope grant must satisfy that same policy.
    config.default_scope = scopes.join(" ");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn funnel_issuer_fallback_precedes_oauth_validation_and_respects_overrides() {
        let resource = url::Url::parse("https://node.example.ts.net:8443/mcp").unwrap();
        let home = tempfile::tempdir().unwrap();
        let config = crate::config::LabConfig {
            auth: Some(crate::config::AuthFileConfig {
                mode: Some("oauth".to_string()),
                google_client_id: Some("client".to_string()),
                google_client_secret: Some("secret".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let env = || {
            vec![
                (
                    "LABBY_HOME".to_string(),
                    home.path().to_string_lossy().into_owned(),
                ),
                ("LABBY_TOKEN_ENCRYPTION_KEY".to_string(), "00".repeat(32)),
                (
                    "LABBY_AUTH_ADMIN_EMAIL".to_string(),
                    "owner@example.com".to_string(),
                ),
            ]
        };
        let resolved =
            super::resolve_self_hosted_auth_config_with_env(&config, &resource, env()).unwrap();
        assert_eq!(
            resolved.public_url.unwrap().as_str(),
            "https://node.example.ts.net:8443/"
        );
        assert_eq!(resolved.resource_path, "/mcp");
        let mut explicit = config.clone();
        explicit.auth.as_mut().unwrap().public_url = Some("https://other.example/".to_string());
        assert!(
            super::resolve_self_hosted_auth_config_with_env(&explicit, &resource, env()).is_err()
        );
        let mut public_urls = config.clone();
        public_urls.public_urls = Some(crate::config::PublicUrlsConfig {
            app: Some("https://other.example/".to_string()),
            ..Default::default()
        });
        assert!(
            super::resolve_self_hosted_auth_config_with_env(&public_urls, &resource, env())
                .is_err()
        );
        public_urls.auth.as_mut().unwrap().public_url =
            Some("https://node.example.ts.net:8443/".to_string());
        super::resolve_self_hosted_auth_config_with_env(&public_urls, &resource, env()).unwrap();
        let mut override_env = env();
        override_env.push((
            "LABBY_PUBLIC_URL".to_string(),
            "https://node.example.ts.net:8443/".to_string(),
        ));
        super::resolve_self_hosted_auth_config_with_env(&explicit, &resource, override_env)
            .unwrap();
        let mut mismatch_env = env();
        mismatch_env.push((
            "LABBY_PUBLIC_URL".to_string(),
            "https://other.example/".to_string(),
        ));
        assert!(
            super::resolve_self_hosted_auth_config_with_env(&config, &resource, mismatch_env)
                .is_err()
        );
    }

    #[test]
    fn funnel_google_callback_rejects_stale_overrides() {
        let issuer = url::Url::parse("https://node.example.ts.net:8443/").unwrap();
        let mut config = labby_auth::config::AuthConfig::default();
        config.google.callback_path = "/auth/google/callback".to_string();
        super::validate_google_callback(&config, &issuer).unwrap();
        config.google.callback_url = Some(issuer.join("auth/google/callback").unwrap());
        super::validate_google_callback(&config, &issuer).unwrap();
        config.google.callback_url =
            Some(url::Url::parse("https://old.example/auth/google/callback").unwrap());
        assert!(super::validate_google_callback(&config, &issuer).is_err());
        config.google.callback_url = None;
        config.google.callback_path = "/legacy/callback".to_string();
        assert!(super::validate_google_callback(&config, &issuer).is_err());
    }

    #[test]
    fn self_hosted_default_scope_grants_every_required_scope() {
        let mut config = labby_auth::config::AuthConfig::default();
        let scopes = vec!["mcp:read".to_string(), "mcp:write".to_string()];
        super::configure_scope_policy(&mut config, &scopes).unwrap();
        assert_eq!(config.scopes_supported, scopes);
        assert_eq!(config.default_scope, "mcp:read mcp:write");
        assert!(super::configure_scope_policy(&mut config, &[]).is_err());
    }
}
