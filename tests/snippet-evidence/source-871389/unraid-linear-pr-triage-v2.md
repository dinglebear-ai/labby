---
name: unraid-linear-pr-triage-v2
description: Bounded org-wide PR triage with opt-in history and handoffs
tags: [unraid, linear, github, readonly]
tools: ["linear-notification-worker::list_issues", "linear-notification-worker::get_handoff", "github::get_me", "github::search_pull_requests"]
---

# Unraid Linear / PR triage

The default batches identity and assigned started issues, then searches open PRs
in groups of at most four issue identifiers plus the user's open organization PRs.
Every search supports bounded pagination. Results are deduplicated before output.
History and handoffs are opt-in using `deep`, `includeHistory`, or `includeHandoffs`.
No GitHub or Linear records are changed. GitHub rate limits are reported, not retried.

Schema version 2 stores each PR once in `pullRequests`, keyed by `owner/repo#number`.
`myOpenPRs`, `openPRs`, and `historicalPRs` contain those keys. A closed PR with
`merged: null` has unknown merge state. Missing history fields mean not requested.
`complete: false`, `coverage`, and `matchStatus` distinguish missing evidence from
no observed PR. Summary counts describe retrieved data before output-budget trimming.

Inputs: team, assignee, state, org, repos (up to six owner/repo names), chunkSize
(1-4 history identifiers), maxPages (1-5, default 3), maxOutputBytes (4000-20000,
default 16000), issueCursor, deep, includeHistory, includeHandoffs, includeReleasePRs.
Repo filters narrow issue matches, not the organization-wide list of personal PRs.
Inputs are validated in JavaScript to preserve the existing camel-case interface;
the current frontmatter parser only permits lower-case declared input names.

Limits: 100 issues, 80 tool calls, four concurrent jobs, and the gateway deadline.
Pagination and output omissions are explicit. Tests and authoring instructions
are in [SNIPPET_HARNESS.md](../dev/SNIPPET_HARNESS.md).

```js
async (input = {}) => {
  const started = Date.now();
  const org = input.org || "unraid", team = input.team || "U8";
  const assignee = input.assignee || "me", state = input.state || "started";
  if (!/^[A-Za-z0-9][A-Za-z0-9-]{0,63}$/.test(org)) throw new Error("Invalid org");
  const repos = input.repos == null ? [] : input.repos;
  if (!Array.isArray(repos) || repos.length > 6 || repos.some(r => typeof r !== "string" || !/^[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+$/.test(r))) throw new Error("repos must contain at most six owner/repo names");
  for (const key of ["deep", "includeHistory", "includeHandoffs", "includeReleasePRs"]) {
    if (input[key] != null && typeof input[key] !== "boolean") throw new Error(key + " must be boolean");
  }
  const integer = (value, fallback, min, max) => {
    if (value === undefined) return fallback;
    if (!Number.isInteger(value) || value < min || value > max) throw new Error("Expected integer between " + min + " and " + max);
    return value;
  };
  const chunkSize = integer(input.chunkSize, 4, 1, 4);
  const maxPages = integer(input.maxPages, 3, 1, 5);
  const maxBytes = integer(input.maxOutputBytes, 16000, 4000, 20000);
  const includeHistory = input.deep === true || input.includeHistory === true;
  const includeHandoffs = input.deep === true || input.includeHandoffs === true;
  const clip = (v, n = 120) => String(v ?? "").slice(0, n);
  const failures = [], incomplete = [], unknownIds = new Set();
  let toolCalls = 0, prSearchCalls = 0, quotaBlocked = false;
  const errorInfo = error => {
    let e = error;
    try { e = JSON.parse(error?.message || String(error)); } catch (_) {}
    const cause = String(e?.cause || e?.message || error?.message || error);
    if (/rate limit|rate_limit/i.test(cause)) return {kind: "rate_limit", message: "Search quota exhausted", retryAfterSeconds: Number(/rate reset in (\d+)s/.exec(cause)?.[1]) || null};
    return {kind: clip(e?.kind || "tool_error", 60), message: clip(cause, 400)};
  };
  const call = (tool, params) => {
    if (toolCalls >= 80) throw new Error("Snippet tool-call budget exceeded");
    toolCalls++;
    return callTool(tool, params);
  };
  const first = await codemode.batch([
    () => call("linear-notification-worker::list_issues", {team, assignee, state, limit: 100,
      ...(input.issueCursor ? {cursor: input.issueCursor} : {}), fields: ["id", "title", "status"]}),
    () => call("github::get_me", {})
  ]);
  for (const x of first.failed) failures.push({phase: x.i === 0 ? "issues" : "identity", ...errorInfo(x.error)});
  const issueResponse = first.ok.find(x => x.i === 0)?.value;
  const identity = first.ok.find(x => x.i === 1)?.value;
  const login = identity?.login || identity?.user?.login || identity?.data?.login || null;
  const validLogin = typeof login === "string" && /^[A-Za-z0-9-]+$/.test(login) ? login : null;
  if (identity && !validLogin) failures.push({phase: "identity", kind: "invalid_result", message: "Missing valid GitHub login"});
  if (issueResponse && !Array.isArray(issueResponse.issues)) failures.push({phase: "issues", kind: "invalid_result", message: "Missing issues array"});
  const rawIssues = Array.isArray(issueResponse?.issues) ? issueResponse.issues : [];
  if (issueResponse?.hasNextPage || issueResponse?.pageInfo?.hasNextPage || rawIssues.length > 100) incomplete.push({phase: "issues", reason: "page_limit", cursor: issueResponse?.cursor || issueResponse?.endCursor || issueResponse?.pageInfo?.endCursor || null});
  const seenIssues = new Set();
  const issues = rawIssues.slice(0, 100).filter(i => {
    if (!/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(i.id || "")) { incomplete.push({phase: "issues", reason: "invalid_identifier"}); return false; }
    if (seenIssues.has(i.id)) return false;
    seenIssues.add(i.id); return true;
  });
  const ids = issues.map(i => i.id);
  const fields = ["number", "title", "body", "state", "draft", "html_url", "repository_url", "user", "pull_request"];
  const search = async (query, meta) => {
    const found = [];
    for (let page = 1; page <= maxPages; page++) {
      if (quotaBlocked) throw Object.assign(new Error("Search rate limit already observed in this run"), {kind: "rate_limit"});
      let value;
      try {
        prSearchCalls++;
        value = await call("github::search_pull_requests", {query, page, perPage: 100, fields});
      } catch (error) { if (errorInfo(error).kind === "rate_limit") quotaBlocked = true; throw error; }
      const payload = value?.data && !Array.isArray(value.data) ? value.data : value;
      const items = payload?.items || payload?.pull_requests || (Array.isArray(value?.data) ? value.data : null);
      if (!Array.isArray(items)) throw new Error("Search result has no PR array");
      found.push(...items);
      const more = typeof payload.total_count === "number" ? page * 100 < payload.total_count : items.length === 100;
      if (payload.incomplete_results || (more && page === maxPages)) {
        incomplete.push({...meta, reason: payload.incomplete_results ? "incomplete_results" : "page_limit", nextPage: more ? page + 1 : null});
        (meta.ids || ids).forEach(id => unknownIds.add(id));
      }
      if (!more) break;
    }
    return found;
  };
  const jobs = [];
  const addSearch = (query, meta) => jobs.push({...meta, run: () => search(query, meta)});
  const repoScopes = [...new Set(repos)];
  if (validLogin) addSearch("author:" + validLogin + " org:" + org + " is:open", {phase: "mine", scope: "org:" + org});
  for (let i = 0; i < ids.length; i += chunkSize) {
    const chunk = ids.slice(i, i + chunkSize);
    for (const scope of repoScopes.length ? repoScopes.map(r => "repo:" + r) : ["org:" + org]) {
      for (const prState of includeHistory ? ["open", "closed"] : ["open"]) {
        addSearch(chunk.map(id => '"' + id + '" ' + scope + " is:" + prState).join(" OR "),
          {phase: prState === "open" ? "open" : "history", scope, ids: chunk});
      }
    }
  }
  if (includeHandoffs) for (const id of ids) jobs.push({phase: "handoff", issue: id, run: () => call("linear-notification-worker::get_handoff", {issue: id})});
  const prs = new Map(), handoffs = new Map();
  for (let offset = 0; offset < jobs.length; offset += 4) {
    const group = jobs.slice(offset, offset + 4);
    const batch = await codemode.batch(group.map(job => job.run));
    for (const x of batch.failed) {
      const {run, ...meta} = group[x.i]; failures.push({...meta, ...errorInfo(x.error)});
      if (meta.phase === "open" || meta.phase === "history") (meta.ids || ids).forEach(id => unknownIds.add(id));
    }
    for (const x of batch.ok) {
      const {run, ...meta} = group[x.i], value = x.value;
      if (meta.phase === "handoff") {
        if (value?.success !== true) { failures.push({...meta, kind: "handoff_failed", message: "Handoff did not report success"}); continue; }
        const s = value.status || {};
        handoffs.set(meta.issue, {state: clip(s.stateName, 60),
          previousHandoffs: Array.isArray(s.previousHandoffs) ? s.previousHandoffs.length : 0,
          previousQaRuns: Array.isArray(s.previousQaRuns) ? s.previousQaRuns.length : 0});
        continue;
      }
      for (const p of value) {
        const repo = String(p.repository_url || "").replace(/^https:\/\/api.github.com\/repos\//, "") || /^https:\/\/github.com\/([^/]+\/[^/]+)\/pull\/\d+/.exec(p.html_url || "")?.[1];
        if (!repo || !/^[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+$/.test(repo) || !Number.isInteger(p.number) || !["open", "closed"].includes(p.state)) {
          incomplete.push({...meta, reason: "invalid_pr"}); (meta.ids || ids).forEach(id => unknownIds.add(id)); continue;
        }
        const inScope = meta.scope.startsWith("repo:") ? repo.toLowerCase() === meta.scope.slice(5).toLowerCase() : repo.split("/")[0].toLowerCase() === org.toLowerCase();
        if (!inScope || p.state !== (meta.phase === "history" ? "closed" : "open")) continue;
        prs.set(repo + "#" + p.number, {...p, repo});
      }
    }
  }
  const matchesId = (value, id) => new RegExp("(^|[^A-Za-z0-9_])" + id + "(?![A-Za-z0-9_])", "i").test(value || "");
  const releaseLike = title => /^(?:chore(?:\([^)]*\))?:\s*)?(?:release\b|changelog\b)|release[- ]please/i.test(title || "");
  const rows = issues.map(i => ({id: i.id, title: clip(i.title), status: clip(i.status, 40),
    matchStatus: unknownIds.has(i.id) ? "incomplete" : "no_observed_pr", openPRs: [],
    ...(includeHistory ? {historicalPRs: []} : {}), ...(includeHandoffs ? {handoff: handoffs.get(i.id) || null} : {})}));
  const pullRequests = {}, myOpenPRs = [];
  for (const [key, p] of prs) {
    const directMatch = ids.some(id => matchesId(p.title, id));
    const releaseNoise = input.includeReleasePRs !== true && releaseLike(p.title) && !directMatch;
    if (releaseNoise) continue;
    const allowedRepo = !repoScopes.length || repoScopes.some(r => r.toLowerCase() === p.repo.toLowerCase());
    const matched = allowedRepo ? rows.filter(i => matchesId(p.title, i.id) || matchesId(p.body, i.id)) : [];
    const mine = validLogin && p.user?.login?.toLowerCase() === validLogin.toLowerCase() && p.state === "open" && p.repo.split("/")[0].toLowerCase() === org.toLowerCase();
    if (!matched.length && !mine) continue;
    pullRequests[key] = {title: clip(p.title), state: p.state, draft: p.draft === true,
      url: "https://github.com/" + p.repo + "/pull/" + p.number,
      author: p.user?.login || null, merged: p.pull_request?.merged_at ? true : (p.state === "open" ? false : null)};
    if (mine) myOpenPRs.push(key);
    for (const row of matched) {
      (p.state === "open" ? row.openPRs : row.historicalPRs)?.push(key);
      if (!unknownIds.has(row.id)) row.matchStatus = "matched";
    }
  }
  const output = {schemaVersion: 2, ok: !failures.length, complete: !failures.length && !incomplete.length,
    team, assignee, state, org, mode: {includeHistory, includeHandoffs},
    summary: {issueCount: rows.length, issuesWithOpenPRs: rows.filter(r => r.openPRs.length).length,
      issuesWithoutObservedPRs: rows.filter(r => !r.openPRs.length && !r.historicalPRs?.length).length,
      myOpenPRCount: myOpenPRs.length, prSearchCalls, relatedPRSearchFailures: failures.filter(f => f.phase === "open" || f.phase === "history").length,
      handoffCalls: includeHandoffs ? ids.length : 0, handoffFailures: failures.filter(f => f.phase === "handoff").length, toolCalls, elapsedMs: Date.now() - started},
    coverage: {incomplete, omittedPRs: 0, omittedIssues: 0}, myOpenPRs, issues: rows, pullRequests, failures};
  const bytes = value => { let n = 0; for (const c of JSON.stringify(value)) { const k = c.codePointAt(0); n += k < 128 ? 1 : k < 2048 ? 2 : k < 65536 ? 3 : 4; } return n; };
  while (bytes(output) > maxBytes) {
    output.complete = false;
    const key = Object.keys(pullRequests).pop();
    if (key) {
      delete pullRequests[key]; output.coverage.omittedPRs++;
      const i = myOpenPRs.indexOf(key); if (i >= 0) myOpenPRs.splice(i, 1);
      for (const row of rows) {
        if (row.openPRs.includes(key) || row.historicalPRs?.includes(key)) row.matchStatus = "output_incomplete";
        row.openPRs = row.openPRs.filter(k => k !== key);
        if (row.historicalPRs) row.historicalPRs = row.historicalPRs.filter(k => k !== key);
      }
    } else if (rows.length) { rows.pop(); output.coverage.omittedIssues++; }
    else return {schemaVersion: 2, ok: false, complete: false, summary: output.summary, coverage: {reason: "diagnostics_exceed_output_budget", failures: failures.length, incomplete: incomplete.length}};
  }
  return output;
}
```
