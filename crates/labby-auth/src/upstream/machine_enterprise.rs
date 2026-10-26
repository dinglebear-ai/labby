//! Enterprise ID-JAG exchange through rmcp and approved OAuth HTTP clients.
use super::*;
use labby_runtime::gateway_config::{
    UpstreamEnterpriseSubjectTokenKind, UpstreamSecretAuthentication,
};
use oauth2::{AccessToken, ClientSecret, RefreshToken, Scope, basic::BasicTokenType};
use rmcp::transport::auth::enterprise::{
    EmaAuthorizationServer, EmaClientAuthentication, EmaError, EmaExchangeRequest,
};

impl UpstreamOauthManager {
    pub(super) async fn exchange_enterprise_grant(
        &self,
        metadata: &AuthorizationMetadata,
        store: &SqliteCredentialStore,
        http: &GrantHttpClient,
    ) -> Result<(), OauthError> {
        let oauth = self.oauth_config()?;
        let UpstreamOauthMode::EnterpriseManaged {
            identity_provider,
            subject_token,
            resource_authentication,
        } = &oauth.mode
        else {
            return Err(OauthError::Internal("enterprise mode is required".into()));
        };
        let UpstreamOauthRegistration::Preregistered {
            client_id,
            client_secret_env,
        } = &oauth.registration
        else {
            return Err(OauthError::Internal(
                "enterprise client metadata is required".into(),
            ));
        };
        let resource = self.upstream_url()?;
        let scopes = self.effective_scopes()?;

        let idp_metadata = discover_published_metadata(&identity_provider.issuer)
            .await?
            .ok_or_else(|| {
                OauthError::Internal(
                    "enterprise identity provider did not publish OAuth metadata".into(),
                )
            })?;
        self.verify_issuer_binding(&idp_metadata)?;
        if idp_metadata.issuer.as_deref() != Some(identity_provider.issuer.as_str())
            || idp_metadata.token_endpoint != identity_provider.token_endpoint
        {
            return Err(OauthError::IssuerMismatch("enterprise identity-provider metadata differs from the approved issuer or token endpoint".into()));
        }
        let auth = |method: UpstreamSecretAuthentication, value: String| match method {
            UpstreamSecretAuthentication::ClientSecretBasic => {
                EmaClientAuthentication::ClientSecretBasic(ClientSecret::new(value))
            }
            UpstreamSecretAuthentication::ClientSecretPost => {
                EmaClientAuthentication::ClientSecretPost(ClientSecret::new(value))
            }
        };
        let idp = EmaAuthorizationServer::new(
            &identity_provider.issuer,
            &identity_provider.token_endpoint,
            &identity_provider.client_id,
        )
        .with_client_authentication(auth(
            identity_provider.authentication,
            secret(&identity_provider.client_secret_env)?,
        ));
        let resource_as = EmaAuthorizationServer::new(
            metadata.issuer.as_deref().ok_or_else(|| {
                OauthError::IssuerMismatch("resource authorization metadata has no issuer".into())
            })?,
            &metadata.token_endpoint,
            client_id,
        )
        .with_client_authentication(auth(
            *resource_authentication,
            secret(client_secret_env.as_deref().ok_or_else(|| {
                OauthError::Internal("enterprise resource client_secret_env is required".into())
            })?)?,
        ));
        let subject = secret(&subject_token.token_env)?;
        let refresh = RefreshToken::new(subject.clone());
        let request = match subject_token.kind {
            UpstreamEnterpriseSubjectTokenKind::RefreshToken => {
                EmaExchangeRequest::new(idp, resource_as, resource.as_str(), &refresh)
            }
            UpstreamEnterpriseSubjectTokenKind::IdToken => {
                EmaExchangeRequest::new_with_id_token(idp, resource_as, resource.as_str(), &subject)
            }
        }
        .with_scopes(scopes);
        let idp_http = GrantHttpClient {
            inner: TrustedOriginOAuthHttpClient::new(&identity_provider.issuer)?,
            rejected: AtomicBool::new(false),
            failure: std::sync::Mutex::new(None),
            invalid_expiry: AtomicBool::new(false),
        };
        let result = request.exchange(&idp_http, http).await;
        if let Some(kind) = *idp_http
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            *http
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(kind);
        }
        let result = match result {
            Ok(result) => result,
            Err(EmaError::InvalidGrant(_)) => {
                http.rejected.store(true, Ordering::Release);
                return Err(OauthError::NeedsReauth(
                    "enterprise provider rejected the configured subject token".into(),
                ));
            }
            Err(_) => {
                return Err(OauthError::NeedsReauth(
                    "enterprise token exchange failed; check approved provider metadata and configured credentials"
                        .into(),
                ));
            }
        };
        let mut granted = result.scopes.into_iter().collect::<Vec<_>>();
        granted.sort();
        let mut token = OAuthTokenResponse::new(
            AccessToken::new(result.access_token.secret().to_owned()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        token.set_expires_in(result.expires_in.as_ref());
        token.set_scopes(Some(granted.iter().cloned().map(Scope::new).collect()));
        store
            .save(
                StoredCredentials::new(
                    client_id.clone(),
                    Some(token),
                    granted,
                    Some(now_unix()? as u64),
                )
                .with_issuer(metadata.issuer.clone()),
            )
            .await
            .map_err(map_auth_error)?;
        Ok(())
    }
}
