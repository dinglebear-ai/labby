//! Share CLI login/logout's file lock with SDK refreshes from long-lived bridges.

use std::{future::Future, path::PathBuf, pin::Pin};

use labby_auth::upstream::store::SqliteCredentialStore;
use rmcp::transport::auth::{
    AuthError, CredentialRefreshGuard, CredentialStore, StoredCredentials,
};

type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AuthError>> + Send + 'a>>;

pub(super) struct ProfileCredentialStore {
    path: PathBuf,
    upstream_name: String,
    inner: SqliteCredentialStore,
}

impl ProfileCredentialStore {
    pub(super) fn new(
        path: PathBuf,
        upstream_name: impl Into<String>,
        inner: SqliteCredentialStore,
    ) -> Self {
        Self {
            path,
            upstream_name: upstream_name.into(),
            inner,
        }
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
        Box::pin(async move {
            let profile = super::read_profile(&self.path)
                .map_err(|error| AuthError::CredentialStoreError(error.to_string()))?;
            if !profile.is_some_and(|profile| profile.upstream_name == self.upstream_name) {
                return Ok(None);
            }
            self.inner.load().await
        })
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
    async fn replaced_bridge_cannot_reload_retained_grant_after_replacement_or_logout() {
        use super::super::{
            SessionProfile, clear_profile_credentials, finish_login, publish_profile,
        };
        use labby_runtime::gateway_config::UpstreamOauthRegistration;
        use oauth2::{AccessToken, basic::BasicTokenType};
        use rmcp::transport::auth::{OAuthTokenResponse, VendorExtraTokenFields};
        let dir = tempfile::tempdir().unwrap();
        let key = super::super::private_key(dir.path(), true).unwrap();
        let sqlite = labby_auth::sqlite::SqliteStore::open(dir.path().join("oauth.db"))
            .await
            .unwrap();
        let inner = SqliteCredentialStore::new(
            sqlite.clone(),
            labby_auth::upstream::encryption::load_key(&key).unwrap(),
            "operator-server-a",
            "gateway",
        );
        inner
            .save(StoredCredentials::new(
                "fixture-client".into(),
                Some(OAuthTokenResponse::new(
                    AccessToken::new("retained-a-grant".into()),
                    BasicTokenType::Bearer,
                    VendorExtraTokenFields::default(),
                )),
                vec![],
                None,
            ))
            .await
            .unwrap();
        let profile = |name: &str| SessionProfile {
            registration: UpstreamOauthRegistration::Preregistered {
                client_id: "fixture-client".into(),
                client_secret_env: None,
            },
            upstream_name: name.into(),
        };
        publish_profile(dir.path(), &profile("operator-server-a")).unwrap();
        let old_bridge =
            ProfileCredentialStore::new(dir.path().to_owned(), "operator-server-a", inner);
        assert!(old_bridge.load().await.unwrap().is_some());
        sqlite.execute_test_statement("CREATE TRIGGER retain_old_identity BEFORE DELETE ON upstream_oauth_credentials WHEN OLD.upstream_name = 'operator-server-a' BEGIN SELECT RAISE(ABORT, 'injected old identity cleanup failure'); END;").await.unwrap();
        finish_login(
            dir.path(),
            &sqlite,
            profile("operator-server-b"),
            Some(profile("operator-server-a")),
            Ok(()),
        )
        .await
        .unwrap();
        assert!(
            old_bridge.inner.load().await.unwrap().is_some(),
            "old grant intentionally retained after failed cleanup"
        );
        assert!(
            old_bridge.load().await.unwrap().is_none(),
            "replacement must fence the old bridge even when old grant cleanup failed"
        );
        assert!(
            clear_profile_credentials(dir.path(), &sqlite)
                .await
                .unwrap()
        );
        assert!(old_bridge.inner.load().await.unwrap().is_some());
        assert!(
            old_bridge.load().await.unwrap().is_none(),
            "logout must keep the old bridge fenced"
        );
        publish_profile(dir.path(), &profile("operator-server-a")).unwrap();
        std::fs::write(dir.path().join("client.json"), b"not json").unwrap();
        assert!(
            old_bridge.load().await.is_err(),
            "malformed active profile must propagate an error"
        );
        #[cfg(unix)]
        {
            std::fs::remove_file(dir.path().join("client.json")).unwrap();
            std::os::unix::fs::symlink(
                dir.path().join("encryption.key"),
                dir.path().join("client.json"),
            )
            .unwrap();
            assert!(
                old_bridge.load().await.is_err(),
                "unsafe active profile must propagate an error"
            );
        }
    }

    #[tokio::test]
    async fn refresh_guard_excludes_login_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let key = super::super::private_key(dir.path(), true).unwrap();
        let sqlite = labby_auth::sqlite::SqliteStore::open(dir.path().join("oauth.db"))
            .await
            .unwrap();
        let store = ProfileCredentialStore::new(
            dir.path().to_owned(),
            "operator-server-test",
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
