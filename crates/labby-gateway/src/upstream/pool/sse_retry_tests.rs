use super::*;

fn config(timeout: Duration) -> StreamableHttpClientTransportConfig {
    HttpTransportBudgets {
        concurrency: 3,
        request_timeout: timeout,
    }
    .config("http://localhost/mcp", 1024)
}

#[test]
fn sse_reconnect_failures_exhaust_a_finite_attempt_budget() {
    let policy = config(Duration::from_secs(30)).retry_config;
    for attempt in 0..3 {
        assert!(
            policy.retry(attempt).is_some(),
            "retry {attempt} remains admitted"
        );
    }
    assert!(
        policy.retry(3).is_none(),
        "three consecutive reconnects exhaust this stream's budget"
    );
    assert!(
        policy.retry(usize::MAX).is_none(),
        "large counters cannot bypass exhaustion"
    );
}

#[test]
fn sse_reconnect_delays_fit_the_request_budget_and_are_deterministic() {
    for timeout in [Duration::from_millis(75), Duration::from_mins(5)] {
        let first = config(timeout).retry_config;
        let second = config(timeout).retry_config;
        let max_delay = (timeout / 3).min(DISCOVERY_TIMEOUT);
        let mut total = Duration::ZERO;
        for attempt in 0..3 {
            let delay = first.retry(attempt).expect("admitted retry");
            assert!(
                delay <= max_delay,
                "retry {attempt} exceeds its delay ceiling: {delay:?}"
            );
            assert_eq!(
                first.retry(attempt),
                second.retry(attempt),
                "stable endpoint jitter"
            );
            total += delay;
        }
        assert!(
            total <= timeout,
            "retry sleeps cannot exceed the request budget"
        );
    }
}

#[test]
fn sse_reconnect_success_reset_does_not_share_exhaustion_between_streams() {
    let policy = config(Duration::from_secs(30)).retry_config;
    let initial = policy.retry(0);
    assert!(policy.retry(3).is_none());
    // rmcp supplies a fresh zero counter after a successful GET reconnect.
    // The shared policy must not retain mutable exhaustion across other streams.
    assert_eq!(policy.retry(0), initial);
    assert!(policy.retry(0).is_some());
}

#[tokio::test]
async fn sse_worker_stops_failed_get_reconnects_without_reposting_the_rpc() {
    use rmcp::model::ClientJsonRpcMessage;
    use rmcp::transport::streamable_http_client::StreamableHttpClientWorker;
    use rmcp::transport::{Transport, WorkerTransport};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        if body["method"] == "server/discover" {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc":"2.0", "id":body["id"], "result":{
                    "protocolVersion":"2026-07-28", "capabilities":{},
                    "serverInfo":{"name":"reconnect-fixture", "version":"1"}
                }
            }))
        } else {
            ResponseTemplate::new(200).set_body_raw(
                "id: cursor-1\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                "text/event-stream")
        }
    }).mount(&server).await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    drop(rustls::crypto::ring::default_provider().install_default());
    let client =
        super::super::super::http_client::BodyCappedHttpClient::new(reqwest::Client::new(), 1024);
    let transport_config = HttpTransportBudgets {
        concurrency: 3,
        request_timeout: Duration::from_millis(30),
    }
    .config(&server.uri(), 1024);
    let mut transport =
        WorkerTransport::spawn(StreamableHttpClientWorker::new(client, transport_config));
    let discovery: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
        "jsonrpc":"2.0", "id":1, "method":"server/discover"
    }))
    .unwrap();
    transport.send(discovery).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), transport.receive_raw())
        .await
        .unwrap()
        .unwrap();
    let request: ClientJsonRpcMessage = serde_json::from_value(serde_json::json!({
        "jsonrpc":"2.0", "id":2, "method":"tools/list"
    }))
    .unwrap();
    transport.send(request).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let requests = server.received_requests().await.unwrap();
            if requests
                .iter()
                .filter(|request| request.method.as_str() == "GET")
                .count()
                >= 3
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("three bounded GET reconnect attempts");
    tokio::time::sleep(Duration::from_millis(75)).await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method.as_str() == "GET")
            .count(),
        3
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method.as_str() == "POST")
            .count(),
        2,
        "reconnect must never replay the tools/list POST"
    );
    transport.close().await.unwrap();
}

#[test]
fn sse_reconnect_declines_excessive_server_hints_instead_of_reconnecting_early() {
    let policy = config(Duration::from_millis(75)).retry_config;
    assert_eq!(
        policy.retry_with_server_hint(0, Some(Duration::from_millis(20))),
        Some(Duration::from_millis(25))
    );
    assert_eq!(
        policy.retry_with_server_hint(0, Some(Duration::from_millis(26))),
        None,
        "a hint above the delay budget must stop reconnection"
    );
    assert_eq!(
        policy.retry_with_server_hint(3, Some(Duration::ZERO)),
        None,
        "a server hint must not bypass finite failure exhaustion"
    );
}
