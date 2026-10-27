//! Offline callback validation against a signed local JWKS fixture.
use super::{tests::insert_bundle, tests::provider, *};
use crate::upstream::manager::CredentialSaveFence;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{
    RsaPrivateKey,
    pkcs8::EncodePrivateKey,
    rand_core::{TryCryptoRng, TryRng, UnwrapErr},
    traits::PublicKeyParts,
};
use std::sync::{OnceLock, atomic::AtomicUsize};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

struct FixtureRng;
impl TryRng for FixtureRng {
    type Error = getrandom::Error;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut bytes = [0; 4];
        getrandom::fill(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(bytes)
    }
}
impl TryCryptoRng for FixtureRng {}

async fn database() -> (tempfile::TempDir, SqliteStore) {
    let root = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_with_key(
        root.path().join("auth.db"),
        Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
            "callback-tests",
        )),
    )
    .await
    .unwrap();
    (root, store)
}

fn key() -> &'static RsaPrivateKey {
    static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
    KEY.get_or_init(|| RsaPrivateKey::new(&mut UnwrapErr(FixtureRng), 2048).unwrap())
}

fn callback_credentials(subject: &str, verified: bool) -> StoredCredentials {
    let now = crate::util::now_unix();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("callback-fixture".into());
    let id_token = encode(
        &header,
        &serde_json::json!({
            "iss": GOOGLE_ISSUER, "aud": "google-client", "sub": subject,
            "iat": now, "exp": now + 3600, "email": "admin@example.com",
            "email_verified": verified,
        }),
        &EncodingKey::from_rsa_pem(key().to_pkcs8_pem(Default::default()).unwrap().as_bytes())
            .unwrap(),
    )
    .unwrap();
    let mut extra = VendorExtraTokenFields::default();
    extra
        .0
        .insert("id_token".into(), serde_json::json!(id_token));
    let mut token = OAuthTokenResponse::new(
        AccessToken::new("callback-access".into()),
        BasicTokenType::Bearer,
        extra,
    );
    token.set_refresh_token(Some(RefreshToken::new("callback-refresh".into())));
    StoredCredentials::new(
        "google-client".into(),
        Some(token),
        vec!["openid".into()],
        Some(now.max(0) as u64),
    )
}

async fn adapter(store: SqliteStore) -> (GoogleProviderCredentialStore, MockServer) {
    drop(rustls::crypto::ring::default_provider().install_default());
    let server = MockServer::start().await;
    let public = key().to_public_key();
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "keys": [{"kid": "callback-fixture", "alg": "RS256", "kty": "RSA", "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(public.n_bytes()), "e": URL_SAFE_NO_PAD.encode(public.e_bytes())}]
    }))).mount(&server).await;
    let provider = Arc::new(
        (*provider())
            .clone()
            .with_jwks_endpoint(server.uri().parse().unwrap()),
    );
    let adapter = GoogleProviderCredentialStore::new(
        store,
        provider,
        Some("google-subject".into()),
        "google-client".into(),
        vec!["openid".into()],
    );
    adapter.load().await.unwrap();
    (adapter, server)
}

fn fence(store: SqliteStore, revocations: Arc<AtomicUsize>, fail: bool) -> CredentialSaveFence {
    let lifecycle = Arc::new(tokio::sync::RwLock::new(()));
    Arc::new(move |validate| {
        let (store, revocations, lifecycle) =
            (store.clone(), revocations.clone(), lifecycle.clone());
        Box::pin(async move {
            let lifecycle_guard = lifecycle.write_owned().await;
            let provider_guard = validate.await?;
            assert!(
                crate::google_refresh::lock("google-subject")
                    .try_lock_owned()
                    .is_err()
            );
            let row = store
                .find_google_provider_credential("google-subject")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                row.refresh_token, "refresh-token",
                "fence must precede replacement"
            );
            revocations.fetch_add(1, Ordering::SeqCst);
            if fail {
                return Err(AuthError::InternalError("revocation unavailable".into()));
            }
            let held: Box<dyn Send> = Box::new((lifecycle_guard, provider_guard));
            Ok(held)
        })
    })
}

#[tokio::test]
async fn rejected_callback_identity_preserves_credentials_and_never_revokes() {
    for rejection in ["client", "subject", "unverified", "malformed", "missing"] {
        let (_root, store) = database().await;
        insert_bundle(&store, vec!["openid".into()]).await;
        let original = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap()
            .unwrap();
        let (adapter, _server) = adapter(store.clone()).await;
        let revocations = Arc::new(AtomicUsize::new(0));
        let adapter = adapter.with_callback_fence(fence(store.clone(), revocations.clone(), false));
        let mut credentials = callback_credentials(
            if rejection == "subject" {
                "wrong-subject"
            } else {
                "google-subject"
            },
            rejection != "unverified",
        );
        if rejection == "client" {
            credentials.client_id = "other-client".into();
        }
        if rejection == "malformed" || rejection == "missing" {
            let token = credentials.token_response.as_mut().unwrap();
            let mut extra = token.extra_fields().clone();
            if rejection == "malformed" {
                extra
                    .0
                    .insert("id_token".into(), serde_json::json!("malformed"));
            } else {
                extra.0.remove("id_token");
            }
            token.set_extra_fields(extra);
        }
        assert!(adapter.save(credentials).await.is_err(), "{rejection}");
        assert_eq!(revocations.load(Ordering::SeqCst), 0, "{rejection}");
        let row = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.generation, original.generation, "{rejection}");
        assert_eq!(row.refresh_token, original.refresh_token, "{rejection}");
    }
}

#[tokio::test]
async fn valid_callback_revokes_before_replacement_and_fence_failure_blocks_save() {
    for fail in [false, true] {
        let (_root, store) = database().await;
        insert_bundle(&store, vec!["openid".into()]).await;
        let original = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap()
            .unwrap();
        let (adapter, _server) = adapter(store.clone()).await;
        let revocations = Arc::new(AtomicUsize::new(0));
        let adapter = adapter.with_callback_fence(fence(store.clone(), revocations.clone(), fail));
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            adapter.save(callback_credentials("google-subject", true)),
        )
        .await
        .expect("callback lock order must settle");
        assert_eq!(result.is_err(), fail);
        assert_eq!(revocations.load(Ordering::SeqCst), 1);
        let row = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.refresh_token,
            if fail {
                "refresh-token"
            } else {
                "callback-refresh"
            }
        );
        assert_eq!(row.generation, original.generation + i64::from(!fail));
    }
}

#[tokio::test]
async fn callback_revalidates_generation_and_revocation_epoch_before_revoking_routes() {
    for revoke in [false, true] {
        let (_root, store) = database().await;
        insert_bundle(&store, vec!["openid".into()]).await;
        let (adapter, _server) = adapter(store.clone()).await;
        let revocations = Arc::new(AtomicUsize::new(0));
        let database = store.clone();
        let counter = revocations.clone();
        let adapter = adapter.with_callback_fence(Arc::new(move |validate| {
            let (database, counter) = (database.clone(), counter.clone());
            Box::pin(async move {
                // A competing replacement/revoke wins before this callback's writer admission.
                if revoke {
                    database
                        .revoke_google_provider_credential(Some("google-subject"))
                        .await
                        .unwrap();
                } else {
                    insert_bundle(&database, vec!["openid".into()]).await;
                }
                let guard = validate.await?;
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(guard)
            })
        }));
        assert!(
            adapter
                .save(callback_credentials("google-subject", true))
                .await
                .is_err()
        );
        assert_eq!(revocations.load(Ordering::SeqCst), 0);
        let row = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap();
        assert_eq!(row.is_none(), revoke);
        if let Some(row) = row {
            assert_eq!(row.refresh_token, "refresh-token");
        }
    }
}
