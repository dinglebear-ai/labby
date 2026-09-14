use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey as _;
use rsa::rand_core::{TryCryptoRng, TryRng, UnwrapErr};
use rsa::traits::PublicKeyParts as _;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

pub(crate) struct AutheliaFixture {
    pub(crate) issuer: String,
    pub(crate) ca_path: std::path::PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    nonce: Arc<Mutex<String>>,
    expected: Arc<Mutex<Option<ExpectedExchange>>>,
    task: Option<tokio::task::JoinHandle<()>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    active_handlers: Arc<AtomicUsize>,
    _root: tempfile::TempDir,
}

struct ActiveHandler(Arc<AtomicUsize>);

impl ActiveHandler {
    fn new(active_handlers: Arc<AtomicUsize>) -> Self {
        active_handlers.fetch_add(1, Ordering::SeqCst);
        Self(active_handlers)
    }
}

impl Drop for ActiveHandler {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ExpectedExchange {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    challenge: String,
}

impl AutheliaFixture {
    pub(crate) async fn start(client_id: &str, subject: &str, email: &str) -> Self {
        let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let ca_path = root.path().join("authelia-ca.pem");
        let encoded = base64::engine::general_purpose::STANDARD.encode(cert.der().as_ref());
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in encoded.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        std::fs::write(&ca_path, pem).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let key = Arc::new(RsaPrivateKey::new(&mut UnwrapErr(FixtureRng), 2048).unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let nonce = Arc::new(Mutex::new(String::new()));
        let expected = Arc::new(Mutex::new(None));
        let task_requests = Arc::clone(&requests);
        let task_nonce = Arc::clone(&nonce);
        let task_expected = Arc::clone(&expected);
        let task_issuer = issuer.clone();
        let active_handlers = Arc::new(AtomicUsize::new(0));
        let task_active_handlers = Arc::clone(&active_handlers);
        let (shutdown, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let client_id = client_id.to_owned();
        let subject = subject.to_owned();
        let email = email.to_owned();
        let task = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
            let mut handlers = tokio::task::JoinSet::new();
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((stream, _)) = accepted else { break };
                let acceptor = acceptor.clone();
                let issuer = task_issuer.clone();
                let key = Arc::clone(&key);
                let requests = Arc::clone(&task_requests);
                let fixture_nonce = Arc::clone(&task_nonce);
                let expected = Arc::clone(&task_expected);
                let client_id = client_id.clone();
                let subject = subject.clone();
                let email = email.clone();
                let active = ActiveHandler::new(Arc::clone(&task_active_handlers));
                handlers.spawn(async move {
                    let _active = active;
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut bytes = Vec::new();
                    let mut chunk = [0; 4096];
                    loop {
                        let Ok(Ok(size)) = tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            stream.read(&mut chunk),
                        )
                        .await
                        else {
                            return;
                        };
                        if size == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&chunk[..size]);
                        if bytes.len() > 64 * 1024 {
                            return;
                        }
                        let Some(headers_end) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
                        else {
                            continue;
                        };
                        let headers = String::from_utf8_lossy(&bytes[..headers_end + 4]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= headers_end + 4 + length {
                            break;
                        }
                    }
                    let request = String::from_utf8_lossy(&bytes).into_owned();
                    requests.lock().unwrap().push(request_summary(&request));
                    let first = request.lines().next().unwrap_or_default();
                    let target = first.split_whitespace().nth(1).unwrap_or("/");
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    let (status, body) = if target.starts_with("/.well-known/openid-configuration") {
                        (200, json!({
                            "issuer": issuer,
                            "authorization_endpoint": format!("{issuer}/api/oidc/authorization"),
                            "token_endpoint": format!("{issuer}/api/oidc/token"),
                            "jwks_uri": format!("{issuer}/jwks.json"),
                            "response_types_supported": ["code"],
                            "grant_types_supported": ["authorization_code"],
                            "code_challenge_methods_supported": ["S256"],
                            "token_endpoint_auth_methods_supported": ["client_secret_basic"],
                            "id_token_signing_alg_values_supported": ["RS256"]
                        }))
                    } else if target.starts_with("/jwks.json") {
                        let public = key.to_public_key();
                        (200, json!({"keys":[{"kid":"q2-authelia-key","alg":"RS256","kty":"RSA","use":"sig",
                            "n":URL_SAFE_NO_PAD.encode(public.n_bytes()),"e":URL_SAFE_NO_PAD.encode(public.e_bytes())}]}))
                    } else if target.starts_with("/api/oidc/token") {
                        if !expected.lock().unwrap().as_ref().is_some_and(|expected| validate_exchange(&request, expected)) {
                            (400, json!({"error":"invalid_grant"}))
                        } else {
                        let nonce = fixture_nonce.lock().unwrap().clone();
                        let claims = json!({"iss":issuer,"aud":client_id,"sub":subject,"email":email,
                            "email_verified":true,"nonce":nonce,"iat":now-10,"exp":now+3600});
                        let mut header = Header::new(Algorithm::RS256);
                        header.kid = Some("q2-authelia-key".into());
                        let pem = key.to_pkcs8_pem(Default::default()).unwrap();
                        let token = encode(
                            &header,
                            &claims,
                            &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
                        )
                        .unwrap();
                        (200, json!({"access_token":"q2-authelia-access","refresh_token":"q2-authelia-refresh",
                            "expires_in":3600,"scope":"openid email profile","id_token":token,"token_type":"Bearer"}))
                        }
                    } else {
                        (404, json!({"error":"not_found"}))
                    };
                    let body = serde_json::to_vec(&body).unwrap();
                    let response = format!(
                        "HTTP/1.1 {status} Fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    drop(tokio::time::timeout(std::time::Duration::from_secs(2), async {
                        stream.write_all(response.as_bytes()).await?;
                        stream.write_all(&body).await?;
                        stream.shutdown().await
                    })
                    .await);
                });
            }
            handlers.shutdown().await;
        });
        Self {
            issuer,
            ca_path,
            requests,
            nonce,
            expected,
            task: Some(task),
            shutdown: Some(shutdown),
            active_handlers,
            _root: root,
        }
    }

    pub(crate) fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
    pub(crate) fn set_nonce(&self, nonce: &str) {
        nonce.clone_into(&mut self.nonce.lock().unwrap());
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

    pub(crate) async fn finish(mut self) -> Result<(), String> {
        const CLEANUP_BOUND: std::time::Duration = std::time::Duration::from_secs(2);
        let mut task = self.task.take().expect("fixture task is present");
        if let Some(shutdown) = self.shutdown.take() {
            let _send_result = shutdown.send(());
        }
        match tokio::time::timeout(CLEANUP_BOUND, &mut task).await {
            Ok(Ok(())) if self.active_handlers.load(Ordering::SeqCst) == 0 => Ok(()),
            Ok(Ok(())) => Err("Authelia fixture retained active handlers".into()),
            Ok(Err(error)) => Err(format!("Authelia fixture failed: {error}")),
            Err(_) => {
                task.abort();
                match tokio::time::timeout(CLEANUP_BOUND, &mut task).await {
                    Ok(Err(error)) if error.is_cancelled() => {}
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        return Err(format!(
                            "Authelia fixture cleanup failed after abort: {error}"
                        ));
                    }
                    Err(_) => {
                        return Err("Authelia fixture task did not stop after abort".into());
                    }
                }
                if self.active_handlers.load(Ordering::SeqCst) != 0 {
                    return Err("Authelia fixture retained active handlers after abort".into());
                }
                Err("Authelia fixture cleanup deadline elapsed".into())
            }
        }
    }
}

impl Drop for AutheliaFixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct FixtureRng;
impl TryRng for FixtureRng {
    type Error = getrandom::Error;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut b = [0; 4];
        getrandom::fill(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut b = [0; 8];
        getrandom::fill(&mut b)?;
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::fill(dst)
    }
}
impl TryCryptoRng for FixtureRng {}

fn validate_exchange(request: &str, expected: &ExpectedExchange) -> bool {
    use sha2::Digest as _;
    const EXACT_FIELDS: [&str; 4] = ["grant_type", "code", "redirect_uri", "code_verifier"];
    let (headers, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut authorization_headers = headers.lines().filter_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                .then_some(value.trim())
        })
    });
    let authorization = authorization_headers.next();
    if authorization_headers.next().is_some() {
        return false;
    }
    let mut content_type_headers = headers.lines().filter_map(|line| {
        line.split_once(':').and_then(|(name, value)| {
            name.eq_ignore_ascii_case("content-type")
                .then_some(value.trim())
        })
    });
    if content_type_headers.next() != Some("application/x-www-form-urlencoded")
        || content_type_headers.next().is_some()
    {
        return false;
    }
    let expected_basic = base64::engine::general_purpose::STANDARD
        .encode(format!("{}:{}", expected.client_id, expected.client_secret));
    let Some(form) = exact_form(body.as_bytes(), &EXACT_FIELDS) else {
        return false;
    };
    let verifier = form["code_verifier"].as_str();
    if !valid_pkce_verifier(verifier) {
        return false;
    }
    let challenge = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    authorization == Some(format!("Basic {expected_basic}").as_str())
        && form
            .get("grant_type")
            .is_some_and(|v| v == "authorization_code")
        && form.get("code").is_some_and(|v| v == "fixture-code")
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

fn request_summary(request: &str) -> String {
    let (headers, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let first = headers.lines().next().unwrap_or_default();
    let has_basic = headers.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && value.trim().to_ascii_lowercase().starts_with("basic ")
        })
    });
    let form = url::form_urlencoded::parse(body.as_bytes())
        .map(|(key, value)| format!("{key}={}", if value.is_empty() { "" } else { "<present>" }))
        .collect::<Vec<_>>()
        .join("&");
    format!(
        "{first}\nAuthorization: {}\n{form}",
        if has_basic {
            "Basic <redacted>"
        } else {
            "<absent>"
        }
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use base64::Engine as _;
    use sha2::Digest as _;

    #[test]
    fn token_exchange_validator_rejects_unbound_pkce_and_client_auth() {
        let verifier = "fixture-verifier-with-at-least-forty-three-characters";
        let expected = super::ExpectedExchange {
            client_id: "client".into(),
            client_secret: "secret".into(),
            redirect_uri: "http://127.0.0.1/callback".into(),
            challenge: super::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes())),
        };
        let basic = base64::engine::general_purpose::STANDARD.encode("client:secret");
        let good = format!(
            "POST /api/oidc/token HTTP/1.1\r\nAuthorization: Basic {basic}\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\ngrant_type=authorization_code&code=fixture-code&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&code_verifier={verifier}"
        );
        assert!(super::validate_exchange(&good, &expected));
        for invalid in [
            good.replace(verifier, "wrong"),
            good.replace(&basic, "invalid"),
            good.replace("authorization_code", "refresh_token"),
            good.replace("fixture-code", "wrong-code"),
            good.replace("callback", "wrong"),
            good.replace(verifier, "short"),
            format!("{good}&code=fixture-code"),
            format!("{good}&client_id=client"),
            good.replace(
                "Content-Type: application/x-www-form-urlencoded",
                "Content-Type: application/json",
            ),
            good.replace(
                &format!("Authorization: Basic {basic}"),
                &format!("Authorization: Basic {basic}\r\nAuthorization: Basic {basic}"),
            ),
        ] {
            assert!(!super::validate_exchange(&invalid, &expected));
        }
    }

    #[tokio::test]
    async fn finish_aborts_a_partial_tls_handler_within_the_cleanup_bound() {
        let fixture = super::AutheliaFixture::start("client", "subject", "user@example.test").await;
        let port = reqwest::Url::parse(&fixture.issuer)
            .unwrap()
            .port()
            .unwrap();
        let _partial = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while fixture.active_handlers.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("partial TLS handler became active");
        tokio::time::timeout(std::time::Duration::from_secs(3), fixture.finish())
            .await
            .expect("fixture cleanup outer deadline")
            .unwrap();
    }
}
