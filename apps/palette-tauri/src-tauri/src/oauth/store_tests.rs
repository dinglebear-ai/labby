use super::*;
use std::env;

fn sample(server: &str, refresh: Option<&str>, expires_at: i64) -> StoredCredentials {
    StoredCredentials {
        client_id: "client-123".to_string(),
        access_token: "access-abc".into(),
        refresh_token: refresh.map(Secret::from),
        token_endpoint: format!("{server}/token"),
        revocation_endpoint: Some(format!("{server}/revoke")),
        expires_at_unix: expires_at,
        scope: "axon:read axon:write".to_string(),
        server_url: server.to_string(),
    }
}

#[test]
fn save_then_load_round_trips() {
    let dir = env::temp_dir().join(format!("axon-oauth-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");

    let creds = sample(
        "https://axon.example.com",
        Some("refresh-xyz"),
        4_102_444_800,
    );
    save(&path, &creds).unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        !disk.contains("access-abc"),
        "access token leaked to oauth.json"
    );
    assert!(
        !disk.contains("refresh-xyz"),
        "refresh token leaked to oauth.json"
    );
    let loaded = load(&path)
        .unwrap()
        .expect("credentials present after save");

    assert_eq!(loaded.client_id, "client-123");
    assert_eq!(loaded.access_token.expose(), "access-abc");
    assert_eq!(
        loaded.refresh_token.as_ref().map(|s| s.expose()),
        Some("refresh-xyz")
    );
    assert_eq!(loaded.token_endpoint, "https://axon.example.com/token");
    assert_eq!(loaded.server_url, "https://axon.example.com");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_rotated_credentials_atomically_replaces_existing_file() {
    let dir = env::temp_dir().join(format!("labby-oauth-rotate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");

    save(
        &path,
        &sample("https://labby.example.com", Some("old-refresh"), 1),
    )
    .unwrap();
    let mut rotated = sample("https://labby.example.com", Some("new-refresh"), 2);
    rotated.access_token = "new-access".into();
    save(&path, &rotated).unwrap();

    let loaded = load(&path).unwrap().expect("rotated credentials present");
    assert_eq!(loaded.access_token.expose(), "new-access");
    assert_eq!(
        loaded.refresh_token.as_ref().map(|secret| secret.expose()),
        Some("new-refresh")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn saved_credentials_are_owner_read_write_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = env::temp_dir().join(format!("labby-oauth-mode-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(
        &path,
        &sample("https://labby.example.com", Some("secret"), 1),
    )
    .unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_missing_file_returns_none() {
    let path = env::temp_dir().join(format!("axon-oauth-missing-{}.json", uuid::Uuid::new_v4()));
    assert!(load(&path).unwrap().is_none());
}

#[test]
fn loading_legacy_plaintext_credentials_migrates_tokens_to_vault() {
    let dir = env::temp_dir().join(format!("labby-oauth-migrate-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    std::fs::write(
        &path,
        r#"{"client_id":"legacy","access_token":"legacy-access","refresh_token":"legacy-refresh","token_endpoint":"https://labby.example/token","revocation_endpoint":null,"expires_at_unix":4102444800,"scope":"lab:read","server_url":"https://labby.example"}"#,
    )
    .unwrap();

    let loaded = load(&path).unwrap().expect("legacy credentials migrate");
    assert_eq!(loaded.access_token.expose(), "legacy-access");
    let migrated = std::fs::read_to_string(&path).unwrap();
    assert!(!migrated.contains("legacy-access"));
    assert!(!migrated.contains("legacy-refresh"));
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clear_removes_the_file_and_is_idempotent() {
    let dir = env::temp_dir().join(format!("axon-oauth-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", None, 0)).unwrap();
    clear(&path).unwrap();
    assert!(load(&path).unwrap().is_none());
    clear(&path).unwrap(); // second clear must not error
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn vault_outage_is_not_reported_as_signed_out() {
    let dir = env::temp_dir().join(format!("labby-oauth-outage-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", None, 0)).unwrap();
    set_test_vault_failures(false, true, false);
    let error = load(&path).expect_err("vault outage must be observable");
    set_test_vault_failures(false, false, false);
    assert!(error.contains("vault"));
    assert!(load(&path).unwrap().is_some());
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_vault_replacement_preserves_old_session() {
    let dir = env::temp_dir().join(format!("labby-oauth-vault-fail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();
    set_test_vault_failures(true, false, false);
    assert!(save(&path, &sample("https://a", Some("new"), 2)).is_err());
    set_test_vault_failures(false, false, false);
    let loaded = load(&path).unwrap().unwrap();
    assert_eq!(loaded.refresh_token.unwrap().expose(), "old");
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_vault_delete_preserves_old_session() {
    let dir = env::temp_dir().join(format!("labby-oauth-delete-fail-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();
    set_test_vault_failures(false, false, true);
    assert!(clear(&path).is_err());
    set_test_vault_failures(false, false, false);
    assert!(load(&path).unwrap().is_some());
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_metadata_write_restores_old_vault_session() {
    let dir = env::temp_dir().join(format!("labby-oauth-file-write-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();
    set_test_file_failures(true, false);
    assert!(save(&path, &sample("https://a", Some("new"), 2)).is_err());
    set_test_file_failures(false, false);
    let loaded = load(&path).unwrap().unwrap();
    assert_eq!(loaded.refresh_token.unwrap().expose(), "old");
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_metadata_delete_restores_old_vault_session() {
    let dir = env::temp_dir().join(format!("labby-oauth-file-delete-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();
    set_test_file_failures(false, true);
    assert!(clear(&path).is_err());
    set_test_file_failures(false, false);
    assert_eq!(
        load(&path)
            .unwrap()
            .unwrap()
            .refresh_token
            .unwrap()
            .expose(),
        "old"
    );
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn vault_read_outage_prevents_save_from_mutating_existing_session() {
    let dir = env::temp_dir().join(format!("labby-oauth-save-read-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();
    let metadata_before = std::fs::read_to_string(&path).unwrap();

    set_test_vault_failures(false, true, false);
    let error = save(&path, &sample("https://a", Some("new"), 2))
        .expect_err("a vault read outage must abort before replacement");
    set_test_vault_failures(false, false, false);

    assert!(error.contains("vault read failure"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), metadata_before);
    assert_eq!(
        load(&path)
            .unwrap()
            .unwrap()
            .refresh_token
            .unwrap()
            .expose(),
        "old"
    );
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn vault_read_outage_prevents_clear_from_mutating_existing_session() {
    let dir = env::temp_dir().join(format!("labby-oauth-clear-read-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();
    let metadata_before = std::fs::read_to_string(&path).unwrap();

    set_test_vault_failures(false, true, false);
    let error = clear(&path).expect_err("a vault read outage must abort before deletion");
    set_test_vault_failures(false, false, false);

    assert!(error.contains("vault read failure"), "{error}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), metadata_before);
    assert_eq!(
        load(&path)
            .unwrap()
            .unwrap()
            .refresh_token
            .unwrap()
            .expose(),
        "old"
    );
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn metadata_write_and_rollback_set_failures_report_uncertain_state() {
    let dir = env::temp_dir().join(format!(
        "labby-oauth-write-compound-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();

    set_test_file_failures(true, false);
    set_test_rollback_failures(true, false);
    let error = save(&path, &sample("https://a", Some("new"), 2))
        .expect_err("failed rollback must not claim the old session was preserved");
    set_test_file_failures(false, false);
    set_test_rollback_failures(false, false);

    assert!(error.contains("metadata write failure"), "{error}");
    assert!(error.contains("rollback write failure"), "{error}");
    assert!(error.contains("state is uncertain"), "{error}");
    assert_eq!(
        load(&path)
            .unwrap()
            .unwrap()
            .refresh_token
            .unwrap()
            .expose(),
        "new",
        "the injected rollback failure intentionally leaves a mixed generation"
    );
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn metadata_write_and_rollback_delete_failures_report_uncertain_state() {
    let dir = env::temp_dir().join(format!(
        "labby-oauth-create-compound-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");

    set_test_file_failures(true, false);
    set_test_rollback_failures(false, true);
    let error = save(&path, &sample("https://a", Some("new"), 2))
        .expect_err("failed cleanup of a new vault entry must be explicit");
    set_test_file_failures(false, false);
    set_test_rollback_failures(false, false);

    assert!(error.contains("metadata write failure"), "{error}");
    assert!(error.contains("rollback delete failure"), "{error}");
    assert!(error.contains("state is uncertain"), "{error}");
    assert!(!path.exists());
    assert!(matches!(
        vault_get_state(&vault_account(&path)).unwrap(),
        VaultState::Present(_)
    ));
    clear(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn metadata_delete_and_rollback_set_failures_report_uncertain_state() {
    let dir = env::temp_dir().join(format!(
        "labby-oauth-delete-compound-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    save(&path, &sample("https://a", Some("old"), 1)).unwrap();

    set_test_file_failures(false, true);
    set_test_rollback_failures(true, false);
    let error = clear(&path).expect_err("failed restore after metadata deletion must be explicit");
    set_test_file_failures(false, false);
    set_test_rollback_failures(false, false);

    assert!(error.contains("metadata delete failure"), "{error}");
    assert!(error.contains("rollback write failure"), "{error}");
    assert!(error.contains("state is uncertain"), "{error}");
    assert!(path.exists());
    assert!(matches!(
        vault_get_state(&vault_account(&path)).unwrap(),
        VaultState::Missing
    ));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn expiry_accounts_for_skew() {
    let creds = sample("https://a", None, 1000);
    assert!(!creds.is_expired(900, 30)); // 900 + 30 < 1000 → valid
    assert!(creds.is_expired(980, 30)); // 980 + 30 >= 1000 → treat as expired
    assert!(creds.is_expired(1000, 0));
}

#[test]
fn matches_server_is_exact_after_trailing_slash_trim() {
    let creds = sample("https://axon.example.com", None, 0);
    assert!(creds.matches_server("https://axon.example.com"));
    assert!(creds.matches_server("https://axon.example.com/"));
    assert!(!creds.matches_server("https://other.example.com"));
}

#[test]
fn debug_redacts_token_fields() {
    let creds = sample("https://axon.example.com", Some("refresh-xyz"), 0);
    let rendered = format!("{creds:?}");
    assert!(
        !rendered.contains("access-abc"),
        "access token leaked: {rendered}"
    );
    assert!(
        !rendered.contains("refresh-xyz"),
        "refresh token leaked: {rendered}"
    );
    assert!(
        rendered.contains("client-123"),
        "non-secret field should remain"
    );
}
