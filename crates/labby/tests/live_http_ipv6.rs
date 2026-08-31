#![cfg(feature = "gateway")]

#[path = "support/lib.rs"]
mod support;

use std::time::Duration;

use reqwest::{Client, StatusCode};
use support::LiveLabbyBuilder;

#[tokio::test]
async fn ipv6_loopback_is_exercised_when_the_kernel_supports_it() {
    let capability = match std::net::TcpListener::bind("[::1]:0") {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("SKIP: IPv6 loopback bind probe failed: {error}");
            return;
        }
    };
    drop(capability);

    let guard = LiveLabbyBuilder::new()
        .bind_ip(std::net::Ipv6Addr::LOCALHOST.into())
        .start()
        .await
        .expect("start IPv6 live Labby");
    let response = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("IPv6 HTTP client")
        .get(&guard.connection().health_url)
        .send()
        .await
        .expect("IPv6 health request");
    assert_eq!(response.status(), StatusCode::OK);
    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
}
