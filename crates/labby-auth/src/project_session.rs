//! Provider-independent storage for source-bound project browser sessions.

use std::path::PathBuf;

use crate::error::AuthError;
use crate::sqlite::SqliteStore;
use crate::types::{BrowserSessionRow, ProjectSessionBinding};
use labby_primitives::product_credential::BoundAccessGrant;

#[derive(Clone)]
pub struct ProjectSessionState {
    pub store: SqliteStore,
    pub cookie_name: String,
}

impl ProjectSessionState {
    pub async fn open(path: PathBuf, cookie_name: impl Into<String>) -> Result<Self, AuthError> {
        let cookie_name = cookie_name.into();
        if !valid_cookie_name(&cookie_name) {
            return Err(AuthError::Config(
                "project session cookie name must use the __Host- prefix".into(),
            ));
        }
        Ok(Self {
            store: SqliteStore::open(path).await?,
            cookie_name,
        })
    }

    #[must_use]
    pub fn from_store(store: SqliteStore, cookie_name: impl Into<String>) -> Option<Self> {
        let cookie_name = cookie_name.into();
        valid_cookie_name(&cookie_name).then_some(Self { store, cookie_name })
    }

    pub async fn create(&self, grant: &BoundAccessGrant) -> Result<BrowserSessionRow, AuthError> {
        let created_at = crate::util::now_unix();
        let expires_at = i64::try_from(grant.expires_at).map_err(|_| {
            AuthError::Validation("credential expiry is outside the supported range".into())
        })?;
        if expires_at <= created_at {
            return Err(AuthError::AuthFailed(
                "product credential is expired".into(),
            ));
        }
        let row = BrowserSessionRow {
            session_id: crate::util::random_token(32)?,
            subject: grant.subject.clone(),
            email: None,
            csrf_token: crate::util::random_token(32)?,
            created_at,
            expires_at,
            project_binding: Some(ProjectSessionBinding::from(grant)),
        };
        self.store.upsert_browser_session(row.clone()).await?;
        Ok(row)
    }

    #[must_use]
    pub fn set_cookie(&self, session_id: &str, max_age: u64) -> String {
        format!(
            "{}={session_id}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}",
            self.cookie_name
        )
    }

    #[must_use]
    pub fn clear_cookie(&self) -> String {
        format!(
            "{}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            self.cookie_name
        )
    }
}

fn valid_cookie_name(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("__Host-") else {
        return false;
    };
    !suffix.is_empty()
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn repairs_v12_store_missing_project_binding_column() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE browser_sessions (
                    session_id TEXT PRIMARY KEY,
                    subject TEXT NOT NULL,
                    email TEXT,
                    csrf_token TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 12;",
            )
            .unwrap();
        drop(connection);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let state = ProjectSessionState::open(path, "__Host-labby-session")
            .await
            .unwrap();
        let expires_at = u64::try_from(crate::util::now_unix() + 300).unwrap();
        let grant = BoundAccessGrant {
            installation_id: "installation".into(),
            issuer: "issuer".into(),
            subject: "subject".into(),
            principal_id: "principal".into(),
            organization_id: "organization".into(),
            project_id: "project".into(),
            loadout_id: "loadout".into(),
            loadout_generation: 1,
            assignment_generation: 1,
            catalog_generation: 1,
            route_id: "route".into(),
            route_generation: 1,
            membership_epoch: 1,
            organization_policy_epoch: 1,
            project_policy_epoch: 1,
            credential_id: "credential".into(),
            credential_generation: 1,
            scopes: vec!["lab:read".into()],
            resource: "lab://project".into(),
            audience: "labby".into(),
            expires_at,
            requires_admin: false,
            destructive: false,
        };

        let session = state.create(&grant).await.unwrap();
        assert!(
            state
                .store
                .find_browser_session(&session.session_id)
                .await
                .unwrap()
                .is_some_and(|row| row.project_binding.is_some())
        );
    }
}
