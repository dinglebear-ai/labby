//! Credential-store adapters for SDK-backed unattended grants.
use super::*;
use std::future::Future;
use std::pin::Pin;

#[derive(Clone)]
pub(super) struct GrantStore(pub(super) Arc<SqliteCredentialStore>);
impl CredentialStore for GrantStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<StoredCredentials>, AuthError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.0.load().await })
    }
    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.0.save(credentials).await })
    }
    fn clear<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Err(AuthError::AuthorizationRequired) })
    }
}

/// Every SDK token read enters the existing credential guard and checks real expiry.
/// Only the returned SDK view omits expiry: renewal belongs to this store, since
/// these grants do not provide an OAuth refresh token. Persisted expiry is unchanged.
pub(super) struct RenewingGrantStore {
    pub(super) manager: UpstreamOauthManager,
    pub(super) subject: String,
}
impl CredentialStore for RenewingGrantStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<StoredCredentials>, AuthError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let (mut credentials, _) = self
                .manager
                .unattended_credentials(&self.subject, 30)
                .await
                .map_err(sdk_error)?;
            credentials
                .token_response
                .as_mut()
                .ok_or(AuthError::AuthorizationRequired)?
                .set_expires_in(None);
            Ok(Some(credentials))
        })
    }
    fn save<'life0, 'async_trait>(
        &'life0 self,
        _credentials: StoredCredentials,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async {
            Err(AuthError::CredentialStoreError(
                "unattended grants own token persistence".into(),
            ))
        })
    }
    fn clear<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Err(AuthError::AuthorizationRequired) })
    }
}
