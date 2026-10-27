//! Durable provider-bundle compare-and-swap across delete/reinsert generation reuse.

use super::SqliteStore;
use crate::at_rest::TokenEncryptionKey;
use crate::types::GoogleProviderCredentialUpdate;

fn update(access: &str, refresh: &str) -> GoogleProviderCredentialUpdate {
    GoogleProviderCredentialUpdate {
        subject: "google-successor".into(),
        email: Some("operator@example.com".into()),
        client_id: "client".into(),
        granted_scopes: vec!["openid".into()],
        access_token: access.into(),
        refresh_token: refresh.into(),
        token_received_at: 1,
        access_token_expires_at: 3601,
        issuer: Some("https://accounts.google.com".into()),
        refreshed: true,
        scope_upgraded: false,
    }
}

#[tokio::test]
async fn google_successor_cas_rejects_reused_generation_across_connections() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth.db");
    let key = TokenEncryptionKey::from_passphrase("google-successor-test-key");
    let writer = SqliteStore::open_with_key(path.clone(), Some(key.clone()))
        .await
        .unwrap();
    let revoker = SqliteStore::open_with_key(path.clone(), Some(key.clone()))
        .await
        .unwrap();
    let observed_epoch = writer.google_provider_fence_epoch().await.unwrap();
    assert!(
        writer
            .insert_google_provider_token_bundle_if_absent(
                update("old-access", "old-refresh"),
                observed_epoch,
            )
            .await
            .unwrap()
    );
    let original = writer
        .find_google_provider_credential("google-successor")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.generation, 1);

    assert!(
        revoker
            .revoke_google_provider_credential(Some("google-successor"))
            .await
            .unwrap()
            .invalidated
    );
    let fresh_epoch = revoker.google_provider_fence_epoch().await.unwrap();
    assert!(fresh_epoch > observed_epoch);
    assert!(
        revoker
            .insert_google_provider_token_bundle_if_absent(
                update("fresh-access", "fresh-refresh"),
                fresh_epoch,
            )
            .await
            .unwrap()
    );
    let fresh = revoker
        .find_google_provider_credential("google-successor")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.generation, original.generation);

    // This direct durable write intentionally bypasses process-local guards.
    let persisted = writer
        .replace_google_provider_token_bundle_if_generation(
            update("late-access", "late-refresh"),
            original.generation,
            observed_epoch,
        )
        .await
        .unwrap();
    let current = writer
        .find_google_provider_credential("google-successor")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        current, fresh,
        "late CAS must preserve the complete fresh bundle"
    );
    assert!(!persisted, "late CAS must report stale authority");

    // A healthy refresh under the new authority still replaces the bundle.
    assert!(
        writer
            .replace_google_provider_token_bundle_if_generation(
                update("healthy-access", "healthy-refresh"),
                fresh.generation,
                fresh_epoch,
            )
            .await
            .unwrap()
    );
    let reopened = SqliteStore::open_with_key(path, Some(key)).await.unwrap();
    let healthy = reopened
        .find_google_provider_credential("google-successor")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(healthy.generation, fresh.generation + 1);
    assert_eq!(healthy.access_token.as_deref(), Some("healthy-access"));
    assert_eq!(healthy.refresh_token, "healthy-refresh");
}

#[tokio::test]
async fn google_successor_invalidation_rejects_reused_generation_across_connections() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("auth.db");
    let key = TokenEncryptionKey::from_passphrase("google-successor-invalidation-test-key");
    let writer = SqliteStore::open_with_key(path.clone(), Some(key.clone()))
        .await
        .unwrap();
    let revoker = SqliteStore::open_with_key(path, Some(key)).await.unwrap();
    let observed_epoch = writer.google_provider_fence_epoch().await.unwrap();
    assert!(
        writer
            .insert_google_provider_token_bundle_if_absent(
                update("old-access", "old-refresh"),
                observed_epoch,
            )
            .await
            .unwrap()
    );
    let original = writer
        .find_google_provider_credential("google-successor")
        .await
        .unwrap()
        .unwrap();
    assert!(
        revoker
            .revoke_google_provider_credential(Some("google-successor"))
            .await
            .unwrap()
            .invalidated
    );
    let fresh_epoch = revoker.google_provider_fence_epoch().await.unwrap();
    assert!(fresh_epoch > observed_epoch);
    assert!(
        revoker
            .insert_google_provider_token_bundle_if_absent(
                update("fresh-access", "fresh-refresh"),
                fresh_epoch,
            )
            .await
            .unwrap()
    );
    let fresh = revoker
        .find_google_provider_credential("google-successor")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.generation, original.generation);

    // A terminal response from the old refresh must not delete the successor.
    let invalidation = writer
        .invalidate_google_provider_credential(
            "google-successor",
            original.generation,
            observed_epoch,
        )
        .await
        .unwrap();
    assert_eq!(
        writer
            .find_google_provider_credential("google-successor")
            .await
            .unwrap(),
        Some(fresh),
        "late invalidation must preserve the complete fresh bundle"
    );
    assert!(!invalidation.invalidated);
    assert_eq!(
        writer.google_provider_fence_epoch().await.unwrap(),
        fresh_epoch
    );
}
