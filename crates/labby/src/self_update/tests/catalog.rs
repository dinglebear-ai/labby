use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn catalog_server(bodies: Vec<(String, bool)>) -> (String, tokio::task::JoinHandle<()>) {
    drop(rustls::crypto::ring::default_provider().install_default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/releases", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        for (index, (body, next)) in bodies.into_iter().enumerate() {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut buffer = [0; 2048];
            let size = stream.read(&mut buffer).await.unwrap();
            let request = std::str::from_utf8(&buffer[..size]).unwrap();
            assert!(
                request.starts_with(&format!("GET /releases?per_page=100&page={} ", index + 1)),
                "{request}"
            );
            // An untrusted next-link URL must never become a request target.
            let link = if next {
                "Link: <http://127.0.0.2:9/steal>; rel=\"next\"\r\n"
            } else {
                ""
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{link}\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (endpoint, task)
}

#[tokio::test]
async fn discovery_reaches_selected_release_on_second_page_without_following_link_host() {
    let dev = json!([{"tag_name":"v1.17.0-dev.10","draft":false,"prerelease":true,
        "assets":[{"name":ASSET},{"name":format!("{ASSET}.sha256")}]}]);
    let (endpoint, task) =
        catalog_server(vec![("[]".into(), true), (dev.to_string(), false)]).await;
    let releases = fetch_release_catalog(
        &reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
        &endpoint,
        10,
    )
    .await
    .unwrap();
    assert_eq!(
        select_release(
            &releases,
            version("1.17.0-dev.2").unwrap(),
            UpdateChannel::Dev
        ),
        Some("v1.17.0-dev.10")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn incomplete_catalog_budget_never_reports_no_update() {
    let (endpoint, task) = catalog_server(vec![("[]".into(), true)]).await;
    let error = fetch_release_catalog(
        &reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap(),
        &endpoint,
        1,
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("Incomplete release discovery"));
    assert!(error.to_string().contains("1-page budget"));
    task.await.unwrap();
}
