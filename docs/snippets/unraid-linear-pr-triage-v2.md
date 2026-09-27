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
default 16000), maxPRsPerIssue (1-25, default 12), staleDays (1-3650, default 14),
issueCursor, pageStarts, deep, includeHistory, includeHandoffs, includeReleasePRs,
and diagnostics. Repo filters narrow issue matches, not the organization-wide
list of personal PRs. Inputs are validated in JavaScript to preserve the existing
camel-case interface; the current frontmatter parser only permits lower-case
declared input names.

Each issue records correlation evidence (title, exact Linear branch text, or body)
with a confidence level. Mechanical attention signals call out stale drafts,
multiple open PRs, QA-ready issues without observed PRs, status/PR mismatches,
and incomplete coverage. pageStarts accepts continuation pages by the stable
job keys returned in coverage gaps.

Limits: 100 issues, 300 retained PRs, 80 tool calls, four concurrent jobs,
12 PR references per issue by default, bounded pages, and the gateway deadline.
Pagination, release suppression, quota skips, and output omissions are explicit.
Tests and authoring instructions are in [SNIPPET_HARNESS.md](../dev/SNIPPET_HARNESS.md).

```js
async (input = {}) => {
  const started = Date.now();
  const org = input.org || "unraid", team = input.team || "U8";
  const assignee = input.assignee || "me", state = input.state || "started";
  if (!/^[A-Za-z0-9][A-Za-z0-9-]{0,63}$/.test(org)) throw new Error("Invalid org");
  const repos = input.repos == null ? [] : input.repos;
  if (!Array.isArray(repos) || repos.length > 6 || repos.some(r => typeof r !== "string" || !/^[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+$/.test(r))) throw new Error("repos must contain at most six owner/repo names");
  const pageStarts = input.pageStarts == null ? {} : input.pageStarts;
  if (!pageStarts || typeof pageStarts !== "object" || Array.isArray(pageStarts)) throw new Error("pageStarts must be an object");
  for (const key of ["deep", "includeHistory", "includeHandoffs", "includeReleasePRs", "diagnostics"]) {
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
  const maxPRsPerIssue = integer(input.maxPRsPerIssue, 12, 1, 25);
  const staleDays = integer(input.staleDays, 14, 1, 3650);
  const includeHistory = input.deep === true || input.includeHistory === true;
  const includeHandoffs = input.deep === true || input.includeHandoffs === true;
  const diagnostics = input.diagnostics === true;
  const clip = (v, n = 120) => String(v ?? "").slice(0, n);
  const gaps = [], failures = [], unknownIds = new Set();
  const trace = [], searchStats = [], suppressedReleaseKeys = [];
  const timings = {bootstrapMs: 0, githubMs: 0, handoffMs: 0, shapingMs: 0, totalMs: 0};
  let toolCalls = 0, prSearchCalls = 0, quotaBlocked = false, quotaFailure = null;
  let duplicatePRs = 0, suppressedReleasePRs = 0;
  const MAX_RETAINED_PRS = 300, SEARCH_CONCURRENCY = 4, HANDOFF_CONCURRENCY = 4;
  const errorInfo = error => {
    let e = error;
    try { e = JSON.parse(error?.message || String(error)); } catch (_) {}
    const cause = String(e?.cause || e?.message || error?.message || error);
    if (/rate limit|rate_limit/i.test(cause)) return {kind: "rate_limit", message: "Search quota exhausted", retryAfterSeconds: Number(/rate reset in (\d+)s/.exec(cause)?.[1]) || null};
    return {kind: clip(e?.kind || "tool_error", 60), message: clip(cause, 400)};
  };
  const call = async (tool, params, phase, detail = {}) => {
    if (toolCalls >= 80) throw new Error("Snippet tool-call budget exceeded");
    toolCalls++;
    const callStarted = Date.now();
    try {
      const value = await callTool(tool, params);
      trace.push({tool, phase, ok: true, ms: Date.now() - callStarted, ...detail});
      return value;
    } catch (error) {
      trace.push({tool, phase, ok: false, ms: Date.now() - callStarted, ...detail});
      throw error;
    }
  };
  const bootstrapStarted = Date.now();
  const first = await codemode.batch([
    () => call("linear-notification-worker::list_issues", {team, assignee, state, limit: 100,
      ...(input.issueCursor ? {cursor: input.issueCursor} : {}),
      fields: ["id", "title", "status", "gitBranchName", "updatedAt"]}, "issues"),
    () => call("github::get_me", {}, "identity")
  ]);
  timings.bootstrapMs = Date.now() - bootstrapStarted;
  for (const x of first.failed) failures.push({phase: x.i === 0 ? "issues" : "identity", ...errorInfo(x.error)});
  const issueResponse = first.ok.find(x => x.i === 0)?.value;
  const identity = first.ok.find(x => x.i === 1)?.value;
  const login = identity?.login || identity?.user?.login || identity?.data?.login || null;
  const validLogin = typeof login === "string" && /^[A-Za-z0-9-]+$/.test(login) ? login : null;
  if (identity && !validLogin) failures.push({phase: "identity", kind: "invalid_result", message: "Missing valid GitHub login"});
  if (issueResponse && !Array.isArray(issueResponse.issues)) failures.push({phase: "issues", kind: "invalid_result", message: "Missing issues array"});
  const rawIssues = Array.isArray(issueResponse?.issues) ? issueResponse.issues : [];
  const nextIssueCursor = issueResponse?.nextCursor || issueResponse?.cursor || issueResponse?.endCursor || issueResponse?.pageInfo?.endCursor || null;
  if (issueResponse?.hasNextPage || issueResponse?.pageInfo?.hasNextPage || rawIssues.length > 100) {
    gaps.push({phase: "issues", reason: "page_limit", continuation: nextIssueCursor ? {issueCursor: nextIssueCursor} : null});
  }
  const seenIssues = new Set();
  const issues = rawIssues.slice(0, 100).filter(i => {
    if (!/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(i.id || "")) { gaps.push({phase: "issues", reason: "invalid_identifier"}); return false; }
    if (seenIssues.has(i.id)) return false;
    seenIssues.add(i.id); return true;
  });
  const ids = issues.map(i => i.id);
  const fields = ["number", "title", "body", "state", "draft", "html_url", "repository_url", "user", "pull_request", "updated_at"];
  const jobKeyFor = meta => [meta.phase, meta.scope || "none", (meta.ids || ["all"]).join(",")].join("|");
  const search = async (query, meta) => {
    const found = [];
    const jobKey = jobKeyFor(meta);
    const startPage = integer(pageStarts[jobKey], 1, 1, 100);
    let pages = 0;
    for (let page = startPage; page < startPage + maxPages; page++) {
      if (quotaBlocked) throw Object.assign(new Error("Search rate limit already observed in this run"), {kind: "rate_limit"});
      let value;
      try {
        prSearchCalls++;
        pages++;
        value = await call("github::search_pull_requests", {query, page, perPage: 100, fields}, "github", {jobKey, page});
      } catch (error) {
        const info = errorInfo(error);
        if (info.kind === "rate_limit") { quotaBlocked = true; quotaFailure = info; }
        throw error;
      }
      const payload = value?.data && !Array.isArray(value.data) ? value.data : value;
      const items = payload?.items || payload?.pull_requests || (Array.isArray(value?.data) ? value.data : null);
      if (!Array.isArray(items)) throw new Error("Search result has no PR array");
      found.push(...items);
      const more = typeof payload.total_count === "number" ? page * 100 < payload.total_count : items.length === 100;
      if (payload.incomplete_results || (more && page === startPage + maxPages - 1)) {
        const nextPage = more ? page + 1 : null;
        gaps.push({...meta, jobKey, reason: payload.incomplete_results ? "incomplete_results" : "page_limit",
          continuation: nextPage ? {pageStarts: {[jobKey]: nextPage}} : null});
        (meta.ids || ids).forEach(id => unknownIds.add(id));
      }
      if (!more) break;
    }
    searchStats.push({jobKey, phase: meta.phase, scope: meta.scope || null, pages, startPage, returned: found.length});
    return found;
  };
  const searchJobs = [], handoffJobs = [];
  const addSearch = (query, meta) => searchJobs.push({...meta, query, run: () => search(query, meta)});
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
  if (includeHandoffs) {
    for (const id of ids) handoffJobs.push({phase: "handoff", issue: id,
      run: () => call("linear-notification-worker::get_handoff", {issue: id}, "handoff", {issue: id})});
  }
  const prs = new Map(), handoffs = new Map();
  const githubStarted = Date.now();
  let searchOffset = 0;
  for (; searchOffset < searchJobs.length; searchOffset += SEARCH_CONCURRENCY) {
    if (quotaBlocked) break;
    const group = searchJobs.slice(searchOffset, searchOffset + SEARCH_CONCURRENCY);
    const batch = await codemode.batch(group.map(job => job.run));
    for (const x of batch.failed) {
      const {run, query, ...meta} = group[x.i];
      const info = errorInfo(x.error);
      const jobKey = jobKeyFor(meta);
      failures.push({...meta, jobKey, ...info});
      gaps.push({...meta, jobKey, reason: info.kind === "rate_limit" ? "rate_limit" : "search_failure",
        retryAfterSeconds: info.retryAfterSeconds || null});
      if (info.kind === "rate_limit") { quotaBlocked = true; quotaFailure = info; }
      if (meta.phase === "open" || meta.phase === "history") (meta.ids || ids).forEach(id => unknownIds.add(id));
    }
    for (const x of batch.ok) {
      const {run, query, ...meta} = group[x.i], value = x.value;
      for (const pr of value) {
        const repo = String(pr.repository_url || "").replace(/^https:\/\/api.github.com\/repos\//, "") || /^https:\/\/github.com\/([^/]+\/[^/]+)\/pull\/\d+/.exec(pr.html_url || "")?.[1];
        if (!repo || !/^[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+$/.test(repo) || !Number.isInteger(pr.number) || !["open", "closed"].includes(pr.state)) {
          gaps.push({...meta, reason: "invalid_pr"}); (meta.ids || ids).forEach(id => unknownIds.add(id)); continue;
        }
        const inScope = meta.scope.startsWith("repo:") ? repo.toLowerCase() === meta.scope.slice(5).toLowerCase() : repo.split("/")[0].toLowerCase() === org.toLowerCase();
        if (!inScope || pr.state !== (meta.phase === "history" ? "closed" : "open")) continue;
        const key = repo + "#" + pr.number;
        if (prs.has(key)) duplicatePRs++;
        if (!prs.has(key) && prs.size >= MAX_RETAINED_PRS) {
          gaps.push({...meta, reason: "pr_retention_cap", limit: MAX_RETAINED_PRS});
          (meta.ids || ids).forEach(id => unknownIds.add(id));
          continue;
        }
        prs.set(key, {...pr, repo});
      }
    }
  }
  if (quotaBlocked && searchOffset < searchJobs.length) {
    for (const job of searchJobs.slice(searchOffset)) {
      const {run, query, ...meta} = job;
      const jobKey = jobKeyFor(meta);
      gaps.push({...meta, jobKey, reason: "rate_limit_skipped", retryAfterSeconds: quotaFailure?.retryAfterSeconds || null});
      if (meta.phase === "open" || meta.phase === "history") (meta.ids || ids).forEach(id => unknownIds.add(id));
    }
  }
  timings.githubMs = Date.now() - githubStarted;
  const handoffStarted = Date.now();
  for (let offset = 0; offset < handoffJobs.length; offset += HANDOFF_CONCURRENCY) {
    const group = handoffJobs.slice(offset, offset + HANDOFF_CONCURRENCY);
    const batch = await codemode.batch(group.map(job => job.run));
    for (const x of batch.failed) {
      const {run, ...meta} = group[x.i];
      const info = errorInfo(x.error);
      failures.push({...meta, ...info});
      gaps.push({...meta, reason: "handoff_failure", kind: info.kind});
    }
    for (const x of batch.ok) {
      const {run, ...meta} = group[x.i], value = x.value;
      if (value?.success !== true) { failures.push({...meta, kind: "handoff_failed", message: "Handoff did not report success"}); continue; }
      const s = value.status || {};
      handoffs.set(meta.issue, {state: clip(s.stateName, 60),
        previousHandoffs: Array.isArray(s.previousHandoffs) ? s.previousHandoffs.length : 0,
        previousQaRuns: Array.isArray(s.previousQaRuns) ? s.previousQaRuns.length : 0});
    }
  }
  timings.handoffMs = Date.now() - handoffStarted;
  const shapingStarted = Date.now();
  const matchesId = (value, id) => new RegExp("(^|[^A-Za-z0-9_])" + id + "(?![A-Za-z0-9_])", "i").test(value || "");
  const matchesBranch = (value, branch) => Boolean(branch) && String(value || "").toLowerCase().includes(String(branch).toLowerCase());
  const releaseLike = title => /^(?:chore(?:\([^)]*\))?:\s*)?(?:release\b|changelog\b)|release[- ]please/i.test(title || "");
  const rows = issues.map(i => ({id: i.id, title: clip(i.title), status: clip(i.status, 40),
    branch: i.gitBranchName ? clip(i.gitBranchName, 180) : null, updatedAt: i.updatedAt || null,
    matchStatus: unknownIds.has(i.id) ? "incomplete" : "no_observed_pr", openPRs: [], matchEvidence: {},
    ...(includeHistory ? {historicalPRs: []} : {}), ...(includeHandoffs ? {handoff: handoffs.get(i.id) || null} : {})}));
  const correlation = (row, pr) => {
    if (matchesId(pr.title, row.id)) return {via: "title", confidence: "strong"};
    if (matchesBranch(pr.title, row.branch) || matchesBranch(pr.body, row.branch)) return {via: "branch", confidence: "strong"};
    if (matchesId(pr.body, row.id)) return {via: "body", confidence: "medium"};
    return null;
  };
  const pullRequests = {}, myOpenPRs = [], perIssueCapGaps = new Set();
  let branchCorrelationCount = 0;
  for (const [key, pr] of prs) {
    const directMatch = rows.some(row => matchesId(pr.title, row.id));
    const releaseNoise = input.includeReleasePRs !== true && releaseLike(pr.title) && !directMatch;
    if (releaseNoise) {
      suppressedReleasePRs++;
      if (diagnostics && suppressedReleaseKeys.length < 50) suppressedReleaseKeys.push(key);
      continue;
    }
    const allowedRepo = !repoScopes.length || repoScopes.some(r => r.toLowerCase() === pr.repo.toLowerCase());
    const matched = allowedRepo ? rows.map(row => [row, correlation(row, pr)]).filter(([, evidence]) => evidence) : [];
    const mine = validLogin && pr.user?.login?.toLowerCase() === validLogin.toLowerCase() && pr.state === "open" && pr.repo.split("/")[0].toLowerCase() === org.toLowerCase();
    if (!matched.length && !mine) continue;
    pullRequests[key] = {title: clip(pr.title), state: pr.state, draft: pr.draft === true,
      url: "https://github.com/" + pr.repo + "/pull/" + pr.number, updatedAt: pr.updated_at || null,
      author: pr.user?.login || null, merged: pr.pull_request?.merged_at ? true : (pr.state === "open" ? false : null)};
    if (mine) myOpenPRs.push(key);
    for (const [row, evidence] of matched) {
      const refs = pr.state === "open" ? row.openPRs : row.historicalPRs;
      if (!refs) continue;
      if (refs.length >= maxPRsPerIssue) {
        row.matchStatus = "output_incomplete";
        if (!perIssueCapGaps.has(row.id)) {
          perIssueCapGaps.add(row.id);
          gaps.push({phase: "output", issue: row.id, reason: "per_issue_pr_cap", limit: maxPRsPerIssue});
        }
        continue;
      }
      refs.push(key);
      row.matchEvidence[key] = evidence;
      if (evidence.via === "branch") branchCorrelationCount++;
      if (!unknownIds.has(row.id)) row.matchStatus = "matched";
    }
  }
  const attention = [];
  let attentionOmitted = 0;
  const addAttention = item => attention.length < 100 ? attention.push(item) : attentionOmitted++;
  for (const row of rows) {
    if (row.openPRs.length > 1) addAttention({issue: row.id, kind: "multiple_open_prs", prs: row.openPRs.slice(0, 8)});
    if (/approved for release/i.test(row.status) && row.openPRs.length) addAttention({issue: row.id, kind: "open_pr_after_release_status", prs: row.openPRs.slice(0, 8)});
    if (/qa ready/i.test(row.status) && row.matchStatus === "no_observed_pr") addAttention({issue: row.id, kind: "qa_ready_no_observed_pr"});
    if (row.matchStatus === "incomplete") addAttention({issue: row.id, kind: "coverage_incomplete"});
    const draftPRs = row.openPRs.filter(key => pullRequests[key]?.draft);
    const updatedMs = Date.parse(row.updatedAt || "");
    const ageDays = Number.isFinite(updatedMs) ? Math.max(0, Math.floor((Date.now() - updatedMs) / 86400000)) : null;
    if (draftPRs.length && ageDays !== null && ageDays >= staleDays) addAttention({issue: row.id, kind: "stale_draft_pr", ageDays, prs: draftPRs.slice(0, 8)});
    else if (draftPRs.length) addAttention({issue: row.id, kind: "draft_pr", prs: draftPRs.slice(0, 8)});
  }
  if (attentionOmitted) gaps.push({phase: "output", reason: "attention_cap", omitted: attentionOmitted});
  const bytes = value => { let n = 0; for (const c of JSON.stringify(value)) { const k = c.codePointAt(0); n += k < 128 ? 1 : k < 2048 ? 2 : k < 65536 ? 3 : 4; } return n; };
  const diagnosticPayload = diagnostics ? {
    budgets: {maxIssues: 100, maxRetainedPRs: MAX_RETAINED_PRS, maxToolCalls: 80, maxPages, maxOutputBytes: maxBytes, maxPRsPerIssue},
    calls: trace.slice(0, 20),
    slowestCalls: [...trace].sort((a, b) => b.ms - a.ms).slice(0, 5),
    searchJobs: searchStats.slice(0, 30),
    suppressedReleasePRs: suppressedReleaseKeys,
    deduplicatedPRs: duplicatePRs,
    retainedPRs: Object.keys(pullRequests).length,
    quotaFailure
  } : null;
  const output = {schemaVersion: 2, ok: !failures.length, complete: !failures.length && !gaps.length,
    team, assignee, state, org, mode: {includeHistory, includeHandoffs, diagnostics},
    summary: {issueCount: rows.length, issuesWithOpenPRs: rows.filter(r => r.openPRs.length).length,
      issuesWithoutObservedPRs: rows.filter(r => !r.openPRs.length && !r.historicalPRs?.length).length,
      myOpenPRCount: myOpenPRs.length, prSearchCalls, relatedPRSearchFailures: failures.filter(f => f.phase === "open" || f.phase === "history").length,
      handoffCalls: includeHandoffs ? ids.length : 0, handoffFailures: failures.filter(f => f.phase === "handoff").length,
      suppressedReleasePRs, branchCorrelationCount, attentionCount: attention.length, toolCalls, elapsedMs: 0},
    coverage: {gaps, nextIssueCursor, omittedPRs: 0, omittedMyOpenPRs: 0, omittedIssues: 0},
    timing: timings, attention, myOpenPRs, issues: rows, pullRequests, failures,
    ...(diagnostics ? {diagnostics: diagnosticPayload} : {})};
  if (output.diagnostics && bytes(output) > maxBytes) {
    output.diagnostics.calls = output.diagnostics.calls.slice(0, 5);
    output.diagnostics.searchJobs = output.diagnostics.searchJobs.slice(0, 10);
    output.diagnostics.slowestCalls = output.diagnostics.slowestCalls.slice(0, 3);
    output.diagnostics.suppressedReleasePRs = output.diagnostics.suppressedReleasePRs.slice(0, 10);
    output.diagnostics.truncated = true;
    if (bytes(output) > maxBytes) {
      output.diagnostics.calls = [];
      output.diagnostics.searchJobs = [];
    }
    if (bytes(output) > maxBytes) {
      output.diagnostics = {
        truncated: true,
        reason: "diagnostics_compacted_to_preserve_triage_data",
        budgets: output.diagnostics.budgets,
        retainedPRs: output.diagnostics.retainedPRs,
        deduplicatedPRs: output.diagnostics.deduplicatedPRs,
        quotaFailure: output.diagnostics.quotaFailure
      };
    }
  }
  while (bytes(output) > maxBytes) {
    output.complete = false;
    const nonMine = Object.keys(pullRequests).filter(key => !myOpenPRs.includes(key));
    const key = nonMine.pop() || Object.keys(pullRequests).pop();
    if (key) {
      delete pullRequests[key]; output.coverage.omittedPRs++;
      const i = myOpenPRs.indexOf(key);
      if (i >= 0) { myOpenPRs.splice(i, 1); output.coverage.omittedMyOpenPRs++; }
      for (const row of rows) {
        if (row.openPRs.includes(key) || row.historicalPRs?.includes(key)) row.matchStatus = "output_incomplete";
        row.openPRs = row.openPRs.filter(k => k !== key);
        if (row.historicalPRs) row.historicalPRs = row.historicalPRs.filter(k => k !== key);
        delete row.matchEvidence[key];
      }
    } else if (rows.length) { rows.pop(); output.coverage.omittedIssues++; }
    else return {schemaVersion: 2, ok: false, complete: false, summary: output.summary,
      coverage: {reason: "diagnostics_exceed_output_budget", failures: failures.length, gaps: gaps.length}};
  }
  if (output.coverage.omittedPRs || output.coverage.omittedIssues) {
    output.complete = false;
    gaps.push({phase: "output", reason: "output_budget", omittedPRs: output.coverage.omittedPRs,
      omittedMyOpenPRs: output.coverage.omittedMyOpenPRs, omittedIssues: output.coverage.omittedIssues});
  }
  timings.shapingMs = Date.now() - shapingStarted;
  timings.totalMs = Date.now() - started;
  output.summary.elapsedMs = timings.totalMs;
  return output;
}
```
