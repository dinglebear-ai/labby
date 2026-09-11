#![cfg(feature = "gateway")]
#![allow(clippy::panic, dead_code)]

#[path = "support/action_matrix.rs"]
mod action_matrix;
#[path = "support/action_scenarios.rs"]
mod action_scenarios;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::{WebSocketStream, tungstenite::client::IntoClientRequest as _};

type Socket = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
const EXTENSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static BROWSER_SOCKET_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn send_version(socket: &mut Socket, version: u32, mut value: Value) {
    value["version"] = json!(version);
    value["request_id"] = json!(uuid::Uuid::new_v4().to_string());
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            value.to_string().into(),
        ))
        .await
        .unwrap();
}

async fn send(socket: &mut Socket, value: Value) {
    send_version(socket, 2, value).await;
}

async fn receive(socket: &mut Socket) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("browser protocol reply deadline")
        .expect("browser socket closed")
        .expect("browser socket message");
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

fn action(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    name: &str,
    params: Value,
) -> reqwest::RequestBuilder {
    client
        .post(format!("{base}/v1/browser"))
        .bearer_auth(token)
        .json(&json!({"action": name, "params": params}))
}

async fn success(request: reqwest::RequestBuilder) -> Value {
    let response = request.send().await.unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    action_scenarios::assert_sanitized(body.to_string().as_bytes(), "browser success");
    assert!(status.is_success(), "browser API status {status}: {body}");
    body
}

async fn failure(request: reqwest::RequestBuilder, status: reqwest::StatusCode, kind: &str) {
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), status);
    let body: Value = response.json().await.unwrap();
    action_scenarios::assert_sanitized(body.to_string().as_bytes(), "browser error");
    assert_eq!(body["kind"], kind, "unexpected browser error: {body}");
}

#[tokio::test]
async fn authenticated_socket_pairs_observes_calls_and_revokes_through_real_http_dispatch() {
    let _socket_test = BROWSER_SOCKET_TEST_LOCK.lock().await;
    let page_state = tempfile::tempdir().expect("owned page callback state");
    let page_record = page_state.path().join("page-record.txt");
    std::fs::write(&page_record, "before callback").unwrap();
    let token = uuid::Uuid::new_v4().to_string();
    let guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", &token)
        .start()
        .await
        .expect("isolated browser gateway");
    let base = &guard.connection().base_url;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let socket_url = format!("{}/browser/socket", base.replacen("http://", "ws://", 1));
    let mut rejected = socket_url.clone().into_client_request().unwrap();
    rejected
        .headers_mut()
        .insert("Origin", "https://untrusted.example".parse().unwrap());
    let error = tokio_tungstenite::connect_async(rejected)
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("origin rejection must be an HTTP policy response: {error}");
    };
    assert_eq!(response.status().as_u16(), 403);
    let mut hostile_host = socket_url.clone().into_client_request().unwrap();
    hostile_host.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{EXTENSION}").parse().unwrap(),
    );
    hostile_host
        .headers_mut()
        .insert("Host", "untrusted.example".parse().unwrap());
    let error = tokio_tungstenite::connect_async(hostile_host)
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("host rejection must be an HTTP policy response: {error}");
    };
    assert_eq!(response.status().as_u16(), 421);
    let unauthenticated = client
        .post(format!("{base}/v1/browser"))
        .json(&json!({"action":"browser.pairing.list","params":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);

    let mut request = socket_url.clone().into_client_request().unwrap();
    request.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{EXTENSION}").parse().unwrap(),
    );
    let (mut socket, upgrade) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(upgrade.status().as_u16(), 101);
    send(&mut socket, json!({"type":"heartbeat"})).await;
    let heartbeat = receive(&mut socket).await;
    assert_eq!(heartbeat["type"], "acknowledged");
    assert_eq!(heartbeat["received"], "heartbeat");
    send(
        &mut socket,
        json!({"type":"pairing_status","pairing_id":"missing-pairing"}),
    )
    .await;
    let missing_pairing = receive(&mut socket).await;
    assert_eq!(missing_pairing["type"], "error");
    assert_eq!(missing_pairing["kind"], "pairing_not_pending");
    let signing = SigningKey::from_bytes(&[41; 32]);
    send(
        &mut socket,
        json!({
            "type":"pairing_request", "display_name":"Protocol acceptance fixture",
            "extension_id":EXTENSION,
            "public_key":URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes())
        }),
    )
    .await;
    let pending = receive(&mut socket).await;
    assert_eq!(pending["type"], "pairing_pending");
    let pairing_fingerprint = pending["pairing_fingerprint"]
        .as_str()
        .expect("pairing fingerprint");
    assert_eq!(pairing_fingerprint.len(), 12);

    let mut legacy_request = socket_url.clone().into_client_request().unwrap();
    legacy_request.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{EXTENSION}").parse().unwrap(),
    );
    let (mut legacy_socket, _) = tokio_tungstenite::connect_async(legacy_request)
        .await
        .unwrap();
    send_version(
        &mut legacy_socket,
        1,
        json!({
            "type":"pairing_request", "display_name":"Legacy client",
            "extension_id":EXTENSION,
            "public_key":URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes())
        }),
    )
    .await;
    let legacy_pairing = receive(&mut legacy_socket).await;
    assert_eq!(legacy_pairing["version"], 1);
    assert_eq!(legacy_pairing["type"], "error");
    assert_eq!(legacy_pairing["kind"], "protocol_upgrade_required");

    let mut other_extension_request = socket_url.clone().into_client_request().unwrap();
    other_extension_request.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{}", "b".repeat(32))
            .parse()
            .unwrap(),
    );
    let (mut other_extension_socket, _) = tokio_tungstenite::connect_async(other_extension_request)
        .await
        .unwrap();
    send(
        &mut other_extension_socket,
        json!({"type":"pairing_status","pairing_id":pending["pairing_id"]}),
    )
    .await;
    let hidden_pairing = receive(&mut other_extension_socket).await;
    assert_eq!(hidden_pairing["type"], "error");
    assert_eq!(hidden_pairing["kind"], "pairing_not_pending");
    other_extension_socket.close(None).await.unwrap();

    let approved = success(action(
        &client,
        base,
        &token,
        "browser.pairing.approve",
        json!({
            "pairing_id":pending["pairing_id"],
            "pairing_fingerprint":pairing_fingerprint
        }),
    ))
    .await;
    let browser_id = approved["id"].as_str().expect("approved browser identity");

    send_version(
        &mut legacy_socket,
        1,
        json!({"type":"auth_challenge","browser_id":browser_id}),
    )
    .await;
    let legacy_nonce = receive(&mut legacy_socket).await;
    assert_eq!(legacy_nonce["version"], 1);
    assert_eq!(legacy_nonce["type"], "auth_nonce");
    let legacy_signature = signing.sign(
        &URL_SAFE_NO_PAD
            .decode(legacy_nonce["nonce"].as_str().unwrap())
            .unwrap(),
    );
    send_version(
        &mut legacy_socket,
        1,
        json!({
            "type":"auth_response",
            "challenge_id":legacy_nonce["challenge_id"],
            "signature":URL_SAFE_NO_PAD.encode(legacy_signature.to_bytes())
        }),
    )
    .await;
    let legacy_authenticated = receive(&mut legacy_socket).await;
    assert_eq!(legacy_authenticated["version"], 1);
    assert_eq!(legacy_authenticated["type"], "authenticated");
    send_version(&mut legacy_socket, 1, json!({"type":"heartbeat"})).await;
    let legacy_heartbeat = receive(&mut legacy_socket).await;
    assert_eq!(legacy_heartbeat["version"], 1);
    assert_eq!(legacy_heartbeat["type"], "acknowledged");
    legacy_socket.close(None).await.unwrap();

    send(
        &mut socket,
        json!({"type":"auth_challenge","browser_id":browser_id}),
    )
    .await;
    let nonce = receive(&mut socket).await;
    assert_eq!(nonce["type"], "auth_nonce");
    let nonce_bytes = URL_SAFE_NO_PAD
        .decode(nonce["nonce"].as_str().unwrap())
        .unwrap();
    let invalid_signing = SigningKey::from_bytes(&[42; 32]);
    let invalid_signature = invalid_signing.sign(&nonce_bytes);
    send(&mut socket, json!({"type":"auth_response", "challenge_id":nonce["challenge_id"], "signature":URL_SAFE_NO_PAD.encode(invalid_signature.to_bytes())})).await;
    let rejected = receive(&mut socket).await;
    assert_eq!(rejected["type"], "error");
    assert_eq!(rejected["kind"], "auth_failed");

    let signature = signing.sign(&nonce_bytes);
    send(&mut socket, json!({"type":"auth_response", "challenge_id":nonce["challenge_id"], "signature":URL_SAFE_NO_PAD.encode(signature.to_bytes())})).await;
    assert_eq!(receive(&mut socket).await["type"], "authenticated");

    // Authentication releases this socket's per-client pre-auth permit. A full
    // client allowance must remain available while the authenticated socket is
    // still established.
    let idle_request = || {
        let mut request = socket_url.clone().into_client_request().unwrap();
        request.headers_mut().insert(
            "Origin",
            format!("chrome-extension://{EXTENSION}").parse().unwrap(),
        );
        request
    };
    let mut idle_sockets = Vec::new();
    for _ in 0..8 {
        let (idle, _) = tokio_tungstenite::connect_async(idle_request())
            .await
            .unwrap();
        idle_sockets.push(idle);
    }
    drop(idle_sockets);

    send(&mut socket, json!({"type":"heartbeat"})).await;
    let heartbeat = receive(&mut socket).await;
    assert_eq!(heartbeat["type"], "acknowledged");
    assert_eq!(heartbeat["received"], "heartbeat");
    send(
        &mut socket,
        json!({
            "type":"observe", "tab_id":7, "document_id":"document-one",
            "origin":"https://page.example", "sanitized_path":"/tools", "page_title":"Fixture page",
            "catalog_revision":1, "catalog_fingerprint":"fixture-fingerprint",
            "tools":[{"name":"echo", "title":"Echo", "origin":"https://page.example",
              "description":"Echo fixture", "input_schema":{"type":"object"}, "annotations":{}}]
        }),
    )
    .await;
    assert_eq!(receive(&mut socket).await["type"], "acknowledged");
    let sessions = success(action(&client, base, &token, "browser.sessions", json!({}))).await;
    let session = success(action(
        &client,
        base,
        &token,
        "browser.session.get",
        json!({"session_id": sessions["sessions"][0]["id"]}),
    ))
    .await;
    assert_eq!(session["enabled"], false);
    assert_eq!(session["tools"][0]["name"], "echo");
    let params = json!({"browser_id":browser_id,"tab_id":7,"document_id":"document-one",
        "catalog_revision":1,"catalog_digest":session["catalog_digest"],"tool_name":"echo","arguments":{"text":"accepted"},"timeout_ms":5000});
    failure(
        action(&client, base, &token, "browser.call", params.clone()),
        reqwest::StatusCode::CONFLICT,
        "stale_document",
    )
    .await;
    success(action(
        &client,
        base,
        &token,
        "browser.session.enable",
        json!({"session_id":session["id"],"enabled":true,"catalog_digest":session["catalog_digest"]}),
    ))
    .await;

    let request = action(&client, base, &token, "browser.call", params.clone());
    let pending_call = tokio::spawn(async move { success(request).await });
    let call = receive(&mut socket).await;
    assert_eq!(call["type"], "tool_call");
    assert_eq!(call["document_id"], "document-one");
    assert_eq!(call["catalog_fingerprint"], "fixture-fingerprint");
    assert_eq!(call["arguments"], json!({"text":"accepted"}));
    // The isolated page callback mutates durable state only after the real
    // authenticated dispatch delivers its exact, consented invocation.
    std::fs::write(&page_record, call["arguments"]["text"].as_str().unwrap()).unwrap();
    send(
        &mut socket,
        json!({"type":"tool_result","call_id":call["call_id"],"result":{"echo":"accepted"}}),
    )
    .await;
    assert_eq!(pending_call.await.unwrap(), json!({"echo":"accepted"}));
    assert_eq!(std::fs::read_to_string(&page_record).unwrap(), "accepted");
    assert_eq!(receive(&mut socket).await["type"], "acknowledged");

    // The peer repeats its claimed revision and fingerprint but changes the
    // actual schema. Neither old consent nor an old call may follow that change.
    send(&mut socket, json!({
        "type":"observe", "tab_id":7, "document_id":"document-one",
        "origin":"https://page.example", "sanitized_path":"/tools", "page_title":"Fixture page",
        "catalog_revision":1, "catalog_fingerprint":"fixture-fingerprint",
        "tools":[{"name":"echo", "title":"Echo", "origin":"https://page.example",
          "description":"Changed fixture", "input_schema":{"type":"object","required":["text"]}, "annotations":{}}]
    })).await;
    assert_eq!(receive(&mut socket).await["type"], "acknowledged");
    let replacement = success(action(&client, base, &token, "browser.sessions", json!({}))).await;
    let replacement = success(action(
        &client,
        base,
        &token,
        "browser.session.get",
        json!({"session_id": replacement["sessions"][0]["id"]}),
    ))
    .await;
    assert_eq!(replacement["enabled"], false);
    assert_ne!(replacement["catalog_digest"], session["catalog_digest"]);
    failure(action(&client, base, &token, "browser.session.enable", json!({
        "session_id":session["id"], "enabled":true, "catalog_digest":session["catalog_digest"]
    })), reqwest::StatusCode::CONFLICT, "stale_document").await;
    success(action(&client, base, &token, "browser.session.enable", json!({
        "session_id":replacement["id"], "enabled":true, "catalog_digest":replacement["catalog_digest"]
    }))).await;
    failure(
        action(&client, base, &token, "browser.call", params.clone()),
        reqwest::StatusCode::CONFLICT,
        "stale_document",
    )
    .await;
    let mut current_params = params.clone();
    current_params["catalog_digest"] = replacement["catalog_digest"].clone();
    let request = action(&client, base, &token, "browser.call", current_params);
    let revoked_call = tokio::spawn(async move {
        failure(
            request,
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "browser_offline",
        )
        .await;
    });
    let pending = receive(&mut socket).await;
    assert_eq!(pending["type"], "tool_call");

    success(action(
        &client,
        base,
        &token,
        "browser.revoke",
        json!({"browser_id":browser_id}),
    ))
    .await;
    let cancellation = receive(&mut socket).await;
    assert_eq!(cancellation["type"], "tool_cancel");
    assert_eq!(cancellation["call_id"], pending["call_id"]);
    revoked_call.await.unwrap();
    failure(
        action(&client, base, &token, "browser.call", params),
        reqwest::StatusCode::CONFLICT,
        "stale_document",
    )
    .await;
    drop(socket);

    let mut stale_request = format!("{}/browser/socket", base.replacen("http://", "ws://", 1))
        .into_client_request()
        .unwrap();
    stale_request.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{EXTENSION}").parse().unwrap(),
    );
    let (mut stale_socket, _) = tokio_tungstenite::connect_async(stale_request)
        .await
        .unwrap();
    send(
        &mut stale_socket,
        json!({"type":"auth_challenge","browser_id":browser_id}),
    )
    .await;
    let rejected_auth = receive(&mut stale_socket).await;
    assert_eq!(rejected_auth["type"], "error");
    assert_eq!(rejected_auth["kind"], "auth_failed");
    stale_socket.close(None).await.unwrap();

    let cleanup = guard.finish().await;
    assert!(
        cleanup.failures.is_empty(),
        "browser gateway cleanup: {:?}",
        cleanup.failures
    );
    assert_eq!(std::fs::read_to_string(&page_record).unwrap(), "accepted");
    page_state
        .close()
        .expect("remove owned page callback state");
    action_scenarios::ActionOutcome {
        key: "browser:browser.call".into(),
        surface: action_matrix::Surface::Api,
        disposition: action_scenarios::Disposition::IsolatedWorkflow,
        evidence: action_matrix::EvidenceLevel::LiveStateTransition,
        owner: action_matrix::ScenarioOwner::StatefulWorkflowRunner,
        outcome_kind: "consented_socket_callback_persisted_page_state".into(),
        recovery: "none_required".into(),
        side_effects: "owned_page_state_read_back_and_removed".into(),
        canary_free: true,
    }
    .record();
}

#[tokio::test]
async fn pairing_creation_is_rate_limited_per_client() {
    let _socket_test = BROWSER_SOCKET_TEST_LOCK.lock().await;
    let token = uuid::Uuid::new_v4().to_string();
    let guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", &token)
        .start()
        .await
        .unwrap();
    let url = format!(
        "{}/browser/socket",
        guard.connection().base_url.replacen("http://", "ws://", 1)
    );
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Origin",
        format!("chrome-extension://{EXTENSION}").parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    for index in 1_u8..=8 {
        let signing = SigningKey::from_bytes(&[index; 32]);
        send(
            &mut socket,
            json!({
                "type":"pairing_request",
                "display_name":format!("Rate fixture {index}"),
                "extension_id":EXTENSION,
                "public_key":URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes())
            }),
        )
        .await;
        assert_eq!(receive(&mut socket).await["type"], "pairing_pending");
    }

    let signing = SigningKey::from_bytes(&[9; 32]);
    send(
        &mut socket,
        json!({
            "type":"pairing_request",
            "display_name":"Rate fixture rejected",
            "extension_id":EXTENSION,
            "public_key":URL_SAFE_NO_PAD.encode(signing.verifying_key().to_bytes())
        }),
    )
    .await;
    let terminal = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("rate-limited socket must terminate promptly");
    match terminal {
        None | Some(Err(_)) | Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {}
        Some(Ok(message)) => panic!("rate-limited pairing unexpectedly received {message:?}"),
    }
    assert!(guard.finish().await.failures.is_empty());
}

#[tokio::test]
async fn global_socket_admission_is_bounded_across_trusted_client_buckets() {
    let _socket_test = BROWSER_SOCKET_TEST_LOCK.lock().await;
    let token = uuid::Uuid::new_v4().to_string();
    let guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", &token)
        .config("[api]\ntrust_forwarded_headers = true\n")
        .start()
        .await
        .unwrap();
    let url = format!(
        "{}/browser/socket",
        guard.connection().base_url.replacen("http://", "ws://", 1)
    );
    let request = |index: u8| {
        let mut request = url.clone().into_client_request().unwrap();
        request.headers_mut().insert(
            "Origin",
            format!("chrome-extension://{EXTENSION}").parse().unwrap(),
        );
        request.headers_mut().insert(
            "X-Forwarded-For",
            format!("198.51.100.{index}").parse().unwrap(),
        );
        request
    };
    let mut sockets = Vec::new();
    for index in 1_u8..=64 {
        let (socket, _) = tokio_tungstenite::connect_async(request(index))
            .await
            .unwrap();
        sockets.push(socket);
    }
    let rejected = tokio_tungstenite::connect_async(request(65))
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = rejected else {
        panic!("global socket exhaustion must return an HTTP error");
    };
    assert_eq!(response.status().as_u16(), 429);
    drop(sockets.pop());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok((socket, _)) = tokio_tungstenite::connect_async(request(65)).await {
            sockets.push(socket);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "global socket permit was not released"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(sockets);
    assert!(guard.finish().await.failures.is_empty());
}

#[tokio::test]
async fn idle_socket_admission_is_bounded_per_client_and_released_after_disconnect() {
    let _socket_test = BROWSER_SOCKET_TEST_LOCK.lock().await;
    let token = uuid::Uuid::new_v4().to_string();
    let guard = live_labby::LiveLabbyBuilder::new()
        .env("LABBY_MCP_HTTP_TOKEN", &token)
        .start()
        .await
        .unwrap();
    let url = format!(
        "{}/browser/socket",
        guard.connection().base_url.replacen("http://", "ws://", 1)
    );
    let request = || {
        let mut request = url.clone().into_client_request().unwrap();
        request.headers_mut().insert(
            "Origin",
            format!("chrome-extension://{EXTENSION}").parse().unwrap(),
        );
        request
    };
    let mut sockets = Vec::new();
    for _ in 0..8 {
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(request()),
        )
        .await
        .unwrap()
        .unwrap();
        sockets.push(socket);
    }
    let rejected = tokio_tungstenite::connect_async(request())
        .await
        .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = rejected else {
        panic!("exhausted socket admission must return an HTTP error");
    };
    assert_eq!(response.status().as_u16(), 429);
    drop(sockets.pop());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok((socket, _)) = tokio_tungstenite::connect_async(request()).await {
            sockets.push(socket);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "socket permit was not released"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(sockets);
    assert!(guard.finish().await.failures.is_empty());
}
