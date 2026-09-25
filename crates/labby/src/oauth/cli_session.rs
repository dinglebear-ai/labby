//! Explicit operator OAuth sessions for a single remote Labby authority.
//!
//! Reuses the encrypted OAuth store, PKCE callback and refresh implementation.
//! A saved session never selects a server and never overrides an explicit token.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use labby_auth::config::AuthConfig;
use labby_auth::upstream::runtime::build_upstream_oauth_runtime_with_redirect;
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamOauthRegistration};
use rusqlite::OptionalExtension as _;
use sha2::{Digest, Sha256};
use url::Url;

const SUBJECT: &str = "gateway";

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionProfile {
    registration: UpstreamOauthRegistration,
    upstream_name: String,
}

fn fresh_upstream_name() -> Result<String> {
    let mut bytes = [0_u8; 18];
    getrandom::fill(&mut bytes).context("generate CLI login identity")?;
    Ok(format!(
        "operator-server-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub(crate) const CLIENT_METADATA_PATH: &str = "/.well-known/labby-cli-client.json";

/// Public native-client identity. It conveys no access authority or credentials.
pub(crate) fn client_metadata(server: &Url) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "client_id": server.join(CLIENT_METADATA_PATH)?.as_str(),
        "client_name": "Labby CLI",
        "redirect_uris": ["http://127.0.0.1/auth/upstream/callback"],
        "token_endpoint_auth_method": "none",
        "application_type": "native",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"]
    }))
}

fn default_registration(
    server: &Url,
    supports_cimd: bool,
    supports_dynamic: bool,
) -> Result<UpstreamOauthRegistration> {
    if supports_cimd {
        return Ok(UpstreamOauthRegistration::ClientMetadataDocument {
            url: server.join(CLIENT_METADATA_PATH)?.to_string(),
        });
    }
    ensure!(
        supports_dynamic,
        "server advertises no automatic client registration; use --client-id"
    );
    Ok(UpstreamOauthRegistration::Dynamic)
}

pub(crate) fn server_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("invalid Labby server URL")?;
    ensure!(
        url.scheme() == "https" && url.host_str().is_some(),
        "CLI sign-in requires an HTTPS Labby server"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "CLI sign-in requires a server origin without credentials, a path, query, or fragment"
    );
    Ok(url)
}

fn profile_path(server: &Url) -> Result<PathBuf> {
    let root = crate::installation::InstallationPaths::resolve()?;
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(server.as_str().as_bytes()));
    Ok(root.root().join("cli-sessions").join(key))
}

async fn choose_registration(
    server: &Url,
    metadata: &rmcp::transport::auth::AuthorizationMetadata,
) -> Result<UpstreamOauthRegistration> {
    let supported = metadata
        .additional_fields
        .get("client_id_metadata_document_supported")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    let available = if supported {
        let client_id = server.join(CLIENT_METADATA_PATH)?;
        match labby_auth::upstream::http_client::fetch_client_metadata_document(
            server.as_str(),
            client_id.as_str(),
        )
        .await?
        {
            Some(document) => {
                ensure!(
                    document
                        .get("redirect_uris")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|uris| uris
                            .iter()
                            .any(|uri| uri.as_str()
                                == Some("http://127.0.0.1/auth/upstream/callback")))
                        && document
                            .get("token_endpoint_auth_method")
                            .and_then(serde_json::Value::as_str)
                            .is_none_or(|method| method == "none"),
                    "published CLI client metadata is incompatible with native login; use --client-id or --client-metadata-url"
                );
                true
            }
            None => false,
        }
    } else {
        false
    };
    default_registration(server, available, metadata.registration_endpoint.is_some())
}

fn upstream(server: &Url, registration: &UpstreamOauthRegistration) -> Result<UpstreamConfig> {
    if let UpstreamOauthRegistration::ClientMetadataDocument { url: raw } = registration {
        let url = Url::parse(raw).context("invalid client metadata URL")?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() != "/",
            "client metadata must have a public HTTPS URL without credentials, query or fragment"
        );
    }
    Ok(serde_json::from_value(serde_json::json!({
        "name": "operator-server", "url": server.join("mcp")?.as_str(),
        "oauth": {"mode": "authorization_code_pkce", "registration": registration,
                  "scopes": ["lab:read", "lab", "lab:admin"]}
    }))?)
}

async fn lock_profile(path: &Path) -> Result<std::fs::File> {
    let lock = labby_auth::util::open_restricted_lock_file(&path.join("session.lock"))?;
    for _ in 0..100 {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    anyhow::bail!("another CLI sign-in or credential refresh is running for this server")
}

fn private_key(path: &Path, create: bool) -> Result<String> {
    let key_path = path.join("encryption.key");
    if !key_path.try_exists()? && create {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).context("generate CLI credential encryption key")?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let mut file = labby_auth::util::create_restricted_secret_file(&key_path)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
    }
    // The restricted opener rejects links, unexpected owners and unsafe ancestors.
    ensure!(
        key_path.try_exists()?,
        "CLI credential encryption key is missing; sign in again"
    );
    let mut file = labby_auth::util::open_restricted_lock_file(&key_path)?;
    ensure!(
        file.metadata()?.len() == 44,
        "invalid CLI credential encryption key"
    );
    let mut key = String::new();
    file.read_to_string(&mut key)?;
    Ok(key)
}

fn auth_config(path: &Path) -> AuthConfig {
    AuthConfig {
        sqlite_path: path.join("oauth.db"),
        ..AuthConfig::default()
    }
}

fn read_profile(path: &Path) -> Result<Option<SessionProfile>> {
    let path = path.join("client.json");
    if !path.try_exists()? {
        return Ok(None);
    }
    let mut file = labby_auth::util::open_restricted_lock_file(&path)?;
    ensure!(
        file.metadata()?.len() <= 8192,
        "invalid CLI registration profile"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    parse_profile(&bytes)
}

fn parse_profile(bytes: &[u8]) -> Result<Option<SessionProfile>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    // Accept the initial CLI profile while preserving its exact client binding.
    if let Some(url) = value.as_str() {
        return Ok(Some(SessionProfile {
            registration: UpstreamOauthRegistration::ClientMetadataDocument {
                url: url.to_owned(),
            },
            upstream_name: "operator-server".into(),
        }));
    }
    if value.is_null() {
        return Ok(None);
    }
    let profile = if value.get("upstream_name").is_some() {
        serde_json::from_value::<SessionProfile>(value)?
    } else {
        SessionProfile {
            registration: serde_json::from_value(value)?,
            upstream_name: "operator-server".into(),
        }
    };
    ensure!(
        profile.upstream_name.starts_with("operator-server")
            && profile.upstream_name.len() <= 64
            && profile
                .upstream_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "invalid CLI session identity"
    );
    Ok(Some(profile))
}

fn previous_profile_for_login(path: &Path) -> Result<Option<SessionProfile>> {
    match read_profile(path) {
        // An explicit sign-in may repair malformed JSON. Filesystem safety and
        // identity-validation errors still fail closed; never clean an unknown row.
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => Ok(None),
        result => result,
    }
}

fn publish_profile(path: &Path, profile: &SessionProfile) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path)?;
    labby_auth::util::harden_secret_file(file.path())?;
    file.write_all(&serde_json::to_vec(profile)?)?;
    file.as_file().sync_all()?;
    file.persist(path.join("client.json"))?;
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

/// Interactive sign-in is only entered by the explicit login command.
pub(crate) async fn login(
    server: &Url,
    selection: Option<UpstreamOauthRegistration>,
) -> Result<()> {
    let path = profile_path(server)?;
    let root = crate::installation::InstallationPaths::resolve()?;
    root.prepare_root()?;
    crate::installation::InstallationPaths::from_root(root.root().join("cli-sessions"))?
        .prepare_root()?;
    crate::installation::InstallationPaths::from_root(&path)?.prepare_root()?;
    let _lock = lock_profile(&path).await?;
    let key = private_key(&path, true)?;
    login_locked(&path, &key, server, selection).await
}

async fn login_locked(
    path: &Path,
    key: &str,
    server: &Url,
    selection: Option<UpstreamOauthRegistration>,
) -> Result<()> {
    let metadata =
        labby_auth::upstream::manager::discover_published_metadata(server.join("mcp")?.as_str())
            .await?
            .context("server does not publish OAuth metadata")?;
    let previous = previous_profile_for_login(path)?;
    let selection = match selection {
        Some(selection) => selection,
        None => match previous.as_ref() {
            Some(saved) => saved.registration.clone(),
            None => choose_registration(server, &metadata).await?,
        },
    };
    let mut config = upstream(server, &selection)?;
    config.name = fresh_upstream_name()?;
    if matches!(selection, UpstreamOauthRegistration::Dynamic) {
        ensure!(
            metadata.registration_endpoint.is_some(),
            "server disables dynamic registration; use --client-metadata-url or --client-id"
        );
    }
    let runtime = super::upstream_stdio::build_stdio_upstream_oauth_runtime(
        std::slice::from_ref(&config),
        &auth_config(path),
        Some(key),
    )
    .await?
    .context("CLI OAuth runtime unavailable")?;
    // A new upstream identity stages encrypted credentials independently.
    // Existing credentials and their binding stay usable on cancellation or error.
    let result = runtime.cache.get_or_build(&config, SUBJECT).await;
    let authorization: Result<()> = match result {
        Ok(client) => client
            .get_access_token()
            .await
            .map(|_| ())
            .map_err(Into::into),
        Err(error) => Err(error.into()),
    };
    finish_login(
        path,
        &runtime.sqlite,
        SessionProfile {
            registration: selection,
            upstream_name: config.name,
        },
        previous,
        authorization,
    )
    .await
}

async fn finish_login(
    path: &Path,
    sqlite: &labby_auth::sqlite::SqliteStore,
    staged: SessionProfile,
    previous: Option<SessionProfile>,
    authorization: Result<()>,
) -> Result<()> {
    if let Err(error) = authorization {
        drop(
            sqlite
                .clear_upstream_oauth_identity(&staged.upstream_name, SUBJECT)
                .await,
        );
        return Err(error);
    }
    publish_profile(path, &staged)?;
    if let Some(previous) = previous
        && sqlite
            .clear_upstream_oauth_identity(&previous.upstream_name, SUBJECT)
            .await
            .is_err()
    {
        tracing::warn!("signed in; old encrypted CLI identity cleanup deferred");
    }
    Ok(())
}

/// Read only the non-secret session binding, without refresh, chmod, locks, or network I/O.
/// The random identity changes after a new login and disappears on logout.
pub(crate) fn stored_identity(server: &Url) -> Result<Option<String>> {
    let Ok(server) = server_url(server.as_str()) else {
        return Ok(None);
    };
    let path = profile_path(&server)?.join("client.json");
    let raw = crate::config::host_write::read_config_snapshot(&path)?;
    if raw.is_empty() {
        return Ok(None);
    }
    ensure!(
        raw.len() <= 8192,
        "CLI session metadata exceeds its size limit"
    );
    Ok(parse_profile(raw.as_bytes())?.map(|profile| profile.upstream_name))
}

/// Stored scopes describe only this CLI's saved grant; they do not describe a
/// separate MCP client's token or prove current authorization at the server.
pub(crate) fn status(server: &Url) -> Result<serde_json::Value> {
    let identity = stored_identity(server)?;
    let configured = identity.is_some();
    let granted_scopes = identity
        .as_deref()
        .map(|identity| saved_granted_scopes(&profile_path(server)?.join("oauth.db"), identity))
        .transpose()?
        .flatten();
    Ok(
        serde_json::json!({"server":server.as_str(),"saved_session":configured,"verified_online":false,
        "granted_scopes":granted_scopes,
        "guidance":if configured { "Scopes, when shown, are from this CLI's saved OAuth grant. This offline check did not refresh credentials or verify remote authorization; it does not inspect Codex or another MCP client's token." } else { "No saved OAuth session exists for this destination. Use labby auth login --server URL." }}),
    )
}

fn saved_granted_scopes(path: &Path, identity: &str) -> Result<Option<Vec<String>>> {
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(metadata.is_file(), "invalid CLI OAuth database path");
    // SQLite's NOFOLLOW also rejects macOS's /var ancestor symlink. Canonicalize
    // the validated parent while retaining NOFOLLOW for the database entry.
    let canonical_path = path
        .parent()
        .context("CLI OAuth database has no parent directory")?
        .canonicalize()?
        .join(
            path.file_name()
                .context("CLI OAuth database has no filename")?,
        );
    let db = rusqlite::Connection::open_with_flags(
        canonical_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    let raw: Option<String> = db
        .query_row(
            "SELECT granted_scopes_json FROM upstream_oauth_credentials WHERE upstream_name = ?1 AND subject = ?2",
            rusqlite::params![identity, SUBJECT],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

async fn clear_profile_credentials(
    path: &Path,
    store: &labby_auth::sqlite::SqliteStore,
) -> Result<bool> {
    let Some(profile) = read_profile(path)? else {
        return Ok(false);
    };
    store
        .clear_upstream_oauth_identity(&profile.upstream_name, SUBJECT)
        .await?;
    std::fs::remove_file(path.join("client.json"))?;
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    Ok(true)
}

/// Sign out locally from one exact authority. Does not revoke provider-wide credentials.
pub(crate) async fn logout(server: &Url) -> Result<bool> {
    let server = server_url(server.as_str())?;
    let path = profile_path(&server)?;
    if !path.try_exists()? || !path.join("client.json").try_exists()? {
        return Ok(false);
    }
    crate::installation::InstallationPaths::from_root(&path)?;
    let _lock = lock_profile(&path).await?;
    if read_profile(&path)?.is_none() {
        return Ok(false);
    }
    let key = private_key(&path, false)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(key)?;
    let key = labby_auth::at_rest::TokenEncryptionKey::from_encoded(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
    )?;
    let store =
        labby_auth::sqlite::SqliteStore::open_with_key(path.join("oauth.db"), Some(key)).await?;
    clear_profile_credentials(&path, &store).await
}

/// Returns no session only when this authority has never been configured.
/// Broken/expired sessions fail closed and do not launch a browser implicitly.
pub(crate) async fn token(server: &Url) -> Result<Option<String>> {
    let Ok(server) = server_url(server.as_str()) else {
        // Existing loopback HTTP and mounted-base targets do not support this
        // login profile; preserve their explicit-token/anonymous behavior.
        return Ok(None);
    };
    let path = profile_path(&server)?;
    if !path.try_exists()? {
        return Ok(None);
    }
    crate::installation::InstallationPaths::from_root(&path)?.prepare_root()?;
    let _lock = lock_profile(&path).await?;
    let key = private_key(&path, false)?;
    let profile =
        read_profile(&path)?.context("CLI session profile is missing; run labby auth login")?;
    let mut config = upstream(&server, &profile.registration)?;
    config.name = profile.upstream_name;
    let runtime = build_upstream_oauth_runtime_with_redirect(
        std::slice::from_ref(&config),
        &auth_config(&path),
        Some(&key),
        "http://127.0.0.1/auth/upstream/callback".to_owned(),
    )
    .await?
    .context("CLI OAuth runtime unavailable")?;
    let manager = runtime
        .managers
        .get(&config.name)
        .context("CLI OAuth manager unavailable")?
        .clone();
    ensure!(
        manager.has_credentials(SUBJECT).await?,
        "no valid CLI session; run labby auth login again"
    );
    let client = manager.build_auth_client(SUBJECT).await?;
    Ok(Some(client.get_access_token().await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_scopes_read_only_from_selected_cli_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE upstream_oauth_credentials (upstream_name TEXT, subject TEXT, granted_scopes_json TEXT);").unwrap();
        db.execute(
            "INSERT INTO upstream_oauth_credentials VALUES (?1, ?2, ?3)",
            rusqlite::params!["operator-server-current", SUBJECT, r#"["lab:read","lab"]"#],
        )
        .unwrap();
        db.execute(
            "INSERT INTO upstream_oauth_credentials VALUES (?1, ?2, ?3)",
            rusqlite::params!["operator-server-old", SUBJECT, r#"["lab:admin"]"#],
        )
        .unwrap();
        drop(db);

        assert_eq!(
            saved_granted_scopes(&path, "operator-server-current").unwrap(),
            Some(vec!["lab:read".into(), "lab".into()])
        );
        assert_eq!(saved_granted_scopes(&path, "missing").unwrap(), None);
        assert_eq!(
            saved_granted_scopes(&dir.path().join("missing.db"), "missing").unwrap(),
            None
        );
        assert!(!dir.path().join("missing.db").exists());
    }

    #[cfg(unix)]
    #[test]
    fn saved_scopes_reject_symlinked_database() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.db");
        std::fs::write(&target, b"untouched").unwrap();
        let path = dir.path().join("oauth.db");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(saved_granted_scopes(&path, "operator-server").is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"untouched");
    }

    #[tokio::test]
    async fn logout_clears_only_the_selected_identity_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = labby_auth::sqlite::SqliteStore::open(dir.path().join("oauth.db"))
            .await
            .unwrap();
        publish_profile(
            dir.path(),
            &SessionProfile {
                registration: UpstreamOauthRegistration::Dynamic,
                upstream_name: "operator-server-selected".into(),
            },
        )
        .unwrap();
        for name in ["operator-server-selected", "operator-server-other"] {
            store
                .save_dynamic_client_registration(name, SUBJECT, name)
                .await
                .unwrap();
        }
        assert!(clear_profile_credentials(dir.path(), &store).await.unwrap());
        assert!(!dir.path().join("client.json").exists());
        assert!(
            store
                .find_dynamic_client_registration("operator-server-selected", SUBJECT)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .find_dynamic_client_registration("operator-server-other", SUBJECT)
                .await
                .unwrap()
                .as_deref(),
            Some("operator-server-other")
        );
        assert!(!clear_profile_credentials(dir.path(), &store).await.unwrap());
    }

    #[test]
    fn explicit_login_can_repair_malformed_profile_contents() {
        let dir = tempfile::tempdir().unwrap();
        let mut file =
            labby_auth::util::create_restricted_secret_file(&dir.path().join("client.json"))
                .unwrap();
        file.write_all(b"{truncated").unwrap();
        assert!(read_profile(dir.path()).is_err());
        assert!(previous_profile_for_login(dir.path()).unwrap().is_none());
        assert_eq!(
            std::fs::read(dir.path().join("client.json")).unwrap(),
            b"{truncated"
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_login_does_not_ignore_unsafe_profile_access() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("other.json");
        std::fs::write(&target, b"{truncated").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("client.json")).unwrap();
        assert!(previous_profile_for_login(dir.path()).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"{truncated");
    }

    #[tokio::test]
    async fn missing_builtin_cimd_uses_advertised_dynamic_registration() {
        let server = wiremock::MockServer::start().await;
        let metadata = serde_json::from_value(serde_json::json!({
            "authorization_endpoint": format!("{}/authorize", server.uri()),
            "token_endpoint": format!("{}/token", server.uri()),
            "registration_endpoint": format!("{}/register", server.uri()),
            "client_id_metadata_document_supported": true
        }))
        .unwrap();
        assert!(matches!(
            choose_registration(&Url::parse(&server.uri()).unwrap(), &metadata)
                .await
                .unwrap(),
            UpstreamOauthRegistration::Dynamic
        ));
    }

    #[tokio::test]
    async fn failed_replacement_preserves_active_profile_and_encrypted_credentials() {
        use wiremock::{Mock, ResponseTemplate, matchers::path as route};
        let server = wiremock::MockServer::start().await;
        Mock::given(route("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": format!("{}/mcp", server.uri()),
                "authorization_servers": [server.uri()]
            })))
            .mount(&server)
            .await;
        Mock::given(route("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "code_challenge_methods_supported": ["S256"]
            })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let key = private_key(dir.path(), true).unwrap();
        publish_profile(
            dir.path(),
            &SessionProfile {
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: "working-client".into(),
                    client_secret_env: None,
                },
                upstream_name: "operator-server".into(),
            },
        )
        .unwrap();
        let before = std::fs::read(dir.path().join("client.json")).unwrap();
        let store =
            labby_auth::sqlite::SqliteStore::open_with_key(dir.path().join("oauth.db"), None)
                .await
                .unwrap();
        store
            .upsert_upstream_oauth_credentials(labby_auth::types::UpstreamOauthCredentialRow {
                upstream_name: "operator-server".into(),
                subject: "gateway".into(),
                client_id: "working-client".into(),
                granted_scopes_json: "[]".into(),
                token_blob: vec![17, 29, 41],
                token_blob_nonce: vec![0; 12],
                token_received_at: 100,
                access_token_expires_at: 200,
                refresh_token_present: true,
            })
            .await
            .unwrap();
        let error = login_locked(
            dir.path(),
            &key,
            &Url::parse(&server.uri()).unwrap(),
            Some(UpstreamOauthRegistration::Preregistered {
                client_id: "replacement".into(),
                client_secret_env: Some("LABBY_TEST_ABSENT_CLI_SECRET_2AB7F1".into()),
            }),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("LABBY_TEST_ABSENT_CLI_SECRET_2AB7F1"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(dir.path().join("client.json")).unwrap(),
            before
        );
        let row = store
            .find_upstream_oauth_credentials("operator-server", "gateway")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.client_id, "working-client");
        assert_eq!(row.token_blob, [17, 29, 41]);
        assert!(row.refresh_token_present);
    }

    #[tokio::test]
    async fn login_completion_cleans_retired_identity_and_preserves_active_session() {
        for authorized in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store =
                labby_auth::sqlite::SqliteStore::open_with_key(dir.path().join("oauth.db"), None)
                    .await
                    .unwrap();
            let profile = |name: &str| SessionProfile {
                registration: UpstreamOauthRegistration::Dynamic,
                upstream_name: name.into(),
            };
            publish_profile(dir.path(), &profile("operator-server-active")).unwrap();
            for name in ["operator-server-active", "operator-server-staged"] {
                store
                    .save_dynamic_client_registration(name, SUBJECT, name)
                    .await
                    .unwrap();
                store
                    .upsert_upstream_oauth_credentials(
                        labby_auth::types::UpstreamOauthCredentialRow {
                            upstream_name: name.into(),
                            subject: SUBJECT.into(),
                            client_id: name.into(),
                            granted_scopes_json: "[]".into(),
                            token_blob: vec![17, 29, 41],
                            token_blob_nonce: vec![0; 12],
                            token_received_at: 100,
                            access_token_expires_at: 200,
                            refresh_token_present: true,
                        },
                    )
                    .await
                    .unwrap();
            }
            let result = finish_login(
                dir.path(),
                &store,
                profile("operator-server-staged"),
                Some(profile("operator-server-active")),
                if authorized {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("authorization denied"))
                },
            )
            .await;
            assert_eq!(result.is_ok(), authorized);
            let (active, retired) = if authorized {
                ("operator-server-staged", "operator-server-active")
            } else {
                ("operator-server-active", "operator-server-staged")
            };
            assert_eq!(
                read_profile(dir.path()).unwrap().unwrap().upstream_name,
                active
            );
            assert!(
                store
                    .find_upstream_oauth_credentials(active, SUBJECT)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(
                store
                    .find_dynamic_client_registration(active, SUBJECT)
                    .await
                    .unwrap()
                    .as_deref(),
                Some(active)
            );
            assert!(
                store
                    .find_upstream_oauth_credentials(retired, SUBJECT)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .find_dynamic_client_registration(retired, SUBJECT)
                    .await
                    .unwrap()
                    .is_none(),
                "retired dynamic registration must be removed (authorized={authorized})"
            );
        }
    }

    #[test]
    fn registration_prefers_cimd_and_falls_back_only_when_advertised() {
        let server = server_url("https://lab.example").unwrap();
        assert!(matches!(default_registration(&server, true, true).unwrap(),
            UpstreamOauthRegistration::ClientMetadataDocument { url }
                if url == "https://lab.example/.well-known/labby-cli-client.json"));
        assert!(matches!(
            default_registration(&server, false, true).unwrap(),
            UpstreamOauthRegistration::Dynamic
        ));
        assert!(default_registration(&server, false, false).is_err());
    }

    #[test]
    fn client_document_is_public_and_bound_to_the_configured_origin() {
        let doc = client_metadata(&server_url("https://lab.example").unwrap()).unwrap();
        assert_eq!(
            doc,
            serde_json::json!({
                "client_id": "https://lab.example/.well-known/labby-cli-client.json",
                "client_name": "Labby CLI",
                "redirect_uris": ["http://127.0.0.1/auth/upstream/callback"],
                "token_endpoint_auth_method": "none", "application_type": "native",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"]
            })
        );
    }

    #[test]
    fn registration_profiles_preserve_client_binding_without_storing_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.json");
        let selection = UpstreamOauthRegistration::Preregistered {
            client_id: "known-client".into(),
            client_secret_env: Some("CLI_SECRET".into()),
        };
        let mut file = labby_auth::util::create_restricted_secret_file(&path).unwrap();
        file.write_all(&serde_json::to_vec(&selection).unwrap())
            .unwrap();
        assert!(
            matches!(read_profile(dir.path()).unwrap().unwrap().registration,
            UpstreamOauthRegistration::Preregistered { client_id, client_secret_env }
                if client_id == "known-client" && client_secret_env.as_deref() == Some("CLI_SECRET"))
        );
        file.set_len(0).unwrap();
        drop(file);
        std::fs::write(&path, br#""https://client.example/id""#).unwrap();
        assert!(
            matches!(read_profile(dir.path()).unwrap().unwrap().registration,
            UpstreamOauthRegistration::ClientMetadataDocument { url } if url == "https://client.example/id")
        );
    }

    #[tokio::test]
    async fn non_login_targets_keep_existing_transport_behavior() {
        assert!(
            token(&Url::parse("http://127.0.0.1:8765/").unwrap())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            token(&Url::parse("https://example.com/gateway/").unwrap())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sessions_for_different_servers_are_isolated() {
        let a = profile_path(&server_url("https://one.example").unwrap()).unwrap();
        let b = profile_path(&server_url("https://two.example").unwrap()).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            a,
            profile_path(&server_url("https://ONE.example:443/").unwrap()).unwrap()
        );
    }

    #[test]
    fn missing_key_is_not_silently_replaced() {
        let dir = tempfile::tempdir().unwrap();
        assert!(private_key(dir.path(), false).is_err());
        assert!(!dir.path().join("encryption.key").exists());
    }

    #[cfg(unix)]
    #[test]
    fn key_symlink_is_rejected_without_changing_its_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "preserve me").unwrap();
        symlink(&target, dir.path().join("encryption.key")).unwrap();
        assert!(private_key(dir.path(), true).is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "preserve me");
    }

    #[test]
    fn server_origins_are_canonical_and_credentials_are_rejected() {
        assert_eq!(
            server_url("https://EXAMPLE.com:443").unwrap().as_str(),
            "https://example.com/"
        );
        for raw in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/mcp",
            "https://example.com/?token=secret",
            "https://example.com/#fragment",
        ] {
            assert!(server_url(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn upstream_uses_the_exact_server_resource_and_operator_scopes() {
        let config = upstream(
            &server_url("https://lab.example").unwrap(),
            &UpstreamOauthRegistration::Dynamic,
        )
        .unwrap();
        assert_eq!(config.url.as_deref(), Some("https://lab.example/mcp"));
        assert_eq!(
            config.oauth.unwrap().scopes.unwrap(),
            ["lab:read", "lab", "lab:admin"]
        );
    }
}
