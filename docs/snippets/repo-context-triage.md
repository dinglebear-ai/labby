---
name: repo-context-triage
title: "Repository Context Triage"
created: "2026-07-30"
updated: "2026-09-16"
description: Quick repository context pass using filesystem, Octocode, GitHub, and time
tags: [repo, triage, research]
inputs:
  repo_path:
    type: string
    required: false
    description: Optional absolute repository path visible to both the filesystem and Octocode upstreams; local evidence is skipped when omitted
  owner:
    type: string
    default: dinglebear-ai
    required: false
    description: GitHub owner
  repo:
    type: string
    default: labby
    required: false
    description: GitHub repository
  topic:
    type: string
    default: Code Mode
    required: false
    description: Search topic
  max_results:
    type: integer
    default: 5
    required: false
    description: Per-source result limit
---

# Repo Context Triage

Use this snippet for a quick evidence pass over one repository topic. GitHub issue
search, a canonical remote file read, and a timestamp always run. When `repo_path`
is supplied and visible to both local upstreams, the snippet also adds one local
documentation read and an Octocode lexical search. The retired Lumen semantic-search
upstream is intentionally not part of the workflow.

## Tutorial: How This Snippet Is Built

Each tool answers a different evidence question:

| Step | Tool | Why it is included | Parameters the user fills |
|---|---|---|---|
| Timestamp | `time::get_current_time` | Records when the triage pass ran | `timezone` |
| Local doc (optional) | `filesystem::read_text_file` | Reads the local snippet docs when `repo_path` is supplied | `path` |
| Local code search (optional) | `octocode::localSearch` | Finds lexical code/text matches when the same checkout is visible to Octocode | `queries[].path`, `queries[].searchText`, `pageSize` |
| GitHub issues | `github::search_issues` | Finds remote issue context | `owner`, `repo`, `query`, `perPage` |
| GitHub file | `github::get_file_contents` | Compares or retrieves a canonical remote file | `owner`, `repo`, `path` |

The calls are independent, so they run through `codemode.batch`. Each job catches
and returns its own failure so one degraded upstream does not discard the rest of
the evidence.

## Why The Inputs Exist

- `repo_path` optionally tells the filesystem and Octocode upstreams which shared local checkout to inspect. There is deliberately no machine-specific default.
- `owner` and `repo` scope GitHub calls without embedding a search qualifier into the natural-language issue query.
- `topic` becomes the local-code and issue-search target.
- `max_results` bounds Octocode and GitHub output.

The remote snippets README remains the fixed GitHub workflow default. Local stages
run only when `repo_path` is supplied; omitting it yields a portable GitHub/time
triage instead of a degraded run against a path that may not exist on the gateway host.

## What Validation Should Catch

The builder should validate both simple and nested schemas:

- `filesystem::read_text_file.path` must be a string.
- `octocode::localSearch.queries` must be an array whose text-search entries include `operation: "text"`, absolute `path`, and `searchText`.
- `github::search_issues.perPage` must be an integer.
- `github::get_file_contents.owner`, `repo`, and `path` must be strings.

Live tool contracts reverified before this update:

- `time::get_current_time`
- `filesystem::read_text_file`
- `octocode::localSearch`
- `github::search_issues`
- `github::get_file_contents`

Run with:

```bash
labby code run --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/repo-context-triage.md)"
```

```js
async (overrides = {}) => {
  const input = {
    repoPath: overrides.repo_path ?? null,
    owner: overrides.owner ?? "dinglebear-ai",
    repo: overrides.repo ?? "labby",
    topic: overrides.topic ?? "Code Mode",
    remoteDoc: "docs/snippets/README.md",
    maxResults: overrides.max_results ?? 5,
    ...overrides
  };
  const localDoc = input.repoPath ? input.repoPath + "/docs/snippets/README.md" : null;

  const preview = (value, limit = 1400) => {
    const text = typeof value === "string" ? value : JSON.stringify(value);
    return text.length > limit ? `${text.slice(0, limit)}...` : text;
  };

  const timed = async (label, id, params, transform = (x) => x) => {
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
    ...(input.repoPath ? [
      () => timed(
        "local_doc",
        "filesystem::read_text_file",
        { path: localDoc, head: 120 },
        (result) => preview(result.content || result, 1000)
      ),
      () => timed(
        "local_code_search",
        "octocode::localSearch",
        {
          queries: [{
            operation: "text",
            path: input.repoPath,
            searchText: input.topic,
            resultView: "detailed",
            pageSize: input.maxResults,
            maxFiles: 100
          }]
        },
        (result) => preview(result)
      )
    ] : []),
    () => timed(
      "github_issues",
      "github::search_issues",
      {
        owner: input.owner,
        repo: input.repo,
        query: input.topic,
        perPage: input.maxResults,
        fields: ["number", "title", "state", "html_url"]
      },
      (result) => ({
        total_count: result.total_count,
        issues: (result.items || []).slice(0, input.maxResults).map((issue) => ({
          number: issue.number,
          title: issue.title,
          state: issue.state,
          url: issue.html_url
        }))
      })
    ),
    () => timed(
      "github_file",
      "github::get_file_contents",
      { owner: input.owner, repo: input.repo, path: input.remoteDoc },
      (result) => preview(result, 1000)
    )
  ];

  const batch = await codemode.batch(jobs);
  const calls = batch.ok
    .sort((a, b) => a.i - b.i)
    .map((entry) => entry.value);
  const batchFailures = batch.failed.map((entry) => ({
    label: `batch_job_${entry.i}`,
    id: "codemode.batch",
    ok: false,
    error: String(entry.error)
  }));
  calls.push(...batchFailures);

  const requiredLabels = new Set([
    "timestamp",
    "github_issues",
    "github_file"
  ]);
  if (input.repoPath) requiredLabels.add("local_code_search");
  const requiredOk = calls
    .filter((call) => requiredLabels.has(call.label))
    .every((call) => call.ok);
  const degraded = calls
    .filter((call) => !call.ok)
    .map((call) => ({ label: call.label, id: call.id, error: call.error }));

  return {
    snippet: "repo_context_triage",
    input,
    ok: requiredOk,
    status: degraded.length ? "degraded" : "ok",
    degraded,
    skipped: input.repoPath ? [] : ["local_doc", "local_code_search"],
    calls,
    next_steps: [
      "Use returned issue URLs and remote-file evidence as follow-up targets.",
      input.repoPath
        ? "Use Octocode localGetFileContent or LSP semantics when an exact symbol needs proof."
        : "Provide repo_path only when the same absolute checkout is visible to both filesystem and Octocode."
    ]
  };
}
```
