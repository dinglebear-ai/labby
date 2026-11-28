//! Linear sign-in through the operator's Linear MCP upstream (the Notification
//! Worker). One consent proves the user's identity and leaves an upstream grant
//! that the gateway uses for that user's MCP calls.

use std::sync::Arc;
use std::time::Duration;

use rmcp_client as rmcp;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use crate::config_providers::LinearMcpLogin;
use crate::error::AuthError;
use crate::google::AuthorizeUrlRequest;
use crate::oauth_provider::{ProviderExchange, UpstreamGrant};
use crate::provider_http::bounded_json;
use crate::sqlite::SqliteStore;
use crate::upstream::encryption::EncryptionKey;

#[derive(Clone, Debug)]
pub(crate) struct McpLogin {
    upstream: String,
    url: Url,
    redirect_uri: Url,
    key: EncryptionKey,
    http: reqwest::Client,
    client: Arc<tokio::sync::OnceCell<Registration>>,
}

#[derive(Clone, Debug)]
struct Registration {
    client_id: String,
    issuer: Option<String>,
    authorization_endpoint: Url,
    token_endpoint: Url,
}

#[derive(Deserialize)]
struct Metadata {
    issuer: Option<String>,
    authorization_endpoint: Url,
    token_endpoint: Url,
    registration_endpoint: Option<Url>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

#[derive(Deserialize)]
struct RegistrationReply {
    client_id: String,
}

#[derive(Deserialize)]
struct LinearUser {
    id: Uuid,
    email: String,
}

impl McpLogin {
    pub(crate) fn new(
        config: LinearMcpLogin,
        redirect_uri: Url,
        key: EncryptionKey,
        http: reqwest::Client,
    ) -> Self {
        Self {
            upstream: config.upstream,
            url: config.url,
            redirect_uri,
            key,
            http,
            client: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    // Endpoints must stay on the upstream's own origin so the metadata
    // document cannot send the browser or the code to another host.
    fn same_origin(&self, url: &Url) -> Result<(), AuthError> {
        if url.origin() == self.url.origin() {
            Ok(())
        } else {
            Err(AuthError::Config(
                "Linear MCP OAuth endpoints must share the upstream origin".into(),
            ))
        }
    }

    async fn registration(&self) -> Result<&Registration, AuthError> {
        self.client
            .get_or_try_init(|| async {
                let mut well_known = self.url.clone();
                well_known.set_path("/.well-known/oauth-authorization-server");
                well_known.set_query(None);
                let response =
                    self.http.get(well_known).send().await.map_err(|_| {
                        AuthError::Network("Linear MCP OAuth discovery failed".into())
                    })?;
                if !response.status().is_success() {
                    return Err(AuthError::Network(
                        "Linear MCP OAuth discovery failed".into(),
                    ));
                }
                let metadata: Metadata =
                    bounded_json(response, "Linear MCP OAuth metadata").await?;
                let registration_endpoint = metadata.registration_endpoint.ok_or_else(|| {
                    AuthError::Config(
                        "Linear MCP upstream does not support client registration".into(),
                    )
                })?;
                if !metadata
                    .code_challenge_methods_supported
                    .iter()
                    .any(|m| m == "S256")
                {
                    return Err(AuthError::Config(
                        "Linear MCP upstream does not support S256 PKCE".into(),
                    ));
                }
                for endpoint in [
                    &metadata.authorization_endpoint,
                    &metadata.token_endpoint,
                    &registration_endpoint,
                ] {
                    self.same_origin(endpoint)?;
                }
                let response = self
                    .http
                    .post(registration_endpoint)
                    .json(&serde_json::json!({
                        "client_name": "Labby",
                        "redirect_uris": [self.redirect_uri.as_str()],
                        "grant_types": ["authorization_code", "refresh_token"],
                        "response_types": ["code"],
                        "token_endpoint_auth_method": "none",
                    }))
                    .send()
                    .await
                    .map_err(|_| {
                        AuthError::Network("Linear MCP client registration failed".into())
                    })?;
                if !response.status().is_success() {
                    return Err(AuthError::Config(
                        "Linear MCP upstream rejected client registration".into(),
                    ));
                }
                let reply: RegistrationReply =
                    bounded_json(response, "Linear MCP registration").await?;
                Ok(Registration {
                    client_id: reply.client_id,
                    issuer: metadata.issuer,
                    authorization_endpoint: metadata.authorization_endpoint,
                    token_endpoint: metadata.token_endpoint,
                })
            })
            .await
    }

    pub(crate) async fn authorize_url(
        &self,
        request: &AuthorizeUrlRequest,
    ) -> Result<Url, AuthError> {
        let registration = self.registration().await?;
        let mut url = registration.authorization_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("client_id", &registration.client_id)
            .append_pair("redirect_uri", self.redirect_uri.as_str())
            .append_pair("response_type", "code")
            .append_pair("state", &request.state)
            .append_pair("code_challenge", &request.code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", self.url.as_str());
        Ok(url)
    }

    /// Exchange the code, then prove who the grant belongs to by asking
    /// Linear's `get_user("me")` through the same upstream. The upstream
    /// admits only its allowed workspace, so `workspace_id` is the operator's
    /// statement of that workspace, not something this call can check.
    pub(crate) async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        workspace_id: Uuid,
    ) -> Result<ProviderExchange, AuthError> {
        let registration = self.registration().await?;
        let response = self
            .http
            .post(registration.token_endpoint.clone())
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", &registration.client_id),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code", code),
                ("code_verifier", verifier),
                ("resource", self.url.as_str()),
            ])
            .send()
            .await
            .map_err(|_| AuthError::Network("Linear MCP token exchange failed".into()))?;
        if !response.status().is_success() {
            return Err(AuthError::InvalidGrant(
                "Linear MCP rejected the authorization code".into(),
            ));
        }
        let token: serde_json::Value = bounded_json(response, "Linear MCP token response").await?;
        let access_token = token
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .filter(|_| {
                token
                    .get("token_type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("bearer"))
            })
            .ok_or_else(|| {
                AuthError::InvalidGrant("Linear MCP did not return a bearer token".into())
            })?
            .to_string();
        let user = self.current_user(&access_token).await?;
        if user.id.is_nil() {
            return Err(AuthError::AuthFailed(
                "Linear MCP did not verify the identity".into(),
            ));
        }
        let email = crate::util::normalize_email(&user.email);
        if !email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@')
        }) || email
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err(AuthError::AuthFailed(
                "Linear MCP did not return an account email".into(),
            ));
        }
        Ok(ProviderExchange {
            subject: format!("{workspace_id}:{}", user.id),
            email: Some(email),
            email_verified: Some(true),
            hosted_domain: None,
            access_token: String::new(),
            refresh_token: None,
            expires_in: None,
            granted_scopes: Vec::new(),
            id_token: None,
            upstream_grant: Some(Box::new(UpstreamGrant {
                client_id: registration.client_id.clone(),
                issuer: registration.issuer.clone(),
                token,
            })),
        })
    }

    async fn current_user(&self, access_token: &str) -> Result<LinearUser, AuthError> {
        use rmcp::ServiceExt;
        use rmcp::model::CallToolRequestParams;
        use rmcp::transport::StreamableHttpClientTransport;
        use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

        let failed = |_| AuthError::AuthFailed("Linear MCP did not verify the identity".into());
        let mut config = StreamableHttpClientTransportConfig::with_uri(self.url.to_string());
        config.auth_header = Some(access_token.to_string());
        let transport = StreamableHttpClientTransport::with_client(self.http.clone(), config);
        let call = async {
            let client = ().serve(transport).await.map_err(|_| ())?;
            let mut arguments = serde_json::Map::new();
            arguments.insert("query".into(), "me".into());
            let result = client
                .call_tool(CallToolRequestParams::new("get_user").with_arguments(arguments))
                .await
                .map_err(|_| ());
            client.cancel().await.ok();
            result
        };
        let result = tokio::time::timeout(Duration::from_secs(15), call)
            .await
            .map_err(|_| AuthError::Network("Linear MCP identity lookup timed out".into()))?
            .map_err(failed)?;
        if result.is_error == Some(true) {
            return Err(failed(()));
        }
        if let Some(value) = result.structured_content {
            if let Ok(user) = serde_json::from_value(value) {
                return Ok(user);
            }
        }
        result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .find_map(|text| serde_json::from_str(&text.text).ok())
            .ok_or_else(|| failed(()))
    }

    /// Store the admitted grant for the gateway. Admin sessions use the
    /// gateway's shared upstream credential, everyone else their own subject,
    /// matching how the gateway selects a credential for each caller.
    pub(crate) async fn save_grant(
        &self,
        store: &SqliteStore,
        subject: &str,
        scope: &str,
        grant: UpstreamGrant,
    ) -> Result<(), AuthError> {
        use rmcp::transport::auth::{CredentialStore, OAuthTokenResponse, StoredCredentials};

        let subject = if scope.split_whitespace().any(|s| s == "lab:admin") {
            "gateway"
        } else {
            subject
        };
        let token: OAuthTokenResponse = serde_json::from_value(grant.token)
            .map_err(|_| AuthError::Decode("invalid Linear MCP token response".into()))?;
        let granted_scopes = oauth2::TokenResponse::scopes(&token)
            .map(|scopes| scopes.iter().map(|scope| scope.to_string()).collect())
            .unwrap_or_default();
        let received = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        crate::upstream::store::SqliteCredentialStore::new(
            store.clone(),
            self.key.clone(),
            &self.upstream,
            subject,
        )
        .save(
            StoredCredentials::new(grant.client_id, Some(token), granted_scopes, Some(received))
                .with_issuer(grant.issuer),
        )
        .await
        .map_err(|error| AuthError::Storage(format!("save Linear MCP grant: {error}")))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, Request, ResponseTemplate,
        matchers::{body_string_contains, method, path},
    };

    pub(crate) fn test_key() -> EncryptionKey {
        use base64::Engine as _;
        crate::upstream::encryption::load_key(
            &base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        )
        .unwrap()
    }

    pub(crate) fn mock_provider(origin: &str, workspace: &str) -> crate::linear::LinearProvider {
        crate::linear::LinearProvider::new(
            crate::config::LinearConfig {
                client_id: String::new(),
                mcp: Some(LinearMcpLogin {
                    upstream: "linear-qa".into(),
                    url: Url::parse(&format!("{origin}/mcp")).unwrap(),
                }),
                workspace_id: workspace.parse().unwrap(),
            },
            Url::parse("https://labby.example/auth/linear/callback").unwrap(),
            Some(test_key()),
        )
        .unwrap()
    }

    /// A Notification Worker stand-in: OAuth metadata, registration, token
    /// exchange, and an MCP endpoint whose `get_user` answers for `email`.
    pub(crate) async fn mount_worker(server: &MockServer, user_id: &str, email: &str) {
        let origin = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": origin,
                "authorization_endpoint": format!("{origin}/authorize"),
                "token_endpoint": format!("{origin}/token"),
                "registration_endpoint": format!("{origin}/register"),
                "code_challenge_methods_supported": ["S256"],
            })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/register"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({"client_id": "worker-client"})),
            )
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("client_id=worker-client"))
            .and(body_string_contains("code_verifier=upstream-verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "worker-access", "token_type": "Bearer",
                "refresh_token": "worker-refresh", "expires_in": 3600,
            })))
            .mount(server)
            .await;
        let user =
            serde_json::json!({"id": user_id, "email": email, "name": "Fixture"}).to_string();
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(move |request: &Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let authorized = request
                    .headers
                    .get("authorization")
                    .is_some_and(|value| value == "Bearer worker-access");
                if !authorized {
                    return ResponseTemplate::new(401);
                }
                let result = match body["method"].as_str() {
                    Some("initialize") => serde_json::json!({
                        "protocolVersion": body["params"]["protocolVersion"],
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "worker", "version": "1"},
                    }),
                    Some("tools/call")
                        if body["params"]["name"] == "get_user"
                            && body["params"]["arguments"]["query"] == "me" =>
                    {
                        serde_json::json!({"content": [{"type": "text", "text": user}]})
                    }
                    Some(_) if body.get("id").is_some() => {
                        return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                            "jsonrpc": "2.0", "id": body["id"],
                            "error": {"code": -32601, "message": "unexpected"},
                        }));
                    }
                    _ => return ResponseTemplate::new(202),
                };
                ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"jsonrpc": "2.0", "id": body["id"], "result": result}),
                )
            })
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn authorize_url_registers_once_and_names_the_upstream_resource() {
        let server = MockServer::start().await;
        mount_worker(
            &server,
            "c71908ed-cbbe-4563-accb-ad5cd0fe26ea",
            "a@example.com",
        )
        .await;
        let provider = mock_provider(&server.uri(), "2a5332a8-b7a7-465e-aaae-fc59f8e65566");
        let request = AuthorizeUrlRequest {
            scope: "lab".into(),
            state: "flow-state".into(),
            code_challenge: "s256-challenge".into(),
            code_challenge_method: "S256".into(),
            offline_access: false,
            force_consent: false,
        };
        for _ in 0..2 {
            let url = provider.authorize_url(&request).await.unwrap();
            assert_eq!(url.path(), "/authorize");
            let params: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
            assert_eq!(params["client_id"], "worker-client");
            assert_eq!(params["state"], "flow-state");
            assert_eq!(params["resource"], format!("{}/mcp", server.uri()));
            assert_eq!(
                params["redirect_uri"],
                "https://labby.example/auth/linear/callback"
            );
        }
    }

    #[tokio::test]
    async fn rejects_metadata_that_points_at_another_origin() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "authorization_endpoint": "https://elsewhere.example/authorize",
                "token_endpoint": format!("{}/token", server.uri()),
                "registration_endpoint": format!("{}/register", server.uri()),
                "code_challenge_methods_supported": ["S256"],
            })))
            .mount(&server)
            .await;
        let provider = mock_provider(&server.uri(), "2a5332a8-b7a7-465e-aaae-fc59f8e65566");
        let request = AuthorizeUrlRequest {
            scope: "lab".into(),
            state: "s".into(),
            code_challenge: "c".into(),
            code_challenge_method: "S256".into(),
            offline_access: false,
            force_consent: false,
        };
        assert!(matches!(
            provider.authorize_url(&request).await,
            Err(AuthError::Config(_))
        ));
    }

    #[tokio::test]
    async fn exchange_fails_closed_when_get_user_does_not_answer() {
        let server = MockServer::start().await;
        mount_worker(&server, "not-a-uuid", "a@example.com").await;
        let provider = mock_provider(&server.uri(), "2a5332a8-b7a7-465e-aaae-fc59f8e65566");
        assert!(matches!(
            provider.exchange_code("code", "upstream-verifier").await,
            Err(AuthError::AuthFailed(_))
        ));
    }

    #[tokio::test]
    async fn admin_grants_go_to_the_shared_gateway_credential() {
        let server = MockServer::start().await;
        mount_worker(
            &server,
            "c71908ed-cbbe-4563-accb-ad5cd0fe26ea",
            "a@example.com",
        )
        .await;
        let provider = mock_provider(&server.uri(), "2a5332a8-b7a7-465e-aaae-fc59f8e65566");
        let exchange = provider
            .exchange_code("code", "upstream-verifier")
            .await
            .unwrap();
        let store =
            SqliteStore::open_with_key(tempfile::tempdir().unwrap().keep().join("auth.db"), None)
                .await
                .unwrap();
        provider
            .mcp
            .as_ref()
            .unwrap()
            .save_grant(
                &store,
                &exchange.subject,
                "lab lab:admin",
                *exchange.upstream_grant.unwrap(),
            )
            .await
            .unwrap();
        assert!(
            store
                .find_upstream_oauth_credentials("linear-qa", "gateway")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .find_upstream_oauth_credentials("linear-qa", &exchange.subject)
                .await
                .unwrap()
                .is_none()
        );
    }
}
