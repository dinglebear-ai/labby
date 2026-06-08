# Axon Fanout Smoke

## Topic
Axum request timeout middleware

## Search
```json
{
  "ok": true,
  "action": "search",
  "subaction": "search",
  "warnings": [
    "outdated axon binary: src is newer than the running executable. Rebuild with `cargo build --bin axon` or `cargo build --release --bin axon`; the Cargo wrapper should copy the fresh binary to ~/.local/bin/axon and plugins/axon/bin/axon."
  ],
  "data": {
    "data": {
      "auto_crawl_status": "queued",
      "crawl_jobs": [
        {
          "job_id": "eb88ac47-c936-4778-b0b6-bc04a5ac83f3",
          "url": "https://stackoverflow.com/questions/73758789/how-to-set-http-timeouts-using-axum-based-on-hyper"
        },
        {
          "job_id": "6f403dd5-78d2-4bef-a9c1-016c80a9e3ad",
          "url": "https://docs.rs/axum/latest/axum/middleware/index.html"
        },
        {
          "job_id": "f2d4a2bc-af81-42ff-86cd-697ba9c872e4",
          "url": "https://github.com/tokio-rs/axum/discussions/1383"
        }
      ],
      "crawl_jobs_rejected": [],
      "limit": 3,
      "offset": 0,
      "query": "Axum request timeout middleware TimeoutLayer HandleErrorLayer",
      "results": [
        {
          "position": 1,
          "snippet": "Client As well as @timeout 600 if you use the http client of IntelliJ ... Without this attribute you will get io.netty.handler.timeout.ReadTimeoutException on RustRover Test You can test it with the following function: ... (Replace with your middleware Error) I hope that it helps.",
          "title": "How to set http timeouts using axum (based on hyper)?",
          "url": "https://stackoverflow.com/questions/73758789/how-to-set-http-timeouts-using-axum-based-on-hyper"
        },
        {
          "position": 2,
          "snippet": "Commonly used middleware Some commonly used middleware are: TraceLayer for high level tracing/logging. CorsLayer for handling CORS. CompressionLayer for automatic compression of responses. RequestIdLayer and PropagateRequestIdLayer set and propagate request ids. TimeoutLayer for timeouts. Ordering When you add middleware with Router::layer (or similar) all previously added routes will be ...",
          "title": "axum::middleware - Rust - Docs.rs",
          "url": "https://docs.rs/axum/latest/axum/middleware/index.html"
        },
        {
          "position": 3,
          "snippet": "Sep 17, 2022 ... It's an overall timeout around the whole request. If you want more low level control I think you have to make your own http_body::Body .",
          "title": "How to set timeouts? · tokio-rs axum · Discussion #1383 - GitHub",
          "url": "https://github.com/tokio-rs/axum/discussions/1383"
        }
      ]
    },
    "response_mode": "auto-inline"
  }
}
```

## Ask
```json
{
  "ok": true,
  "action": "ask",
  "subaction": "ask",
  "warnings": [
    "outdated axon binary: src is newer than the running executable. Rebuild with `cargo build --bin axon` or `cargo build --release --bin axon`; the Cargo wrapper should copy the fresh binary to ~/.local/bin/axon and plugins/axon/bin/axon."
  ],
  "data": {
    "artifact": {
      "artifact_handle": {
        "bytes": 1362,
        "display_path": "/home/jmagar/.axon/artifacts/code-mode-artifacts/ask/how-should-an-axum-service-implement-request-timeouts.json",
        "job_id": null,
        "kind": "json",
        "line_count": 17,
        "relative_path": "ask/how-should-an-axum-service-implement-request-timeouts.json",
        "url": null
      },
      "bytes": 1362,
      "display_path": "/home/jmagar/.axon/artifacts/code-mode-artifacts/ask/how-should-an-axum-service-implement-request-timeouts.json",
      "kind": "json",
      "line_count": 17,
      "path": "ask/how-should-an-axum-service-implement-request-timeouts.json",
      "relative_path": "ask/how-should-an-axum-service-implement-request-timeouts.json",
      "sha256": "c14337acb8420bfcddb8d434752431b0bce9890004d38fcf7741db42b09c3b5e"
    },
    "artifact_handle": {
      "bytes": 1362,
      "display_path": "/home/jmagar/.axon/artifacts/code-mode-artifacts/ask/how-should-an-axum-service-implement-request-timeouts.json",
      "job_id": null,
      "kind": "json",
      "line_count": 17,
      "relative_path": "ask/how-should-an-axum-service-implement-request-timeouts.json",
      "url": null
    },
    "key_fields": {
      "answer": "Axum’s docs point to `tower-http`’s `TimeoutLayer` for request timeouts, and the example shows layering `TimeoutLayer::new(Duration::from_secs(10))` onto the app/router. [S2][S1]\n\nIn that example, timeout errors are mapped to `StatusCode::REQUEST_TIMEOUT`, and the docs note that `error_handling` covers more of axum’s error-handling model. [S1]\n\nGaps: The sources do not describe server-level read/write timeout configuration or compare alternative timeout strategies. They also do not explain the exact timeout error type or how to customize the response beyond returning `REQUEST_TIMEOUT`.\n\n## Sources\n- [S1] https://github.com/jmagar/rust-bin/blob/main/docs/references/axum-llms.txt#L3223-L3272\n- [S2] https://github.com/tokio-rs/axum/discussions/1383"
    },
    "response_mode": "path",
    "shape": {
      "answer": "<string 755>",
      "diagnostics": null,
      "explain": null,
      "query": "How should an Axum service implement request timeouts?",
      "timing_ms": {
        "context_build": 31,
        "llm": 15257,
        "llm_ttft_ms": 22485,
        "retrieval": 7454,
        "streamed": true,
        "total": 22809
      },
      "warnings": {
        "sample": [
          "<string 279>"
        ],
        "total": 1
      }
    }
  }
}
```

## Follow-Up Code Mode Snippet
```js
async () => {
  return await callTool("axon::axon", { action: "ask", query: "What timeout errors must Axum map to 408?" });
}
```