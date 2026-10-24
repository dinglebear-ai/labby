//! Encrypted fixture credentials shared with the real MCP transport regression.

#![cfg(target_os = "linux")]

use super::*;
use oauth2::{AccessToken, RefreshToken, basic::BasicTokenType};
use rmcp::transport::auth::{
    CredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
};

pub(crate) async fn seed_bridge_test_profile(
    root: &Path,
    server: &Url,
) -> Result<labby_auth::upstream::store::SqliteCredentialStore> {
    let path = profile_path_for_root(root, server)?;
    crate::installation::InstallationPaths::from_root(&path)?.prepare_root()?;
    let key = private_key(&path, true)?;
    publish_profile(
        &path,
        &SessionProfile {
            registration: UpstreamOauthRegistration::Preregistered {
                client_id: "bridge-client".into(),
                client_secret_env: None,
            },
            upstream_name: "operator-server-bridge-fixture".into(),
        },
    )?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(&key)?;
    let sqlite = labby_auth::sqlite::SqliteStore::open_with_key(
        path.join("oauth.db"),
        Some(labby_auth::at_rest::TokenEncryptionKey::from_encoded(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        )?),
    )
    .await?;
    let credentials = labby_auth::upstream::store::SqliteCredentialStore::new(
        sqlite,
        labby_auth::upstream::encryption::load_key(&key)?,
        "operator-server-bridge-fixture",
        SUBJECT,
    );
    let mut token = OAuthTokenResponse::new(
        AccessToken::new("bridge-initial-token".into()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    token.set_expires_in(Some(&Duration::from_hours(1)));
    token.set_refresh_token(Some(RefreshToken::new("bridge-refresh-1".into())));
    let received_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    credentials
        .save(StoredCredentials::new(
            "bridge-client".into(),
            Some(token),
            vec![],
            Some(received_at),
        ))
        .await?;
    Ok(credentials)
}
