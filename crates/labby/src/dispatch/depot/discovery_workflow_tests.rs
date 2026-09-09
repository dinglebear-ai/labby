use super::super::{manager::SecretSnapshot, network::NetworkPolicy};
use super::*;
use crate::config::depot::DepotPreferences;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    endpoint: String,
    requests: Arc<Mutex<Vec<Value>>>,
    epoch: Arc<Mutex<String>>,
    source_capability: Arc<Mutex<Value>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fixture(prefix: &'static str, hours: Vec<u8>) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let epoch = Arc::new(Mutex::new("catalog".to_owned()));
    let serving_epoch = epoch.clone();
    let source_capability = Arc::new(Mutex::new(
        json!({"version":"source-origin/v1","values":["ard"]}),
    ));
    let serving_capability = source_capability.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let header_end = loop {
                let mut chunk = [0; 4096];
                let size = stream.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                bytes.extend_from_slice(&chunk[..size]);
                assert!(bytes.len() < 16384);
                if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            while bytes.len() < header_end + length {
                let mut chunk = [0; 4096];
                let size = stream.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                bytes.extend_from_slice(&chunk[..size]);
            }
            let mut response = json!({"contractVersion":"depot.discovery/v1","deploymentId":prefix,
                "deploymentEpoch":"boot","authorityEpoch":"authority","listingEpoch":*serving_epoch.lock().unwrap(),
                "snapshotContinuations":true,"maxPageSize":2,
                "filters":{"sourceOrigin":*serving_capability.lock().unwrap()},
                "feeds":{"new":{"rankingVersion":"new/v1","windowSeconds":604800,"acceptsAsOf":true}}});
            if length > 0 {
                let body: Value =
                    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                recorded.lock().unwrap().push(body.clone());
                let offset = body["cursor"]
                    .as_str()
                    .map(|value| value.parse::<usize>().unwrap())
                    .unwrap_or(0);
                let limit = body["limit"].as_u64().unwrap() as usize;
                let rows: Vec<_> = hours.iter().enumerate().skip(offset).take(limit).map(|(index,hour)| json!({
                    "id":format!("{prefix}-{index}"),"sourceOrigin":"ard","firstSeenAt":format!("2026-09-08T{hour:02}:00:00Z")
                })).collect();
                let next = offset + rows.len();
                let mut result = json!({"artifacts":rows,"total":hours.len(),"feed":"new","asOf":body["asOf"],
                    "rankingVersion":"new/v1","coverage":{"population":"hosted","complete":false,"unknownFirstSeen":0}});
                if next < hours.len() {
                    result["nextCursor"] = json!(next.to_string());
                }
                if body.get("sourceOrigin").is_some() {
                    result["sourceOrigin"] = body["sourceOrigin"].clone();
                }
                response["result"] = result;
            }
            let body = response.to_string();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    Fixture {
        endpoint,
        requests,
        epoch,
        source_capability,
        task,
    }
}

#[tokio::test]
async fn new_workflow_refills_dominant_source_keeps_fixed_batches_and_expires_changed_epoch() {
    let a = fixture("a", vec![11, 10, 9, 8, 7, 6]).await;
    let b = fixture("b", vec![5, 4, 3]).await;
    let preferences: DepotPreferences = toml::from_str(&format!(
        r#"
public_enabled = false
read_project_id = "team-project"
[[local_providers]]
id = "a"
name = "A"
endpoint = "{}"
bearer_token_env = "LABBY_DEPOT_TEST_A_TOKEN"
[[local_providers]]
id = "b"
name = "B"
endpoint = "{}"
bearer_token_env = "LABBY_DEPOT_TEST_B_TOKEN"
"#,
        a.endpoint, b.endpoint
    ))
    .unwrap();
    let manager = Manager::new(
        &preferences,
        SecretSnapshot::from_values(BTreeMap::from([
            ("LABBY_DEPOT_TEST_A_TOKEN".into(), "test-a".into()),
            ("LABBY_DEPOT_TEST_B_TOKEN".into(), "test-b".into()),
        ])),
        NetworkPolicy::default(),
    );
    let selected: Vec<_> = manager
        .snapshot()
        .providers
        .values()
        .filter(|provider| provider.view.enabled)
        .cloned()
        .collect();
    let mut federation = Federation {
        start: 0,
        as_of: Some("2026-09-08T12:00:00Z".into()),
        ranked_started: false,
        providers: selected
            .iter()
            .map(|provider| provider_state(provider, Err(ProviderError::Pending)))
            .collect(),
    };
    let request = DiscoveryRequest {
        source_origin: Some(SourceOrigin::Ard),
        feed: Some(DiscoveryFeed::New),
        provider: None,
        query: String::new(),
        limit: 3,
        cursor: None,
    };
    let admission = manager
        .scheduler
        .admit("workflow", tokio::time::Instant::now())
        .await
        .unwrap();
    let first = fetch_new_page(&selected, &mut federation, &request, &admission)
        .await
        .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|row| row["artifactId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a-0", "a-1", "a-2"],
        "state={federation:?}; requests a={:?} b={:?}",
        a.requests.lock().unwrap(),
        b.requests.lock().unwrap()
    );
    // Reconstruct a retained pre-output pending state with a fetched buffer.
    // Recovery must reuse those rows rather than refetch or stay pending forever.
    federation.ranked_started = false;
    federation
        .providers
        .iter_mut()
        .find(|provider| provider.id == "b")
        .unwrap()
        .page
        .outcome = "pending".into();
    let second = fetch_new_page(&selected, &mut federation, &request, &admission)
        .await
        .unwrap();
    assert_eq!(
        second
            .iter()
            .map(|row| row["artifactId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a-3", "a-4", "a-5"]
    );
    for fixture in [&a, &b] {
        for body in fixture.requests.lock().unwrap().iter() {
            assert_eq!(body["limit"], 2);
            assert_eq!(body["asOf"], "2026-09-08T12:00:00Z");
            assert_eq!(body["feed"], "new");
            assert_eq!(body["sourceOrigin"], "ard");
        }
    }
    assert_eq!(
        b.requests.lock().unwrap().len(),
        1,
        "older source buffer must remain reusable"
    );
    // Even retained ranked buffers lose eligibility when a capability vanishes.
    let valid_capability = b.source_capability.lock().unwrap().clone();
    *b.source_capability.lock().unwrap() = Value::Null;
    assert!(matches!(
        fetch_new_page(&selected, &mut federation, &request, &admission).await,
        Err(DiscoveryError::CursorExpired)
    ));
    *b.source_capability.lock().unwrap() = valid_capability;

    let mut ordinary = Federation {
        start: 0,
        as_of: None,
        ranked_started: false,
        providers: selected
            .iter()
            .map(|provider| provider_state(provider, Err(ProviderError::Pending)))
            .collect(),
    };
    let mut ordinary_request = request.clone();
    ordinary_request.feed = None;
    fetch_pages(&selected, &mut ordinary, &ordinary_request, &admission)
        .await
        .unwrap();
    for fixture in [&a, &b] {
        let requests = fixture.requests.lock().unwrap();
        let body = requests.last().unwrap();
        assert_eq!(body["sourceOrigin"], "ard");
        assert!(body.get("feed").is_none());
    }
    // No upstream unfiltered fallback is allowed, including when rows are buffered.
    *b.source_capability.lock().unwrap() = json!({"version":"wrong", "values":["ard"]});
    let requests_before = b.requests.lock().unwrap().len();
    assert!(matches!(
        fetch_pages(&selected, &mut ordinary, &ordinary_request, &admission).await,
        Err(DiscoveryError::ProviderUnavailable)
    ));
    assert_eq!(b.requests.lock().unwrap().len(), requests_before);
    *b.source_capability.lock().unwrap() = json!({"version":"source-origin/v1","values":["ard"]});
    *b.epoch.lock().unwrap() = "changed".into();
    assert!(matches!(
        fetch_new_page(&selected, &mut federation, &request, &admission).await,
        Err(DiscoveryError::CursorExpired)
    ));
}
