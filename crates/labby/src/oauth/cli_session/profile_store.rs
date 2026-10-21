//! Share CLI login/logout's file lock with SDK refreshes from long-lived bridges.

use std::{future::Future, path::PathBuf, pin::Pin};

use labby_auth::upstream::store::SqliteCredentialStore;
use rmcp::transport::auth::{
    AuthError, CredentialRefreshGuard, CredentialStore, StoredCredentials,
};

type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AuthError>> + Send + 'a>>;

pub(super) struct ProfileCredentialStore {
    path: PathBuf,
    inner: SqliteCredentialStore,
}

impl ProfileCredentialStore {
    pub(super) fn new(path: PathBuf, inner: SqliteCredentialStore) -> Self {
        Self { path, inner }
    }
}

impl CredentialStore for ProfileCredentialStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> StoreFuture<'async_trait, Option<StoredCredentials>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.load()
    }

    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> StoreFuture<'async_trait, ()>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.save(credentials)
    }

    fn clear<'life0, 'async_trait>(&'life0 self) -> StoreFuture<'async_trait, ()>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.inner.clear()
    }

    fn acquire_refresh_guard<'life0, 'async_trait>(
        &'life0 self,
    ) -> StoreFuture<'async_trait, Option<CredentialRefreshGuard>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            // Match startup/login ordering: file lock, then the SDK store guard.
            let file = super::lock_profile(&self.path)
                .await
                .map_err(|error| AuthError::CredentialStoreError(error.to_string()))?;
            let inner = self.inner.acquire_refresh_guard().await?;
            Ok(Some(CredentialRefreshGuard::new((file, inner))))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refresh_guard_excludes_login_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let key = super::super::private_key(dir.path(), true).unwrap();
        let sqlite = labby_auth::sqlite::SqliteStore::open(dir.path().join("oauth.db"))
            .await
            .unwrap();
        let store = ProfileCredentialStore::new(
            dir.path().to_owned(),
            SqliteCredentialStore::new(
                sqlite,
                labby_auth::upstream::encryption::load_key(&key).unwrap(),
                "operator-server-test",
                "gateway",
            ),
        );
        let guard = store.acquire_refresh_guard().await.unwrap();
        let contender =
            labby_auth::util::open_restricted_lock_file(&dir.path().join("session.lock")).unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(guard);
        contender.try_lock().unwrap();
    }
}
