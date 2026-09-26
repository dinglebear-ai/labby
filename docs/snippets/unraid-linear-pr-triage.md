---
name: unraid-linear-pr-triage
description: Bounded, read-only Unraid PR triage with opt-in history and handoffs
tags: [unraid, linear, github, readonly]
tools: ["linear-notification-worker::list_issues", "linear-notification-worker::get_handoff", "github::get_me", "github::search_pull_requests"]
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
represented as proof that an issue has no PR. Inputs are validated in JavaScript for compatibility with existing gateways.
Output is capped at 16,000 UTF-8 bytes. Issue matching uses whole identifiers, not substring matches.

Output schemaVersion 2 stores each PR once in pullRequests, keyed by owner/repo#number.
myOpenPRs and each issue's openPRs/historicalPRs contain those keys. Resolve a key
through pullRequests to obtain its title, URL, state and other metadata. The key
already contains the repository and PR number; cards do not repeat those fields.
Under output pressure, titles are shortened before relationships are omitted,
and summary.metadataCompacted reports this separately from coverage. This
replaces repeated PR objects while preserving every relationship that fits the
explicit output budget. At most 100 issues are processed from the fetched page.

```js
async (input = {}) => {
  const started = Date.now();
  const team = input.team ?? "U8";
  const assignee = input.assignee ?? "me";
  const state = input.state ?? "started";
  const org = input.org ?? "unraid";
  for (const [name, value] of Object.entries({ team, assignee, state, org })) {
    if (typeof value !== "string" || value.length === 0 || value.length > 128) throw new Error(name + " must be a bounded string");
  }
  if (!/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,62}[A-Za-z0-9])?$/.test(org)) throw new Error("Invalid org");
  const repos = input.repos == null ? [] : input.repos;
  if (!Array.isArray(repos) || repos.length > 6 || repos.some(r =>
    typeof r !== "string" || r.length > 255 || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(r))) {
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
  const clip = (value, max = 100) => {
    const text = String(value ?? "");
    let end = Math.min(max, text.length);
    const last = text.charCodeAt(end - 1);
    if (last >= 0xd800 && last <= 0xdbff) end--;
    return text.slice(0, end);
  };
  const errorInfo = e => {
    let value = e;
    try { value = JSON.parse(e.message || String(e)); } catch (_) {}
    const detail = String(value?.cause || value?.message || e?.message || e);
    const limited = /rate limit exceeded|secondary rate limit|too many requests/i.test(detail);
    const reset = /rate reset in (\d+)s/.exec(detail);
    return { kind: limited ? "rate_limited" : clip(value?.kind || "tool_error", 60),
      message: limited ? "GitHub rate limit exceeded; retry after reset" : clip(detail, 180),
      ...(reset ? { retryAfterSeconds: Number(reset[1]) } : {}) };
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
    if (typeof i.id !== "string" || i.id.length > 96 || !/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(i.id)) { incomplete.push("issues:invalid_identifier"); return false; }
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
      for (const prState of [includeHistory ? null : "open"]) {
        add({ phase: "related", ids: chunk, scope, state: prState }, "github::search_pull_requests", {
          query: chunk.map(id => '"' + id + '" ' + scope + (prState ? " is:" + prState : "")).join(" OR "),
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
  if (jobs.length > 126) throw new Error("Scope requires more than 128 calls; narrow the team, repositories, or chunk size");
  const second = {ok: [], failed: []};
  for (let offset = 0; offset < jobs.length; offset += 8) {
    const batch = await codemode.batch(jobs.slice(offset, offset + 8));
    second.ok.push(...batch.ok.map(x => ({...x, i: x.i + offset})));
    second.failed.push(...batch.failed.map(x => ({...x, i: x.i + offset})));
  }
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
      if (!repo || !Number.isInteger(p.number) || p.number <= 0 || !["open", "closed"].includes(p.state) ||
          typeof p.title !== "string" || p.html_url !== "https://github.com/" + repo + "/pull/" + p.number) {
        incomplete.push(m.phase + ":invalid_pr");
        if (m.ids) m.ids.forEach(id => unknownIds.add(id));
        continue;
      }
      const scopeMatches = m.phase === "mine" ? repo.split("/")[0].toLowerCase() === org.toLowerCase() :
        (m.scope.startsWith("repo:") ? repo.toLowerCase() === m.scope.slice(5).toLowerCase() : repo.split("/")[0].toLowerCase() === org.toLowerCase());
      if (!scopeMatches || (m.phase === "mine" ? p.state !== "open" : (m.state && p.state !== m.state))) continue;
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
  const catalog = Object.create(null);
  const reference = (compact, state, merged = false) => {
    const key = compact.repo + "#" + compact.number;
    if (catalog[key] && catalog[key].state !== state) incomplete.push("pr:conflicting_state:" + key);
    if (!catalog[key]) {
      const { repo, number, ...details } = compact;
      catalog[key] = { ...details, state, ...(state === "closed" ? { merged } : {}) };
    }
    return key;
  };
  const rows = issues.map(i => {
    const related = [...prs.values()].filter(p => p.ids.has(i.id));
    const open = related.filter(p => p.state === "open");
    const history = related.filter(p => p.state === "closed");
    return {
      id: i.id, title: clip(i.title), status: clip(i.status, 40),
      matchStatus: unknownIds.has(i.id) ? "incomplete" : related.length ? "matched" : "no_observed_pr",
      openPRs: open.map(p => reference(p.compact, p.state)).sort(),
      ...(includeHistory ? { historicalPRs: history.map(p => reference(p.compact, p.state, p.merged)).sort() } : {}),
      ...(includeHandoffs ? { handoff: handoffs.get(i.id) || null } : {})
    };
  });
  const myOpenPRs = [...mine.values()].map(p => reference(p, "open")).sort();
  const output = {
    schemaVersion: 2,
    ok: failures.length === 0, complete: failures.length === 0 && incomplete.length === 0,
    team, assignee, state, org, mode: { includeHistory, includeHandoffs },
    summary: {
      issueCount: Array.isArray(issueResponse?.issues) ? rows.length : null,
      issuesWithOpenPRs: Array.isArray(issueResponse?.issues) ? rows.filter(r => r.openPRs.length).length : null,
      issuesWithoutObservedPRs: Array.isArray(issueResponse?.issues) ? rows.filter(r => r.matchStatus === "no_observed_pr").length : null,
      myOpenPRCount: mine.size, uniquePRCount: Object.keys(catalog).length,
      relatedPRSearchCalls: meta.filter(m => m.phase === "related").length,
      relatedPRSearchFailures: failures.filter(f => f.phase === "related").length,
      handoffCalls: includeHandoffs ? ids.length : 0,
      handoffFailures: failures.filter(f => f.phase === "handoff").length,
      toolCalls: 2 + jobs.length, outputOmitted: 0, metadataCompacted: false, failureCount: failures.length,
      failuresOmitted: Math.max(0, failures.length - 12), incompleteOmitted: Math.max(0, new Set(incomplete).size - 20)
    },
    myOpenPRs, issues: rows, pullRequests: catalog, failures: failures.slice(0, 12),
    incomplete: [...new Set(incomplete)].slice(0, 20)
  };
  const discardUnreferenced = () => {
    const retained = new Set(output.myOpenPRs);
    for (const row of output.issues) {
      for (const key of row.openPRs) retained.add(key);
      for (const key of row.historicalPRs || []) retained.add(key);
    }
    for (const key of Object.keys(output.pullRequests)) if (!retained.has(key)) delete output.pullRequests[key];
  };
  const bytes = value => {
    const text = JSON.stringify(value); let n = 0;
    for (let i = 0; i < text.length; i++) { const c = text.codePointAt(i); n += c < 128 ? 1 : c < 2048 ? 2 : c < 65536 ? 3 : 4; if (c > 65535) i++; }
    return n;
  };
  output.summary.elapsedMs = Date.now() - started;
  for (const titleLimit of [80, 60, 40]) {
    if (bytes(output) <= 15900) break;
    for (const card of Object.values(output.pullRequests)) card.title = clip(card.title, titleLimit);
    for (const row of output.issues) row.title = clip(row.title, titleLimit);
    output.summary.metadataCompacted = true;
  }
  while (bytes(output) > 15900) {
    const row = output.issues.find(r => r.historicalPRs?.length > 1) || output.issues.find(r => r.openPRs.length > 1) || output.issues.find(r => r.historicalPRs?.length > 0);
    if (row) (row.historicalPRs?.length ? row.historicalPRs : row.openPRs).pop();
    else if (output.myOpenPRs.length) output.myOpenPRs.pop();
    else if (output.issues.length) output.issues.pop();
    else if (output.failures.length) { output.failures.pop(); output.summary.failuresOmitted++; }
    else if (output.incomplete.length) { output.incomplete.pop(); output.summary.incompleteOmitted++; }
    else break;
    output.summary.outputOmitted++;
    discardUnreferenced();
  }
  if (output.summary.outputOmitted) output.complete = false;
  return output;
}
```
