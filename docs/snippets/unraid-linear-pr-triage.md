---
name: unraid-linear-pr-triage
description: Bounded, read-only Unraid PR triage with opt-in history and handoffs
tags: [unraid, linear, github, readonly]
tools: ["linear-notification-worker::list_issues", "linear-notification-worker::get_handoff", "github::get_me", "github::search_pull_requests"]
inputs:
  team:
    type: string
    default: U8
    description: Linear team identifier
  assignee:
    type: string
    default: me
    description: Linear assignee filter
  state:
    type: string
    default: started
    description: Linear state filter
  org:
    type: string
    default: unraid
    description: GitHub organization
  repos:
    type: array
    default: []
    description: Optional owner/repository overrides
  chunkSize:
    type: integer
    default: 4
    description: Issue identifiers per search, from one to four
  deep:
    type: boolean
    default: false
    description: Include historical PRs and compact handoffs
  includeHistory:
    type: boolean
    default: false
    description: Include historical PRs without enabling handoffs
  includeHandoffs:
    type: boolean
    default: false
    description: Include compact handoffs without enabling history
  includeReleasePRs:
    type: boolean
    default: false
    description: Include release and changelog sweep PRs
---

# Unraid Linear PR triage

Default: assigned U8 started issues, organization-wide open PRs, and all of the
current GitHub user's open Unraid PRs. History and handoffs require explicit
opt-in. The first batch loads issues and identity; the second performs independent
PR searches and optional handoffs. No writes are made to GitHub or Linear.

Optional inputs: team, assignee, state, org, repos (owner/repo array), chunkSize
(integer 1 to 4), deep, includeHistory, includeHandoffs, includeReleasePRs.
Searches fetch at most 100 PRs per query and one 250-issue page. Incomplete
upstream pages and bounded output omissions are explicitly reported, never
represented as proof that an issue has no PR. Output is capped at 16,000 UTF-8
bytes. Issue matching uses whole identifiers, not substring matches. Historical
references retain repository, PR number, URL, and merge status without repeating
titles and draft flags, preserving more coverage within the output budget.

```js
async (input = {}) => {
  const started = Date.now();
  const team = input.team || "U8";
  const assignee = input.assignee || "me";
  const state = input.state || "started";
  const org = input.org || "unraid";
  if ([team, assignee, state].some(v => typeof v !== "string" || v.length > 120)) throw new Error("Issue filters must be strings of at most 120 characters");
  if (!/^[A-Za-z0-9-]{1,64}$/.test(org)) throw new Error("Invalid org");
  const repos = input.repos == null ? [] : input.repos;
  if (!Array.isArray(repos) || repos.length > 6 || repos.some(r =>
    typeof r !== "string" || r.length > 160 || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(r))) {
    throw new Error("repos must contain at most six owner/repo names");
  }
  const chunkSize = input.chunkSize ?? 4;
  if (!Number.isInteger(chunkSize) || chunkSize < 1 || chunkSize > 4) {
    throw new Error("chunkSize must be an integer from 1 to 4");
  }
  for (const key of ["deep", "includeHistory", "includeHandoffs", "includeReleasePRs"]) {
    if (input[key] != null && typeof input[key] !== "boolean") throw new Error(key + " must be boolean");
  }
  const includeHistory = input.deep === true || input.includeHistory === true;
  const includeHandoffs = input.deep === true || input.includeHandoffs === true;
  const clip = (value, max = 100) => String(value ?? "").slice(0, max);
  const errorInfo = e => {
    let value = e;
    try { value = JSON.parse(e.message || String(e)); } catch (_) {}
    const cause = String(value?.cause || value?.message || e?.message || e);
    const reset = cause.match(/rate reset in (\d+)s/i);
    return { kind: /rate limit/i.test(cause) ? "rate_limited" : clip(value?.kind || "tool_error", 60),
      message: /rate limit/i.test(cause) ? "GitHub search rate limit reached; coverage is incomplete" : cause.slice(-180),
      ...(reset ? {retryAfterSeconds: Number(reset[1])} : {}) };
  };
  const failures = [];
  const incomplete = [];
  const first = await codemode.batch([
    () => callTool("linear-notification-worker::list_issues", {
      team, assignee, state, limit: 250, fields: ["id", "title", "status"]
    }),
    () => callTool("github::get_me", {})
  ]);
  const issueResponse = first.ok.find(x => x.i === 0)?.value;
  const identity = first.ok.find(x => x.i === 1)?.value;
  for (const x of first.failed) failures.push({ phase: x.i === 0 ? "issues" : "identity", ...errorInfo(x.error) });
  const login = identity?.login || identity?.user?.login || identity?.data?.login || null;
  if (first.ok.some(x => x.i === 1) && (!login || !/^[A-Za-z0-9-]+$/.test(login))) failures.push({ phase: "identity", kind: "invalid_result", message: "GitHub identity has no valid login" });
  const validLogin = login && /^[A-Za-z0-9-]+$/.test(login) ? login : null;
  const rawIssues = Array.isArray(issueResponse?.issues) ? issueResponse.issues : [];
  if (first.ok.some(x => x.i === 0) && !Array.isArray(issueResponse?.issues)) failures.push({ phase: "issues", kind: "invalid_result", message: "Linear result has no issues array" });
  if (issueResponse?.hasNextPage || issueResponse?.pageInfo?.hasNextPage || rawIssues.length > 100) incomplete.push("issues:page_limit");
  const seenIssues = new Set();
  const issues = rawIssues.slice(0, 100).filter(i => {
    if (!/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(i.id || "")) { incomplete.push("issues:invalid_identifier"); return false; }
    if (seenIssues.has(i.id)) return false;
    seenIssues.add(i.id); return true;
  });
  const ids = issues.map(i => i.id);
  const fields = ["number", "title", "body", "state", "draft", "html_url", "repository_url", "user", "pull_request"];
  const jobs = [], meta = [];
  const add = (m, id, params) => { meta.push(m); jobs.push(() => callTool(id, params)); };
  const scopes = repos.length ? [...new Set(repos)].map(r => "repo:" + r) : ["org:" + org];
  for (let i = 0; i < ids.length; i += chunkSize) {
    const chunk = ids.slice(i, i + chunkSize);
    for (const scope of scopes) {
      for (const prState of [includeHistory ? "all" : "open"]) {
        add({ phase: "related", ids: chunk, scope, state: prState }, "github::search_pull_requests", {
          query: chunk.map(id => '"' + id + '" ' + scope + (prState === "all" ? "" : " is:" + prState)).join(" OR "),
          perPage: 100, fields
        });
      }
    }
  }
  if (validLogin) add({ phase: "mine" }, "github::search_pull_requests", {
    query: "author:" + validLogin + " is:open org:" + org,
    perPage: 100, sort: "updated", order: "desc",
    fields: ["number", "title", "state", "draft", "html_url", "repository_url", "user"]
  });
  if (includeHandoffs) for (const id of ids) add({ phase: "handoff", issue: id }, "linear-notification-worker::get_handoff", { issue: id });
  if (jobs.length > 250) throw new Error("Requested scope exceeds 252 calls; narrow repos or increase chunkSize");
  const second = await codemode.batch(jobs);
  const prs = new Map(), mine = new Map(), handoffs = new Map();
  const unknownIds = new Set();
  const repoOf = p => String(p.repository_url || "").replace(/^https:\/\/api.github.com\/repos\//, "") ||
    (/^https:\/\/github.com\/([^/]+\/[^/]+)\/pull\/\d+/.exec(p.html_url || "") || [])[1];
  const matchesId = (text, id) => new RegExp("(^|[^A-Za-z0-9_-])" + id + "(?![A-Za-z0-9_-])", "i").test(text || "");
  const releaseLike = title => /^(?:chore(?:\([^)]*\))?:\s*)?(?:release\b|changelog\b)|release[- ]please/i.test(title || "");
  for (const x of second.failed) {
    const m = meta[x.i]; failures.push({ ...m, ...errorInfo(x.error) });
    if (m.phase === "related") m.ids.forEach(id => unknownIds.add(id));
  }
  for (const x of second.ok) {
    const m = meta[x.i], value = x.value;
    if (m.phase === "handoff") {
      if (value?.success !== true) { failures.push({ ...m, kind: "handoff_failed", message: "Handoff did not report success" }); continue; }
      const s = value.status || {};
      handoffs.set(m.issue, {
        state: clip(s.stateName, 60), stateType: clip(s.stateType, 30),
        previousHandoffs: Array.isArray(s.previousHandoffs) ? s.previousHandoffs.length : 0,
        previousQaRuns: Array.isArray(s.previousQaRuns) ? s.previousQaRuns.length : 0
      });
      continue;
    }
    const payload = value?.data && !Array.isArray(value.data) ? value.data : value;
    const items = payload?.items || payload?.pull_requests || (Array.isArray(value?.data) ? value.data : null);
    if (!Array.isArray(items)) {
      failures.push({ ...m, kind: "invalid_result", message: "Search result has no PR array" });
      if (m.ids) m.ids.forEach(id => unknownIds.add(id));
      continue;
    }
    if (payload.incomplete_results || payload.total_count > items.length || items.length === 100) {
      incomplete.push(m.phase + ":" + (m.scope || org) + ":page_limit");
      if (m.ids) m.ids.forEach(id => unknownIds.add(id));
    }
    for (const p of items) {
      const repo = repoOf(p);
      if (!repo || !Number.isInteger(p.number) || !["open", "closed"].includes(p.state)) {
        incomplete.push(m.phase + ":invalid_pr");
        if (m.ids) m.ids.forEach(id => unknownIds.add(id));
        continue;
      }
      const scopeMatches = m.phase === "mine" ? repo.split("/")[0].toLowerCase() === org.toLowerCase() :
        (m.scope.startsWith("repo:") ? repo.toLowerCase() === m.scope.slice(5).toLowerCase() : repo.split("/")[0].toLowerCase() === org.toLowerCase());
      if (!scopeMatches || (m.phase === "mine" ? p.state !== "open" : m.state !== "all" && p.state !== m.state)) continue;
      const key = repo + "#" + p.number;
      const compact = { repo, number: p.number, title: clip(p.title), url: p.html_url, draft: p.draft === true };
      if (m.phase === "mine") {
        if (p.user?.login && p.user.login.toLowerCase() !== validLogin.toLowerCase()) continue;
        if (input.includeReleasePRs === true || !releaseLike(p.title) || ids.some(id => matchesId(p.title, id))) mine.set(key, compact);
      } else {
        const matched = ids.filter(id => matchesId(p.title, id) ||
          ((input.includeReleasePRs === true || !releaseLike(p.title)) && matchesId(p.body, id)));
        const previous = prs.get(key);
        prs.set(key, { compact, state: p.state, merged: Boolean(p.pull_request?.merged_at), ids: new Set([...(previous?.ids || []), ...matched]) });
      }
    }
  }
  let omitted = 0;
  const rows = issues.map(i => {
    const related = [...prs.values()].filter(p => p.ids.has(i.id));
    const open = related.filter(p => p.state === "open");
    const history = related.filter(p => p.state === "closed");
    omitted += Math.max(0, open.length - 8) + Math.max(0, history.length - 8);
    return {
      id: i.id, title: clip(i.title), status: clip(i.status, 40),
      matchStatus: unknownIds.has(i.id) ? "incomplete" : related.length ? "matched" : "no_observed_pr",
      openPRs: open.slice(0, 8).map(p => p.compact),
      ...(includeHistory ? { historicalPRs: history.slice(0, 8).map(p => ({ repo: p.compact.repo, number: p.compact.number, url: p.compact.url, merged: p.merged })) } : {}),
      ...(includeHandoffs ? { handoff: handoffs.get(i.id) || null } : {})
    };
  });
  const output = {
    ok: failures.length === 0, complete: failures.length === 0 && incomplete.length === 0,
    team, assignee, state, org, mode: { includeHistory, includeHandoffs },
    summary: {
      issueCount: rows.length, issuesWithOpenPRs: rows.filter(r => r.openPRs.length).length,
      issuesWithoutObservedPRs: rows.filter(r => !r.openPRs.length && !r.historicalPRs?.length).length,
      myOpenPRCount: mine.size, relatedPRSearchCalls: meta.filter(m => m.phase === "related").length,
      relatedPRSearchFailures: failures.filter(f => f.phase === "related").length,
      handoffCalls: includeHandoffs ? ids.length : 0,
      handoffFailures: failures.filter(f => f.phase === "handoff").length,
      toolCalls: 2 + jobs.length, failures: failures.length, failuresOmitted: Math.max(0, failures.length - 20), outputOmitted: omitted
    },
    myOpenPRs: [...mine.values()], issues: rows, failures: failures.slice(0, 20),
    incomplete: [...new Set(incomplete)].slice(0, 20)
  };
  const bytes = value => {
    const text = JSON.stringify(value); let n = 0;
    for (let i = 0; i < text.length; i++) { const c = text.codePointAt(i); n += c < 128 ? 1 : c < 2048 ? 2 : c < 65536 ? 3 : 4; if (c > 65535) i++; }
    return n;
  };
  output.summary.elapsedMs = Date.now() - started;
  while (bytes(output) > 15900) {
    const row = output.issues.find(r => r.historicalPRs?.length > 0) || output.issues.find(r => r.openPRs.length > 1);
    if (row) (row.historicalPRs?.length ? row.historicalPRs : row.openPRs).pop();
    else if (output.myOpenPRs.length) output.myOpenPRs.pop();
    else if (output.issues.length) output.issues.pop();
    else break;
    output.summary.outputOmitted++;
  }
  if (output.summary.outputOmitted) output.complete = false;
  return output;
}
```
