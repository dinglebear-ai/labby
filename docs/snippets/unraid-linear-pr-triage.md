---
name: unraid-linear-pr-triage
description: Triage Linear issues against organization-wide PRs with an open-only fast path and explicit deep mode
tags: [unraid, linear, github, triage]
tools:
  - linear-notification-worker::list_issues
  - linear-notification-worker::get_handoff
  - github::get_me
  - github::search_pull_requests
---

# Linear and pull-request triage

Defaults: team U8, assignee me, state started, organization unraid. History and
handoffs are opt-in via deep, includeHistory, or includeHandoffs. repos optionally
restricts related searches; myOpenPRs remains organization-wide. maxIssues defaults
to 250, chunkSize to 4 (maximum 4), maxCalls to 40, maxOutputBytes to 16000.

Output version 2 uses PR references in issue.openPRs, issue.historicalPRs, and
myOpenPRs. Resolve each reference through pullRequests. This avoids repeating PR
titles and URLs throughout the result. PR draft and merged flags are emitted only
when true; an absent merged flag is unknown, not proof of an unmerged PR.
Optional includeDetails adds issue branch
and project. One bounded page is fetched per search; coverage reports additional
pages, incomplete search results, and every failed call. A budget overflow returns
a compact explicit error, never an apparently complete truncated report.

```js
async (input = {}) => {
  if (!input || typeof input !== "object" || Array.isArray(input)) throw new Error("input must be an object");
  const booleanInputs = ["deep", "includeHistory", "includeHandoffs", "includeDetails", "includeReleasePRs"];
  for (const key of booleanInputs) if (input[key] !== undefined && typeof input[key] !== "boolean")
    throw new Error(key + " must be a boolean");
  for (const key of ["team", "assignee", "state", "org"]) if (input[key] !== undefined &&
      (typeof input[key] !== "string" || !input[key].trim())) throw new Error(key + " must be a nonempty string");
  const integer = (value, fallback, min, max) => {
    if (value === undefined) return fallback;
    if (!Number.isInteger(value) || value < min || value > max)
      throw new Error("Expected an integer between " + min + " and " + max);
    return value;
  };
  const team = input.team || "U8", assignee = input.assignee || "me";
  const state = input.state || "started", org = input.org || "unraid";
  if (typeof org !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9-]*$/.test(org))
    throw new Error("org must be a GitHub organization name");
  const repos = input.repos === undefined ? [] : input.repos;
  if (!Array.isArray(repos) || repos.length > 8 || repos.some(r =>
    typeof r !== "string" || !/^[\w.-]+\/[\w.-]+$/.test(r)))
    throw new Error("repos must contain at most eight owner/repository names");
  const scopes = repos.length ? [...new Set(repos)] : [null];
  const chunkSize = integer(input.chunkSize, 4, 1, 4);
  const maxIssues = integer(input.maxIssues, 250, 1, 250);
  const maxCalls = integer(input.maxCalls, 40, 3, 128);
  const maxOutputBytes = integer(input.maxOutputBytes, 16000, 1024, 18000);
  const deep = input.deep === true;
  const includeHistory = deep || input.includeHistory === true;
  const includeHandoffs = deep || input.includeHandoffs === true;
  const failures = [], incomplete = [];
  const compactError = error => {
    let value = error;
    if (error && typeof error.message === "string") {
      try { value = JSON.parse(error.message); } catch (_) {}
    }
    return { kind: value?.kind || "tool_error",
      message: String(value?.cause || value?.message || error).slice(0, 400) };
  };
  const initial = await codemode.batch([
    () => callTool("linear-notification-worker::list_issues", {
      team, assignee, state, limit: maxIssues,
      fields: ["id", "title", "status", "project", "gitBranchName"]
    }),
    () => callTool("github::get_me", {})
  ]);
  let issues = [], login = null;
  for (const row of initial.ok) {
    if (row.i === 0) {
      if (!Array.isArray(row.value?.issues)) {
        failures.push({type: "issues", kind: "invalid_response", message: "Linear returned no issues array"});
        continue;
      }
      issues = row.value.issues;
      if (row.value.hasNextPage || row.value.pageInfo?.hasNextPage)
        incomplete.push({type: "issues", reason: "more_pages"});
    } else {
      login = row.value?.login || row.value?.user?.login || row.value?.data?.login;
      if (!login || !/^[\w-]+$/.test(login)) {
        login = null;
        failures.push({type: "identity", kind: "invalid_response", message: "GitHub returned no valid login"});
      }
    }
  }
  for (const row of initial.failed)
    failures.push({type: row.i === 0 ? "issues" : "identity", ...compactError(row.error)});
  const invalidIds = issues.filter(i => !/^[A-Z][A-Z0-9]*-\d+$/i.test(i.id || ""));
  if (invalidIds.length) failures.push({type: "issues", kind: "invalid_identifier",
    message: invalidIds.length + " Linear identifiers were excluded"});
  issues = issues.filter(i => /^[A-Z][A-Z0-9]*-\d+$/i.test(i.id || ""));
  issues = [...new Map(issues.map(i => [i.id, i])).values()];
  const ids = issues.map(i => i.id);
  const metadata = [], jobs = [];
  const add = (meta, fn) => { metadata.push(meta); jobs.push(fn); };
  const fields = ["number", "title", "body", "state", "draft", "html_url",
    "repository_url", "user", "updated_at", "pull_request"];
  for (let start = 0; start < ids.length; start += chunkSize) {
    const chunk = ids.slice(start, start + chunkSize);
    for (const repo of scopes) {
      const qualifier = (repo ? "repo:" + repo : "org:" + org) + (includeHistory ? "" : " is:open");
      const query = chunk.map(id => '"' + id + '" ' + qualifier).join(" OR ");
      add({type: "related", repo, issueIds: chunk}, () => callTool("github::search_pull_requests", {
        query, perPage: 100, page: 1, fields
      }));
    }
  }
  if (login) add({type: "mine"}, () => callTool("github::search_pull_requests", {
    query: "author:" + login + " is:open org:" + org,
    perPage: 100, page: 1, sort: "updated", order: "desc", fields
  }));
  if (includeHandoffs) for (const id of ids)
    add({type: "handoff", issue: id}, () => callTool("linear-notification-worker::get_handoff", {issue: id}));
  if (jobs.length + 2 > maxCalls) return {
    ok: false, error: {kind: "call_budget_exceeded",
      message: "Selection requires " + (jobs.length + 2) + " calls; narrow maxIssues/repos or explicitly increase maxCalls"},
    summary: {issueCount: issues.length, requiredCalls: jobs.length + 2, maxCalls}
  };
  const batch = {ok: [], failed: []};
  for (let offset = 0; offset < jobs.length; offset += 8) {
    const part = await codemode.batch(jobs.slice(offset, offset + 8));
    batch.ok.push(...part.ok.map(row => ({...row, i: row.i + offset})));
    batch.failed.push(...part.failed.map(row => ({...row, i: row.i + offset})));
  }
  const allPRs = new Map(), related = new Set(), mine = new Set(), handoffs = new Map();
  const items = value => {
    const result = value?.items || value?.pull_requests || value?.data?.items || value?.data;
    return Array.isArray(result) ? result : null;
  };
  const inScope = (repo, meta) => meta.type === "mine" || !repos.length
    ? repo.split("/")[0].toLowerCase() === org.toLowerCase()
    : repos.some(r => r.toLowerCase() === repo.toLowerCase());
  for (const row of batch.ok) {
    const meta = metadata[row.i], value = row.value;
    if (meta.type === "handoff") {
      const s = value?.status || {};
      handoffs.set(meta.issue, {success: value?.success === true,
        state: s.stateName || value?.issue?.state || null,
        previousHandoffs: Array.isArray(s.previousHandoffs) ? s.previousHandoffs.length : 0,
        previousQaRuns: Array.isArray(s.previousQaRuns) ? s.previousQaRuns.length : 0});
      if (value?.success === false) failures.push({...meta, kind: "handoff_failed", message: "Handoff reported success=false"});
      continue;
    }
    const values = items(value);
    if (!values) { failures.push({...meta, kind: "invalid_response", message: "GitHub returned no PR array"}); continue; }
    const total = value?.total_count ?? value?.data?.total_count;
    if (value?.incomplete_results || value?.data?.incomplete_results)
      incomplete.push({...meta, reason: "incomplete_search"});
    if ((typeof total === "number" && total > values.length) || (total === undefined && values.length === 100))
      incomplete.push({...meta, reason: "more_pages", returned: values.length, total: total ?? null});
    for (const p of values) {
      const repo = String(p.repository_url || "").replace(/^https:\/\/api\.github\.com\/repos\//, "")
        || String(p.html_url || "").match(/^https:\/\/github\.com\/([^/]+\/[^/]+)\/pull\/\d+/)?.[1];
      if (!repo || !inScope(repo, meta) || !Number.isInteger(p.number)) continue;
      const ref = repo + "#" + p.number;
      const pr = {...p, repo, ref};
      allPRs.set(ref, pr);
      if (meta.type === "mine") {
        if (p.state === "open" && p.user?.login?.toLowerCase() === login.toLowerCase()) mine.add(ref);
      } else related.add(ref);
    }
  }
  for (const row of batch.failed) failures.push({...metadata[row.i], ...compactError(row.error)});
  const matches = (text, id) => new RegExp("(^|[^A-Za-z0-9])" + id + "(?![A-Za-z0-9])", "i").test(text || "");
  const release = title => /(^|:\s*)(release\b|release[- ]please\b|changelog\b)/i.test(title || "");
  const mention = (p, id) => matches(p.title, id) ||
    (matches(p.body, id) && (input.includeReleasePRs === true || !release(p.title)));
  const unknownIds = new Set([...failures, ...incomplete]
    .filter(item => item.type === "related").flatMap(item => item.issueIds || []));
  const used = new Set(mine);
  const triage = issues.map(issue => {
    const found = [...related].map(ref => allPRs.get(ref)).filter(p => mention(p, issue.id));
    const openPRs = found.filter(p => p.state === "open").map(p => p.ref);
    const historicalPRs = includeHistory ? found.filter(p => p.state === "closed").map(p => p.ref) : [];
    for (const ref of [...openPRs, ...historicalPRs]) used.add(ref);
    const item = {id: issue.id, title: issue.title, status: issue.status, openPRs,
      matchStatus: unknownIds.has(issue.id) ? "incomplete" : found.length ? "matched" : "no_observed_pr"};
    if (includeHistory) item.historicalPRs = historicalPRs;
    if (includeHandoffs) item.handoff = handoffs.get(issue.id) || null;
    if (input.includeDetails === true) { item.project = issue.project || null; item.branch = issue.gitBranchName || null; }
    return item;
  });
  const pullRequests = [...used].map(ref => {
    const p = allPRs.get(ref);
    return {ref, title: p.title, state: p.state,
      url: p.html_url, author: p.user?.login || null,
      ...(p.draft === true ? {draft: true} : {}),
      ...(p.pull_request?.merged_at ? {merged: true} : {})};
  });
  const output = {ok: failures.length === 0 && incomplete.length === 0, outputVersion: 2,
    team, assignee, state, org,
    summary: {issueCount: issues.length,
      issuesWithOpenPRs: triage.filter(i => i.openPRs.length).length,
      historyOnlyIssues: triage.filter(i => !i.openPRs.length && i.historicalPRs?.length).length,
      issuesWithoutObservedPRs: triage.filter(i => i.matchStatus === "no_observed_pr").length,
      myOpenPRCount: mine.size, relatedPRSearchCalls: metadata.filter(m => m.type === "related").length,
      relatedPRSearchFailures: failures.filter(m => m.type === "related").length,
      handoffFailures: failures.filter(m => m.type === "handoff").length,
      toolCalls: jobs.length + 2},
    myOpenPRs: [...mine], issues: triage, pullRequests,
    coverage: {complete: failures.length === 0 && incomplete.length === 0, incomplete}, failures};
  let bytes = 0;
  for (const char of JSON.stringify(output)) {
    const cp = char.codePointAt(0);
    bytes += cp <= 127 ? 1 : cp <= 2047 ? 2 : cp <= 65535 ? 3 : 4;
  }
  if (bytes > maxOutputBytes) return {ok: false, outputVersion: 2,
    error: {kind: "output_budget_exceeded", message: "Result requires " + bytes + " bytes; narrow maxIssues/repos or disable deep/details"},
    summary: output.summary, coverage: output.coverage, outputBytes: bytes, maxOutputBytes};
  return output;
}

```
