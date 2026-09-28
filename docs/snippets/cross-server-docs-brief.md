---
name: cross-server-docs-brief
title: "Cross-Server Docs Brief"
created: "2026-07-30"
updated: "2026-09-16"
description: Build a compact docs brief from Context7, web search, GitHub, Axon, and time
tags: [docs, research, cross-server]
inputs:
  topic:
    type: string
    default: Model Context Protocol Rust SDK
    required: false
    description: Main research topic
  library_name:
    type: string
    default: Tokio
    required: false
    description: Context7 library search name
  library_id:
    type: string
    default: /websites/rs_tokio_tokio
    required: false
    description: Concrete Context7 library id
  max_results:
    type: integer
    default: 3
    required: false
    description: Per-source result limit where the tool supports one
---

# Cross-Server Docs Brief

Use this snippet for a compact documentation brief from several independent
sources. The contracts below were rediscovered and smoke-tested through Labby on
2026-09-16.

## Current Tool Contracts

| Step | Tool | Current parameters |
| --- | --- | --- |
| Timestamp | `time::get_current_time` | `timezone` |
| Library discovery | `context7::resolve-library-id` | `libraryName`, `query` |
| Library docs | `context7::query-docs` | `libraryId`, `query` |
| Web search | `searxng::searxng_web_search` | `query`, `num_results`, optional response controls |
| Cloudflare docs | `cloudflare-docs::search_cloudflare_documentation` | `query` only |
| GitHub repos | `github::search_repositories` | `query`, `perPage`, optional projection fields |
| Axon search | `Axon::axon` | `action: "search"`, `query`, `limit` |

Older versions of this snippet used removed parameters such as Context7
`tokens`, SearXNG `count`, Cloudflare `limit`, and the lowercase
`axon::axon` tool id. Those shapes are not current.

The calls are independent, so the snippet uses `codemode.batch`. Each call also
returns its own timing/error envelope, allowing the brief to degrade without
throwing away healthy evidence.

## Input Notes

- `topic` drives the web and Axon searches.
- `library_name` is used for Context7 discovery.
- `library_id` is the exact Context7 id for the documentation query. Keep it in
  sync with the selected library; discovery does not automatically mutate the
  explicit query target.
- `max_results` bounds tools that expose a limit. Cloudflare's current search
  schema has no caller-provided result limit, so the snippet slices its returned
  results locally.

Run with:

```bash
labby code run --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/cross-server-docs-brief.md)"
```

```js
async (overrides = {}) => {
  const input = {
    topic: overrides.topic ?? "Model Context Protocol Rust SDK",
    libraryName: overrides.library_name ?? "Tokio",
    libraryId: overrides.library_id ?? "/websites/rs_tokio_tokio",
    libraryQuestion: overrides.library_question ?? "spawn blocking task",
    cloudflareQuery: overrides.cloudflare_query ?? "workers durable objects",
    githubRepoQuery: overrides.github_repo_query ?? "modelcontextprotocol rust sdk",
    maxResults: overrides.max_results ?? 3
  };

  const preview = (value, limit = 1200) => {
    const text = typeof value === "string" ? value : JSON.stringify(value);
    return text.length > limit ? `${text.slice(0, limit)}...` : text;
  };

  const timed = async (label, id, params, transform = (value) => value) => {
    const started = Date.now();
    try {
      const result = await callTool(id, params);
      return {
        label,
        id,
        ok: true,
        ms: Date.now() - started,
        result: transform(result)
      };
    } catch (error) {
      return {
        label,
        id,
        ok: false,
        ms: Date.now() - started,
        error: String(error)
      };
    }
  };

  const jobs = [
    () => timed("timestamp", "time::get_current_time", { timezone: "America/New_York" }),
    () => timed(
      "context7_library_candidates",
      "context7::resolve-library-id",
      { libraryName: input.libraryName, query: input.libraryQuestion },
      (result) => preview(result)
    ),
    () => timed(
      "context7_docs",
      "context7::query-docs",
      { libraryId: input.libraryId, query: input.libraryQuestion },
      (result) => preview(result)
    ),
    () => timed(
      "searxng_web",
      "searxng::searxng_web_search",
      {
        query: input.topic,
        num_results: input.maxResults,
        response_format: "json",
        result_detail: "compact"
      },
      (result) => preview(result)
    ),
    () => timed(
      "cloudflare_docs",
      "cloudflare-docs::search_cloudflare_documentation",
      { query: input.cloudflareQuery },
      (result) => ({
        results: (result.results || []).slice(0, input.maxResults).map((item) => ({
          title: item.title,
          url: item.url,
          similarity: item.similarity,
          text: preview(item.text, 500)
        }))
      })
    ),
    () => timed(
      "github_repositories",
      "github::search_repositories",
      {
        query: input.githubRepoQuery,
        perPage: input.maxResults,
        minimal_output: true
      },
      (result) => ({
        total_count: result.total_count,
        repositories: (result.items || []).slice(0, input.maxResults).map((repo) => ({
          full_name: repo.full_name,
          description: repo.description,
          language: repo.language,
          stars: repo.stargazers_count,
          url: repo.html_url
        }))
      })
    ),
    () => timed(
      "axon_search",
      "Axon::axon",
      { action: "search", query: input.topic, limit: input.maxResults },
      (result) => preview(result)
    )
  ];

  const batch = await codemode.batch(jobs);
  const calls = batch.ok
    .sort((a, b) => a.i - b.i)
    .map((entry) => entry.value);
  calls.push(...batch.failed.map((entry) => ({
    label: `batch_job_${entry.i}`,
    id: "codemode.batch",
    ok: false,
    error: String(entry.error)
  })));

  const requiredLabels = new Set([
    "timestamp",
    "context7_library_candidates",
    "context7_docs",
    "cloudflare_docs",
    "github_repositories",
    "axon_search"
  ]);
  const requiredOk = calls
    .filter((call) => requiredLabels.has(call.label))
    .every((call) => call.ok);
  const degraded = calls
    .filter((call) => !call.ok)
    .map((call) => ({ label: call.label, id: call.id, error: call.error }));

  return {
    snippet: "cross_server_docs_brief",
    input,
    ok: requiredOk,
    status: degraded.length ? "degraded" : "ok",
    degraded,
    calls,
    notes: [
      "Context7 query_docs needs a concrete libraryId; update input.libraryId when changing libraryName.",
      "SearXNG is optional for overall success because public search availability can vary by instance."
    ]
  };
}
```
