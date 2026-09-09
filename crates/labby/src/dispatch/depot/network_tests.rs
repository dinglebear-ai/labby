use super::network::{
    NetworkClient, NetworkError, NetworkPolicy, Operation, Secret, validate_addresses,
};
use std::net::IpAddr;
use std::time::Duration;

#[tokio::test]
async fn host_local_transport_binds_provider_endpoint_and_never_redirects() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    for (status, expected) in [
        (200, Ok(serde_json::json!({}))),
        (302, Err(NetworkError::Status(302))),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let secret = Secret::local_bearer("local-team", &endpoint, "local-read-secret").unwrap();
        assert!(NetworkClient::local("other-provider", &endpoint, secret.clone()).is_err());
        assert!(NetworkClient::local("local-team", "http://127.0.0.1:1", secret.clone()).is_err());
        assert!(
            NetworkClient::new(&endpoint, Some(secret.clone()), NetworkPolicy::default()).is_err()
        );
        let client = NetworkClient::local("local-team", &endpoint, secret).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 8192];
            let count = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8(bytes[..count].to_vec()).unwrap();
            assert!(request.starts_with("GET /api/discovery HTTP/1.1"));
            assert!(request.contains("authorization: Bearer local-read-secret"));
            stream.write_all(format!("HTTP/1.1 {status} Test\r\nLocation: http://169.254.169.254/\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes()).await.unwrap();
        });
        assert_eq!(
            client
                .call(
                    Operation::Identity,
                    None,
                    tokio::time::Instant::now() + Duration::from_secs(2)
                )
                .await,
            expected
        );
        server.await.unwrap();
    }
    assert!(Secret::local_bearer("local-team", "http://127.0.0.1:4100", "").is_err());
}

#[tokio::test]
async fn host_local_transport_ignores_proxy_environment() {
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", proxy.local_addr().unwrap());
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "dispatch::depot::network_tests::host_local_transport_binds_provider_endpoint_and_never_redirects"])
        .env("HTTP_PROXY", &endpoint)
        .env("HTTPS_PROXY", &endpoint)
        .env("ALL_PROXY", &endpoint)
        .env("http_proxy", &endpoint)
        .stdout(std::process::Stdio::null())
        .status().unwrap();
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), proxy.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn host_local_ipv6_literal_connects_without_dns() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let secret = Secret::local_bearer("ipv6-local", &endpoint, "ipv6-read-token").unwrap();
    let client = NetworkClient::local("ipv6-local", &endpoint, secret).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let size = stream.read(&mut request).await.unwrap();
        assert!(
            String::from_utf8_lossy(&request[..size])
                .contains("authorization: Bearer ipv6-read-token")
        );
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .await
            .unwrap();
    });
    assert_eq!(
        client
            .call(
                Operation::Identity,
                None,
                tokio::time::Instant::now() + Duration::from_secs(2)
            )
            .await
            .unwrap(),
        serde_json::json!({})
    );
    server.await.unwrap();
}

#[test]
fn private_host_policy_cannot_grant_cloud_metadata_addresses() {
    let config = toml::from_str(
        r#"[private_hosts]
"internal.test" = ["10.1.2.3", "fd00:ec2::254", "fd20:ce::254"]
"#,
    )
    .unwrap();
    let policy = super::manager::host_policy(&config).unwrap();
    assert!(validate_addresses("internal.test", &["10.1.2.3".parse().unwrap()], &policy).is_ok());
    for ip in ["fd00:ec2::254", "fd20:ce::254"] {
        assert_eq!(
            validate_addresses("internal.test", &[ip.parse().unwrap()], &policy),
            Err(NetworkError::Blocked)
        );
    }
    assert_eq!(
        validate_addresses("other.test", &["10.1.2.3".parse().unwrap()], &policy),
        Err(NetworkError::Blocked)
    );
}

#[tokio::test]
async fn pinned_tls_preserves_hostname_sni_and_bound_authorization() {
    let (client, received) =
        tls_fixture("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".into()).await;
    assert_eq!(
        client
            .call(
                Operation::Identity,
                None,
                tokio::time::Instant::now() + Duration::from_secs(2)
            )
            .await
            .unwrap(),
        serde_json::json!({})
    );
    let (sni, request) = received.await.unwrap();
    assert_eq!(sni, "depot.test");
    assert!(request.contains("host: depot.test:"));
    assert!(request.contains("authorization: Bearer fixture-token"));
    assert!(request.starts_with("GET /prefix/api/discovery HTTP/1.1"));
}

#[tokio::test]
async fn proxy_environment_cannot_redirect_pinned_tls() {
    if std::env::var_os("LABBY_TEST_PROXY_CHILD").is_some() {
        let (client, received) =
            tls_fixture("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".into()).await;
        client
            .call(
                Operation::Identity,
                None,
                tokio::time::Instant::now() + Duration::from_secs(2),
            )
            .await
            .unwrap();
        assert!(
            received
                .await
                .unwrap()
                .1
                .contains("authorization: Bearer fixture-token")
        );
        return;
    }
    let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", proxy.local_addr().unwrap());
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "dispatch::depot::network_tests::proxy_environment_cannot_redirect_pinned_tls",
        ])
        .env("LABBY_TEST_PROXY_CHILD", "1")
        .env("HTTPS_PROXY", &endpoint)
        .env("HTTP_PROXY", &endpoint)
        .env("ALL_PROXY", &endpoint)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), proxy.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn expired_dns_lease_discards_old_pool_before_revalidation() {
    let (mut client, received) =
        tls_fixture("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".into()).await;
    client
        .call(
            Operation::Identity,
            None,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap();
    received.await.unwrap();
    client
        .expire_test_lease(vec!["169.254.169.254".parse().unwrap()])
        .await;
    assert_eq!(
        client
            .call(
                Operation::Identity,
                None,
                tokio::time::Instant::now() + Duration::from_secs(2)
            )
            .await,
        Err(NetworkError::Blocked)
    );
}

#[tokio::test]
async fn hostile_headers_encoding_bodies_and_depth_are_bounded() {
    let bodies = [
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n100001\r\n{}\r\n0\r\n\r\n",
            "x".repeat(1_048_577)
        ),
        format!(
            "HTTP/1.1 200 OK\r\nX-Large: {}\r\nContent-Length: 2\r\n\r\n{{}}",
            "x".repeat(33 * 1024)
        ),
        "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n\r\n{}".into(),
        "HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n".into(),
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 131\r\n\r\n{}0{}",
            "[".repeat(65),
            "]".repeat(65)
        ),
        "HTTP/1.1 302 Found\r\nLocation: https://127.0.0.1/private\r\nContent-Length: 0\r\n\r\n"
            .into(),
    ];
    for body in bodies {
        let (client, _received) = tls_fixture(body).await;
        assert!(
            client
                .call(
                    Operation::Identity,
                    None,
                    tokio::time::Instant::now() + Duration::from_secs(2)
                )
                .await
                .is_err()
        );
    }
}

pub(super) async fn tls_fixture(
    response: String,
) -> (
    NetworkClient,
    tokio::sync::oneshot::Receiver<(String, String)>,
) {
    tls_fixture_delayed(response, Duration::ZERO).await
}

pub(super) async fn tls_fixture_delayed(
    response: String,
    delay: Duration,
) -> (
    NetworkClient,
    tokio::sync::oneshot::Receiver<(String, String)>,
) {
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["depot.test".into()]).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
        )
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "https://depot.test:{}/prefix",
        listener.local_addr().unwrap().port()
    );
    let policy = NetworkPolicy {
        allow_test_loopback: true,
        ..Default::default()
    };
    let secret = Secret::bearer(&endpoint, "fixture-token").unwrap();
    let client = NetworkClient::new(&endpoint, Some(secret), policy)
        .unwrap()
        .with_test_connection(vec!["127.0.0.1".parse().unwrap()], tls);
    let (sender, received) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = tokio_rustls::TlsAcceptor::from(Arc::new(server))
            .accept(stream)
            .await
            .unwrap();
        let sni = stream.get_ref().1.server_name().unwrap().to_owned();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let size = stream.read(&mut buffer).await.unwrap();
            if size == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..size]);
        }
        drop(sender.send((sni, String::from_utf8(request).unwrap())));
        tokio::time::sleep(delay).await;
        drop(stream.write_all(response.as_bytes()).await);
        drop(stream.shutdown().await);
    });
    (client, received)
}

#[tokio::test]
async fn absolute_deadline_clips_slow_response() {
    let (client, received) = tls_fixture_delayed(
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".into(),
        Duration::from_millis(250),
    )
    .await;
    assert_eq!(
        client
            .call(
                Operation::Identity,
                None,
                tokio::time::Instant::now() + Duration::from_millis(50)
            )
            .await,
        Err(NetworkError::Timeout)
    );
    received.await.unwrap();
}

#[test]
fn mixed_dns_and_mapped_ipv6_are_rejected_as_a_set() {
    let policy = NetworkPolicy::default();
    for addresses in [
        vec!["8.8.8.8", "127.0.0.1"],
        vec!["::ffff:8.8.8.8"],
        vec!["169.254.169.254"],
        vec!["::"],
        vec!["fe80::1"],
    ] {
        let addresses: Vec<IpAddr> = addresses.iter().map(|s| s.parse().unwrap()).collect();
        assert!(validate_addresses("depot.example", &addresses, &policy).is_err());
    }
    assert!(validate_addresses("depot.example", &["8.8.8.8".parse().unwrap()], &policy).is_ok());
}

#[test]
fn credentials_are_bound_to_entire_canonical_base_and_redacted() {
    let secret = Secret::bearer("https://example.com/a", "never-print-this").unwrap();
    assert!(!format!("{secret:?}").contains("never-print-this"));
    assert!(
        NetworkClient::new(
            "https://example.com/b",
            Some(secret),
            NetworkPolicy::default()
        )
        .is_err()
    );
}

#[tokio::test]
async fn blocked_listener_receives_no_connection_or_authorization() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let secret = Secret::bearer(&endpoint, "must-not-be-sent").unwrap();
    let client = NetworkClient::new(&endpoint, Some(secret), NetworkPolicy::default()).unwrap();
    assert!(matches!(
        client
            .call(
                Operation::Identity,
                None,
                tokio::time::Instant::now() + Duration::from_secs(1)
            )
            .await,
        Err(NetworkError::Blocked)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}
