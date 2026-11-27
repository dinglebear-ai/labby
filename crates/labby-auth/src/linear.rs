pub(crate) const LINEAR_ISSUER: &str = "https://linear.app";

#[cfg(feature = "http-axum")]
mod client {
    use std::time::Duration;

    use serde::Deserialize;
    use url::Url;
    use uuid::Uuid;

    use crate::config::LinearConfig;
    use crate::error::AuthError;
    use crate::google::AuthorizeUrlRequest;
    use crate::oauth_provider::ProviderExchange;
    use crate::provider_http::{bounded_json, install_rustls_default_once};

    #[derive(Clone, Debug)]
    pub struct LinearProvider {
        pub client_id: String,
        workspace_id: Uuid,
        redirect_uri: Url,
        token_endpoint: Url,
        graphql_endpoint: Url,
        http: reqwest::Client,
    }

    #[derive(Deserialize)]
    struct TokenReply {
        access_token: String,
        token_type: String,
    }

    #[derive(Deserialize)]
    struct ViewerReply {
        data: Option<ViewerData>,
        #[serde(default)]
        errors: Vec<serde_json::Value>,
    }

    #[derive(Deserialize)]
    struct ViewerData {
        viewer: Option<Viewer>,
    }

    #[derive(Deserialize)]
    struct Viewer {
        id: Uuid,
        email: String,
        active: bool,
        guest: bool,
        app: bool,
        organization: Organization,
    }

    #[derive(Deserialize)]
    struct Organization {
        id: Uuid,
    }

    fn endpoint(value: &str) -> Result<Url, AuthError> {
        Url::parse(value).map_err(|_| AuthError::Config("invalid built-in Linear endpoint".into()))
    }

    impl LinearProvider {
        pub fn new(config: LinearConfig, redirect_uri: Url) -> Result<Self, AuthError> {
            install_rustls_default_once();
            let http = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| {
                    AuthError::Config("could not create the Linear OAuth client".into())
                })?;
            Ok(Self {
                client_id: config.client_id,
                workspace_id: config.workspace_id,
                redirect_uri,
                token_endpoint: endpoint("https://api.linear.app/oauth/token")?,
                graphql_endpoint: endpoint("https://api.linear.app/graphql")?,
                http,
            })
        }

        pub fn authorize_url(&self, request: &AuthorizeUrlRequest) -> Result<Url, AuthError> {
            if request.code_challenge_method != "S256"
                || request.code_challenge.is_empty()
                || request.state.is_empty()
            {
                return Err(AuthError::Validation(
                    "Linear OAuth requires state and S256 PKCE".into(),
                ));
            }
            let mut url = endpoint("https://linear.app/oauth/authorize")?;
            url.query_pairs_mut()
                .append_pair("client_id", &self.client_id)
                .append_pair("redirect_uri", self.redirect_uri.as_str())
                .append_pair("response_type", "code")
                .append_pair("scope", "read")
                .append_pair("actor", "user")
                .append_pair("state", &request.state)
                .append_pair("code_challenge", &request.code_challenge)
                .append_pair("code_challenge_method", "S256");
            Ok(url)
        }

        pub async fn exchange_code(
            &self,
            code: &str,
            verifier: &str,
        ) -> Result<ProviderExchange, AuthError> {
            if code.is_empty() || verifier.is_empty() {
                return Err(AuthError::InvalidGrant(
                    "Linear authorization code or verifier is missing".into(),
                ));
            }
            let response = self
                .http
                .post(self.token_endpoint.clone())
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", &self.client_id),
                    ("redirect_uri", self.redirect_uri.as_str()),
                    ("code", code),
                    ("code_verifier", verifier),
                ])
                .send()
                .await
                .map_err(|_| AuthError::Network("Linear token exchange failed".into()))?;
            if !response.status().is_success() {
                return Err(AuthError::InvalidGrant(
                    "Linear rejected the authorization code".into(),
                ));
            }
            let token: TokenReply = bounded_json(response, "Linear token response")
                .await
                .map_err(|_| AuthError::Decode("invalid Linear token response".into()))?;
            if token.access_token.is_empty() || !token.token_type.eq_ignore_ascii_case("bearer") {
                return Err(AuthError::InvalidGrant(
                    "Linear did not return a bearer token".into(),
                ));
            }
            let response = self.http.post(self.graphql_endpoint.clone())
                .bearer_auth(&token.access_token)
                .json(&serde_json::json!({"query": "{ viewer { id email active guest app organization { id } } }"}))
                .send().await.map_err(|_| AuthError::Network("Linear identity lookup failed".into()))?;
            if !response.status().is_success() {
                return Err(AuthError::AuthFailed(
                    "Linear rejected the identity lookup".into(),
                ));
            }
            let reply: ViewerReply = bounded_json(response, "Linear identity response")
                .await
                .map_err(|_| AuthError::Decode("invalid Linear identity response".into()))?;
            if !reply.errors.is_empty() {
                return Err(AuthError::AuthFailed(
                    "Linear did not verify the identity".into(),
                ));
            }
            let viewer = reply.data.and_then(|data| data.viewer).ok_or_else(|| {
                AuthError::AuthFailed("Linear did not verify the identity".into())
            })?;
            if viewer.organization.id != self.workspace_id
                || !viewer.active
                || viewer.guest
                || viewer.app
                || viewer.id.is_nil()
            {
                return Err(AuthError::AuthFailed(
                    "Linear identity is not an active member of the configured workspace".into(),
                ));
            }
            let email = crate::util::normalize_email(&viewer.email);
            if !email.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty() && !domain.is_empty() && !domain.contains('@')
            }) || email
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            {
                return Err(AuthError::AuthFailed(
                    "Linear did not return an account email".into(),
                ));
            }
            Ok(ProviderExchange {
                subject: format!("{}:{}", self.workspace_id, viewer.id),
                email: Some(email),
                email_verified: Some(true),
                hosted_domain: None,
                // Inbound identity only. Never retain or forward the Linear grant.
                access_token: String::new(),
                refresh_token: None,
                expires_in: None,
                granted_scopes: Vec::new(),
                id_token: None,
            })
        }
    }

    #[cfg(test)]
    pub(crate) fn mock_provider(origin: &str) -> LinearProvider {
        let mut provider = LinearProvider::new(
            LinearConfig {
                client_id: "test-client".into(),
                workspace_id: "2a5332a8-b7a7-465e-aaae-fc59f8e65566".parse().unwrap(),
            },
            Url::parse("https://labby.example/auth/linear/callback").unwrap(),
        )
        .unwrap();
        provider.token_endpoint = Url::parse(&format!("{origin}/oauth/token")).unwrap();
        provider.graphql_endpoint = Url::parse(&format!("{origin}/graphql")).unwrap();
        provider
    }
}

#[cfg(feature = "http-axum")]
pub use client::LinearProvider;
#[cfg(all(test, feature = "http-axum"))]
pub(crate) use client::mock_provider;

#[cfg(all(test, feature = "http-axum"))]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[test]
    fn authorization_requests_only_read_scope_user_actor_and_s256() {
        let provider = mock_provider("http://127.0.0.1:3000");
        let mut request = crate::google::AuthorizeUrlRequest {
            scope: "lab:admin".into(),
            state: "flow-state".into(),
            code_challenge: "s256-challenge".into(),
            code_challenge_method: "S256".into(),
            offline_access: true,
            force_consent: true,
        };
        let url = provider.authorize_url(&request).unwrap();
        assert_eq!(url.host_str(), Some("linear.app"));
        let params: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(params["scope"], "read");
        assert_eq!(params["actor"], "user");
        assert_eq!(params["state"], "flow-state");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["code_challenge"], "s256-challenge");
        assert!(!params.contains_key("access_type"));
        request.code_challenge_method = "plain".into();
        assert!(provider.authorize_url(&request).is_err());
    }

    #[tokio::test]
    async fn rejects_wrong_workspace_guest_inactive_and_app_identities() {
        for (workspace, active, guest, app) in [
            ("36846048-b7d7-40eb-a031-bd6e36f7cd63", true, false, false),
            ("2a5332a8-b7a7-465e-aaae-fc59f8e65566", false, false, false),
            ("2a5332a8-b7a7-465e-aaae-fc59f8e65566", true, true, false),
            ("2a5332a8-b7a7-465e-aaae-fc59f8e65566", true, false, true),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/oauth/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "access_token":"test-access", "token_type":"Bearer"
                })))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"data":{"viewer":{
                        "id":"c71908ed-cbbe-4563-accb-ad5cd0fe26ea", "email":"admin@example.com",
                        "active":active,"guest":guest,"app":app,"organization":{"id":workspace}
                    }}}),
                ))
                .mount(&server)
                .await;
            assert!(matches!(
                mock_provider(&server.uri())
                    .exchange_code("code", "verifier")
                    .await,
                Err(crate::error::AuthError::AuthFailed(_))
            ));
        }
    }
}
