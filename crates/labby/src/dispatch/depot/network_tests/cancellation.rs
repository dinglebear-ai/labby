use super::*;

async fn stalled_exchange_is_cancelled(expire_deadline: bool) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let secret = Secret::local_bearer("cancel-fixture", &endpoint, "fixture-token").unwrap();
    let client = NetworkClient::local("cancel-fixture", &endpoint, secret).unwrap();
    let (started, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
        }
        assert_eq!(
            String::from_utf8(request)
                .unwrap()
                .matches("GET /api/discovery ")
                .count(),
            1,
        );
        // Deliver headers and a partial body so cancellation must also stop
        // response-body collection, not just an unstarted request.
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{")
            .await
            .unwrap();
        started.send(()).unwrap();
        let remaining = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
            .await
            .expect("cancelled exchange must close its connection")
            .unwrap();
        assert_eq!(
            remaining, 0,
            "no detached exchange or duplicate request may remain"
        );
    });
    let deadline = tokio::time::Instant::now()
        + if expire_deadline {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(3)
        };
    let mut call = Box::pin(client.call(Operation::Identity, None, deadline));
    tokio::select! {
        () = async { ready.await.unwrap() } => {},
        result = &mut call => panic!("stalled exchange completed early: {result:?}"),
    }
    if expire_deadline {
        assert_eq!(call.await, Err(NetworkError::Timeout));
    } else {
        drop(call);
    }
    server.await.unwrap();
}

#[tokio::test]
async fn dropping_network_call_stops_its_owned_exchange() {
    stalled_exchange_is_cancelled(false).await;
}

#[tokio::test]
async fn network_deadline_stops_its_owned_exchange() {
    stalled_exchange_is_cancelled(true).await;
}
