# Axon Research Brief Smoke Output

Generated from a real Labby Code Mode CLI run using:

```bash
labby gateway code exec --json --code 'async () => { ... }'
```

The snippet called Axon through Code Mode with:

```js
callTool("axon::axon", args)
```

## Input

```json
{
  "topic": "adding request timeout middleware to an Axum Rust server"
}
```

## Result

The snippet completed successfully through Labby Code Mode. It ran `help`, then fanned out `search`, `query`, and `research`, then selected two evidence URLs and ran `scrape` plus `summarize` for each.

## Selected Sources

| Source | Reason |
| --- | --- |
| `https://docs.rs/axum/latest/axum/middleware/index.html` | `research: axum::middleware - Rust - Docs.rs` |
| `https://github.com/tokio-rs/axum/discussions/1383` | `research: How to set timeouts? · tokio-rs axum · Discussion #1383 - GitHub` |

## Research Summary

For Axum, request timeouts are normally implemented as Tower middleware rather than as an Axum-specific server setting. The recommended path is `tower_http::timeout::TimeoutLayer`, usually applied to a `Router` with `tower::ServiceBuilder`.

If the application needs a clean HTTP response when a request times out, pair the timeout layer with an error handler such as `HandleErrorLayer` and map timeout errors to `408 Request Timeout` or an application-specific error response.

Axum middleware can be applied to a whole router, a route group, or individual handlers. Layer ordering matters because Tower layers wrap inward; place the timeout where it should apply in the stack.

The timeout is an overall request timeout. It is not the same as lower-level socket, read, write, or header-read timeouts. The Axum discussion also notes that timeout behavior depends on async polling: if a handler completes synchronously and never yields, the timeout future may not get a chance to fire.

## Example Shape

```rust
use axum::{routing::get, Router};
use std::time::Duration;
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

let app = Router::new()
    .route("/", get(handler))
    .layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(TimeoutLayer::new(Duration::from_secs(30))),
    );
```

## Evidence Summaries

### docs.rs Axum Middleware

Artifact: `summarize/https-docs-rs-axum-latest-axum-middleware-index-html.json`

The docs.rs Axum middleware page is the Axum 0.8.9 API reference for middleware, extractors, routing, handlers, and response types. It lists middleware helpers such as `from_fn`, `from_extractor`, `map_request`, `map_response`, and state-aware variants. It also covers router APIs and service adapters used to turn handlers into Tower services.

### tokio-rs/axum Discussion 1383

Artifact: `summarize/https-github-com-tokio-rs-axum-discussions-1383.json`

The GitHub Q&A discusses how to add timeouts in Axum. The main answer points to `tower-http` timeout middleware. Follow-up comments clarify that the timeout applies to the whole request, and a shared example uses `ServiceBuilder`, `HandleErrorLayer`, and timeout middleware to map timeout errors to `408 REQUEST_TIMEOUT`. Later comments distinguish this from timing out slow clients while they are still sending headers, which is a lower-level Hyper concern.

## Timings

| Call | Status | Time |
| --- | --- | ---: |
| `help` | ok | 279ms |
| `search` | ok | 1,974ms |
| `query` | ok | 8,896ms |
| `research` | ok | 21,039ms |
| `evidence.1.scrape` | ok | 534ms |
| `evidence.1.summarize` | ok | 8,250ms |
| `evidence.2.scrape` | ok | 261ms |
| `evidence.2.summarize` | ok | 3,821ms |

## Code Mode Calls

All calls used `axon::axon`.

| Call | Status | Time |
| --- | --- | ---: |
| `axon::axon` | ok | 279ms |
| `axon::axon` | ok | 1,972ms |
| `axon::axon` | ok | 8,893ms |
| `axon::axon` | ok | 21,036ms |
| `axon::axon` | ok | 532ms |
| `axon::axon` | ok | 8,248ms |
| `axon::axon` | ok | 259ms |
| `axon::axon` | ok | 3,819ms |

## Notes

The first live run exposed a snippet bug: Axon path-mode `shape` can contain placeholder strings like `<string 126>`, and the source picker tried to treat one as a URL. The reusable snippet now filters candidate URLs to real `http(s)` URLs before running evidence calls.

During proxy generation Labby warned that `cortex` auth failed and omitted it from the generated `codemode` proxy. That did not affect the Axon calls because the snippet used direct `callTool("axon::axon", args)`.
