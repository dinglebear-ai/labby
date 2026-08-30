use std::path::PathBuf;

use rusqlite::params;

use rusqlite::Connection;

use crate::at_rest::TokenEncryptionKey;
use crate::jwt::{AccessClaims, SigningKeys};
use crate::types::{
    AllowedUserRow, AuthorizationCodeRow, BrowserSessionRow, GoogleProviderCredentialUpdate,
    RefreshTokenRow, RegisteredClient, UpstreamOauthCredentialRow, UpstreamOauthStateRow,
};

use crate::util::now_unix;

use super::{SQLITE_POOL_SIZE, SqliteStore, hash_token, migrations, sqlite_error};

#[tokio::test]
async fn sqlite_store_enables_wal_and_busy_timeout() {
    let store = temp_store().await;
    assert_eq!(pragma(&store, "journal_mode").await, "wal");
    assert!(pragma_ms(&store, "busy_timeout").await >= 5_000);
}

#[tokio::test]
async fn sqlite_store_opens_multiple_connections() {
    let store = temp_store().await;
    assert_eq!(store.connection_count(), SQLITE_POOL_SIZE);
}

#[tokio::test]
async fn sqlite_store_redeems_auth_code_only_once_under_race() {
    let store = temp_store().await;
    store.insert_auth_code(sample_code()).await.unwrap();
    let (a, b) = tokio::join!(
        store.redeem_auth_code("code-123"),
        store.redeem_auth_code("code-123"),
    );
    assert!(a.is_ok() ^ b.is_ok(), "a={a:?} b={b:?}");
}

#[tokio::test]
async fn sqlite_store_does_not_consume_auth_code_when_grant_verification_fails() {
    let store = temp_store().await;
    store.insert_auth_code(sample_code()).await.unwrap();

    for (client_id, redirect_uri, resource, challenge, method) in [
        (
            "wrong-client",
            "http://127.0.0.1:7777/callback",
            "https://lab.example.com/mcp",
            "challenge",
            "S256",
        ),
        (
            "client",
            "https://attacker.example/callback",
            "https://lab.example.com/mcp",
            "challenge",
            "S256",
        ),
        (
            "client",
            "http://127.0.0.1:7777/callback",
            "https://other.example/mcp",
            "challenge",
            "S256",
        ),
        (
            "client",
            "http://127.0.0.1:7777/callback",
            "https://lab.example.com/mcp",
            "wrong-challenge",
            "S256",
        ),
        (
            "client",
            "http://127.0.0.1:7777/callback",
            "https://lab.example.com/mcp",
            "challenge",
            "plain",
        ),
    ] {
        assert!(
            store
                .redeem_verified_auth_code(
                    "code-123",
                    client_id,
                    redirect_uri,
                    Some(resource),
                    challenge,
                    method,
                )
                .await
                .is_err()
        );
    }

    let redeemed = store
        .redeem_verified_auth_code(
            "code-123",
            "client",
            "http://127.0.0.1:7777/callback",
            Some("https://lab.example.com/mcp"),
            "challenge",
            "S256",
        )
        .await
        .unwrap();
    assert_eq!(redeemed.code, "code-123");
}

#[cfg(unix)]
#[tokio::test]
async fn sqlite_store_refuses_world_readable_database_file() {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_db_path();
    std::fs::write(&path, []).unwrap();
    std::fs::set_permissions(&path, PermissionsExt::from_mode(0o644)).unwrap();
    let err = SqliteStore::open(path).await.unwrap_err();
    assert!(err.to_string().contains("permissions"));
}

#[tokio::test]
async fn sqlite_store_rejects_expired_authorization_code() {
    let store = temp_store().await;
    let mut code = sample_code();
    code.expires_at = now_unix() - 1;
    store.insert_auth_code(code).await.unwrap();
    let err = store.redeem_auth_code("code-123").await.unwrap_err();
    assert!(err.to_string().contains("expired"));
}

#[tokio::test]
async fn sqlite_store_ignores_expired_refresh_token() {
    let store = temp_store().await;
    register_test_client(&store, "client").await;
    store
        .upsert_refresh_token(RefreshTokenRow {
            refresh_token: "refresh-token".to_string(),
            client_id: "client".to_string(),
            subject: "google-user".to_string(),
            resource: "https://lab.example.com/mcp".to_string(),
            scope: "lab".to_string(),
            provider_refresh_token: Some("provider-refresh".to_string()),
            created_at: now_unix() - 300,
            expires_at: now_unix() - 1,
        })
        .await
        .unwrap();
    assert!(
        store
            .find_refresh_token("refresh-token")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn has_refresh_token_for_client_reflects_unexpired_rows_only() {
    let store = temp_store().await;
    register_test_client(&store, "client").await;
    assert!(!store.has_refresh_token_for_client("client").await.unwrap());

    let mut expired = sample_refresh_token("client", "expired-refresh");
    expired.created_at = now_unix() - 300;
    expired.expires_at = now_unix() - 1;
    store.upsert_refresh_token(expired).await.unwrap();
    assert!(
        !store.has_refresh_token_for_client("client").await.unwrap(),
        "an expired-only store should not count as having a refresh token"
    );

    store
        .upsert_refresh_token(sample_refresh_token("client", "live-refresh"))
        .await
        .unwrap();
    assert!(store.has_refresh_token_for_client("client").await.unwrap());
}

#[tokio::test]
async fn has_refresh_token_for_client_does_not_leak_across_clients() {
    let store = temp_store().await;
    // Only "codex-client" is registered — "claude-client" deliberately
    // isn't, since it's only ever queried below, never inserted.
    register_test_client(&store, "codex-client").await;
    store
        .upsert_refresh_token(sample_refresh_token("codex-client", "codex-refresh"))
        .await
        .unwrap();

    assert!(
        store
            .has_refresh_token_for_client("codex-client")
            .await
            .unwrap()
    );
    assert!(
        !store
            .has_refresh_token_for_client("claude-client")
            .await
            .unwrap(),
        "a brand-new client must not inherit another client's refresh-token state"
    );
}

#[tokio::test]
async fn google_provider_credential_is_subject_scoped_and_email_reusable_across_clients() {
    let store = temp_store().await;
    store
        .upsert_google_provider_credential(
            "google-subject-123",
            Some("Admin@Example.com"),
            "provider-refresh",
        )
        .await
        .unwrap();

    let credential = store
        .find_google_provider_credential("google-subject-123")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(credential.refresh_token, "provider-refresh");
    assert_eq!(credential.email.as_deref(), Some("admin@example.com"));
    assert_eq!(credential.generation, 1);
    assert!(
        store
            .has_google_provider_credential_for_subject("google-subject-123")
            .await
            .unwrap()
    );
    assert!(
        store
            .has_google_provider_credential_for_email("ADMIN@example.com")
            .await
            .unwrap(),
        "one verified account credential must be reusable across downstream client IDs"
    );
    assert!(
        !store
            .has_google_provider_credential_for_email("other@example.com")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn google_provider_token_bundle_round_trips_scopes_access_token_and_metadata() {
    let store = temp_store().await;
    let now = now_unix();
    store
        .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
            subject: "google-subject-123".to_string(),
            email: Some("Admin@Example.com".to_string()),
            client_id: "google-client".to_string(),
            granted_scopes: vec![
                "profile".to_string(),
                "openid".to_string(),
                "profile".to_string(),
            ],
            access_token: "provider-access".to_string(),
            refresh_token: "provider-refresh".to_string(),
            token_received_at: now,
            access_token_expires_at: now + 3600,
            issuer: Some("https://accounts.google.com".to_string()),
            refreshed: false,
            scope_upgraded: true,
        })
        .await
        .unwrap();

    let row = store
        .find_google_provider_credential_by_selector(Some("ADMIN@example.com"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.subject, "google-subject-123");
    assert_eq!(row.email.as_deref(), Some("admin@example.com"));
    assert_eq!(row.client_id, "google-client");
    assert_eq!(row.granted_scopes, vec!["openid", "profile"]);
    assert_eq!(row.access_token.as_deref(), Some("provider-access"));
    assert_eq!(row.refresh_token, "provider-refresh");
    assert_eq!(row.token_received_at, Some(now));
    assert_eq!(row.access_token_expires_at, Some(now + 3600));
    assert_eq!(row.issuer.as_deref(), Some("https://accounts.google.com"));
    let persisted_scope_upgrade = row.last_scope_upgrade_at.expect("scope upgrade timestamp");
    assert!(
        (now..=now_unix()).contains(&persisted_scope_upgrade),
        "scope upgrade timestamp must be recorded during the write"
    );
}

#[tokio::test]
async fn google_provider_token_bundle_cas_rejects_a_stale_generation() {
    let store = temp_store().await;
    let now = now_unix();
    let update = |access_token: &str, refresh_token: &str| GoogleProviderCredentialUpdate {
        subject: "google-subject-cas".to_string(),
        email: Some("admin@example.com".to_string()),
        client_id: "google-client".to_string(),
        granted_scopes: vec!["openid".to_string()],
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        token_received_at: now,
        access_token_expires_at: now + 3600,
        issuer: Some("https://accounts.google.com".to_string()),
        refreshed: true,
        scope_upgraded: false,
    };

    store
        .upsert_google_provider_token_bundle(update("access-v1", "refresh-v1"))
        .await
        .unwrap();
    let original = store
        .find_google_provider_credential("google-subject-cas")
        .await
        .unwrap()
        .unwrap();

    assert!(
        store
            .replace_google_provider_token_bundle_if_generation(
                update("access-v2", "refresh-v2"),
                original.generation,
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .replace_google_provider_token_bundle_if_generation(
                update("stale-access", "stale-refresh"),
                original.generation,
            )
            .await
            .unwrap(),
        "a stale exchange must not overwrite the newer provider generation"
    );
    let current = store
        .find_google_provider_credential("google-subject-cas")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.generation, original.generation + 1);
    assert_eq!(current.access_token.as_deref(), Some("access-v2"));
    assert_eq!(current.refresh_token, "refresh-v2");
    assert_eq!(current.granted_scopes, vec!["openid".to_string()]);
}

#[tokio::test]
async fn google_provider_revoke_epoch_prevents_stale_cross_store_recreation() {
    let path = temp_db_path();
    let key = TokenEncryptionKey::from_passphrase("revocation-cross-store-key");
    let writer = SqliteStore::open_with_key(path.clone(), Some(key.clone()))
        .await
        .unwrap();
    let revoker = SqliteStore::open_with_key(path, Some(key)).await.unwrap();
    let now = now_unix();
    let update = GoogleProviderCredentialUpdate {
        subject: "google-subject-revoked".to_string(),
        email: Some("admin@example.com".to_string()),
        client_id: "google-client".to_string(),
        granted_scopes: vec!["openid".to_string()],
        access_token: "stale-access".to_string(),
        refresh_token: "stale-refresh".to_string(),
        token_received_at: now,
        access_token_expires_at: now + 3600,
        issuer: Some("https://accounts.google.com".to_string()),
        refreshed: false,
        scope_upgraded: true,
    };
    writer
        .upsert_google_provider_token_bundle(update.clone())
        .await
        .unwrap();
    let observed_epoch = writer
        .google_provider_revocation_epoch("google-subject-revoked")
        .await
        .unwrap();
    let generation = writer
        .find_google_provider_credential("google-subject-revoked")
        .await
        .unwrap()
        .unwrap()
        .generation;

    assert!(
        revoker
            .invalidate_google_provider_credential("google-subject-revoked", generation)
            .await
            .unwrap()
            .invalidated
    );
    assert!(
        !writer
            .insert_google_provider_token_bundle_if_absent(update.clone(), observed_epoch)
            .await
            .unwrap(),
        "a writer paused before revoke must not recreate the deleted credential"
    );
    assert!(
        writer
            .find_google_provider_credential("google-subject-revoked")
            .await
            .unwrap()
            .is_none()
    );
    let new_authorization_epoch = writer.google_provider_fence_epoch().await.unwrap();
    assert!(
        writer
            .insert_google_provider_token_bundle_if_absent(update, new_authorization_epoch,)
            .await
            .unwrap(),
        "an authorization deliberately started after revoke must use the current fence"
    );
}

#[tokio::test]
async fn google_provider_bundle_is_encrypted_at_rest_and_readable_after_reopen() {
    let path = temp_db_path();
    let key = TokenEncryptionKey::from_passphrase("google-broker-test-key");
    let store = SqliteStore::open_with_key(path.clone(), Some(key.clone()))
        .await
        .unwrap();
    let now = now_unix();
    store
        .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
            subject: "google-subject-encrypted".to_string(),
            email: Some("admin@example.com".to_string()),
            client_id: "google-client".to_string(),
            granted_scopes: vec!["openid".to_string()],
            access_token: "sensitive-access-token".to_string(),
            refresh_token: "sensitive-refresh-token".to_string(),
            token_received_at: now,
            access_token_expires_at: now + 3600,
            issuer: Some("https://accounts.google.com".to_string()),
            refreshed: false,
            scope_upgraded: true,
        })
        .await
        .unwrap();
    drop(store);

    let conn = Connection::open(&path).unwrap();
    let (stored_access, stored_refresh): (String, String) = conn
            .query_row(
                "SELECT access_token, refresh_token FROM google_provider_credentials WHERE subject = ?1",
                ["google-subject-encrypted"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert!(stored_access.starts_with("enc2:"));
    assert!(stored_refresh.starts_with("enc2:"));
    assert!(!stored_access.contains("sensitive-access-token"));
    assert!(!stored_refresh.contains("sensitive-refresh-token"));
    drop(conn);
    crate::util::set_restrictive_permissions(&path).unwrap();

    let reopened = SqliteStore::open_with_key(path, Some(key)).await.unwrap();
    let row = reopened
        .find_google_provider_credential("google-subject-encrypted")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.access_token.as_deref(), Some("sensitive-access-token"));
    assert_eq!(row.refresh_token, "sensitive-refresh-token");
}

#[tokio::test]
async fn offline_auth_recovery_set_restores_real_schema_ciphertext_and_signing_key() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let backup = root.path().join("backup");
    let restored = root.path().join("restored");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::create_dir_all(&restored).unwrap();

    let database = live.join("auth.db");
    let signing_key_path = live.join("auth-jwt.pem");
    let encoded_encryption_key = "5c".repeat(32);
    let encryption_key = TokenEncryptionKey::from_encoded(&encoded_encryption_key).unwrap();
    let store = SqliteStore::open_with_key(database.clone(), Some(encryption_key))
        .await
        .unwrap();
    let signer = SigningKeys::load_or_create(&signing_key_path).unwrap();
    let now = now_unix();
    store
        .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
            subject: "backup-subject".to_string(),
            email: Some("backup@example.com".to_string()),
            client_id: "backup-google-client".to_string(),
            granted_scopes: vec!["openid".to_string()],
            access_token: "backup-sensitive-access".to_string(),
            refresh_token: "backup-sensitive-refresh".to_string(),
            token_received_at: now,
            access_token_expires_at: now + 3600,
            issuer: Some("https://accounts.google.com".to_string()),
            refreshed: false,
            scope_upgraded: false,
        })
        .await
        .unwrap();
    let token = signer
        .issue_access_token(&AccessClaims {
            iss: "https://lab.example.com".to_string(),
            sub: "backup-subject".to_string(),
            aud: "https://lab.example.com".to_string(),
            exp: usize::try_from(now + 3600).unwrap(),
            nbf: None,
            iat: usize::try_from(now).unwrap(),
            jti: "backup-jti".to_string(),
            scope: "lab:read".to_string(),
            azp: "backup-client".to_string(),
            identity_issuer: Some("https://accounts.google.com".to_string()),
            identity_credential_id: Some("backup-subject".to_string()),
        })
        .unwrap();

    // The documented offline workflow stops all writers before copying the
    // database and its matching key material as one recovery set.
    drop(store);
    std::fs::copy(&database, backup.join("auth.db")).unwrap();
    std::fs::copy(&signing_key_path, backup.join("auth-jwt.pem")).unwrap();
    std::fs::write(backup.join("token-encryption.key"), &encoded_encryption_key).unwrap();
    for name in ["auth.db", "auth-jwt.pem", "token-encryption.key"] {
        std::fs::copy(backup.join(name), restored.join(name)).unwrap();
    }
    crate::util::set_restrictive_permissions(&restored.join("auth.db")).unwrap();
    crate::util::set_restrictive_permissions(&restored.join("auth-jwt.pem")).unwrap();
    crate::util::set_restrictive_permissions(&restored.join("token-encryption.key")).unwrap();

    let restored_key = std::fs::read_to_string(restored.join("token-encryption.key")).unwrap();
    let restored_store = SqliteStore::open_with_key(
        restored.join("auth.db"),
        Some(TokenEncryptionKey::from_encoded(&restored_key).unwrap()),
    )
    .await
    .unwrap();
    let restored_credential = restored_store
        .find_google_provider_credential("backup-subject")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restored_credential.access_token.as_deref(),
        Some("backup-sensitive-access")
    );
    assert_eq!(
        restored_credential.refresh_token,
        "backup-sensitive-refresh"
    );

    let restored_signer = SigningKeys::load_or_create(&restored.join("auth-jwt.pem")).unwrap();
    assert_eq!(restored_signer.key_id, signer.key_id);
    let restored_claims = restored_signer
        .validate_access_token_with_issuer(
            &token,
            "https://lab.example.com",
            "https://lab.example.com",
        )
        .unwrap();
    assert_eq!(restored_claims.sub, "backup-subject");
}

#[tokio::test]
async fn google_provider_bundle_refuses_plaintext_persistence_without_a_key() {
    let path = temp_db_path();
    let store = SqliteStore::open(path.clone()).await.unwrap();
    let now = now_unix();
    let error = store
        .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
            subject: "must-not-persist".to_string(),
            email: Some("admin@example.com".to_string()),
            client_id: "google-client".to_string(),
            granted_scopes: vec!["openid".to_string()],
            access_token: "sensitive-access".to_string(),
            refresh_token: "sensitive-refresh".to_string(),
            token_received_at: now,
            access_token_expires_at: now + 3600,
            issuer: None,
            refreshed: false,
            scope_upgraded: false,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("TOKEN_ENCRYPTION_KEY"));

    let conn = Connection::open(path).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM google_provider_credentials WHERE subject = ?1",
            ["must-not-persist"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "failed encryption must not write any credential row"
    );
}

#[tokio::test]
async fn legacy_plaintext_google_provider_row_is_not_served_without_a_key() {
    let path = temp_db_path();
    let store = SqliteStore::open(path).await.unwrap();
    let now = now_unix();
    store
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO google_provider_credentials (
                        subject, client_id, granted_scopes_json, access_token, refresh_token,
                        generation, created_at, updated_at
                     ) VALUES (?1, ?2, '[]', ?3, ?4, 1, ?5, ?5)",
                params!["legacy-no-key", "google-client", "access", "refresh", now],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
        .unwrap();

    let error = store
        .find_google_provider_credential("legacy-no-key")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("TOKEN_ENCRYPTION_KEY"));
}

#[tokio::test]
async fn opening_with_a_key_encrypts_legacy_plaintext_provider_tokens() {
    let path = temp_db_path();
    let plaintext_store = SqliteStore::open(path.clone()).await.unwrap();
    let now = now_unix();
    plaintext_store
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO google_provider_credentials (
                    subject, email, client_id, granted_scopes_json, access_token,
                    refresh_token, token_received_at, access_token_expires_at, issuer,
                    generation, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?7, ?7)",
                params![
                    "legacy-google-subject",
                    "legacy@example.com",
                    "google-client",
                    "[\"openid\"]",
                    "legacy-access",
                    "legacy-refresh",
                    now,
                    now + 3600,
                    "https://accounts.google.com"
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
        .unwrap();
    drop(plaintext_store);

    let conn = Connection::open(&path).unwrap();
    let before: (String, String) = conn
            .query_row(
                "SELECT access_token, refresh_token FROM google_provider_credentials WHERE subject = ?1",
                ["legacy-google-subject"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(
        before,
        ("legacy-access".to_string(), "legacy-refresh".to_string())
    );
    drop(conn);
    crate::util::set_restrictive_permissions(&path).unwrap();

    let key = TokenEncryptionKey::from_passphrase("legacy-upgrade-key");
    let encrypted_store = SqliteStore::open_with_key(path.clone(), Some(key))
        .await
        .unwrap();
    let row = encrypted_store
        .find_google_provider_credential("legacy-google-subject")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.access_token.as_deref(), Some("legacy-access"));
    assert_eq!(row.refresh_token, "legacy-refresh");
    drop(encrypted_store);

    let conn = Connection::open(path).unwrap();
    let after: (String, String) = conn
            .query_row(
                "SELECT access_token, refresh_token FROM google_provider_credentials WHERE subject = ?1",
                ["legacy-google-subject"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert!(after.0.starts_with("enc2:"));
    assert!(after.1.starts_with("enc2:"));
}

#[tokio::test]
async fn legacy_encryption_snapshot_cannot_overwrite_a_newer_cross_store_bundle() {
    let path = temp_db_path();
    let migration_key = TokenEncryptionKey::from_passphrase("legacy-race-key");
    let writer = SqliteStore::open_with_key(path.clone(), Some(migration_key.clone()))
        .await
        .unwrap();
    let now = now_unix();
    let update = move |access: &str, refresh: &str| GoogleProviderCredentialUpdate {
        subject: "legacy-race-subject".to_string(),
        email: Some("admin@example.com".to_string()),
        client_id: "google-client".to_string(),
        granted_scopes: vec!["openid".to_string()],
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        token_received_at: now,
        access_token_expires_at: now + 3600,
        issuer: Some("https://accounts.google.com".to_string()),
        refreshed: false,
        scope_upgraded: false,
    };
    writer
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO google_provider_credentials (
                    subject, email, client_id, granted_scopes_json, access_token,
                    refresh_token, token_received_at, access_token_expires_at,
                    generation, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?7, ?7)",
                params![
                    "legacy-race-subject",
                    "admin@example.com",
                    "google-client",
                    "[\"openid\"]",
                    "old-access",
                    "old-refresh",
                    now,
                    now + 3600
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
        .unwrap();
    let (reached_tx, reached_rx) = std::sync::mpsc::sync_channel(0);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
    *super::google_credentials::LEGACY_ENCRYPTION_SNAPSHOT_HOOK
        .lock()
        .unwrap() = Some((reached_tx, resume_rx));

    let encrypted_path = path.clone();
    let opener = tokio::spawn(async move {
        SqliteStore::open_with_key(
            encrypted_path,
            Some(TokenEncryptionKey::from_passphrase("legacy-race-key")),
        )
        .await
        .unwrap()
    });
    tokio::task::spawn_blocking(move || reached_rx.recv().unwrap())
        .await
        .unwrap();
    let newer = tokio::spawn(async move {
        writer
            .upsert_google_provider_token_bundle(update("new-access", "new-refresh"))
            .await
            .unwrap();
    });
    resume_tx.send(()).unwrap();
    let encrypted = opener.await.unwrap();
    newer.await.unwrap();
    let row = encrypted
        .find_google_provider_credential("legacy-race-subject")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.access_token.as_deref(), Some("new-access"));
    assert_eq!(row.refresh_token, "new-refresh");
}

#[tokio::test]
async fn google_provider_selector_requires_account_when_multiple_rows_exist() {
    let store = temp_store().await;
    for (subject, email) in [
        ("google-subject-a", "a@example.com"),
        ("google-subject-b", "b@example.com"),
        ("google-subject-c", "google-subject-b"),
    ] {
        store
            .upsert_google_provider_credential(subject, Some(email), "provider-refresh")
            .await
            .unwrap();
    }

    let error = store
        .find_google_provider_credential_by_selector(None)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("multiple Google provider credentials")
    );
    assert_eq!(
        store
            .find_google_provider_credential_by_selector(Some("b@example.com"))
            .await
            .unwrap()
            .unwrap()
            .subject,
        "google-subject-b"
    );
    assert_eq!(
        store
            .find_google_provider_credential_by_selector(Some("google-subject-b"))
            .await
            .unwrap()
            .unwrap()
            .subject,
        "google-subject-b",
        "an exact stable subject must win over another row whose email has the same text"
    );
}

#[tokio::test]
async fn google_provider_invalidation_is_generation_safe_and_revokes_dependent_grants() {
    let store = temp_store().await;
    register_test_client(&store, "chatgpt-client").await;
    register_test_client(&store, "claude-client").await;
    store
        .upsert_google_provider_credential(
            "google-subject-123",
            Some("admin@example.com"),
            "provider-refresh-v1",
        )
        .await
        .unwrap();
    let first_generation = store
        .find_google_provider_credential("google-subject-123")
        .await
        .unwrap()
        .unwrap()
        .generation;

    for (client_id, token) in [
        ("chatgpt-client", "chatgpt-refresh"),
        ("claude-client", "claude-refresh"),
    ] {
        let mut row = sample_refresh_token(client_id, token);
        row.subject = "google-subject-123".to_string();
        row.provider_refresh_token = None;
        store.upsert_refresh_token(row).await.unwrap();
    }
    let mut pending_code = sample_code();
    pending_code.code = "pending-provider-code".to_string();
    pending_code.client_id = "chatgpt-client".to_string();
    pending_code.subject = "google-subject-123".to_string();
    pending_code.provider_refresh_token = None;
    store.insert_auth_code(pending_code).await.unwrap();

    store
        .upsert_google_provider_credential(
            "google-subject-123",
            Some("admin@example.com"),
            "provider-refresh-v2",
        )
        .await
        .unwrap();
    let current = store
        .find_google_provider_credential("google-subject-123")
        .await
        .unwrap()
        .unwrap();
    assert!(current.generation > first_generation);

    let stale = store
        .invalidate_google_provider_credential("google-subject-123", first_generation)
        .await
        .unwrap();
    assert!(!stale.invalidated);
    assert!(
        store
            .find_refresh_token("chatgpt-refresh")
            .await
            .unwrap()
            .is_some(),
        "a stale invalidation must not revoke sessions backed by a newer credential"
    );

    let invalidation = store
        .invalidate_google_provider_credential("google-subject-123", current.generation)
        .await
        .unwrap();
    assert!(invalidation.invalidated);
    assert_eq!(invalidation.revoked_refresh_tokens, 2);
    assert_eq!(invalidation.revoked_authorization_codes, 1);
    assert!(
        store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .find_refresh_token("chatgpt-refresh")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .find_refresh_token("claude-refresh")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn refresh_token_insert_fails_for_unregistered_client() {
    let store = temp_store().await;
    // Deliberately skip register_test_client — "ghost-client" was never
    // registered, so the FOREIGN KEY constraint on
    // refresh_tokens.client_id must reject this insert.
    let err = store
        .upsert_refresh_token(sample_refresh_token("ghost-client", "ghost-refresh"))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("FOREIGN KEY"),
        "expected a foreign key violation, got: {err}"
    );
}

#[tokio::test]
async fn schema_migration_v4_preserves_existing_refresh_tokens() {
    let path = temp_db_path();
    // Hand-build a pre-v4 database: the v3 shape (refresh_tokens with no
    // FOREIGN KEY on client_id), with a legitimate row already present,
    // to prove the v3->v4 migration (which recreates the table to add
    // the constraint) doesn't lose or corrupt existing data.
    let raw_token = "pre-existing-refresh-token";
    let token_hash = hash_token(raw_token);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE registered_clients (
                    client_id TEXT PRIMARY KEY,
                    redirect_uris TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE refresh_tokens (
                    refresh_token_hash TEXT PRIMARY KEY,
                    client_id TEXT NOT NULL,
                    subject TEXT NOT NULL,
                    resource TEXT NOT NULL DEFAULT '',
                    scope TEXT NOT NULL,
                    provider_refresh_token TEXT,
                    created_at INTEGER NOT NULL,
                    expires_at INTEGER NOT NULL
                );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO registered_clients (client_id, redirect_uris, created_at) \
                 VALUES ('client', '[\"http://127.0.0.1:7777/callback\"]', 0)",
            [],
        )
        .unwrap();
        conn.execute(
                "INSERT INTO refresh_tokens (
                    refresh_token_hash, client_id, subject, resource, scope,
                    provider_refresh_token, created_at, expires_at
                 ) VALUES (?1, 'client', 'google-user', 'https://lab.example.com/mcp', 'lab', NULL, ?2, ?3)",
                rusqlite::params![token_hash, now_unix(), now_unix() + 3600],
            )
            .unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
    }
    // SqliteStore::open validates restrictive permissions on an
    // already-existing file; the raw Connection::open above created it
    // with default (too-open) OS permissions.
    crate::util::set_restrictive_permissions(&path).unwrap();

    // Reopening through the real API runs migration v4 (table
    // recreation with the FK added) against this hand-built database.
    let store = SqliteStore::open(path).await.unwrap();
    let found = store
        .find_refresh_token(raw_token)
        .await
        .unwrap()
        .expect("pre-existing refresh token must survive the v3->v4 migration");
    assert_eq!(found.client_id, "client");

    // The constraint is live post-migration: a fresh insert for an
    // unregistered client still fails.
    let err = store
        .upsert_refresh_token(sample_refresh_token("still-unregistered", "new-refresh"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"));
}

#[test]
fn schema_migration_v4_stamps_only_its_own_version() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
             CREATE TABLE registered_clients (
               client_id TEXT PRIMARY KEY,
               redirect_uris TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE refresh_tokens (
               refresh_token_hash TEXT PRIMARY KEY,
               client_id TEXT NOT NULL,
               subject TEXT NOT NULL,
               resource TEXT NOT NULL DEFAULT '',
               scope TEXT NOT NULL,
               provider_refresh_token TEXT,
               created_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL
             );
             PRAGMA user_version = 3;",
    )
    .unwrap();

    migrations::migrate_v4(&conn).unwrap();

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);
    let has_assertion_jtis: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'assertion_jtis')",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert!(!has_assertion_jtis);
}

#[test]
fn schema_migration_v4_rejects_orphaned_refresh_tokens() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
             CREATE TABLE registered_clients (
               client_id TEXT PRIMARY KEY,
               redirect_uris TEXT NOT NULL,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE refresh_tokens (
               refresh_token_hash TEXT PRIMARY KEY,
               client_id TEXT NOT NULL,
               subject TEXT NOT NULL,
               resource TEXT NOT NULL DEFAULT '',
               scope TEXT NOT NULL,
               provider_refresh_token TEXT,
               created_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL
             );
             INSERT INTO refresh_tokens VALUES (
               'hash', 'missing-client', 'subject', '', 'scope', NULL, 1, 2
             );
             PRAGMA user_version = 3;",
    )
    .unwrap();

    let error = migrations::migrate_v4(&conn).unwrap_err();
    assert!(error.to_string().contains("foreign key"), "{error}");
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(foreign_keys, 1);
}

#[tokio::test]
async fn schema_migration_repairs_legacy_falsely_stamped_v8_database() {
    let path = temp_db_path();
    let store = SqliteStore::open(path.clone()).await.unwrap();
    drop(store);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE assertion_jtis;
                 DROP TABLE google_provider_credentials;
                 CREATE TABLE google_provider_credentials (
                   subject TEXT PRIMARY KEY,
                   email TEXT,
                   refresh_token TEXT NOT NULL,
                   generation INTEGER NOT NULL,
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );
                 ALTER TABLE refresh_tokens DROP COLUMN refresh_claim_expires_at;
                 ALTER TABLE refresh_tokens DROP COLUMN refresh_claim_id;
                 PRAGMA user_version = 8;",
        )
        .unwrap();
    }
    crate::util::set_restrictive_permissions(&path).unwrap();

    let repaired = SqliteStore::open(path).await.unwrap();
    let refresh_columns = table_columns(&repaired, "refresh_tokens").await;
    let broker_columns = table_columns(&repaired, "google_provider_credentials").await;
    assert!(refresh_columns.contains(&"refresh_claim_id".to_string()));
    assert!(refresh_columns.contains(&"refresh_claim_expires_at".to_string()));
    assert!(broker_columns.contains(&"client_id".to_string()));
    assert!(broker_columns.contains(&"granted_scopes_json".to_string()));
    let assertion_table_exists = repaired
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'assertion_jtis')",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sqlite_error)
            })
            .await
            .unwrap();
    assert!(assertion_table_exists);
}

#[test]
fn schema_migration_repair_rejects_unrelated_v8_corruption() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE refresh_tokens (
               refresh_token_hash TEXT PRIMARY KEY,
               client_id TEXT NOT NULL,
               subject TEXT NOT NULL,
               resource TEXT NOT NULL DEFAULT '',
               scope TEXT NOT NULL,
               provider_refresh_token TEXT,
               created_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL,
               refresh_claim_id TEXT,
               refresh_claim_expires_at INTEGER
             );
             CREATE TABLE google_provider_credentials (
               subject TEXT PRIMARY KEY,
               email TEXT,
               client_id TEXT NOT NULL DEFAULT '',
               granted_scopes_json TEXT NOT NULL,
               access_token TEXT,
               token_received_at INTEGER,
               access_token_expires_at INTEGER,
               issuer TEXT,
               last_refresh_at INTEGER,
               last_scope_upgrade_at INTEGER,
               generation INTEGER NOT NULL,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE assertion_jtis (
               issuer TEXT NOT NULL,
               jti TEXT NOT NULL,
               expires_at INTEGER NOT NULL,
               PRIMARY KEY (issuer, jti)
             );
             PRAGMA user_version = 8;",
    )
    .unwrap();

    let error = migrations::run_migrations(&conn).unwrap_err();
    assert!(error.to_string().contains("refresh_token"), "{error}");
}

#[tokio::test]
async fn schema_migration_v7_promotes_the_newest_subject_provider_credential() {
    let path = temp_db_path();
    let store = SqliteStore::open(path.clone()).await.unwrap();
    register_test_client(&store, "client").await;

    let mut older = sample_refresh_token("client", "older-local-refresh");
    older.subject = "google-subject-123".to_string();
    older.provider_refresh_token = Some("older-provider-refresh".to_string());
    older.created_at = now_unix() - 60;
    store.upsert_refresh_token(older).await.unwrap();

    let mut newer = sample_refresh_token("client", "newer-local-refresh");
    newer.subject = "google-subject-123".to_string();
    newer.provider_refresh_token = Some("newer-provider-refresh".to_string());
    newer.created_at = now_unix();
    store.upsert_refresh_token(newer).await.unwrap();
    drop(store);

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE google_provider_credentials; PRAGMA user_version = 6;")
            .unwrap();
    }
    crate::util::set_restrictive_permissions(&path).unwrap();

    let migrated = SqliteStore::open_with_key(
        path,
        Some(TokenEncryptionKey::from_passphrase("v7-migration-test-key")),
    )
    .await
    .unwrap();
    let credential = migrated
        .find_google_provider_credential("google-subject-123")
        .await
        .unwrap()
        .expect("v7 migration must promote a provider credential");
    assert_eq!(credential.refresh_token, "newer-provider-refresh");
    assert_eq!(credential.generation, 1);
    assert!(credential.email.is_none());
}

#[tokio::test]
async fn schema_migration_v7_skips_ambiguous_newest_provider_credentials() {
    for provider_tokens in [["provider-a", "provider-b"], ["provider-b", "provider-a"]] {
        let path = temp_db_path();
        let store = SqliteStore::open(path.clone()).await.unwrap();
        register_test_client(&store, "client").await;
        for (index, provider_token) in provider_tokens.into_iter().enumerate() {
            let mut row =
                sample_refresh_token("client", &format!("local-refresh-{index}-{provider_token}"));
            row.subject = "ambiguous-subject".to_string();
            row.provider_refresh_token = Some(provider_token.to_string());
            row.created_at = 1234;
            store.upsert_refresh_token(row).await.unwrap();
        }
        drop(store);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("DROP TABLE google_provider_credentials; PRAGMA user_version = 6;")
                .unwrap();
        }
        crate::util::set_restrictive_permissions(&path).unwrap();

        let migrated = SqliteStore::open(path).await.unwrap();
        assert!(
            migrated
                .find_google_provider_credential("ambiguous-subject")
                .await
                .unwrap()
                .is_none(),
            "ambiguous legacy credentials must force reauthorization"
        );
    }
}

#[tokio::test]
async fn fresh_and_v4_upgraded_schema_are_identical() {
    let fresh_path = temp_db_path();
    let fresh = SqliteStore::open(fresh_path).await.unwrap();
    let fresh_schema = auth_schema_snapshot(&fresh).await;

    let upgraded_path = temp_db_path();
    let upgraded = SqliteStore::open(upgraded_path.clone()).await.unwrap();
    drop(upgraded);
    {
        let conn = Connection::open(&upgraded_path).unwrap();
        conn.execute_batch(
            "DROP TABLE assertion_jtis;
                 DROP TABLE google_provider_credentials;
                 ALTER TABLE refresh_tokens DROP COLUMN refresh_claim_expires_at;
                 ALTER TABLE refresh_tokens DROP COLUMN refresh_claim_id;
                 PRAGMA user_version = 4;",
        )
        .unwrap();
    }
    crate::util::set_restrictive_permissions(&upgraded_path).unwrap();
    let upgraded = SqliteStore::open(upgraded_path).await.unwrap();
    let upgraded_schema = auth_schema_snapshot(&upgraded).await;

    assert_eq!(fresh_schema, upgraded_schema);
}

#[tokio::test]
async fn fresh_schema_keeps_refresh_claim_lease_only_on_refresh_tokens() {
    let store = SqliteStore::open(temp_db_path()).await.unwrap();
    let authorization_code_columns = table_columns(&store, "authorization_codes").await;
    let refresh_token_columns = table_columns(&store, "refresh_tokens").await;

    assert!(!authorization_code_columns.contains(&"refresh_claim_id".to_string()));
    assert!(!authorization_code_columns.contains(&"refresh_claim_expires_at".to_string()));
    assert!(refresh_token_columns.contains(&"refresh_claim_id".to_string()));
    assert!(refresh_token_columns.contains(&"refresh_claim_expires_at".to_string()));
}

#[tokio::test]
async fn fresh_and_v8_upgraded_schemas_include_v11_refresh_replays() {
    let path = temp_db_path();
    let fresh = SqliteStore::open(path.clone()).await.unwrap();
    assert!(
        table_columns(&fresh, "google_provider_revocations")
            .await
            .contains(&"epoch".to_string())
    );
    assert!(
        table_columns(&fresh, "authorization_requests")
            .await
            .contains(&"native_poll_token_hash".to_string())
    );
    assert_eq!(
        table_columns(&fresh, "native_authorization_results").await,
        vec![
            "poll_token_hash".to_string(),
            "code".to_string(),
            "created_at".to_string(),
            "expires_at".to_string(),
        ]
    );
    assert!(
        !table_columns(&fresh, "assertion_jtis")
            .await
            .contains(&"native_poll_token_hash".to_string())
    );
    assert_eq!(
        table_columns(&fresh, "refresh_token_replays").await,
        vec![
            "predecessor_token_hash".to_string(),
            "client_id".to_string(),
            "resource".to_string(),
            "response".to_string(),
            "replacement_token_hash".to_string(),
            "created_at".to_string(),
            "expires_at".to_string(),
        ]
    );
    drop(fresh);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE google_provider_revocations;
             DROP TABLE native_authorization_results;
             CREATE TABLE native_authorization_results (
               state TEXT PRIMARY KEY,
               code TEXT NOT NULL,
               created_at INTEGER NOT NULL,
               expires_at INTEGER NOT NULL
             );
             INSERT INTO native_authorization_results
               (state, code, created_at, expires_at)
             VALUES ('attacker-known-state', 'legacy-code', 1, 9999999999);
             PRAGMA user_version = 8;",
    )
    .unwrap();
    drop(conn);
    crate::util::set_restrictive_permissions(&path).unwrap();
    let upgraded = SqliteStore::open(path.clone()).await.unwrap();
    assert!(
        table_columns(&upgraded, "google_provider_revocations")
            .await
            .contains(&"epoch".to_string())
    );
    assert!(
        table_columns(&upgraded, "authorization_requests")
            .await
            .contains(&"native_poll_token_hash".to_string())
    );
    assert_eq!(
        table_columns(&upgraded, "native_authorization_results").await,
        vec![
            "poll_token_hash".to_string(),
            "code".to_string(),
            "created_at".to_string(),
            "expires_at".to_string(),
        ]
    );
    let legacy_rows = upgraded
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM native_authorization_results",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(sqlite_error)
        })
        .await
        .unwrap();
    assert_eq!(legacy_rows, 0);
    assert!(
        table_columns(&upgraded, "refresh_token_replays")
            .await
            .contains(&"replacement_token_hash".to_string())
    );
    upgraded
            .with_conn(|conn| {
                let foreign_key_action: String = conn
                    .query_row(
                        "SELECT on_delete FROM pragma_foreign_key_list('refresh_token_replays')
                         WHERE \"table\" = 'refresh_tokens'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(sqlite_error)?;
                assert_eq!(foreign_key_action, "CASCADE");
                let has_expiry_index: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_index_list('refresh_token_replays')
                         WHERE name = 'idx_refresh_token_replays_expiry')",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(sqlite_error)?;
                assert!(has_expiry_index);
                conn.execute(
                    "INSERT OR IGNORE INTO registered_clients (client_id, redirect_uris, created_at)
                     VALUES ('client', '[]', 1)",
                    [],
                )
                .map_err(sqlite_error)?;
                conn.execute(
                    "INSERT INTO refresh_tokens
                     (refresh_token_hash, client_id, subject, resource, scope, created_at, expires_at)
                     VALUES ('replacement-hash', 'client', 'subject', '', 'lab', 1, 9999999999)",
                    [],
                )
                .map_err(sqlite_error)?;
                conn.execute(
                    "INSERT INTO refresh_token_replays
                     (predecessor_token_hash, client_id, resource, response,
                      replacement_token_hash, created_at, expires_at)
                     VALUES ('predecessor-hash', 'client', '', 'encrypted',
                             'replacement-hash', 1, 9999999999)",
                    [],
                )
                .map_err(sqlite_error)?;
                conn.execute(
                    "DELETE FROM refresh_tokens WHERE refresh_token_hash = 'replacement-hash'",
                    [],
                )
                .map_err(sqlite_error)?;
                let replay_count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM refresh_token_replays", [], |row| row.get(0))
                    .map_err(sqlite_error)?;
                assert_eq!(replay_count, 0);
                Ok(())
            })
            .await
            .unwrap();
    drop(upgraded);
    let conn = Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        12
    );
}

#[tokio::test]
async fn schema_migration_v8_preserves_v7_provider_refresh_token_and_adds_broker_metadata() {
    let path = temp_db_path();
    let store = SqliteStore::open(path.clone()).await.unwrap();
    drop(store);

    let created_at = now_unix() - 120;
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE google_provider_credentials;
                 CREATE TABLE google_provider_credentials (
                   subject TEXT PRIMARY KEY,
                   email TEXT,
                   refresh_token TEXT NOT NULL,
                   generation INTEGER NOT NULL,
                   created_at INTEGER NOT NULL,
                   updated_at INTEGER NOT NULL
                 );
                 CREATE INDEX idx_google_provider_credentials_email
                   ON google_provider_credentials(email COLLATE NOCASE);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO google_provider_credentials (
                   subject, email, refresh_token, generation, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 7, ?4, ?4)",
            rusqlite::params![
                "google-subject-v7",
                "admin@example.com",
                "v7-provider-refresh",
                created_at,
            ],
        )
        .unwrap();
        conn.execute_batch("PRAGMA user_version = 7;").unwrap();
    }
    crate::util::set_restrictive_permissions(&path).unwrap();

    let migrated = SqliteStore::open_with_key(
        path,
        Some(TokenEncryptionKey::from_passphrase("v8-migration-test-key")),
    )
    .await
    .unwrap();
    let schema_version = migrated
        .with_conn(|conn| {
            conn.query_row("PRAGMA user_version;", [], |row| row.get::<_, i64>(0))
                .map_err(sqlite_error)
        })
        .await
        .unwrap();
    assert_eq!(schema_version, 12);
    let row = migrated
        .find_google_provider_credential("google-subject-v7")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.refresh_token, "v7-provider-refresh");
    assert_eq!(row.generation, 7);
    assert_eq!(row.client_id, "");
    assert_eq!(row.granted_scopes, vec!["email", "openid", "profile"]);
    assert!(row.access_token.is_none());
    assert!(row.token_received_at.is_none());
    assert!(row.access_token_expires_at.is_none());
    assert!(row.issuer.is_none());
    assert!(row.last_refresh_at.is_none());
    assert!(row.last_scope_upgrade_at.is_none());
    assert_eq!(row.created_at, created_at);
}

#[test]
fn v9_migration_fault_rolls_back_table_and_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE refresh_tokens (
           refresh_token_hash TEXT, client_id TEXT, subject TEXT, resource TEXT,
           scope TEXT, provider_refresh_token TEXT, created_at INTEGER,
           expires_at INTEGER, refresh_claim_id TEXT, refresh_claim_expires_at INTEGER
         );
         CREATE TABLE google_provider_credentials (
           subject TEXT, email TEXT, refresh_token TEXT, generation INTEGER,
           created_at INTEGER, updated_at INTEGER, client_id TEXT,
           granted_scopes_json TEXT, access_token TEXT, token_received_at INTEGER,
           access_token_expires_at INTEGER, issuer TEXT, last_refresh_at INTEGER,
           last_scope_upgrade_at INTEGER
         );
         CREATE TABLE assertion_jtis (issuer TEXT, jti TEXT, expires_at INTEGER);
         PRAGMA user_version = 8;",
    )
    .unwrap();

    let error = migrations::run_migrations_with_fault(&conn, "v9_after_table")
        .expect_err("fault must abort v9");
    assert!(error.to_string().contains("injected v9 migration fault"));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 8);
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'google_provider_revocations')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!exists);
}

#[test]
fn v10_migration_fault_rolls_back_column_and_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE authorization_requests (state TEXT PRIMARY KEY);
         PRAGMA user_version = 9;",
    )
    .unwrap();

    let error = migrations::run_migrations_with_fault(&conn, "v10_after_column")
        .expect_err("fault must abort v10");
    assert!(error.to_string().contains("injected v10 migration fault"));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 9);
    let columns = {
        let mut statement = conn
            .prepare("PRAGMA table_info(authorization_requests)")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(columns, vec!["state"]);
}

#[test]
fn v11_migration_fault_rolls_back_table_and_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA user_version = 10;").unwrap();

    let error = migrations::run_migrations_with_fault(&conn, "v11_after_table")
        .expect_err("fault must abort v11");
    assert!(error.to_string().contains("injected v11 migration fault"));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 10);
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'refresh_token_replays')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!exists);
}

#[test]
fn v12_migration_fault_rolls_back_columns_and_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE upstream_oauth_state (state TEXT PRIMARY KEY);
             PRAGMA user_version = 11;",
    )
    .unwrap();

    let error = migrations::run_migrations_with_fault(&conn, "v12_after_expected_issuer")
        .expect_err("fault must abort v12");
    assert!(error.to_string().contains("injected v12 migration fault"));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 11);
    let columns = {
        let mut statement = conn
            .prepare("PRAGMA table_info(upstream_oauth_state)")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(columns, vec!["state"]);
}

#[tokio::test]
async fn sqlite_store_cleanup_expired_removes_stale_rows() {
    let store = temp_store().await;
    let now = now_unix();
    register_test_client(&store, "client").await;

    // Insert an expired auth code.
    let mut code = sample_code();
    code.expires_at = now - 10;
    store.insert_auth_code(code).await.unwrap();

    // Insert an expired refresh token.
    store
        .upsert_refresh_token(RefreshTokenRow {
            refresh_token: "expired-refresh".to_string(),
            client_id: "client".to_string(),
            subject: "google-user".to_string(),
            resource: "https://lab.example.com/mcp".to_string(),
            scope: "lab".to_string(),
            provider_refresh_token: None,
            created_at: now - 600,
            expires_at: now - 10,
        })
        .await
        .unwrap();

    // Insert an expired authorization request.
    use crate::types::AuthorizationRequestRow;
    store
        .insert_authorization_request(AuthorizationRequestRow {
            state: "expired-state".to_string(),
            client_id: "client".to_string(),
            redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
            client_state: "cs".to_string(),
            native_poll_token_hash: None,
            resource: "https://lab.example.com/mcp".to_string(),
            scope: "lab".to_string(),
            provider_code_verifier: "verifier".to_string(),
            code_challenge: "challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            created_at: now - 600,
            expires_at: now - 10,
        })
        .await
        .unwrap();

    // Insert a valid (non-expired) refresh token.
    store
        .upsert_refresh_token(RefreshTokenRow {
            refresh_token: "valid-refresh".to_string(),
            client_id: "client".to_string(),
            subject: "google-user".to_string(),
            resource: "https://lab.example.com/mcp".to_string(),
            scope: "lab".to_string(),
            provider_refresh_token: None,
            created_at: now,
            expires_at: now + 3600,
        })
        .await
        .unwrap();

    let deleted = store.cleanup_expired().await.unwrap();
    assert_eq!(deleted, 3, "should delete exactly 3 expired rows");

    // The valid refresh token should still exist.
    assert!(
        store
            .find_refresh_token("valid-refresh")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn sqlite_store_cleanup_expired_honors_per_table_batch_limit() {
    let store = temp_store().await;
    let now = now_unix();
    register_test_client(&store, "client").await;
    for index in 0..3 {
        let mut code = sample_code();
        code.code = format!("expired-code-{index}");
        code.expires_at = now - 1;
        store.insert_auth_code(code).await.unwrap();
    }

    assert_eq!(store.cleanup_expired_bounded(2).await.unwrap(), 2);
    assert_eq!(store.cleanup_expired_bounded(2).await.unwrap(), 1);
    assert_eq!(store.cleanup_expired_bounded(2).await.unwrap(), 0);
}

async fn temp_store() -> SqliteStore {
    SqliteStore::open_with_key(
        temp_db_path(),
        Some(TokenEncryptionKey::from_passphrase(
            "sqlite-test-provider-key",
        )),
    )
    .await
    .unwrap()
}

async fn pragma(store: &SqliteStore, name: &str) -> String {
    store.pragma(name).await.unwrap()
}

async fn pragma_ms(store: &SqliteStore, name: &str) -> u64 {
    pragma(store, name).await.parse().unwrap()
}

async fn auth_schema_snapshot(store: &SqliteStore) -> Vec<String> {
    store
        .with_conn(|conn| {
            let mut table_statement = conn
                .prepare(
                    "SELECT name FROM sqlite_master
                         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                         ORDER BY name",
                )
                .map_err(sqlite_error)?;
            let tables = table_statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;
            let mut snapshot = Vec::new();
            for table in tables {
                snapshot.push(format!("table:{table}"));
                let escaped = table.replace('"', "\"\"");
                let mut column_statement = conn
                    .prepare(&format!("PRAGMA table_info(\"{escaped}\")"))
                    .map_err(sqlite_error)?;
                let columns = column_statement
                    .query_map([], |row| {
                        Ok(format!(
                            "column:{}:{}:{}:{:?}:{}",
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, i64>(5)?
                        ))
                    })
                    .map_err(sqlite_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sqlite_error)?;
                snapshot.extend(columns);
            }
            let mut index_statement = conn
                .prepare(
                    "SELECT name, COALESCE(sql, '') FROM sqlite_master
                         WHERE type = 'index' AND name NOT LIKE 'sqlite_%'
                         ORDER BY name",
                )
                .map_err(sqlite_error)?;
            let indexes = index_statement
                .query_map([], |row| {
                    Ok(format!(
                        "index:{}:{}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?
                    ))
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;
            snapshot.extend(indexes);
            Ok(snapshot)
        })
        .await
        .unwrap()
}

async fn table_columns(store: &SqliteStore, table: &'static str) -> Vec<String> {
    store
        .with_conn(move |conn| {
            let mut statement = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .map_err(sqlite_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)
        })
        .await
        .unwrap()
}

fn temp_db_path() -> PathBuf {
    tempfile::tempdir().unwrap().keep().join("auth.db")
}

fn sample_code() -> AuthorizationCodeRow {
    let now = now_unix();
    AuthorizationCodeRow {
        code: "code-123".to_string(),
        client_id: "client".to_string(),
        subject: "google-user".to_string(),
        redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
        resource: "https://lab.example.com/mcp".to_string(),
        scope: "lab".to_string(),
        code_challenge: "challenge".to_string(),
        code_challenge_method: "S256".to_string(),
        provider_refresh_token: Some("provider-refresh".to_string()),
        created_at: now,
        expires_at: now + 300,
    }
}

/// Registers `client_id` in `registered_clients` so a subsequently
/// inserted `refresh_tokens` row satisfies the FOREIGN KEY constraint
/// on `refresh_tokens.client_id`.
async fn register_test_client(store: &SqliteStore, client_id: &str) {
    store
        .register_client(RegisteredClient {
            client_id: client_id.to_string(),
            redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
            created_at: now_unix(),
            token_endpoint_auth_method: "none".to_string(),
            token_endpoint_auth_methods: Vec::new(),
            jwks: None,
            jwks_uri: None,
        })
        .await
        .unwrap();
}

fn sample_refresh_token(client_id: &str, refresh_token: &str) -> RefreshTokenRow {
    let now = now_unix();
    RefreshTokenRow {
        refresh_token: refresh_token.to_string(),
        client_id: client_id.to_string(),
        subject: "google-user".to_string(),
        resource: "https://lab.example.com/mcp".to_string(),
        scope: "lab".to_string(),
        provider_refresh_token: None,
        created_at: now,
        expires_at: now + 3600,
    }
}

#[tokio::test]
async fn browser_session_round_trip_succeeds() {
    let store = temp_store().await;
    let row = BrowserSessionRow {
        session_id: "sess_123".into(),
        subject: "user_1".into(),
        email: Some("jmagar@example.com".into()),
        csrf_token: "csrf_123".into(),
        created_at: 1,
        expires_at: now_unix() + 9_999,
    };

    store.upsert_browser_session(row.clone()).await.unwrap();
    let fetched = store
        .find_browser_session("sess_123")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(fetched.session_id, row.session_id);
    assert_eq!(fetched.subject, row.subject);
    assert_eq!(fetched.csrf_token, row.csrf_token);
}

fn sample_upstream_credentials() -> UpstreamOauthCredentialRow {
    let now = now_unix();
    UpstreamOauthCredentialRow {
        upstream_name: "acme".to_string(),
        subject: "alice".to_string(),
        client_id: "client-xyz".to_string(),
        granted_scopes_json: "[\"mcp\"]".to_string(),
        token_blob: vec![1, 2, 3, 4],
        token_blob_nonce: vec![0u8; 12],
        token_received_at: now,
        access_token_expires_at: now + 3600,
        refresh_token_present: true,
    }
}

fn sample_upstream_state() -> UpstreamOauthStateRow {
    let now = now_unix();
    UpstreamOauthStateRow {
        upstream_name: "acme".to_string(),
        subject: "alice".to_string(),
        csrf_token: "csrf-1".to_string(),
        pkce_verifier: "verifier-1".to_string(),
        expected_issuer: None,
        require_issuer: false,
        requested_scopes: Vec::new(),
        created_at: now,
        expires_at: now + 300,
    }
}

#[tokio::test]
async fn sqlite_store_upsert_upstream_oauth_credentials_round_trip() {
    let store = temp_store().await;
    let row = sample_upstream_credentials();
    store
        .upsert_upstream_oauth_credentials(row.clone())
        .await
        .unwrap();

    let fetched = store
        .find_upstream_oauth_credentials("acme", "alice")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(fetched.upstream_name, row.upstream_name);
    assert_eq!(fetched.subject, row.subject);
    assert_eq!(fetched.client_id, row.client_id);
    assert_eq!(fetched.granted_scopes_json, row.granted_scopes_json);
    assert_eq!(fetched.token_blob, row.token_blob);
    assert_eq!(fetched.token_blob_nonce, row.token_blob_nonce);
    assert_eq!(fetched.token_received_at, row.token_received_at);
    assert_eq!(fetched.access_token_expires_at, row.access_token_expires_at);
    assert_eq!(fetched.refresh_token_present, row.refresh_token_present);
}

#[tokio::test]
async fn sqlite_store_takes_upstream_oauth_state_only_once_under_race() {
    let store = temp_store().await;
    store
        .save_upstream_oauth_state(sample_upstream_state())
        .await
        .unwrap();
    let now = now_unix();
    let (a, b) = tokio::join!(
        store.take_upstream_oauth_state("acme", "alice", "csrf-1", now),
        store.take_upstream_oauth_state("acme", "alice", "csrf-1", now),
    );
    let a_some = matches!(a, Ok(Some(_)));
    let b_some = matches!(b, Ok(Some(_)));
    assert!(
        a_some ^ b_some,
        "exactly one take should win: a={a:?} b={b:?}"
    );
}

#[tokio::test]
async fn sqlite_store_rejects_state_ttl_over_600s() {
    let store = temp_store().await;
    let mut row = sample_upstream_state();
    row.created_at = 1_000;
    row.expires_at = 1_000 + 601;
    let err = store.save_upstream_oauth_state(row).await.unwrap_err();
    assert!(err.to_string().contains("600"));
}

#[tokio::test]
async fn malformed_requested_scopes_state_is_rejected_before_exchange() {
    let store = temp_store().await;
    let now = now_unix();
    store
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO upstream_oauth_state (
                    upstream_name, subject, csrf_token, pkce_verifier,
                    expected_issuer, require_issuer, requested_scopes_json,
                    created_at, expires_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    "acme",
                    "alice",
                    "csrf-malformed-scopes",
                    "verifier",
                    "https://auth.example",
                    1_i64,
                    "not-json",
                    now,
                    now + 300,
                ],
            )
            .map(|_| ())
            .map_err(sqlite_error)
        })
        .await
        .unwrap();

    let error = store
        .take_upstream_oauth_state("acme", "alice", "csrf-malformed-scopes", now)
        .await
        .expect_err("malformed requested scopes must stop callback processing");
    assert!(matches!(error, crate::error::AuthError::Storage(_)));
}

#[tokio::test]
async fn sqlite_store_cleanup_expired_drops_state() {
    let store = temp_store().await;
    let now = now_unix();
    let row = UpstreamOauthStateRow {
        upstream_name: "acme".to_string(),
        subject: "alice".to_string(),
        csrf_token: "csrf-expired".to_string(),
        pkce_verifier: "verifier-expired".to_string(),
        expected_issuer: None,
        require_issuer: false,
        requested_scopes: Vec::new(),
        created_at: now - 400,
        expires_at: now - 10,
    };
    store.save_upstream_oauth_state(row).await.unwrap();

    store.cleanup_expired().await.unwrap();

    let fetched = store
        .take_upstream_oauth_state("acme", "alice", "csrf-expired", now)
        .await
        .unwrap();
    assert!(fetched.is_none(), "expired state should be gone");
}

#[tokio::test]
async fn sqlite_store_credentials_isolated_per_subject() {
    let store = temp_store().await;
    let mut row1 = sample_upstream_credentials();
    row1.subject = "alice".to_string();
    let mut row2 = sample_upstream_credentials();
    row2.subject = "bob".to_string();
    store.upsert_upstream_oauth_credentials(row1).await.unwrap();
    store.upsert_upstream_oauth_credentials(row2).await.unwrap();

    store
        .delete_upstream_oauth_credentials("acme", "alice")
        .await
        .unwrap();

    assert!(
        store
            .find_upstream_oauth_credentials("acme", "alice")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .find_upstream_oauth_credentials("acme", "bob")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn sqlite_store_upsert_overwrites_existing_credentials() {
    let store = temp_store().await;
    let row1 = sample_upstream_credentials();
    store.upsert_upstream_oauth_credentials(row1).await.unwrap();

    let mut row2 = sample_upstream_credentials();
    row2.client_id = "client-rotated".to_string();
    row2.token_blob = vec![9, 9, 9];
    store.upsert_upstream_oauth_credentials(row2).await.unwrap();

    let fetched = store
        .find_upstream_oauth_credentials("acme", "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.client_id, "client-rotated");
    assert_eq!(fetched.token_blob, vec![9, 9, 9]);
}

#[tokio::test]
async fn dynamic_client_registration_round_trip() {
    let store = temp_store().await;

    // Nothing stored yet.
    assert!(
        store
            .find_dynamic_client_registration("acme", "alice")
            .await
            .unwrap()
            .is_none()
    );

    // Save and retrieve.
    store
        .save_dynamic_client_registration("acme", "alice", "client-dyn-1")
        .await
        .unwrap();
    let found = store
        .find_dynamic_client_registration("acme", "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, "client-dyn-1");

    // Upsert with a new client_id (server re-registered).
    store
        .save_dynamic_client_registration("acme", "alice", "client-dyn-2")
        .await
        .unwrap();
    let found2 = store
        .find_dynamic_client_registration("acme", "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found2, "client-dyn-2");

    // Delete and confirm gone; other subjects unaffected.
    store
        .save_dynamic_client_registration("acme", "bob", "client-dyn-bob")
        .await
        .unwrap();
    store
        .delete_dynamic_client_registration("acme", "alice")
        .await
        .unwrap();
    assert!(
        store
            .find_dynamic_client_registration("acme", "alice")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .find_dynamic_client_registration("acme", "bob")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn clearing_upstream_identity_rolls_back_when_registration_delete_fails() {
    let store = temp_store().await;
    store
        .upsert_upstream_oauth_credentials(sample_upstream_credentials())
        .await
        .unwrap();
    store
        .save_dynamic_client_registration("acme", "alice", "client-dyn")
        .await
        .unwrap();
    store
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_dynamic_delete
                 BEFORE DELETE ON upstream_oauth_dynamic_clients
                 BEGIN SELECT RAISE(ABORT, 'injected second delete failure'); END;",
            )
            .map_err(sqlite_error)
        })
        .await
        .unwrap();

    let error = store
        .clear_upstream_oauth_identity("acme", "alice")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("injected second delete failure"));
    assert!(
        store
            .find_upstream_oauth_credentials("acme", "alice")
            .await
            .unwrap()
            .is_some(),
        "the first delete must roll back with the failed registration delete"
    );
    assert!(
        store
            .find_dynamic_client_registration("acme", "alice")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn revoking_browser_session_removes_it() {
    let store = temp_store().await;
    let row = BrowserSessionRow {
        session_id: "sess_123".into(),
        subject: "user_1".into(),
        email: None,
        csrf_token: "csrf_123".into(),
        created_at: 1,
        expires_at: now_unix() + 9_999,
    };

    store.upsert_browser_session(row).await.unwrap();
    store.revoke_browser_session("sess_123").await.unwrap();

    assert!(
        store
            .find_browser_session("sess_123")
            .await
            .unwrap()
            .is_none()
    );
}

// ── allowed_users tests ─────────────────────────────────────────────────

#[tokio::test]
async fn allowed_users_add_and_list() {
    let store = temp_store().await;
    store
        .add_allowed_user("alice@example.com", "admin", now_unix())
        .await
        .unwrap();
    let rows = store.list_allowed_users().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email, "alice@example.com");
    assert_eq!(rows[0].added_by, "admin");
}

#[tokio::test]
async fn allowed_users_duplicate_returns_validation_error() {
    let store = temp_store().await;
    let now = now_unix();
    store
        .add_allowed_user("bob@example.com", "admin", now)
        .await
        .unwrap();
    let err = store
        .add_allowed_user("bob@example.com", "admin2", now)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crate::error::AuthError::Validation(_)),
        "expected Validation, got {err:?}"
    );
}

#[tokio::test]
async fn allowed_users_input_is_lowercased() {
    let store = temp_store().await;
    store
        .add_allowed_user("Alice@Example.COM", "admin", now_unix())
        .await
        .unwrap();
    let rows = store.list_allowed_users().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email, "alice@example.com");
}

#[tokio::test]
async fn allowed_users_remove_nonexistent_is_idempotent() {
    let store = temp_store().await;
    // Must not error even when no row exists.
    store
        .remove_allowed_user("nobody@example.com")
        .await
        .unwrap();
}

#[tokio::test]
async fn allowed_users_list_ordered_by_created_at_asc() {
    let store = temp_store().await;
    let base = now_unix();
    store
        .add_allowed_user("third@example.com", "admin", base + 2)
        .await
        .unwrap();
    store
        .add_allowed_user("first@example.com", "admin", base)
        .await
        .unwrap();
    store
        .add_allowed_user("second@example.com", "admin", base + 1)
        .await
        .unwrap();
    let rows = store.list_allowed_users().await.unwrap();
    let emails: Vec<&str> = rows.iter().map(|r| r.email.as_str()).collect();
    assert_eq!(
        emails,
        vec![
            "first@example.com",
            "second@example.com",
            "third@example.com"
        ]
    );
}

#[tokio::test]
async fn allowed_users_schema_bootstrap_is_idempotent() {
    // Open the same file twice; second open must not error.
    let path = temp_db_path();
    let _store1 = SqliteStore::open(path.clone()).await.unwrap();
    let _store2 = SqliteStore::open(path).await.unwrap();
}

#[tokio::test]
async fn assertion_replay_rejection_survives_store_reopen() {
    let path = temp_db_path();
    let now = now_unix();
    let store = SqliteStore::open(path.clone()).await.unwrap();
    assert!(
        store
            .consume_assertion_jti("https://issuer.example", "one-shot", now, now + 60, now)
            .await
            .unwrap()
    );
    drop(store);

    let reopened = SqliteStore::open(path).await.unwrap();
    assert!(
        !reopened
            .consume_assertion_jti("https://issuer.example", "one-shot", now, now + 60, now)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn concurrent_assertion_consumption_has_exactly_one_winner() {
    let store = temp_store().await;
    let now = now_unix();
    let first = store.clone();
    let second = store.clone();
    let (first, second) = tokio::join!(
        first.consume_assertion_jti("https://issuer.example", "concurrent", now, now + 60, now),
        second.consume_assertion_jti("https://issuer.example", "concurrent", now, now + 60, now)
    );
    assert_ne!(first.unwrap(), second.unwrap());
}

// Ensure AllowedUserRow is importable as the right type in tests.
#[allow(dead_code)]
fn _assert_allowed_user_row_type() -> AllowedUserRow {
    AllowedUserRow {
        email: String::new(),
        added_by: String::new(),
        created_at: 0,
    }
}
