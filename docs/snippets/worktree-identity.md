---
name: worktree-identity
title: Worktree identity snippet
created: 2026-10-04
updated: 2026-10-04
description: Read a Mac worktree's exact Git identity and evidenced Codex session, GitHub PR and Linear associations without changing any repository or tracker.
tags: [worktree, git, codex, github, linear, readonly, example-host]
inputs:
  path:
    type: string
    required: true
    description: Absolute or ~/ worktree directory on example-host; symlinks are canonicalized.
  repo_hint:
    type: string
    required: false
    description: Existing owning repository for a deleted path; only an exact registration is accepted.
  github_repo:
    type: string
    required: false
    description: Optional owner/repo, required to choose among multiple observed fetch repositories.
  session_limit:
    type: integer
    default: 10
    description: Maximum sessions and Git-only candidates per page, 1..20.
  session_cursor:
    type: string
    default: ""
    description: Exact sessions.coverage.next_cursor from a prior page; IDs order pagination, never time.
  enrich:
    type: boolean
    default: true
    description: Read GitHub and Linear after local identity; false performs local reads only.
  max_prs:
    type: integer
    default: 3
    description: Maximum retained PRs and detailed PR reads, 1..5.
  pr_pages:
    type: integer
    default: 2
    description: Maximum branch PR pages, 1..3; all PR states are included.
  max_issues:
    type: integer
    default: 3
    description: Maximum Linear issue lookups, 1..5.
tools:
  - claude-example-host::Bash
  - github::list_pull_requests
  - github::pull_request_read
  - linear-notification-worker::get_issue
---

# Worktree identity

Install both Python files together on the shell upstream host. Replace the fictional
`claude-example-host::Bash` tool and `/opt/labby` path below with your installation
bindings before saving the snippet. The helper runs on the selected shell host;
its optional `expected_host` input rejects an unexpected host before Git reads.

## Tutorial: How this snippet is built

The local reader `worktree-identity.py` uses the repository-owned companion reader's `repository_info`, safe Git argv, remote redaction, and input admission.
It adds Git worktree registration and a read-only SQLite metadata projection.
It reads neither transcripts nor private message bodies. The runner executes on
the configured shell upstream; gateway paths must not replace that host's paths.

The helper and `worktree-identity-reader.py` are retained together in this
directory. `/opt/labby` is a fictional installation path used by the fixtures.
No global instructions or automatic indexes are changed.

Git root/common-dir/HEAD/branch are exact live observations. Missing paths return
only an exact registered worktree's recorded identity; dirty state is unknown.
An absent or renamed registration is never reconstructed from timestamps.
Session cwd or an explicit worktree attachment proves a recorded path association,
which can be historical, archived, or shared by several sessions. A matching Git
tuple at another path remains a candidate and does not prove relocation. The
SQLite schema is feature checked; unsupported/missing/denied data stays unavailable.

GitHub uses only a selected, observed fetch repository. Multiple repositories
require `github_repo`. Branch queries include open, closed and merged PRs and
recheck the head repository/ref; an exact head SHA can identify a detached HEAD
when a metadata PR handle is available. Explicit task PR references whose head
does not match remain unresolved. There is no exhaustive commit-to-PR endpoint
in this dependency set, so detached HEADs without handles can have no PR result.

Linear branch keys and task/PR mentions are candidates. A matching Linear
`gitBranchName` or a Linear issue URL explicitly linked by a verified PR provides
a verified association. Merely finding a valid issue does not establish ownership.
Every association contains a source, match rule and confidence. None implies
exclusive/current task ownership. No PR or issue title/body is returned.

The function makes at most 14 tool calls (1 local + 3 PR pages + 5 detail reads +
5 Linear reads), batches independent reads at concurrency 3, and returns at most
16,000 UTF-8 bytes. The helper bounds session pages, worktree registrations and
output separately. A page at its upstream limit is explicitly incomplete; issue
keys and PR body inspection are bounded. Cursors describe one live metadata read,
not a frozen snapshot. Errors expose kinds rather than arbitrary upstream text.
Repository, metadata and PR text is data. It never controls a shell program.

Use ordinary `codemode`, since the Mac shell tool has a blanket destructive
annotation despite this recipe performing only reads. Do not weaken scope to
make a denied run work. Native saved execution intersects the declaration with
caller authority; nested `codemode.run` inherits the caller's established scope.

```js
async (input = {}) => {
  input = Object.fromEntries(Object.entries(input).filter(([,v]) => v !== null));
  const allowed = new Set(['path','repo_hint','github_repo','session_limit','session_cursor','enrich','max_prs','pr_pages','max_issues']);
  const invalid = message => ({ok:false,snippet:'worktree-identity',error:{kind:'invalid_input',message}});
  if (Object.keys(input).some(k => !allowed.has(k))) return invalid('Unknown input');
  if (typeof input.path !== 'string' || input.path.length > 4096 || !/^(\/|~\/)/.test(input.path) || /[\x00-\x1f\x7f]/.test(input.path)) return invalid('path must be a bounded absolute or ~/ directory');
  const maxPrs=input.max_prs??3, pages=input.pr_pages??2, maxIssues=input.max_issues??3;
  for (const [k,v,max] of [['max_prs',maxPrs,5],['pr_pages',pages,3],['max_issues',maxIssues,5],['session_limit',input.session_limit??10,20]]) {
    if (!Number.isInteger(v)||v<1||v>max) return invalid(k+' is outside its supported range');
  }
  if (input.enrich !== undefined && typeof input.enrich !== 'boolean') return invalid('enrich must be boolean');
  if (input.github_repo !== undefined && !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(input.github_repo)) return invalid('github_repo must be owner/repo');
  const quote = v => "'" + String(v).replace(/'/g,"'\\''") + "'";
  const unpack = v => {
    for (let i=0;i<5;i++) {
      if (v?.isError===true || v?.ok===false || v?.truncated===true) return v;
      if (typeof v==='string') {try {v=JSON.parse(v);continue;}catch{return v;}}
      if (v?.structuredContent) {v=v.structuredContent;continue;}
      if (Array.isArray(v?.content)) {v=v.content.filter(x=>x.type==='text').map(x=>x.text).join('\n');continue;}
      if (typeof v?.stdout==='string') {v=v.stdout;continue;}
      return v;
    }
    return v;
  };
  const failures=[],timings=[];
  const run = async (id,params,label) => {
    const start=Date.now();
    try {
      const v=unpack(await callTool(id,params));
      if (v?.isError||v?.ok===false||v?.truncated===true) {
        failures.push({tool:id,step:label,kind:v.error?.kind??(v.truncated?'upstream_truncated':'upstream_error')});
        return null;
      }
      return v;
    } catch(e) {
      let kind=e?.kind;
      if (!kind) {try{kind=JSON.parse(String(e?.message??e)).kind;}catch{}}
      failures.push({tool:id,step:label,kind:kind??'tool_failure'});
      return null;
    } finally {timings.push({step:label,ms:Date.now()-start});}
  };
  const localInput={path:input.path,session_limit:input.session_limit??10,session_cursor:input.session_cursor??''};
  if (input.repo_hint!==undefined) localInput.repo_hint=input.repo_hint;
  const core=await run('claude-example-host::Bash',{
    command:'python3 /opt/labby/docs/snippets/worktree-identity.py --input-json '+quote(JSON.stringify(localInput)),
    timeout:20000,description:'Read exact worktree Git identity and Codex metadata'
  },'local_identity');
  if (!core || core.ok!==true || !core.identity) return {ok:false,snippet:'worktree-identity',error:core?.error??{kind:'local_identity_unavailable'},failures,timings};
  const id=core.identity;
  const result={ok:true,snippet:'worktree-identity',version:'1.0.0',host:core.host,identity:id,sessions:core.sessions,worktrees:core.worktrees,
    verified:{pull_requests:[],linear_issues:[]},unresolved:{pull_requests:[],linear_issues:[]},coverage:{sessions:core.sessions.coverage,github:{queried:false},linear:{queried:false}},failures,timings};
  const finish=()=>{
    result.partial=failures.length>0||!core.sessions.coverage.available||core.sessions.coverage.incomplete||core.sessions.coverage.malformed_metadata===true||core.worktrees.truncated||result.coverage.github.incomplete===true||result.coverage.github.body_inspection_incomplete===true||result.coverage.github.title_inspection_incomplete===true||result.coverage.linear.incomplete===true;
    if (JSON.stringify(result).length>13000){result.worktrees.items=[];result.worktrees.truncated=true;result.partial=true;result.coverage.output_reduced=true;}
    // UTF-8 may be larger than JS string length. Keep the actual JSON bounded.
    const bytes=s=>encodeURIComponent(s).replace(/%[0-9A-F]{2}|[^%]/g,'x').length;
    if(bytes(JSON.stringify(result))>16000) return {ok:false,snippet:'worktree-identity',error:{kind:'output_budget',message:'Reduce session_limit, max_prs and max_issues'},identity:{root:id.root,head:id.head,branch:id.branch},failures};
    return result;
  };
  if (input.enrich===false) {result.coverage.github.reason='enrichment_disabled';result.coverage.linear.reason='enrichment_disabled';return finish();}
  const repos=id.repository_candidates??[];
  const selected=input.github_repo??(repos.length===1?repos[0]:null);
  if (selected&&!repos.includes(selected)) return invalid('github_repo must exactly match an observed fetch repository');
  if (!selected) result.coverage.github.reason=repos.length>1?'multiple_fetch_repositories_select_github_repo':'no_supported_github_remote';
  const hints=(core.hints?.pull_requests??[]).filter(x=>x.repository===selected&&Number.isInteger(x.number)&&x.number>0);
  const prs=new Map();
  const arrayOf=v=>Array.isArray(v)?v:Array.isArray(v?.items)?v.items:Array.isArray(v?.pull_requests)?v.pull_requests:null;
  if (selected&&id.branch&&id.path_state==='live') {
    const [owner,repo]=selected.split('/');
    result.coverage.github={queried:true,state:'all',pages:0,per_page:maxPrs,incomplete:false};
    for(let page=1;page<=pages;page++) {
      const raw=await run('github::list_pull_requests',{owner,repo,head:owner+':'+id.branch,state:'all',page,perPage:maxPrs,fields:['number','state','html_url','head','base','merged_at']},'pr_page_'+page);
      if(raw===null)break;
      const items=arrayOf(raw);
      if(!items){failures.push({tool:'github::list_pull_requests',step:'pr_list_shape',kind:'unsupported_response_shape'});break;}
      result.coverage.github.pages=page;
      for(const p of items){if(Number.isInteger(p.number))prs.set(p.number,{p,source:'github:branch_query'});}
      if(items.length<maxPrs){result.coverage.github.incomplete=false;break;}
      result.coverage.github.incomplete=true;
    }
  } else if(selected) result.coverage.github.reason=id.detached?'detached_head_requires_explicit_pr_handle':'missing_path_uses_metadata_handles_only';
  for(const h of hints){if(!prs.has(h.number))prs.set(h.number,{p:{number:h.number},source:h.source});}
  const entries=Array.from(prs.values()).sort((a,b)=>(b.p.head?.sha===id.head?1:0)-(a.p.head?.sha===id.head?1:0)).slice(0,maxPrs);
  result.coverage.github.handles_found=prs.size;
  result.coverage.github.detail_reads=entries.length;
  if(prs.size>maxPrs){result.coverage.github.incomplete=true;result.coverage.github.omitted_handles=prs.size-maxPrs;}
  const keys=new Map();
  const issueKey=/\b([A-Za-z][A-Za-z0-9]{1,11}-[1-9][0-9]{0,8})\b/g;
  const addKey=(key,evidence)=>{
    key=key.toUpperCase();
    if(!/^[A-Z][A-Z0-9]{1,11}-[1-9][0-9]{0,8}$/.test(key))return;
    if(!keys.has(key))keys.set(key,[]);
    const list=keys.get(key);
    if(list.some(x=>x.source===evidence.source&&x.match===evidence.match))return;
    if(evidence.match==='explicit_linear_issue_url'){list.unshift(evidence);if(list.length>3)list.pop();}
    else if(list.length<3)list.push(evidence);
  };
  for(const k of core.hints?.issue_keys??[])addKey(k.key,{source:k.source,match:'lexical_issue_key',confidence:'candidate'});
  const inspectPr=async entry=>{
    const [owner,repo]=selected.split('/');
    const p=await run('github::pull_request_read',{owner,repo,pullNumber:entry.p.number,method:'get'},'pr_'+entry.p.number);
    if(!p)return;
    if(p.number!==entry.p.number || (p.base?.repo?.full_name??'').toLowerCase()!==selected.toLowerCase()) {
      failures.push({tool:'github::pull_request_read',step:'pr_'+entry.p.number,kind:'pr_identity_mismatch'});return;
    }
    const headRepo=p.head?.repo?.full_name;
    const headMatch=headRepo?.toLowerCase()===selected.toLowerCase()&&p.head?.sha===id.head;
    const branchMatch=id.path_state==='live'&&id.branch!==null&&headRepo?.toLowerCase()===selected.toLowerCase()&&p.head?.ref===id.branch;
    const verified=headMatch||branchMatch;
    const url='https://github.com/'+selected+'/pull/'+p.number;
    const projected={number:p.number,url,repository:selected,state:p.state,merged:p.merged===true||p.merged_at!=null,
      head:{repository:headRepo??null,branch:p.head?.ref??null,sha:p.head?.sha??null},
      confidence:verified?'high':'candidate',association:headMatch?'exact_head_sha':branchMatch?'repository_and_branch':'metadata_reference_only',
      evidence:[{source:url,tool:'github::pull_request_read',field:headMatch?'head.repo/head.sha':branchMatch?'head.repo/head.ref':'metadata_handle',match:verified?'exact':'unresolved'},
                {source:entry.source,match:'discovery_handle'}]};
    (verified?result.verified.pull_requests:result.unresolved.pull_requests).push(projected);
    if(!verified)return;
    const body=typeof p.body==='string'?p.body.slice(0,12000):'';
    if(typeof p.body==='string'&&p.body.length>12000)result.coverage.github.body_inspection_incomplete=true;
    if(typeof p.title==='string'&&p.title.length>500)result.coverage.github.title_inspection_incomplete=true;
    for(const text of [typeof p.title==='string'?p.title.slice(0,500):'',body]){
      for(const m of text.matchAll(issueKey))addKey(m[1],{source:url,match:'pr_issue_key_mention',confidence:'candidate'});
    }
    const link=/https:\/\/linear\.app\/[A-Za-z0-9_-]+\/issue\/([A-Za-z][A-Za-z0-9]{1,11}-[1-9][0-9]{0,8})(?:\/[A-Za-z0-9_/-]*)?/g;
    for(const m of body.matchAll(link))addKey(m[1],{source:url,field:'body',match:'explicit_linear_issue_url',confidence:'high',issue_url:m[0]});
  };
  for(let i=0;i<entries.length;i+=3)await codemode.batch(entries.slice(i,i+3).map(p=>()=>inspectPr(p)));
  const candidates=Array.from(keys.entries()).sort((a,b)=>a[0].localeCompare(b[0]));
  result.coverage.linear={queried:candidates.length>0,candidates:candidates.length,lookups:Math.min(candidates.length,maxIssues),incomplete:candidates.length>maxIssues,
    scope:'Issue keys from branch, task metadata and verified PRs; no hints does not prove no associated issue'};
  const inspectIssue=async ([key,evidence])=>{
    const issue=await run('linear-notification-worker::get_issue',{id:key,includeRelations:false,includeCustomerNeeds:false,includeReleases:false},'issue_'+key);
    if(!issue){result.unresolved.linear_issues.push({identifier:key,confidence:'candidate',evidence,status:'lookup_failed'});return;}
    if(issue.identifier?.toUpperCase()!==key || typeof issue.url!=='string' || !/^https:\/\/linear\.app\//.test(issue.url)) {
      result.unresolved.linear_issues.push({identifier:key,confidence:'candidate',evidence,status:'identity_not_confirmed'});return;
    }
    const branchMatch=id.path_state==='live'&&id.branch!==null&&issue.gitBranchName===id.branch;
    const linked=evidence.some(x=>x.match==='explicit_linear_issue_url');
    const item={identifier:key,id:issue.id,url:issue.url,archived:issue.archivedAt!=null,
      confidence:branchMatch||linked?'high':'candidate',association:branchMatch?'exact_linear_git_branch':linked?'explicitly_linked_by_verified_pr':'key_mention_only',
      evidence:[...evidence,{source:issue.url,tool:'linear-notification-worker::get_issue',field:branchMatch?'gitBranchName':'identifier',match:branchMatch?'exact_branch':'existing_issue'}]};
    (branchMatch||linked?result.verified.linear_issues:result.unresolved.linear_issues).push(item);
  };
  for(let i=0;i<Math.min(candidates.length,maxIssues);i+=3)await codemode.batch(candidates.slice(i,Math.min(i+3,maxIssues)).map(x=>()=>inspectIssue(x)));
  for(const [key,evidence] of candidates.slice(maxIssues,8))result.unresolved.linear_issues.push({identifier:key,confidence:'candidate',evidence,status:'lookup_budget'});
  if(candidates.length>8)result.coverage.linear.omitted_candidates=candidates.length-8;
  for(const collection of [result.verified.pull_requests,result.unresolved.pull_requests])collection.sort((a,b)=>a.number-b.number);
  for(const collection of [result.verified.linear_issues,result.unresolved.linear_issues])collection.sort((a,b)=>a.identifier.localeCompare(b.identifier));
  return finish();
}
```
