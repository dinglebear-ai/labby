# Axon Research Brief Output: Axum Request Timeout Middleware

Topic: `adding request timeout middleware to an Axum Rust server`

Focus: official Axum/Tower/Tower HTTP APIs, concrete Rust code, timeout layer behavior, error handling

Workflow: `axon_research_brief`

Runtime: `46.7s total`

## Answer

For Axum request timeouts, use Tower/Tower HTTP middleware rather than an Axum-specific timeout API. The most directly relevant source was the Tokio/Axum discussion on setting timeouts, which points to Tower HTTP timeout middleware for request-level timeouts. The practical pattern is to build a middleware stack with `ServiceBuilder`, add `TimeoutLayer`, and use `HandleErrorLayer` to convert timeout errors into an HTTP response such as `408 Request Timeout`.

This is a request-processing timeout, not a Go-style socket/header/read/write timeout. It wraps the async request handler/service. If a handler does not yield or await, the timeout may not fire because the future is not being polled in a way that lets the timeout complete.

## Implementation Recipe

1. Add Tower/Tower HTTP timeout support.

```toml
[dependencies]
axum = "0.8"
tokio = { version = "1", features = ["full"] }
tower = { version = "0.5", features = ["timeout"] }
tower-http = { version = "0.6", features = ["timeout"] }
```

2. Build your router normally.

```rust
use axum::{routing::get, Router};

async fn handler() -> &'static str {
    "ok"
}

let app = Router::new().route("/", get(handler));
```

3. Wrap the router in a Tower middleware stack.

Use `ServiceBuilder` when combining multiple layers. It keeps middleware order explicit and easier to reason about.

```rust
use axum::{
    error_handling::HandleErrorLayer,
    http::StatusCode,
    routing::get,
    Router,
};
use std::{error::Error, time::Duration};
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;

async fn handler() -> &'static str {
    "ok"
}

let app = Router::new()
    .route("/", get(handler))
    .layer(
        ServiceBuilder::new()
            .layer(HandleErrorLayer::new(
                |err: Box<dyn Error + Send + Sync>| async move {
                    if err.is::<tower::timeout::error::Elapsed>() {
                        StatusCode::REQUEST_TIMEOUT
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                },
            ))
            .layer(TimeoutLayer::new(Duration::from_secs(10))),
    );
```

4. Return `408 Request Timeout` for elapsed timeout errors.

The Axum discussion examples use `HandleErrorLayer` and check for `tower::timeout::error::Elapsed`. That maps timeout middleware errors into a normal HTTP response instead of letting middleware errors leak through unhandled.

5. Distinguish request timeout from server-level connection timeouts.

The selected sources distinguish request execution timeout from lower-level header-read, socket read, or write timeouts. For header-read/client-request-head timeouts, the Axum discussion points toward Hyper rather than Axum middleware.

## Minimal Shape

```rust
use axum::{
    error_handling::HandleErrorLayer,
    http::StatusCode,
    routing::get,
    Router,
};
use std::{error::Error, time::Duration};
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;

async fn slow_handler() -> &'static str {
    tokio::time::sleep(Duration::from_secs(30)).await;
    "finished"
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(slow_handler))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(
                    |err: Box<dyn Error + Send + Sync>| async move {
                        if err.is::<tower::timeout::error::Elapsed>() {
                            StatusCode::REQUEST_TIMEOUT
                        } else {
                            StatusCode::INTERNAL_SERVER_ERROR
                        }
                    },
                ))
                .layer(TimeoutLayer::new(Duration::from_secs(10))),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    axum::serve(listener, app).await.unwrap();
}
```

This code shape is inferred from the selected summaries. Before using it verbatim, verify exact crate versions and feature flags against your project.

## Evidence Table

| Source | Why It Matters | Concrete Facts |
| --- | --- | --- |
| `https://github.com/tokio-rs/axum/discussions/1383` | Most direct match for the question. It asks how to set timeouts in Axum. | Main answer recommends Tower HTTP timeout middleware. Replies show `ServiceBuilder`, `HandleErrorLayer`, `TimeoutLayer`, checking `tower::timeout::error::Elapsed`, and returning `408 Request Timeout`. It clarifies this is request-level timeout, not server socket/read/write timeout. It also notes the timeout needs async polling and may not fire if the handler never awaits. |
| `https://docs.rs/axum/latest/axum/middleware/index.html` | Official Axum API reference for middleware-related APIs. | Lists middleware helpers and related layer/service types such as `from_fn`, `from_fn_with_state`, `from_extractor`, `map_request`, `map_response`, and response helpers. It confirms Axum’s middleware model is layer-based, though this page is broad and not timeout-specific. |
| `https://www.rustfaq.org/en/how-to-use-tower-middleware-with-axum/` | Practical explanation of Tower middleware with Axum. | Explains middleware as layers wrapping a router, requests flowing inward and responses outward. Recommends `Router::layer` for one layer, `ServiceBuilder` for multiple layers, and `route_layer` for route-specific middleware. Notes that middleware order matters. |
| `https://github.com/tokio-rs/axum` | Axum project overview. | Axum is built on Hyper and the Tower ecosystem. The README highlights Tower/Tower HTTP ecosystem integration for middleware like timeouts, tracing, compression, and auth. Notes `0.8.x` is current on crates.io while `main` may contain breaking changes toward `0.9`. |

## Selected Sources

The snippet selected these sources after applying source-fit scoring:

1. `https://docs.rs/axum/latest/axum/middleware/index.html`
2. `https://github.com/tokio-rs/axum/discussions/1383`
3. `https://www.rustfaq.org/en/how-to-use-tower-middleware-with-axum/`
4. `https://github.com/tokio-rs/axum`

Top scored sources from the run:

| Score | URL | Reason |
| ---: | --- | --- |
| 110 | `https://docs.rs/axum/latest/axum/middleware/index.html` | Search result: `axum::middleware - Rust - Docs.rs` |
| 95 | `https://github.com/tokio-rs/axum/discussions/1383` | Search result: `How to set timeouts?` |
| 92 | `https://www.rustfaq.org/en/how-to-use-tower-middleware-with-axum/` | Search result: Tower middleware with Axum |
| 86 | `https://github.com/tokio-rs/axum` | Search result: Axum repository |
| 72 | `https://www.reddit.com/r/rust/comments/183inqf/unable_to_apply_middleware_with_axum_shared_state/` | Search result: Reddit thread |
| 60 | `https://stackoverflow.com/questions/73758789/how-to-set-http-timeouts-using-axum-based-on-hyper` | Search result: StackOverflow timeout question |

## Gaps And Risks

- The selected Axum docs.rs page is broad. It is useful for middleware context but not timeout-specific.
- The best exact source was the Axum discussion, not an official API page for `TimeoutLayer`.
- The workflow did not surface a `docs.rs/tower-http/.../timeout` page. That is likely a discovery/query issue, not a summarization issue.
- The code sample above is inferred from summaries and should be verified against the exact `tower`, `tower-http`, and `axum` versions in use.
- Timeout middleware is request-processing timeout. It should not be confused with lower-level Hyper connection/header/socket timeouts.

## Follow-Up Calls

These follow-up Axon calls would reduce uncertainty:

```json
{ "action": "search", "query": "tower_http TimeoutLayer axum HandleErrorLayer Elapsed" }
```

```json
{ "action": "search", "query": "docs.rs tower-http timeout TimeoutLayer" }
```

```json
{ "action": "scrape", "url": "https://docs.rs/tower-http/latest/tower_http/timeout/index.html" }
```

```json
{ "action": "summarize", "url": "https://docs.rs/tower-http/latest/tower_http/timeout/index.html" }
```

### Follow-Up Code Mode Snippet

Paste this into Labby Code Mode `execute` to gather the missing Tower HTTP timeout evidence and return a compact bundle of selected sources, summaries, and timings.

```js
async () => {
  const input = {
    topic: "tower_http TimeoutLayer axum HandleErrorLayer Elapsed",
    knownUrls: [
      "https://docs.rs/tower-http/latest/tower_http/timeout/index.html",
      "https://docs.rs/tower/latest/tower/timeout/index.html",
      "https://docs.rs/tower-http/latest/tower_http/timeout/struct.TimeoutLayer.html",
      "https://docs.rs/tower/latest/tower/timeout/error/struct.Elapsed.html"
    ]
  };

  const axon = (args) => callTool("axon::axon", args);

  const parseTool = (result) => {
    const text = result?.content?.[0]?.text;
    if (typeof text !== "string") return result;
    try {
      return JSON.parse(text);
    } catch {
      return { raw: text };
    }
  };

  const timed = async (label, args) => {
    const started = Date.now();
    try {
      const result = await axon(args);
      const parsed = parseTool(result);
      return {
        label,
        ok: parsed?.ok ?? !result?.isError,
        ms: Date.now() - started,
        args,
        key_fields: parsed?.data?.key_fields,
        shape: parsed?.data?.shape,
        artifact:
          parsed?.data?.artifact?.path ??
          parsed?.data?.artifact_handle?.path ??
          parsed?.artifact?.path,
        data: parsed?.data?.data,
        error: parsed?.error?.message
      };
    } catch (error) {
      return {
        label,
        ok: false,
        ms: Date.now() - started,
        args,
        error: String(error)
      };
    }
  };

  const discoveryCalls = [
    ["search.timeoutlayer", { action: "search", query: input.topic }],
    ["search.docsrs", { action: "search", query: "docs.rs tower-http timeout TimeoutLayer" }],
    ["query.indexed", { action: "query", query: input.topic }]
  ];

  const started = Date.now();
  const discovery = await Promise.all(
    discoveryCalls.map(([label, args]) => timed(label, args))
  );

  const candidates = [];
  const addCandidate = (url, reason) => {
    if (!url || candidates.some((candidate) => candidate.url === url)) return;
    candidates.push({ url, reason });
  };

  for (const url of input.knownUrls) {
    addCandidate(url, "known high-value follow-up URL");
  }

  for (const result of discovery) {
    const searchSamples = result.shape?.search_results?.sample ?? result.data?.results ?? [];
    for (const item of searchSamples) {
      addCandidate(item.url, `${result.label}: ${item.title ?? item.source ?? "untitled"}`);
    }

    const querySamples = result.shape?.results?.sample ?? [];
    for (const item of querySamples) {
      addCandidate(item.url ?? item.source, `${result.label}: indexed match`);
    }
  }

  const queryTerms = new Set(
    input.topic
      .toLowerCase()
      .split(/[^a-z0-9_:-]+/)
      .filter((term) => term.length > 3)
  );

  const scoreCandidate = (candidate) => {
    const url = candidate.url.toLowerCase();
    const reason = candidate.reason.toLowerCase();
    let score = 0;

    if (url.includes("docs.rs/tower-http")) score += 120;
    if (url.includes("docs.rs/tower/")) score += 110;
    if (url.includes("struct.timeoutlayer")) score += 80;
    if (url.includes("struct.elapsed")) score += 70;
    if (url.includes("timeout")) score += 45;
    if (url.includes("github.com/tokio-rs/axum/discussions/1383")) score += 60;

    for (const term of queryTerms) {
      if (url.includes(term)) score += 12;
      if (reason.includes(term)) score += 8;
    }

    if (/github\.com\/jmagar\//.test(url)) score -= 80;
    if (url.includes("/docs/references/")) score -= 60;
    if (url.includes(".txt#l")) score -= 50;
    if (url.includes("llms.txt")) score -= 50;
    if (url.includes("blog") || url.includes("medium.com")) score -= 20;

    return score;
  };

  const selected = candidates
    .map((candidate) => ({ ...candidate, score: scoreCandidate(candidate) }))
    .sort((a, b) => b.score - a.score)
    .slice(0, 5);

  const evidenceCalls = selected.flatMap((candidate, index) => [
    [`evidence.${index + 1}.scrape`, { action: "scrape", url: candidate.url }],
    [`evidence.${index + 1}.summarize`, { action: "summarize", url: candidate.url }]
  ]);

  const evidence = await Promise.all(
    evidenceCalls.map(([label, args]) => timed(label, args))
  );

  return {
    workflow: "axon_research_brief_followup",
    total_ms: Date.now() - started,
    input,
    selected_sources: selected,
    timings: [...discovery, ...evidence].map((result) => ({
      label: result.label,
      ok: result.ok,
      ms: result.ms,
      artifact: result.artifact,
      error: result.error
    })),
    summaries: evidence
      .filter((result) => result.ok && result.key_fields?.summary)
      .map((result) => ({
        label: result.label,
        url: result.args.url,
        summary: result.key_fields.summary
      }))
  };
}
```

## Timings

| Call | Status | Time | Artifact |
| --- | --- | ---: | --- |
| `search` | ok | `1.476s` | |
| `research` | ok | `20.512s` | `research/adding-request-timeout-middleware-to-an-axum-rust-server.json` |
| `query` | ok | `4.714s` | `query/adding-request-timeout-middleware-to-an-axum-rust-server.json` |
| `evidence.1.scrape` | ok | `0.277s` | `scrape/https-docs-rs-axum-latest-axum-middleware-index-html.json` |
| `evidence.1.summarize` | ok | `21.707s` | `summarize/https-docs-rs-axum-latest-axum-middleware-index-html.json` |
| `evidence.2.scrape` | ok | `0.148s` | `scrape/https-github-com-tokio-rs-axum-discussions-1383.json` |
| `evidence.2.summarize` | ok | `17.694s` | `summarize/https-github-com-tokio-rs-axum-discussions-1383.json` |
| `evidence.3.scrape` | ok | `0.025s` | `scrape/https-www-rustfaq-org-en-how-to-use-tower-middleware-wit.json` |
| `evidence.3.summarize` | ok | `3.504s` | `summarize/https-www-rustfaq-org-en-how-to-use-tower-middleware-wit.json` |
| `evidence.4.scrape` | ok | `0.377s` | `scrape/https-github-com-tokio-rs-axum.json` |
| `evidence.4.summarize` | ok | `26.136s` | `summarize/https-github-com-tokio-rs-axum.json` |

Total workflow time: `46.654s`
