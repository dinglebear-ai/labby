use std::sync::Arc;

use anyhow::{Context, Result};

use crate::config::LabConfig;
use crate::oauth::upstream::cache::OauthClientCache;
use crate::oauth::upstream::encryption::{EncryptionKey, load_key};
use crate::oauth::upstream::manager::UpstreamOauthManager;

pub(crate) struct UpstreamOauthRuntime {
    pub(crate) managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    pub(crate) cache: OauthClientCache,
    pub(crate) sqlite: lab_auth::sqlite::SqliteStore,
    pub(crate) key: EncryptionKey,
    pub(crate) redirect_uri: String,
}

pub(crate) async fn build_upstream_oauth_runtime(
    config: &LabConfig,
    auth_config: &lab_auth::config::AuthConfig,
) -> Result<Option<UpstreamOauthRuntime>> {
    let Some(public_url) = auth_config.public_url.as_ref() else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime disabled because no public url is configured"
        );
        return Ok(None);
    };
    let Ok(encryption_key_raw) = std::env::var("LAB_OAUTH_ENCRYPTION_KEY") else {
        tracing::info!(
            subsystem = "gateway_client",
            phase = "oauth.runtime.disabled",
            "upstream oauth runtime disabled because LAB_OAUTH_ENCRYPTION_KEY is unset"
        );
        return Ok(None);
    };
    anyhow::ensure!(
        public_url.scheme() == "https",
        "LAB_PUBLIC_URL must be absolute https:// when upstream oauth is configured"
    );
    let key = load_key(&encryption_key_raw)
        .map_err(|error| anyhow::anyhow!("invalid LAB_OAUTH_ENCRYPTION_KEY: {error}"))?;
    let sqlite = lab_auth::sqlite::SqliteStore::open(auth_config.sqlite_path.clone())
        .await
        .context("open sqlite store for upstream oauth")?;
    let redirect_uri = build_upstream_oauth_callback_uri(public_url)?;

    Ok(Some(build_upstream_oauth_runtime_from_parts(
        config,
        sqlite,
        key,
        redirect_uri,
    )))
}

pub(crate) fn build_upstream_oauth_runtime_from_parts(
    config: &LabConfig,
    sqlite: lab_auth::sqlite::SqliteStore,
    key: EncryptionKey,
    redirect_uri: String,
) -> UpstreamOauthRuntime {
    let managers = Arc::new(dashmap::DashMap::new());
    for upstream in config
        .upstream
        .iter()
        .filter(|upstream| upstream.oauth.is_some())
    {
        managers.insert(
            upstream.name.clone(),
            UpstreamOauthManager::new(
                sqlite.clone(),
                key.clone(),
                upstream.clone(),
                redirect_uri.clone(),
            ),
        );
    }
    let cache = OauthClientCache::new(Arc::clone(&managers));
    tracing::info!(
        subsystem = "gateway_client",
        phase = "oauth.runtime.ready",
        oauth_upstream_count = managers.len(),
        "upstream oauth runtime initialized"
    );
    UpstreamOauthRuntime {
        managers,
        cache,
        sqlite,
        key,
        redirect_uri,
    }
}

pub(crate) fn build_upstream_oauth_callback_uri(public_url: &url::Url) -> Result<String> {
    let mut redirect_uri = public_url.clone();
    let base_path = redirect_uri.path().trim_end_matches('/');
    let next_path = if base_path.is_empty() {
        "/auth/upstream/callback".to_string()
    } else {
        format!("{base_path}/auth/upstream/callback")
    };
    redirect_uri.set_path(&next_path);
    redirect_uri.set_query(None);
    redirect_uri.set_fragment(None);
    Ok(redirect_uri.to_string())
}
