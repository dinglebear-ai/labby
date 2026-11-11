use super::*;

fn random_signing_key() -> [u8; 32] {
    let mut key = [0; 32];
    getrandom::fill(&mut key).expect("generate a webhook signing key for the test");
    key
}

#[test]
fn standard_webhooks_signs_the_exact_bytes() {
    let key = WebhookSecret::from_bytes(vec![b'a'; 32]).unwrap();
    let body = br#"{"message":"ready"}"#;
    let signature = sign_webhook(&key, "notice_vector", 1_700_000_000, body);
    assert_eq!(signature, "v1,4373CS2hzQF5XAy4/Pzw5DW/XXg1BLfOJ0bwaI6ALss=");
    assert_ne!(
        signature,
        sign_webhook(&key, "notice_vector2", 1_700_000_000, body)
    );
    assert_ne!(
        signature,
        sign_webhook(&key, "notice_vector", 1_700_000_001, body)
    );
    assert_ne!(
        signature,
        sign_webhook(&key, "notice_vector", 1_700_000_000, b"other bytes")
    );
}

#[test]
fn rejects_a_mixed_answer_and_relay_addresses() {
    let public = "1.1.1.1:443".parse::<SocketAddr>().unwrap();
    assert!(validate_addresses(&[public]).is_ok());
    for candidate in [
        "127.0.0.1:443",
        "[::ffff:192.168.1.8]:443",
        "[64:ff9b:1::1]:443",
        "[2001::2]:443",
    ] {
        let private = candidate.parse::<SocketAddr>().unwrap();
        assert_eq!(
            validate_addresses(&[public, private]),
            Err(EventError::Callback("destination_blocked"))
        );
    }
    for url in [
        "http://example.com",
        "https://@example.com",
        "https://secret@example.com",
        "https://example.com#secret",
        "https://lab.internal.",
        "https://[::ffff:127.0.0.1]",
    ] {
        assert!(parse_callback(url).is_err(), "accepted {url}");
    }
    let too_long = format!("https://example.com/{}", "a".repeat(2029));
    assert_eq!(too_long.len(), 2_049);
    assert!(parse_callback(&too_long).is_err());
}

pub(in crate::dispatch::agent_events) async fn receiver(
    requests: usize,
) -> (
    HttpsWebhookSender,
    String,
    String,
    tokio::sync::mpsc::Receiver<(String, serde_json::Value)>,
    tokio::task::JoinHandle<()>,
) {
    receiver_with_offer_check(requests, None).await
}

type OfferCheck = (
    crate::dispatch::codemode_notices::NoticeStore,
    crate::dispatch::codemode_notices::NoticeRecipient,
);

pub(in crate::dispatch::agent_events) async fn receiver_with_offer_check(
    requests: usize,
    ack: Option<OfferCheck>,
) -> (
    HttpsWebhookSender,
    String,
    String,
    tokio::sync::mpsc::Receiver<(String, serde_json::Value)>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _};
    drop(rustls::crypto::ring::default_provider().install_default());
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["event-receiver.test".into()]).unwrap();
    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let client = client_builder()
        .tls_certs_only([reqwest::Certificate::from_der(cert.der().as_ref()).unwrap()])
        .resolve("event-receiver.test", address)
        .build()
        .unwrap();
    let sender = HttpsWebhookSender {
        connector: Some(client),
        ..Default::default()
    };
    let key = random_signing_key();
    let secret = format!("whsec_{}", STANDARD.encode(key));
    let (tx, rx) = tokio::sync::mpsc::channel(requests);
    let task = tokio::spawn(async move {
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        tokio::time::timeout(Duration::from_secs(15), async {
            for _ in 0..requests {
                let (stream, _) = listener.accept().await.unwrap();
                let stream = acceptor.accept(stream).await.unwrap();
                assert_eq!(stream.get_ref().1.server_name(), Some("event-receiver.test"));
                let mut reader = tokio::io::BufReader::new(stream);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let path = line.split_whitespace().nth(1).unwrap().to_owned();
                let mut headers = std::collections::HashMap::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    if line == "\r\n" { break; }
                    let (name, value) = line.split_once(':').unwrap();
                    headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
                }
                assert!(headers["x-mcp-subscription-id"].starts_with("sub_"));
                let mut body = vec![0; headers["content-length"].parse::<usize>().unwrap()];
                reader.read_exact(&mut body).await.unwrap();
                let event: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let id = &headers["webhook-id"];
                // Independently verify the on-wire string. No call to the
                // sender's signing helper and no re-serialization of the body.
                let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
                mac.update(id.as_bytes());
                mac.update(b".");
                mac.update(headers["webhook-timestamp"].as_bytes());
                mac.update(b".");
                mac.update(&body);
                let signature = STANDARD.decode(headers["webhook-signature"].strip_prefix("v1,").unwrap()).unwrap();
                mac.verify_slice(&signature).unwrap();
                if event.get("type").is_none() {
                    assert_eq!(event["eventId"], *id);
                }
                if event.get("type").is_none()
                    && let Some((store, who)) = &ack
                {
                    let ids = vec![id.clone()];
                    let (_, actual) = store.controls(who.clone(), false, Some(ids.clone())).await.unwrap();
                    assert_eq!(actual, Some(ids), "recipient must be able to ACK before responding");
                }
                tx.send((path.clone(), event.clone())).await.unwrap();
                let mut status = "200 OK";
                let mut extra = "";
                let response = match path.as_str() {
                    "/wrong" => r#"{"challenge":"wrong"}"#.into(),
                    "/large" => "x".repeat(8_193),
                    "/redirect" => {
                        status = "302 Found";
                        extra = "Location: https://127.0.0.1/forbidden\r\n";
                        String::new()
                    }
                    _ => serde_json::json!({"challenge":event["challenge"]}).to_string(),
                };
                reader.get_mut().write_all(format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len(),
                ).as_bytes()).await.unwrap();
                reader.get_mut().shutdown().await.unwrap();
            }
        }).await.unwrap();
    });
    (
        sender,
        format!("https://event-receiver.test:{}", address.port()),
        secret,
        rx,
        task,
    )
}

#[tokio::test]
async fn verifies_real_https_challenge_and_rejects_bad_or_oversized_replies() {
    let (sender, base, secret, mut requests, task) = receiver(4).await;
    let key = WebhookSecret::parse(&secret).unwrap();
    let id = "sub_fixture";
    for (path, expected) in [
        ("/valid", Ok(())),
        ("/wrong", Err(EventError::Callback("challenge_failed"))),
        ("/large", Err(EventError::Callback("response_too_large"))),
        ("/redirect", Err(EventError::Callback("challenge_failed"))),
    ] {
        let url = parse_callback(&format!("{base}{path}")).unwrap();
        assert_eq!(sender.verify(&url, id, &key).await, expected);
        let (observed, event) = requests.recv().await.unwrap();
        assert_eq!(observed, path);
        assert_eq!(event["type"], "verification");
    }
    task.await.unwrap();
}

#[test]
fn rotation_signs_with_both_keys_until_the_overlap_expires() {
    let current = random_signing_key();
    let previous = random_signing_key();
    let mut attempt = WebhookAttempt {
        url: Url::parse("https://example.com").unwrap(),
        subscription_id: "sub_fixture".into(),
        event_id: "notice_fixture".into(),
        body: br#"{"value":"ready"}"#.to_vec(),
        key: WebhookSecret::from_bytes(current.to_vec()).unwrap(),
        previous_key: Some((
            WebhookSecret::from_bytes(previous.to_vec()).unwrap(),
            now_ms() + 300_000,
        )),
        offer: None,
    };
    let time = now_ms() / 1000;
    let signature = sign(&attempt, time);
    let mut signatures = signature.split(' ');
    for secret in [current, previous] {
        let bytes = STANDARD
            .decode(signatures.next().unwrap().strip_prefix("v1,").unwrap())
            .unwrap();
        let mut verifier = Hmac::<Sha256>::new_from_slice(&secret).unwrap();
        verifier.update(format!("notice_fixture.{time}.").as_bytes());
        verifier.update(&attempt.body);
        verifier.verify_slice(&bytes).unwrap();
    }
    assert_eq!(signatures.next(), None);
    attempt.previous_key.as_mut().unwrap().1 = now_ms();
    assert_eq!(sign(&attempt, time).split(' ').count(), 1);
}
