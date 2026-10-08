//! Callback-only fence after validated token exchange and before durable replacement.
use rmcp::transport::auth::{
    AuthError, CredentialRefreshGuard, CredentialStore, StoredCredentials,
};
use rmcp_client as rmcp;
use std::{future::Future, pin::Pin, sync::Arc};

type SaveFenceFuture = Pin<Box<dyn Future<Output = Result<Box<dyn Send>, AuthError>> + Send>>;
pub type CredentialSaveFence = Arc<dyn Fn() -> SaveFenceFuture + Send + Sync>;

pub(super) struct FencedCredentialStore<S> {
    pub(super) inner: S,
    pub(super) fence: CredentialSaveFence,
}
impl<S: CredentialStore> CredentialStore for FencedCredentialStore<S> {
    fn acquire_refresh_guard<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<CredentialRefreshGuard>, AuthError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.acquire_refresh_guard()
    }

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
        Box::pin(async move { self.inner.load().await })
    }
    fn clear<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { self.inner.clear().await })
    }
    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let _guard = (self.fence)().await?;
            self.inner.save(credentials).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::transport::auth::InMemoryCredentialStore;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn credentials() -> StoredCredentials {
        StoredCredentials::new("test".into(), None, vec![], None)
    }
    #[tokio::test]
    async fn fence_failure_prevents_credential_replacement() {
        let store = FencedCredentialStore {
            inner: InMemoryCredentialStore::default(),
            fence: Arc::new(|| {
                Box::pin(async { Err(AuthError::InternalError("revocation unavailable".into())) })
            }),
        };
        assert!(store.save(credentials()).await.is_err());
        assert!(store.load().await.unwrap().is_none());
    }
    #[tokio::test]
    async fn callback_fence_is_invoked_only_on_save() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let store = FencedCredentialStore {
            inner: InMemoryCredentialStore::default(),
            fence: Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    let held: Box<dyn Send> = Box::new(());
                    Ok(held)
                })
            }),
        };
        store.load().await.unwrap();
        store.clear().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        store.save(credentials()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(store.load().await.unwrap().is_some());
    }
}
