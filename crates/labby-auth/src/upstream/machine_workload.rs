//! Workload JWT bearer grants through the approved SDK OAuth client.

use super::*;
use oauth2::{AccessToken, Scope, basic::BasicTokenType};
use rmcp::transport::auth::enterprise::{EmaAuthorizationServer, EmaError, JwtBearerGrantRequest};

impl UpstreamOauthManager {
    pub(super) async fn exchange_workload_grant(
        &self,
        metadata: &AuthorizationMetadata,
        store: &SqliteCredentialStore,
        http: &GrantHttpClient,
    ) -> Result<(), OauthError> {
        let oauth = self.oauth_config()?;
        let UpstreamOauthMode::WorkloadIdentityFederation { assertion_env } = &oauth.mode else {
            return Err(OauthError::Internal(
                "workload identity mode is required".into(),
            ));
        };
        let UpstreamOauthRegistration::Preregistered {
            client_id,
            client_secret_env: None,
        } = &oauth.registration
        else {
            return Err(OauthError::Internal(
                "public workload client metadata is required".into(),
            ));
        };
        let grant = "urn:ietf:params:oauth:grant-type:jwt-bearer";
        let advertised = |field: &str, value: &str| {
            metadata
                .additional_fields
                .get(field)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|values| values.iter().any(|item| item.as_str() == Some(value)))
        };
        if !advertised("grant_types_supported", grant)
            || !advertised("token_endpoint_auth_methods_supported", "none")
        {
            return Err(OauthError::NeedsReauth(
                "authorization metadata does not approve the configured public workload grant"
                    .into(),
            ));
        }
        let issuer = metadata.issuer.as_deref().ok_or_else(|| {
            OauthError::IssuerMismatch("workload authorization metadata has no issuer".into())
        })?;
        let server = EmaAuthorizationServer::new(issuer, &metadata.token_endpoint, client_id);
        let result = JwtBearerGrantRequest::new(
            server,
            self.upstream_url()?.as_str(),
            secret(assertion_env)?,
        )
        .with_scopes(self.effective_scopes()?)
        .exchange(http)
        .await;
        let result = match result {
            Ok(result) => result,
            Err(EmaError::InvalidGrant(_)) => {
                http.rejected.store(true, Ordering::Release);
                return Err(OauthError::NeedsReauth("workload provider rejected the configured assertion".into()));
            }
            Err(_) => return Err(OauthError::NeedsReauth("workload assertion exchange failed; check approved provider metadata and configured assertion".into())),
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
