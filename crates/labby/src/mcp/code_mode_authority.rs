//! Request-bound personal OAuth authority carried into Code Mode by token.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};

use labby_auth::VerifiedIdentity;
use labby_gateway::gateway::code_mode::oauth::CodeModePersonalOauthProvider;
use labby_gateway::gateway::manager::GatewayManager;
use labby_runtime::error::ToolError;
use serde_json::Value;

use crate::access::{AccessRuntime, AuthorityCeiling};

struct PersonalAuthorityContext {
    access: Arc<AccessRuntime>,
    identity: VerifiedIdentity,
    ceiling: AuthorityCeiling,
    subject: String,
    allowed_upstreams: Option<BTreeSet<String>>,
}

static CONTEXTS: LazyLock<dashmap::DashMap<String, PersonalAuthorityContext>> =
    LazyLock::new(dashmap::DashMap::new);

pub(crate) struct PersonalAuthorityGuard(String);

impl PersonalAuthorityGuard {
    pub(crate) fn token(&self) -> &str {
        &self.0
    }
}

impl Drop for PersonalAuthorityGuard {
    fn drop(&mut self) {
        CONTEXTS.remove(&self.0);
    }
}

pub(crate) fn register_personal_authority(
    access: Arc<AccessRuntime>,
    identity: VerifiedIdentity,
    ceiling: AuthorityCeiling,
    subject: String,
    allowed_upstreams: Option<BTreeSet<String>>,
) -> PersonalAuthorityGuard {
    let token = format!("oauthctx_{}", ulid::Ulid::new());
    CONTEXTS.insert(
        token.clone(),
        PersonalAuthorityContext {
            access,
            identity,
            ceiling,
            subject,
            allowed_upstreams,
        },
    );
    PersonalAuthorityGuard(token)
}

pub(crate) struct CanonicalPersonalOauthProvider;

impl CodeModePersonalOauthProvider for CanonicalPersonalOauthProvider {
    fn authorize<'a>(
        &'a self,
        manager: &'a GatewayManager,
        authority_token: &'a str,
        params: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let context = CONTEXTS.get(authority_token).ok_or_else(denied)?;
            let access = Arc::clone(&context.access);
            let identity = context.identity.clone();
            let ceiling = context.ceiling.clone();
            let subject = context.subject.clone();
            let allowed_upstreams = context.allowed_upstreams.clone();
            drop(context);

            let store = access.store().await.map_err(|error| {
                crate::dispatch::access_errors::map_runtime_error("gateway", error)
            })?;
            let installation_id = store
                .installation_id()
                .await
                .map_err(|error| {
                    crate::dispatch::access_errors::map_store_error("gateway", error, denied)
                })?
                .unwrap_or_else(|| "installation".to_owned());
            let authority = crate::access::authorize_gateway_action(
                &access,
                identity,
                ceiling,
                &installation_id,
                None,
                "gateway.oauth.authorize",
            )
            .await?
            .ok_or_else(denied)?;
            authority.validate_before_external_effect().await?;
            crate::dispatch::gateway::dispatch_with_manager_scoped(
                manager,
                "gateway.oauth.authorize",
                params,
                crate::dispatch::gateway::GatewayEnrichmentScope {
                    route_visible_upstreams: allowed_upstreams,
                    oauth_subject: Some(subject),
                },
            )
            .await
        })
    }
}

fn denied() -> ToolError {
    ToolError::Forbidden {
        message: "Personal OAuth authorization is not available for this caller".to_owned(),
        required_scopes: vec!["lab".to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    #[tokio::test]
    async fn personal_authority_binds_oauth_to_verified_caller_and_expires_with_request() {
        let provider = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": format!("{}/mcp", provider.uri()),
                "authorization_endpoint": format!("{}/authorize", provider.uri()),
                "token_endpoint": format!("{}/token", provider.uri()),
                "code_challenge_methods_supported": ["S256"]
            })))
            .mount(&provider)
            .await;
        let dir = tempfile::Builder::new()
            .prefix("labby-code-mode-oauth-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let store = labby_auth::sqlite::SqliteStore::open(dir.path().join("auth.db"))
            .await
            .unwrap();
        let config: labby_runtime::gateway_config::UpstreamConfig =
            serde_json::from_value(serde_json::json!({
                "name": "personal", "enabled": true,
                "url": format!("{}/mcp", provider.uri()),
                "oauth": {"mode": "authorization_code_pkce", "registration": {
                    "strategy": "preregistered", "client_id": "fixture"
                }}
            }))
            .unwrap();
        let key = crate::oauth::upstream::encryption::load_key(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        )
        .unwrap();
        let callback = "https://lab.example.com/auth/upstream/callback";
        let oauth = labby_auth::upstream::manager::UpstreamOauthManager::new(
            store.clone(),
            key.clone(),
            config.clone(),
            callback.into(),
        );
        let managers = Arc::new(dashmap::DashMap::new());
        managers.insert("personal".to_owned(), oauth);
        let manager = crate::dispatch::gateway::config_store::test_gateway_manager(
            dir.path().join("lab.toml"),
            Default::default(),
        )
        .with_oauth_resources(store.clone(), key, callback.into())
        .with_upstream_oauth_managers(managers);
        manager.replace_config_for_tests(vec![config]).await;

        let identity =
            VerifiedIdentity::local_credential(labby_auth::Authenticator::StaticBearer, "reader")
                .unwrap();
        let access = Arc::new(AccessRuntime::initialize(dir.path().join("access.db")).await);
        access
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity.clone(), "Personal", "Default")
                    .unwrap(),
            )
            .await
            .unwrap();
        access
            .store()
            .await
            .unwrap()
            .execute_test_statement(
                "UPDATE platform_administrators SET status='revoked', revoked_at=11",
            )
            .await
            .unwrap();
        let auth = labby_auth::auth_context::AuthContext {
            sub: "reader".to_owned(),
            actor_key: None,
            scopes: vec!["lab".to_owned()],
            issuer: "test".to_owned(),
            via_session: false,
            csrf_token: None,
            email: None,
        };
        let guard = register_personal_authority(
            access,
            identity,
            AuthorityCeiling::from_auth_context(&auth),
            auth.sub,
            None,
        );
        let token = guard.token().to_owned();
        let result = CanonicalPersonalOauthProvider
            .authorize(
                &manager,
                &token,
                serde_json::json!({"upstream": "personal"}),
            )
            .await
            .unwrap();
        let authorization = url::Url::parse(result["authorization_url"].as_str().unwrap()).unwrap();
        assert_eq!(authorization.path(), "/authorize");
        let state = authorization
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_eq!(
            store
                .find_upstream_oauth_state_owner(&state, now)
                .await
                .unwrap(),
            Some(("personal".to_owned(), "reader".to_owned()))
        );
        drop(guard);
        assert_eq!(
            CanonicalPersonalOauthProvider
                .authorize(
                    &manager,
                    &token,
                    serde_json::json!({"upstream": "personal"})
                )
                .await
                .unwrap_err()
                .kind(),
            "forbidden"
        );
    }
}
