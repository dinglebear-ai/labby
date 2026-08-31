use std::sync::Arc;

use anyhow::{Context, Result};
use labby_runtime::gateway_config::UpstreamConfig;

use crate::config::AuthConfig;
use crate::sqlite::SqliteStore;
use crate::upstream::cache::OauthClientCache;
use crate::upstream::encryption::{EncryptionKey, load_key};
use crate::upstream::manager::UpstreamOauthManager;

pub struct UpstreamOauthRuntime {
    pub managers: Arc<dashmap::DashMap<String, UpstreamOauthManager>>,
    pub cache: OauthClientCache,
    pub sqlite: SqliteStore,
    pub key: EncryptionKey,
    pub redirect_uri: String,
}

async fn initialize_runtime_parts(
    upstreams: &[UpstreamConfig],
    auth_config: &AuthConfig,
    encryption_key_raw: Option<&str>,
) -> Result<Option<(SqliteStore, EncryptionKey)>> {
    if !upstreams.iter().any(|upstream| upstream.oauth.is_some()) {
        return Ok(None);
    }
    let encryption_key_raw = encryption_key_raw
        .and_then(|value| (!value.trim().is_empty()).then_some(value))
        .context("LABBY_OAUTH_ENCRYPTION_KEY is required when upstream OAuth is configured")?;
    let shared_google_enabled = upstreams.iter().any(|upstream| {
        upstream
            .oauth
            .as_ref()
            .is_some_and(|oauth| oauth.credential.is_google_provider())
    });
    anyhow::ensure!(
        !shared_google_enabled || auth_config.token_encryption_key.is_some(),
        "{prefix}_TOKEN_ENCRYPTION_KEY is required when an upstream uses credential.source=google_provider",
        prefix = auth_config.env_prefix
    );
    let key = load_key(encryption_key_raw)
        .map_err(|error| anyhow::anyhow!("invalid upstream OAuth encryption key: {error}"))?;
    let sqlite = SqliteStore::open_with_key(
        auth_config.sqlite_path.clone(),
        auth_config.token_encryption_key.clone(),
    )
    .await
    .context("open sqlite store for upstream oauth")?;
    Ok(Some((sqlite, key)))
}

/// Build the upstream OAuth runtime for the upstreams that declare an `oauth`
/// block.
///
/// Takes the upstream slice directly rather than a whole `LabConfig` so this
/// runtime stays decoupled from the product binary's config type: `labby-auth`
/// reads only the upstream list, never the rest of `LabConfig`.
pub async fn build_upstream_oauth_runtime(
    upstreams: &[UpstreamConfig],
    auth_config: &AuthConfig,
    encryption_key_raw: Option<&str>,
) -> Result<Option<UpstreamOauthRuntime>> {
    if !upstreams.iter().any(|upstream| upstream.oauth.is_some()) {
        return Ok(None);
    }
    let Some(public_url) = auth_config.public_url.as_ref() else {
        anyhow::bail!(
            "LABBY_PUBLIC_URL is required when upstream OAuth is configured; it must be the explicit public callback base"
        );
    };
    anyhow::ensure!(
        public_url.scheme() == "https",
        "public_url must be absolute https:// when upstream oauth is configured"
    );
    let Some((sqlite, key)) =
        initialize_runtime_parts(upstreams, auth_config, encryption_key_raw).await?
    else {
        return Ok(None);
    };
    let redirect_uri = build_upstream_oauth_callback_uri(public_url)?;

    Ok(Some(build_upstream_oauth_runtime_from_parts(
        upstreams,
        sqlite,
        key,
        redirect_uri,
    )))
}

/// Build the outbound OAuth runtime with a host-provided callback URI.
///
/// This is used by transports that cannot expose Labby's HTTP server, notably
/// stdio. The host binds a loopback-only callback listener first, passes its
/// URI here, and owns the browser/callback orchestration. Keeping SQLite/key
/// setup here ensures the stdio path uses exactly the same encrypted stores
/// and validation as the public HTTP path.
pub async fn build_upstream_oauth_runtime_with_redirect(
    upstreams: &[UpstreamConfig],
    auth_config: &AuthConfig,
    encryption_key_raw: Option<&str>,
    redirect_uri: String,
) -> Result<Option<UpstreamOauthRuntime>> {
    if !upstreams.iter().any(|upstream| upstream.oauth.is_some()) {
        return Ok(None);
    }
    let redirect_url = url::Url::parse(&redirect_uri)
        .with_context(|| format!("invalid upstream OAuth redirect URI: {redirect_uri}"))?;
    anyhow::ensure!(
        matches!(redirect_url.scheme(), "http" | "https"),
        "upstream OAuth redirect URI must use http:// or https://"
    );

    let Some((sqlite, key)) =
        initialize_runtime_parts(upstreams, auth_config, encryption_key_raw).await?
    else {
        return Ok(None);
    };

    Ok(Some(build_upstream_oauth_runtime_from_parts(
        upstreams,
        sqlite,
        key,
        redirect_uri,
    )))
}

/// Assemble the runtime from pre-loaded parts.
///
/// `upstreams` is the upstream slice (decoupled from `LabConfig`); only the
/// entries with an `oauth` block get a manager.
pub fn build_upstream_oauth_runtime_from_parts(
    upstreams: &[UpstreamConfig],
    sqlite: SqliteStore,
    key: EncryptionKey,
    redirect_uri: String,
) -> UpstreamOauthRuntime {
    let managers = Arc::new(dashmap::DashMap::new());
    for upstream in upstreams.iter().filter(|upstream| upstream.oauth.is_some()) {
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

pub fn build_upstream_oauth_callback_uri(public_url: &url::Url) -> Result<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::gateway_config::{
        UpstreamOauthConfig, UpstreamOauthCredentialSource, UpstreamOauthMode,
        UpstreamOauthRegistration,
    };

    fn test_oauth_upstream(credential: UpstreamOauthCredentialSource) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "calendar".to_string(),
            url: Some("https://calendar.example.com/mcp".to_string()),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            command: None,
            args: Vec::new(),
            bearer_token_env: None,
            env: Default::default(),
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
                registration: UpstreamOauthRegistration::Dynamic,
                scopes: None,
                credential,
                prefer_client_metadata_document: None,
            }),
            imported_from: None,
            priority: 1.0,
        }
    }

    #[tokio::test]
    async fn configured_upstream_oauth_requires_explicit_public_callback_base() {
        let auth = AuthConfig::default();
        let error = build_upstream_oauth_runtime(
            &[test_oauth_upstream(
                UpstreamOauthCredentialSource::Dedicated,
            )],
            &auth,
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
        )
        .await
        .err()
        .expect("upstream OAuth without a callback base must fail closed");

        assert!(error.to_string().contains("LABBY_PUBLIC_URL"));
        assert!(error.to_string().contains("public callback base"));
    }

    #[tokio::test]
    async fn shared_google_runtime_requires_provider_token_encryption_key() {
        let dir = tempfile::tempdir().unwrap();
        let auth = AuthConfig {
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            token_encryption_key: None,
            ..AuthConfig::default()
        };
        let mut upstream =
            test_oauth_upstream(UpstreamOauthCredentialSource::GoogleProvider { account: None });
        upstream.name = "google-calendar".to_string();
        upstream.url = Some("https://calendarmcp.googleapis.com/mcp/v1".to_string());
        upstream.oauth.as_mut().unwrap().registration = UpstreamOauthRegistration::Preregistered {
            client_id: "google-client".to_string(),
            client_secret_env: Some("GOOGLE_SECRET".to_string()),
        };
        upstream.oauth.as_mut().unwrap().scopes = Some(vec!["calendar".to_string()]);

        let error = build_upstream_oauth_runtime(
            &[upstream],
            &auth,
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
        )
        .await
        .err()
        .expect("shared Google runtime must require provider token encryption");
        assert!(error.to_string().contains("TOKEN_ENCRYPTION_KEY"));
    }
}
