---
name: repo-status-gh-pulse
title: "Repository Status GitHub Pulse"
created: "2026-07-30"
updated: "2026-09-16"
description: Read-only GitHub PR pulse plus explicit shell parity for workflow runs
tags: [repo, github, ci, readonly]
inputs:
  owner:
    type: string
    default: dinglebear-ai
    required: false
    description: GitHub repository owner
  repo:
    type: string
    default: labby
    required: false
    description: GitHub repository name
  branch:
    type: string
    default: ""
    required: false
    description: Optional PR head branch to focus
  run_limit:
    type: integer
    default: 20
    required: false
    description: Maximum recent workflow runs in the returned shell-parity commands
  pr_limit:
    type: integer
    default: 20
    required: false
    description: Maximum open PRs to request
  include_workflow_runs:
    type: boolean
    default: true
    required: false
    description: Return shell-only workflow-run commands when Code Mode lacks that surface
---

# Repository Status GitHub Pulse

Use this snippet for the GitHub half of a repository-status sweep. The current
Labby GitHub catalog exposes a dedicated `github::search_pull_requests` tool, so
PR discovery no longer needs to masquerade as generic issue search. Workflow-run
listing is still a shell-only parity gap in the active GitHub Code Mode catalog.

## Current Calls

- `github::search_pull_requests` with explicit `owner` and `repo` for open PRs.
- A second pull-request search scoped by `head:<branch>` when `branch` is provided.
- Returned `gh run list` commands for workflow evidence because no live workflow-run tool is discoverable in the current GitHub upstream.

The pull-request calls are independent and run through `codemode.batch`.
This snippet is read-only and complements local Git status/worktree/diff evidence.

A live call against `dinglebear-ai/labby` was reverified on 2026-09-16 with
`github::search_pull_requests`.

```js
async (overrides = {}) => {
  const input = {
    owner: overrides.owner ?? "dinglebear-ai",
    repo: overrides.repo ?? "labby",
    branch: overrides.branch ?? "",
    runLimit: overrides.run_limit ?? 20,
    prLimit: overrides.pr_limit ?? 20,
    includeWorkflowRuns: overrides.include_workflow_runs ?? true
  };

  const timed = async (label, params) => {
    const started = Date.now();
    try {
      const result = await callTool("github::search_pull_requests", params);
      return {
        label,
        id: "github::search_pull_requests",
        ok: true,
        ms: Date.now() - started,
        result: {
          total_count: result.total_count,
          items: (result.items || []).map((item) => ({
            number: item.number,
            title: item.title,
            state: item.state,
            draft: item.draft,
            url: item.html_url,
            updated_at: item.updated_at,
            user: item.user?.login
          }))
        }
      };
    } catch (error) {
      return {
        label,
        id: "github::search_pull_requests",
        ok: false,
        ms: Date.now() - started,
        error: String(error)
      };
    }
  };

  const jobs = [
    () => timed("open_prs", {
      owner: input.owner,
      repo: input.repo,
      query: "is:open",
      perPage: input.prLimit,
      sort: "updated",
      order: "desc",
      fields: ["number", "title", "state", "draft", "html_url", "updated_at", "user"]
    })
  ];

  if (input.branch) {
    jobs.push(() => timed("focused_prs", {
      owner: input.owner,
      repo: input.repo,
      query: `is:open head:${input.branch}`,
      perPage: Math.min(input.prLimit, 10),
      sort: "updated",
      order: "desc",
      fields: ["number", "title", "state", "draft", "html_url", "updated_at", "user"]
    }));
  }

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

  const repoSlug = `${input.owner}/${input.repo}`;
  const workflowRunCommands = [
    `gh run list --repo ${repoSlug} --limit ${input.runLimit} --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url`,
    input.branch
      ? `gh run list --repo ${repoSlug} --branch ${input.branch} --limit 10 --json databaseId,headBranch,headSha,status,conclusion,workflowName,updatedAt,url`
      : null
  ].filter(Boolean);
  const workflowRuns = input.includeWorkflowRuns
    ? {
        available: false,
        status: "shell_only",
        reason: "The current GitHub Code Mode catalog does not expose a workflow-run tool.",
        gh_commands: workflowRunCommands
      }
    : { available: false, status: "not_requested", gh_commands: [] };

  const requiredCallsOk = calls.every((call) => call.ok);
  const byLabel = Object.fromEntries(calls.map((call) => [call.label, call]));

  return {
    snippet: "repo_status_gh_pulse",
    input,
    ok: requiredCallsOk && !input.includeWorkflowRuns,
    status: requiredCallsOk
      ? input.includeWorkflowRuns ? "degraded" : "ok"
      : "error",
    open_prs: byLabel.open_prs ?? null,
    focused_prs: byLabel.focused_prs ?? null,
    workflow_runs: workflowRuns,
    gh_equivalent_commands: [
      `gh pr list --repo ${repoSlug} --state open --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,updatedAt,url`,
      ...workflowRunCommands,
      input.branch
        ? `gh pr view ${input.branch} --repo ${repoSlug} --json number,title,headRefName,baseRefName,isDraft,mergeable,reviewDecision,statusCheckRollup,reviews,comments,updatedAt,url`
        : null
    ].filter(Boolean),
    calls,
    next_steps: [
      "Pair this GitHub pulse with local git status, worktree, diff, and mergeability evidence.",
      "Run the workflow-run gh commands from a shell-capable context when CI evidence is required."
    ]
  };
}
```
