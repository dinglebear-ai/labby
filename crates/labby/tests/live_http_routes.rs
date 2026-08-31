#![cfg(feature = "gateway")]

#[allow(dead_code)]
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/route_matrix.rs"]
mod route_matrix;
#[path = "support/lib.rs"]
mod support;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use live_identity::LiveIdentity;
use reqwest::{Client, Method, StatusCode, header};
use route_matrix::{RouteCase, invariant_for, route_cases};
use support::LiveLabbyBuilder;

const SHARD_DEADLINE: Duration = Duration::from_secs(90);
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);

#[tokio::test]
async fn every_registered_route_is_live_or_declared_runtime_conditional() {
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("start live Labby");
    let client = Client::builder()
        .timeout(REQUEST_DEADLINE)
        .build()
        .expect("HTTP client");
    let deadline = tokio::time::Instant::now() + SHARD_DEADLINE;
    let mut failures = Vec::new();
    let mut expected_evidence = Vec::new();

    for case in route_cases().expect("route recipes") {
        if tokio::time::Instant::now() >= deadline {
            failures.push(format!(
                "absolute shard deadline exhausted before {}",
                case.key()
            ));
            break;
        }
        match tokio::time::timeout_at(
            deadline,
            probe(&client, guard.connection().base_url.as_str(), &case),
        )
        .await
        {
            Ok(Ok(evidence)) => expected_evidence.push(evidence),
            Ok(Err(error)) => failures.push(error),
            Err(_) => {
                failures.push(format!(
                    "absolute shard deadline expired during {}",
                    case.key()
                ));
                break;
            }
        }
    }
    if failures.is_empty() {
        match tokio::time::timeout_at(
            deadline,
            verify_route_evidence(&guard, &expected_evidence, deadline),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(error),
            Err(_) => failures.push(
                "absolute shard deadline expired while correlating route evidence".to_string(),
            ),
        }
    }

    let diagnostics = guard.diagnostics(failures.first().map(String::as_str));
    let cleanup = guard.finish().await;
    assert!(
        cleanup.is_clean(),
        "route matrix cleanup failed: {:?}",
        cleanup.failures
    );
    assert!(
        failures.is_empty(),
        "route matrix failures:\n{}\n{diagnostics}",
        failures.join("\n")
    );
}

#[derive(Debug)]
struct ExpectedRouteEvidence {
    request_id: String,
    template: String,
    group: String,
    handler: String,
    conditional_404: bool,
}

async fn probe(
    client: &Client,
    base_url: &str,
    case: &RouteCase,
) -> Result<ExpectedRouteEvidence, String> {
    let method = Method::from_bytes(case.descriptor.method.as_bytes())
        .map_err(|error| format!("{} invalid method: {error}", case.key()))?;
    let request_id = format!("route-matrix-{}", stable_id(&case.key()));
    let mut request = client
        .request(method, format!("{base_url}{}", case.path))
        .header("x-request-id", &request_id);
    if let Some(body) = case.body {
        request = request
            .header(header::CONTENT_TYPE, "application/json")
            .body(body);
    }
    if case.descriptor.bootstrap_proof {
        request = request.header("x-labby-bootstrap-proof", "invalid-bootstrap-proof");
    } else if case.descriptor.auth_required {
        request = request.bearer_auth("deliberately-invalid-route-matrix-token");
    }
    if case.descriptor.host_validation {
        request = request.header(header::HOST, "lab.example.test");
    } else if case.descriptor.handler_group == "protected_mcp" {
        request = request.header(header::HOST, "mcp.example.test");
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("{} request failed: {error}", case.key()))?;
    let status = response.status();
    let response_request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.unwrap_or_default();

    if status == StatusCode::NOT_FOUND && !case.permits_runtime_absence() {
        return Err(format!(
            "{} [{}::{:?}] returned undeclared 404; request_id={response_request_id:?}; body={}",
            case.key(),
            case.descriptor.handler_identity,
            case.class,
            bounded(&body)
        ));
    }
    invariant_for(case.class)
        .validate_descriptor(&case.descriptor)
        .map_err(|error| format!("{} descriptor oracle failed: {error}", case.key()))?;
    invariant_for(case.class)
        .validate_invalid_outcome(&case.descriptor, status)
        .map_err(|error| {
            format!(
                "{error}; request_id={response_request_id:?}; body={}",
                bounded(&body)
            )
        })?;
    if response_request_id.as_deref() != Some(request_id.as_str()) {
        return Err(format!(
            "{} [{}::{:?}] did not propagate request id; got={response_request_id:?}; status={status}; body={}",
            case.key(),
            case.descriptor.handler_identity,
            case.class,
            bounded(&body)
        ));
    }
    Ok(ExpectedRouteEvidence {
        request_id,
        template: case.descriptor.path.clone(),
        group: case.descriptor.handler_group.clone(),
        handler: case.descriptor.handler_identity.clone(),
        conditional_404: case.permits_runtime_absence() && status == StatusCode::NOT_FOUND,
    })
}

async fn verify_route_evidence(
    guard: &support::LiveLabbyGuard,
    expected: &[ExpectedRouteEvidence],
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    loop {
        let stdout = std::fs::read_to_string(guard.root().join("stdout.log")).unwrap_or_default();
        let stderr = std::fs::read_to_string(guard.root().join("stderr.log")).unwrap_or_default();
        let lines = stdout.lines().chain(stderr.lines()).collect::<Vec<_>>();
        let mut missing = Vec::new();
        for route in expected {
            let evidence = lines.iter().find(|line| {
                line.contains("http_route_evidence") && line.contains(&route.request_id)
            });
            match evidence {
                Some(line)
                    if line.contains(&route.template)
                        && line.contains(&route.group)
                        && line.contains(&route.handler)
                        && (line.contains("mounted_route_match")
                            || (route.conditional_404
                                && line.contains("declared_conditional_absence"))) => {}
                Some(line) => missing.push(format!(
                    "{} evidence mismatch; expected template={} group={} handler={} mounted-or-conditional-absence={}; line={}",
                    route.request_id,
                    route.template,
                    route.group,
                    route.handler,
                    route.conditional_404,
                    bounded(line)
                )),
                None => missing.push(format!("{} evidence missing", route.request_id)),
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "route evidence incomplete at absolute deadline ({}/{} correlated):\n{}",
                expected.len().saturating_sub(missing.len()),
                expected.len(),
                missing.join("\n")
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn public_routes_remain_public_with_an_invalid_bearer() {
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("start live Labby");
    let client = Client::builder()
        .timeout(REQUEST_DEADLINE)
        .build()
        .expect("HTTP client");
    for path in [
        "/health",
        "/ready",
        "/.well-known/labby.json",
        "/apps/assets/labby-app-host.js",
    ] {
        let response = client
            .get(format!("{}{path}", guard.connection().base_url))
            .bearer_auth("invalid")
            .send()
            .await
            .expect("public request");
        assert_ne!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "public route {path} was bearer-wrapped"
        );
        assert_ne!(
            response.status(),
            StatusCode::FORBIDDEN,
            "public route {path} was bearer-wrapped"
        );
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "public route {path} was not mounted"
        );
    }
    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
}

#[tokio::test]
async fn protected_routes_reach_their_route_class_with_a_public_credential() {
    let identity = LiveIdentity::bootstrap("route-matrix@example.invalid")
        .await
        .expect("bootstrap public identity");
    let status = identity
        .protected_mcp_initialize()
        .await
        .expect("protected MCP request");
    let cleanup = identity.cleanup().await.expect("identity cleanup");
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
    assert_eq!(
        status,
        StatusCode::OK,
        "route-bound credential did not reach protected MCP"
    );
}

#[tokio::test]
async fn representative_route_class_negatives_fail_closed() {
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("start live Labby");
    let client = Client::builder()
        .timeout(REQUEST_DEADLINE)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");
    let base = &guard.connection().base_url;

    for path in ["/v1/catalog", "/v1/doctor", "/mcp", "/apps"] {
        let response = client
            .get(format!("{base}{path}"))
            .send()
            .await
            .expect("missing-auth request");
        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "{path} did not reject missing auth: {}",
            response.status()
        );
    }

    let unknown = client
        .get(format!("{base}/definitely-not-a-labby-route"))
        .send()
        .await
        .expect("unknown route");
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let trailing = client
        .get(format!("{base}/health/"))
        .send()
        .await
        .expect("trailing slash");
    assert_eq!(trailing.status(), StatusCode::NOT_FOUND);
    let wrong_method = client
        .post(format!("{base}/health"))
        .send()
        .await
        .expect("wrong method");
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
    let malformed = client
        .post(format!("{base}/v1/doctor"))
        .header(header::CONTENT_TYPE, "application/json")
        .bearer_auth("invalid")
        .body("{")
        .send()
        .await
        .expect("malformed body");
    assert!(matches!(
        malformed.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    let wrong_type = client
        .post(format!("{base}/v1/doctor"))
        .header(header::CONTENT_TYPE, "text/plain")
        .bearer_auth("invalid")
        .body("{}")
        .send()
        .await
        .expect("wrong content type");
    assert!(matches!(
        wrong_type.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));

    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
}

#[tokio::test]
async fn public_and_protected_posture_survives_restart() {
    let mut guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("start live Labby");
    let client = Client::builder()
        .timeout(REQUEST_DEADLINE)
        .build()
        .expect("HTTP client");

    for generation in 0..=1 {
        let public = client
            .get(format!("{}/health", guard.connection().base_url))
            .send()
            .await
            .expect("public health request");
        assert_eq!(public.status(), StatusCode::OK, "generation {generation}");

        let protected = client
            .get(format!("{}/v1/catalog", guard.connection().base_url))
            .send()
            .await
            .expect("protected request");
        assert!(
            matches!(
                protected.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ),
            "protected route opened after generation {generation}: {}",
            protected.status()
        );
        if generation == 0 {
            guard.restart().await.expect("restart live Labby");
        }
    }

    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
}

#[tokio::test]
async fn method_and_transport_abuse_is_bounded_and_fail_closed() {
    let guard = LiveLabbyBuilder::new()
        .start()
        .await
        .expect("start live Labby");
    let client = Client::builder()
        .timeout(REQUEST_DEADLINE)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");
    let base = &guard.connection().base_url;

    for (method, path) in [
        (Method::HEAD, "/health"),
        (Method::OPTIONS, "/health"),
        (Method::HEAD, "/v1/catalog"),
        (Method::OPTIONS, "/v1/catalog"),
        (Method::HEAD, "/apps/assets/labby-app-host.js"),
        (Method::OPTIONS, "/apps/assets/labby-app-host.js"),
    ] {
        let response = client
            .request(method.clone(), format!("{base}{path}"))
            .send()
            .await
            .expect("route-class method posture request");
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{method} posture bypassed the mounted route class for {path}"
        );
    }

    let oversized = client
        .post(format!("{base}/v1/doctor"))
        .bearer_auth("invalid")
        .header(header::CONTENT_TYPE, "application/json")
        .body(vec![b'x'; 2 * 1024 * 1024])
        .send()
        .await
        .expect("oversized request");
    assert!(!oversized.status().is_success());

    for headers in [
        vec![
            ("authorization", "Bearer one"),
            ("authorization", "Bearer two"),
        ],
        vec![("cookie", "a=one"), ("cookie", "a=two")],
        vec![("x-csrf-token", "one"), ("x-csrf-token", "two")],
    ] {
        let mut request = client.get(format!("{base}/v1/catalog"));
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send().await.expect("duplicate header request");
        assert!(!response.status().is_success());
    }

    let forwarded = client
        .get(format!("{base}/v1/catalog"))
        .header(
            "forwarded",
            "for=203.0.113.10;host=attacker.invalid;proto=https",
        )
        .header("x-forwarded-for", "203.0.113.10")
        .send()
        .await
        .expect("forwarded-header request");
    assert!(matches!(
        forwarded.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));

    let hidden_a = client
        .delete(format!("{base}/v1/access/credentials/known-shaped-id"))
        .bearer_auth("invalid")
        .send()
        .await
        .expect("hidden resource A");
    let hidden_b = client
        .delete(format!(
            "{base}/v1/access/credentials/definitely-missing-id"
        ))
        .bearer_auth("invalid")
        .send()
        .await
        .expect("hidden resource B");
    assert_eq!(hidden_a.status(), hidden_b.status());
    assert_eq!(
        hidden_a.headers().get(header::CONTENT_TYPE),
        hidden_b.headers().get(header::CONTENT_TYPE),
        "hidden-resource denial leaked through response classification"
    );
    assert_eq!(
        hidden_a.text().await.expect("hidden body A"),
        hidden_b.text().await.expect("hidden body B"),
        "hidden-resource denial leaked resource existence"
    );

    for target in [
        "/health?x=one&x=two",
        "/health/../health",
        "/health%2f..%2fhealth",
        "/%zz",
    ] {
        let status = raw_status(connection_address(&guard), target, &[]);
        assert!(
            (400..500).contains(&status) || (target.starts_with("/health?") && status == 200),
            "unexpected encoded-path posture for {target}: {status}"
        );
    }

    let many_headers = (0..160)
        .map(|index| (format!("x-padding-{index}"), "x".repeat(64)))
        .collect::<Vec<_>>();
    let borrowed = many_headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let status = raw_status(connection_address(&guard), "/health", &borrowed);
    assert!(
        (200..500).contains(&status),
        "invalid header-count response: {status}"
    );

    // Drop a partial body mid-stream. The server must promptly remain usable.
    let mut cancelled = TcpStream::connect(connection_address(&guard)).expect("connect stream");
    cancelled
        .write_all(b"POST /v1/doctor HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 100000\r\nContent-Type: application/json\r\n\r\n{")
        .expect("write partial body");
    drop(cancelled);
    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("health after cancellation");
    assert_eq!(health.status(), StatusCode::OK);

    let cleanup = guard.finish().await;
    assert!(cleanup.is_clean(), "cleanup failed: {:?}", cleanup.failures);
}

fn raw_status(address: std::net::SocketAddr, target: &str, headers: &[(&str, &str)]) -> u16 {
    let mut stream = TcpStream::connect_timeout(&address, REQUEST_DEADLINE).expect("raw connect");
    stream
        .set_read_timeout(Some(REQUEST_DEADLINE))
        .expect("raw read timeout");
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n"
    )
    .expect("raw request line");
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").expect("raw request header");
    }
    stream.write_all(b"\r\n").expect("finish raw request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read raw response");
    response
        .split_whitespace()
        .nth(1)
        .expect("HTTP status")
        .parse()
        .expect("numeric HTTP status")
}

fn connection_address(guard: &support::LiveLabbyGuard) -> std::net::SocketAddr {
    let url = reqwest::Url::parse(&guard.connection().base_url).expect("live base URL");
    let host = url.host_str().expect("live host");
    let port = url.port_or_known_default().expect("live port");
    format!("{host}:{port}")
        .parse()
        .expect("live socket address")
}

fn stable_id(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

fn bounded(value: &str) -> &str {
    value.get(..value.len().min(512)).unwrap_or(value)
}
