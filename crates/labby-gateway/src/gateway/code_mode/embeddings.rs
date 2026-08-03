//! TEI (Text Embeddings Inference) HTTP client and cosine-similarity ranking
//! for Code Mode's semantic search blend.
//!
//! All vector math lives here, host-side — no raw floats are ever serialized
//! into the QuickJS sandbox. Every function here is designed to be wrapped in
//! a fail-open caller (see `code_mode_host.rs::semantic_rank`); this module
//! itself returns ordinary `Result`s and does not implement the
//! cooldown/fail-open policy — that is the caller's responsibility.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use labby_runtime::error::ToolError;

/// Safe fallback for TEI's per-client `/embed` request limit when `/info` is
/// temporarily unavailable. The live limit is advertised as
/// `max_client_batch_size`; `max_batch_requests` is scheduler capacity and must
/// never be mistaken for the size of one HTTP request.
pub(crate) const TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE: usize = 128;

/// Preserve the previous 512-input work window without issuing an invalid
/// 512-input HTTP request: up to four server-compliant batches run concurrently.
pub(crate) const TEI_MAX_INPUT_WINDOW: usize = 512;
pub(crate) const TEI_MAX_PARALLEL_BATCHES: usize = 4;

/// Per-request timeout for one `POST /embed` call. Hardcoded, not
/// configurable — see the plan's YAGNI rationale (the one required knob is
/// `tei_url`; timeout/cooldown are engineering constants).
pub(crate) const TEI_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum accepted TEI response body size before JSON decoding. Guards
/// against a misbehaving or compromised TEI endpoint forcing unbounded
/// memory use.
pub(crate) const TEI_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Shared `reqwest::Client` for all TEI requests. `reqwest::Client` is
/// internally `Arc`-wrapped and holds a connection pool, so one process-wide
/// client reuses connections across `/embed` calls instead of paying a fresh
/// connector (and TLS handshake) per request. The per-request timeout stays
/// on the request builder ([`TEI_REQUEST_TIMEOUT`]).
static TEI_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    // See upstream/pool.rs::UpstreamPool::new for why this call is needed
    // under "rustls-no-provider" -- idempotent, safe to ignore Err.
    drop(rustls::crypto::ring::default_provider().install_default());
    reqwest::Client::new()
});

/// One Labby process has one configured TEI endpoint in normal operation, but
/// keying the cache by URL keeps tests and future multi-endpoint deployments
/// correct without repeating `/info` on every semantic-search call.
static TEI_BATCH_SIZE_CACHE: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Deserialize)]
struct TeiEmbedResponse(Vec<Vec<f32>>);

#[derive(Debug, Deserialize)]
struct TeiInfoResponse {
    max_client_batch_size: Option<usize>,
}

/// Batch-embed `texts` via one or more server-compliant
/// `POST {url}/embed` calls. The per-request limit is discovered from TEI's
/// `max_client_batch_size` field and cached per endpoint. Multiple batches run
/// concurrently within the existing 512-input work window, and `buffered`
/// preserves input order even when requests complete out of order.
pub(crate) async fn embed_via_tei(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let batch_size = resolve_tei_batch_size(url).await;
    let parallel_batches = TEI_MAX_INPUT_WINDOW
        .div_ceil(batch_size)
        .clamp(1, TEI_MAX_PARALLEL_BATCHES);
    let base_url = url.to_string();
    let owned_batches = texts
        .chunks(batch_size)
        .map(<[String]>::to_vec)
        .collect::<Vec<_>>();
    let batch_results = futures::stream::iter(owned_batches.into_iter().map(|chunk| {
        let base_url = base_url.clone();
        async move { embed_batch(&base_url, &chunk).await }
    }))
    .buffered(parallel_batches)
    .collect::<Vec<_>>()
    .await;
    let mut all_vectors = Vec::with_capacity(texts.len());
    for result in batch_results {
        all_vectors.extend(result?);
    }
    Ok(all_vectors)
}

async fn resolve_tei_batch_size(url: &str) -> usize {
    let base = url.trim_end_matches('/');
    if let Ok(cache) = TEI_BATCH_SIZE_CACHE.lock()
        && let Some(size) = cache.get(base)
    {
        return *size;
    }
    match fetch_tei_batch_size(base).await {
        Ok(discovered) => {
            if let Ok(mut cache) = TEI_BATCH_SIZE_CACHE.lock() {
                cache.insert(base.to_string(), discovered);
            }
            discovered
        }
        Err(_) => TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE,
    }
}

async fn fetch_tei_batch_size(base: &str) -> Result<usize, ToolError> {
    let endpoint = format!("{base}/info");
    let response = TEI_CLIENT
        .get(&endpoint)
        .timeout(TEI_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("TEI /info request failed: {err}"),
        })?;
    if !response.status().is_success() {
        return Err(ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message: format!("TEI /info returned HTTP {}", response.status()),
        });
    }
    let body = read_tei_body_capped(response).await?;
    let parsed: TeiInfoResponse = serde_json::from_slice(&body).map_err(|err| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("failed to decode TEI /info response: {err}"),
    })?;
    parsed
        .max_client_batch_size
        .filter(|size| *size > 0)
        .map(|size| size.min(TEI_MAX_INPUT_WINDOW))
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "decode_error".to_string(),
            message: "TEI /info omitted a valid max_client_batch_size".to_string(),
        })
}

async fn embed_batch(url: &str, texts: &[String]) -> Result<Vec<Vec<f32>>, ToolError> {
    let endpoint = format!("{}/embed", url.trim_end_matches('/'));
    let response = TEI_CLIENT
        .post(&endpoint)
        .timeout(TEI_REQUEST_TIMEOUT)
        .json(&json!({ "inputs": texts }))
        .send()
        .await
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("TEI request failed: {err}"),
        })?;
    if !response.status().is_success() {
        return Err(ToolError::Sdk {
            sdk_kind: "upstream_error".to_string(),
            message: format!("TEI returned HTTP {}", response.status()),
        });
    }
    let body = read_tei_body_capped(response).await?;
    let parsed: TeiEmbedResponse = serde_json::from_slice(&body).map_err(|err| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!("failed to decode TEI /embed response: {err}"),
    })?;
    if parsed.0.len() != texts.len() {
        return Err(ToolError::Sdk {
            sdk_kind: "decode_error".to_string(),
            message: format!(
                "TEI returned {} vectors for {} inputs",
                parsed.0.len(),
                texts.len()
            ),
        });
    }
    Ok(parsed.0)
}

/// Read a TEI response body into a `Vec<u8>` while actually bounding memory
/// at [`TEI_MAX_RESPONSE_BYTES`]: reject early on a declared `Content-Length`
/// over the cap, then count bytes as `bytes_stream()` yields chunks and abort
/// the moment the running total exceeds the cap — never buffering the whole
/// oversized body first. Mirrors `upstream::http_client::read_body_capped`.
///
/// The cap breach keeps the pre-existing `decode_error` kind, so callers'
/// fail-open handling (cooldown + empty result) is unchanged.
async fn read_tei_body_capped(response: reqwest::Response) -> Result<Vec<u8>, ToolError> {
    read_tei_body_capped_with_limit(response, TEI_MAX_RESPONSE_BYTES).await
}

async fn read_tei_body_capped_with_limit(
    response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, ToolError> {
    let too_large = |observed: usize| ToolError::Sdk {
        sdk_kind: "decode_error".to_string(),
        message: format!(
            "TEI response body is {observed} bytes, exceeding the {max_response_bytes} byte cap"
        ),
    };
    // Fast reject when a hostile/misbehaving endpoint declares the oversized
    // body up front.
    let declared = response.content_length();
    if let Some(cl) = declared
        && cl > max_response_bytes as u64
    {
        return Err(too_large(usize::try_from(cl).unwrap_or(usize::MAX)));
    }
    // Preallocate only when Content-Length is present and honest (≤ cap).
    let initial_cap = declared
        .map(|cl| cl.min(max_response_bytes as u64) as usize)
        .unwrap_or(0);
    let mut body: Vec<u8> = Vec::with_capacity(initial_cap);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ToolError::Sdk {
            sdk_kind: "network_error".to_string(),
            message: format!("failed to read TEI response body: {err}"),
        })?;
        let running_total = body.len().saturating_add(chunk.len());
        if running_total > max_response_bytes {
            return Err(too_large(running_total));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Cosine similarity between two equal-length vectors. Returns `0.0` for a
/// zero-magnitude vector (rather than dividing by zero / NaN) — this can
/// legitimately happen for a degenerate embedding and should score as "no
/// similarity", not poison the sort with NaN.
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    (dot / (mag_a * mag_b)).clamp(-1.0, 1.0)
}

/// Rank catalog entries by cosine similarity to `query_vector`. Returns
/// `(id, similarity)` pairs sorted descending by similarity — callers decide
/// how many to keep.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn rank_by_similarity(
    query_vector: &[f32],
    catalog_vectors: &[(String, Vec<f32>)],
) -> Vec<(String, f32)> {
    rank_top_k_by_similarity(query_vector, catalog_vectors, catalog_vectors.len())
}

/// Rank at most `top_k` catalog entries by cosine similarity without cloning
/// every vector id or fully sorting entries that will be discarded.
pub(crate) fn rank_top_k_by_similarity(
    query_vector: &[f32],
    catalog_vectors: &[(String, Vec<f32>)],
    top_k: usize,
) -> Vec<(String, f32)> {
    let limit = top_k.max(1).min(catalog_vectors.len());
    let mut scored: Vec<(&str, f32)> = catalog_vectors
        .iter()
        .map(|(id, vector)| (id.as_str(), cosine_similarity(query_vector, vector)))
        .collect();
    if scored.len() > limit {
        scored.select_nth_unstable_by(limit, |a, b| b.1.total_cmp(&a.1));
        scored.truncate(limit);
    }
    scored.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .into_iter()
        .map(|(id, score)| (id.to_string(), score))
        .collect()
}

#[cfg(test)]
// panic! in match arms below is a normal test-assertion idiom, not production
// code the lint is meant to guard.
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical_vectors_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        assert!((cosine_similarity(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite_vectors_is_negative_one() {
        let v = vec![1.0, 2.0, 3.0];
        let neg: Vec<f32> = v.iter().map(|x| -x).collect();
        assert!((cosine_similarity(&v, &neg) - -1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector_returns_zero_not_nan() {
        let result = cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]);
        // Exact by construction: the zero-magnitude guard clause returns a
        // literal 0.0, not an arithmetic result.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(result, 0.0);
        }
        assert!(!result.is_nan());
    }

    #[test]
    fn cosine_similarity_mismatched_lengths_returns_zero() {
        // Exact by construction: the length-mismatch guard clause returns a
        // literal 0.0, not an arithmetic result.
        #[allow(clippy::float_cmp)]
        {
            assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
        }
    }

    #[test]
    fn rank_by_similarity_sorts_descending() {
        let query = vec![1.0, 0.0];
        let catalog = vec![
            ("low".to_string(), vec![0.0, 1.0]),
            ("high".to_string(), vec![1.0, 0.0]),
            ("mid".to_string(), vec![0.7, 0.7]),
        ];
        let ranked = rank_by_similarity(&query, &catalog);
        assert_eq!(ranked[0].0, "high");
        assert_eq!(ranked[2].0, "low");
    }

    #[test]
    fn rank_top_k_by_similarity_keeps_only_best_matches() {
        let query = vec![1.0, 0.0];
        let catalog = vec![
            ("low".to_string(), vec![0.0, 1.0]),
            ("high".to_string(), vec![1.0, 0.0]),
            ("mid".to_string(), vec![0.7, 0.7]),
        ];

        let ranked = rank_top_k_by_similarity(&query, &catalog, 2);

        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, "high");
        assert_eq!(ranked[1].0, "mid");
    }

    #[tokio::test]
    async fn embed_via_tei_empty_input_returns_empty_without_http_call() {
        let result = embed_via_tei("http://127.0.0.1:1", &[]).await;
        assert_eq!(result.unwrap(), Vec::<Vec<f32>>::new());
    }

    #[tokio::test]
    async fn embed_via_tei_unreachable_server_returns_network_error() {
        // Port 1 is a reserved/unused low port — connection refused, fast.
        let result = embed_via_tei("http://127.0.0.1:1", &["test".to_string()]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn embed_via_tei_uses_client_limit_not_scheduler_capacity() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/info"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "max_client_batch_size": 128,
                    "max_batch_requests": 512
                })),
            )
            .expect(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/embed"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(vec![vec![1.0_f32]; 128]),
            )
            .expect(4)
            .mount(&server)
            .await;
        let texts = (0..TEI_MAX_INPUT_WINDOW)
            .map(|index| format!("input-{index}"))
            .collect::<Vec<_>>();

        let vectors = embed_via_tei(&server.uri(), &texts)
            .await
            .expect("server-compliant concurrent batches");

        assert_eq!(vectors.len(), TEI_MAX_INPUT_WINDOW);
        let requests = server.received_requests().await.expect("request recording");
        let embed_requests = requests
            .iter()
            .filter(|request| request.url.path() == "/embed")
            .collect::<Vec<_>>();
        assert_eq!(embed_requests.len(), TEI_MAX_PARALLEL_BATCHES);
        for request in embed_requests {
            let body: serde_json::Value =
                serde_json::from_slice(&request.body).expect("valid TEI request JSON");
            assert_eq!(
                body["inputs"].as_array().map(Vec::len),
                Some(TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE)
            );
        }
    }

    #[tokio::test]
    async fn embed_via_tei_falls_back_to_safe_client_limit_when_info_fails() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/info"))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/embed"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(vec![vec![1.0_f32]; TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE]),
            )
            .expect(1)
            .mount(&server)
            .await;
        let texts = (0..TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE)
            .map(|index| format!("fallback-{index}"))
            .collect::<Vec<_>>();

        let vectors = embed_via_tei(&server.uri(), &texts)
            .await
            .expect("fallback batch remains valid");

        assert_eq!(vectors.len(), TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE);
    }

    #[tokio::test]
    async fn embed_via_tei_oversized_declared_body_is_rejected() {
        // A body over TEI_MAX_RESPONSE_BYTES with an honest Content-Length is
        // rejected via the header pre-check — before buffering anything —
        // with the same decode_error kind callers already fail open on.
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/embed"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(vec![
                b'0';
                TEI_MAX_RESPONSE_BYTES
                    + 1
            ]))
            .mount(&server)
            .await;
        let err = embed_via_tei(&server.uri(), &["x".to_string()])
            .await
            .expect_err("over-cap response must be rejected");
        match err {
            ToolError::Sdk { sdk_kind, message } => {
                assert_eq!(sdk_kind, "decode_error");
                assert!(
                    message.contains("byte cap"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected decode_error cap breach, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_tei_body_streaming_over_cap_aborts_mid_stream() {
        // Exercise the streaming memory cap independently from the production
        // request timeout. A small test-only cap keeps this deterministic even
        // when the CI host is heavily loaded.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        const TEST_CAP: usize = 64 * 1024;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 4096];
            drop(socket.read(&mut request).await);
            const CRLF: &[u8] = &[13, 10];
            const END_HEADERS: &[u8] = &[13, 10, 13, 10];
            let response_head = [
                b"HTTP/1.1 200 OK".as_slice(),
                CRLF,
                b"Content-Type: application/json".as_slice(),
                CRLF,
                b"Connection: close".as_slice(),
                END_HEADERS,
            ]
            .concat();
            if socket.write_all(&response_head).await.is_err() {
                return;
            }
            let body = vec![b'1'; TEST_CAP * 2];
            drop(socket.write_all(&body).await);
            drop(socket.shutdown().await);
        });

        let response = TEI_CLIENT
            .post(format!("http://{addr}/embed"))
            .json(&json!({"inputs": ["x"]}))
            .send()
            .await
            .expect("test response");
        let err = read_tei_body_capped_with_limit(response, TEST_CAP)
            .await
            .expect_err("over-cap streamed response must be rejected");
        match err {
            ToolError::Sdk { sdk_kind, message } => {
                assert_eq!(sdk_kind, "decode_error");
                assert!(
                    message.contains("byte cap"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected decode_error cap breach, got {other:?}"),
        }
    }

    #[test]
    fn tei_parallel_batches_preserve_the_512_input_work_window() {
        assert_eq!(
            TEI_FALLBACK_MAX_CLIENT_BATCH_SIZE * TEI_MAX_PARALLEL_BATCHES,
            TEI_MAX_INPUT_WINDOW
        );
    }
}
