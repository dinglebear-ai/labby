use super::*;
use std::{collections::HashMap, env, sync::Mutex};

#[derive(Default)]
struct FakeVault {
    values: Mutex<HashMap<String, String>>,
    fail_put: bool,
    fail_get: bool,
    fail_delete: bool,
}
impl CredentialVault for FakeVault {
    fn scope(&self) -> &str {
        VAULT_SERVICE
    }
    fn put(&self, handle: &str, value: &str) -> Result<(), String> {
        if self.fail_put {
            return Err("vault write failed".into());
        }
        self.values
            .lock()
            .unwrap()
            .insert(handle.into(), value.into());
        Ok(())
    }
    fn get(&self, handle: &str) -> Result<String, String> {
        if self.fail_get {
            return Err("vault read failed".into());
        }
        self.values
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or_else(|| "missing vault record".into())
    }
    fn delete(&self, handle: &str) -> Result<(), String> {
        if self.fail_delete {
            return Err("vault delete failed".into());
        }
        self.values.lock().unwrap().remove(handle);
        Ok(())
    }
}

fn sample(server: &str, refresh: Option<&str>, expires_at: i64) -> StoredCredentials {
    StoredCredentials {
        client_id: "client-123".into(),
        access_token: "access-abc".into(),
        refresh_token: refresh.map(Secret::from),
        token_endpoint: format!("{server}/token"),
        revocation_endpoint: Some(format!("{server}/revoke")),
        expires_at_unix: expires_at,
        scope: "lab:read lab:write".into(),
        server_url: server.into(),
    }
}
fn temp_path(label: &str) -> (PathBuf, PathBuf) {
    let dir = env::temp_dir().join(format!("labby-oauth-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oauth.json");
    (dir, path)
}

#[test]
fn metadata_never_contains_token_bytes() {
    let (dir, path) = temp_path("metadata");
    let vault = FakeVault::default();
    save_to(
        &path,
        &sample("https://labby.example.com", Some("refresh-xyz"), 42),
        &vault,
    )
    .unwrap();
    let json = std::fs::read_to_string(&path).unwrap();
    assert!(!json.contains("access-abc"));
    assert!(!json.contains("refresh-xyz"));
    assert!(json.contains("vault_handle"));
    let loaded = load_from(&path, &vault).unwrap().unwrap();
    assert_eq!(loaded.access_token.expose(), "access-abc");
    assert_eq!(loaded.refresh_token.unwrap().expose(), "refresh-xyz");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn legacy_plaintext_is_removed_after_verified_migration() {
    let (dir, path) = temp_path("migration");
    let legacy = sample("https://labby.example.com", Some("legacy-refresh"), 42);
    std::fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    let vault = FakeVault::default();
    let loaded = load_from(&path, &vault).unwrap().unwrap();
    assert_eq!(loaded.refresh_token.unwrap().expose(), "legacy-refresh");
    let json = std::fs::read_to_string(&path).unwrap();
    assert!(!json.contains("access-abc"));
    assert!(!json.contains("legacy-refresh"));
    assert_eq!(vault.values.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn failed_vault_migration_preserves_legacy_file() {
    for vault in [
        FakeVault {
            fail_put: true,
            ..FakeVault::default()
        },
        FakeVault {
            fail_get: true,
            ..FakeVault::default()
        },
    ] {
        let (dir, path) = temp_path("migration-failure");
        let original = serde_json::to_vec_pretty(&sample(
            "https://labby.example.com",
            Some("legacy-refresh"),
            42,
        ))
        .unwrap();
        std::fs::write(&path, &original).unwrap();
        assert!(load_from(&path, &vault).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(dir).ok();
    }
}

#[test]
fn failed_rotation_preserves_prior_recoverable_state() {
    let (dir, path) = temp_path("rotation-failure");
    let vault = FakeVault::default();
    save_to(
        &path,
        &sample("https://labby.example.com", Some("old-refresh"), 1),
        &vault,
    )
    .unwrap();
    let original_metadata = std::fs::read(&path).unwrap();
    let failing = FakeVault {
        values: Mutex::new(vault.values.lock().unwrap().clone()),
        fail_put: true,
        fail_get: false,
        fail_delete: false,
    };
    assert!(
        save_to(
            &path,
            &sample("https://labby.example.com", Some("new-refresh"), 2),
            &failing
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original_metadata);
    assert_eq!(
        load_from(&path, &failing)
            .unwrap()
            .unwrap()
            .refresh_token
            .unwrap()
            .expose(),
        "old-refresh"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn failed_delete_retains_retryable_handles_for_rotation_and_clear() {
    let (dir, path) = temp_path("delete-recovery");
    let vault = FakeVault::default();
    save_to(
        &path,
        &sample("https://one.example", Some("old"), 1),
        &vault,
    )
    .unwrap();
    let old_handle = read_metadata(&path).unwrap().vault_handle;
    let failing = FakeVault {
        values: Mutex::new(vault.values.lock().unwrap().clone()),
        fail_delete: true,
        ..FakeVault::default()
    };
    save_to(
        &path,
        &sample("https://two.example", Some("new"), 2),
        &failing,
    )
    .unwrap();
    let metadata = read_metadata(&path).unwrap();
    assert!(metadata.retiring_handles.contains(&old_handle));
    assert!(clear_from(&path, &failing).is_err());
    assert!(path.is_file(), "failed clear must retain recovery metadata");

    let recovered = FakeVault {
        values: Mutex::new(failing.values.lock().unwrap().clone()),
        ..FakeVault::default()
    };
    load_from(&path, &recovered).unwrap().unwrap();
    assert!(read_metadata(&path).unwrap().retiring_handles.is_empty());
    clear_from(&path, &recovered).unwrap();
    assert!(!path.exists());
    assert!(recovered.values.lock().unwrap().is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn failed_metadata_write_removes_new_vault_record_without_exposing_tokens() {
    let (dir, path) = temp_path("metadata-failure");
    std::fs::remove_file(&path).ok();
    std::fs::create_dir(&path).unwrap();
    let vault = FakeVault::default();
    let error = save_to(
        &path,
        &sample(
            "https://labby.example.com",
            Some("metadata-failure-secret"),
            1,
        ),
        &vault,
    )
    .unwrap_err();
    assert!(!error.contains("metadata-failure-secret"));
    assert!(vault.values.lock().unwrap().is_empty());
    assert!(path.is_dir());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn vault_is_palette_scoped_with_per_record_handles() {
    let (dir, path) = temp_path("scope");
    let vault = FakeVault::default();
    assert_eq!(vault.scope(), "tv.tootie.lab.palette.oauth");
    save_to(&path, &sample("https://one.example", None, 1), &vault).unwrap();
    let first = read_metadata(&path).unwrap().vault_handle;
    save_to(&path, &sample("https://two.example", None, 2), &vault).unwrap();
    let second = read_metadata(&path).unwrap().vault_handle;
    assert_ne!(first, second);
    assert!(!vault.values.lock().unwrap().contains_key(&first));
    assert!(vault.values.lock().unwrap().contains_key(&second));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn expiry_matching_and_debug_contracts_remain_safe() {
    let creds = sample("https://labby.example.com", Some("refresh-xyz"), 1_000);
    assert!(!creds.is_expired(900, 30));
    assert!(creds.is_expired(980, 30));
    assert!(creds.matches_server("https://labby.example.com/"));
    assert!(!creds.matches_server("https://other.example.com"));
    let rendered = format!("{creds:?}");
    assert!(!rendered.contains("access-abc"));
    assert!(!rendered.contains("refresh-xyz"));
}
