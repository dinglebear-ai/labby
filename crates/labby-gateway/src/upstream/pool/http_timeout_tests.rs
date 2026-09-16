use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{DEFAULT_REQUEST_TIMEOUT, UpstreamPool};

#[tokio::test]
async fn http_response_can_outlive_default_request_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let received = stream.read(&mut request).await.unwrap();
        assert!(
            received > 0,
            "client must send a request before the response delay"
        );
        tokio::time::sleep(DEFAULT_REQUEST_TIMEOUT + Duration::from_secs(1)).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    let pool = UpstreamPool::new().with_request_timeout(Duration::from_secs(40));
    let result = tokio::time::timeout(pool.request_timeout(), async {
        pool.shared_http_client
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await;
    server.abort();
    assert_eq!(result.unwrap(), "ok");
}
