use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey as _;
use rsa::rand_core::{TryCryptoRng, TryRng, UnwrapErr};
use rsa::traits::PublicKeyParts as _;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub(crate) struct GoogleFixture {
    pub(crate) server: MockServer,
    expected: Arc<Mutex<Option<ExpectedExchange>>>,
}

#[derive(Clone)]
struct ExpectedExchange {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    challenge: String,
}

#[derive(Clone)]
struct TokenResponder {
    expected: Arc<Mutex<Option<ExpectedExchange>>>,
    id_token: String,
}

impl Respond for TokenResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let valid = self
            .expected
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|expected| validate_exchange(&request.body, expected));
        if !valid {
            return ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"}));
        }
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token":"q2-google-access-token", "refresh_token":"q2-google-refresh-token",
            "expires_in":3600, "scope":"openid email profile", "id_token":self.id_token
        }))
    }
}

impl GoogleFixture {
    pub(crate) async fn start(client_id: &str, subject: &str, email: &str) -> Self {
        let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
        let server = MockServer::start().await;
        let key = RsaPrivateKey::new(&mut UnwrapErr(FixtureRng), 2048).expect("fixture RSA key");
        let public_key = key.to_public_key();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = json!({
            "iss": "https://accounts.google.com",
            "aud": client_id,
            "sub": subject,
            "email": email,
            "email_verified": true,
            "iat": now - 10,
            "exp": now + 3600,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("q2-google-key".into());
        let pem = key.to_pkcs8_pem(Default::default()).unwrap();
        let id_token = encode(
            &header,
            &claims,
            &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
        )
        .unwrap();
        let expected = Arc::new(Mutex::new(None));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(TokenResponder {
                expected: Arc::clone(&expected),
                id_token,
            })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "keys": [{
                    "kid": "q2-google-key",
                    "alg": "RS256",
                    "kty": "RSA",
                    "use": "sig",
                    "n": URL_SAFE_NO_PAD.encode(public_key.n_bytes()),
                    "e": URL_SAFE_NO_PAD.encode(public_key.e_bytes()),
                }]
            })))
            .mount(&server)
            .await;
        Self { server, expected }
    }

    pub(crate) fn token_endpoint(&self) -> String {
        format!("{}/token", self.server.uri())
    }

    pub(crate) fn jwks_endpoint(&self) -> String {
        format!("{}/jwks", self.server.uri())
    }

    pub(crate) fn expect_exchange(
        &self,
        client_id: &str,
        client_secret: &str,
        redirect_uri: &str,
        challenge: &str,
    ) {
        *self.expected.lock().unwrap() = Some(ExpectedExchange {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect_uri: redirect_uri.into(),
            challenge: challenge.into(),
        });
    }
}

fn validate_exchange(body: &[u8], expected: &ExpectedExchange) -> bool {
    use sha2::Digest as _;
    const EXACT_FIELDS: [&str; 6] = [
        "grant_type",
        "code",
        "client_id",
        "client_secret",
        "redirect_uri",
        "code_verifier",
    ];
    let Some(form) = exact_form(body, &EXACT_FIELDS) else {
        return false;
    };
    let verifier = form["code_verifier"].as_str();
    if !valid_pkce_verifier(verifier) {
        return false;
    }
    let challenge = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    form.get("grant_type")
        .is_some_and(|v| v == "authorization_code")
        && form.get("code").is_some_and(|v| v == "fixture-code")
        && form
            .get("client_id")
            .is_some_and(|v| v == expected.client_id.as_str())
        && form
            .get("client_secret")
            .is_some_and(|v| v == expected.client_secret.as_str())
        && form
            .get("redirect_uri")
            .is_some_and(|v| v == expected.redirect_uri.as_str())
        && challenge == expected.challenge
}

fn exact_form(
    body: &[u8],
    expected_fields: &[&str],
) -> Option<std::collections::BTreeMap<String, String>> {
    let mut form = std::collections::BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        if !expected_fields.contains(&key.as_ref())
            || form.insert(key.into_owned(), value.into_owned()).is_some()
        {
            return None;
        }
    }
    (form.len() == expected_fields.len()).then_some(form)
}

fn valid_pkce_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

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

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(dst)
    }
}

impl TryCryptoRng for FixtureRng {}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use sha2::Digest as _;

    #[tokio::test]
    async fn rejects_unbound_token_exchange_before_success_response() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let fixture = super::GoogleFixture::start("client", "subject", "user@example.test").await;
        let response = reqwest::Client::new()
            .post(fixture.token_endpoint())
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", "fixture-code"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validator_rejects_each_mismatched_exchange_binding() {
        let verifier = "fixture-verifier-with-at-least-forty-three-characters";
        let expected = super::ExpectedExchange {
            client_id: "client".into(),
            client_secret: "secret".into(),
            redirect_uri: "http://127.0.0.1/callback".into(),
            challenge: super::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes())),
        };
        let good = format!(
            "grant_type=authorization_code&code=fixture-code&client_id=client&client_secret=secret&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&code_verifier={verifier}"
        );
        assert!(super::validate_exchange(good.as_bytes(), &expected));
        for invalid in [
            good.replace("authorization_code", "refresh_token"),
            good.replace("fixture-code", "wrong-code"),
            good.replace("client_id=client", "client_id=wrong"),
            good.replace("client_secret=secret", "client_secret=wrong"),
            good.replace("callback", "wrong"),
            good.replace(verifier, "wrong-verifier"),
            good.replace(verifier, "short"),
            format!("{good}&code=fixture-code"),
            format!("{good}&unexpected=value"),
        ] {
            assert!(!super::validate_exchange(invalid.as_bytes(), &expected));
        }
    }
}
