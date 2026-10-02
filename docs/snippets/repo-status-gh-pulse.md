---
name: repo-status-gh-pulse
description: Audit local checkout, branches and worktrees, merge conflicts, patch risks, and GitHub PR checks with conservative readiness classifications.
tags: [repo, git, github, ci, readonly]
tools:
  - claude-macpoo::Bash
  - github::search_pull_requests
  - github::pull_request_read
  - github-actions::actions_list
inputs:
  root:
    type: string
    required: true
    description: Absolute Git checkout on macpoo or GitHub owner/repo resolved under workspace and unraid
  skill_dir:
    type: string
    default: /Users/jmagar/.codex/plugins/cache/agentic-unraid-marketplace/vibin/0.1.0/skills/repo-status
    description: Installed repo-status skill containing the context and merge collectors
  owner:
    type: string
    required: false
  repo:
    type: string
    required: false
  branch:
    type: string
    required: false
  max_branches:
    type: integer
    default: 10
  pr_limit:
    type: integer
    default: 4
  max_probes:
    type: integer
    default: 1
  check_mergeability:
    type: boolean
    default: true
---

# Repository Status Evidence Sweep

Follows vibin:repo-status using its installed collectors instead of a second Git inventory implementation. The required root accepts an absolute checkout or GitHub owner/repo. Slugs resolve only verified origin matches under ~/workspace and ~/unraid (including one organization/product directory level); missing or ambiguous matches require an absolute path. Discovery never clones. The resolved root binds local evidence to one checkout; GitHub identity derives from origin and explicit overrides must match. Local collection precedes sequential temporary-worktree probes. No fetch, cleanup, commit, rebase, push, merge of user branches, or test execution occurs. Merge probes create and remove disposable worktrees only. Treat interrupted probes as incomplete and inspect retained worktrees before retrying.

The output records conflicts, PR-head check runs, reviews, review threads, risk signals, branch limits and missing evidence. It never automatically marks ready_to_merge because required policy and implementation completeness need the agent's follow-up review. Apply the skill's readiness and merge-order rules to this evidence; inspect all unresolved thread pages and repository test conventions. Null stale fields are unknown. Old commits alone are not cleanup candidates. Non-GitHub remotes retain local evidence with forge state unverified.

Read-only evidence calls run with caller authority; Bash is broadly write-capable but this body restricts it to collectors and disposable probes. Source changes require rediscovering tools and validating fixtures. Default caps keep one invocation bounded; rerun with branch focus and deliberate limits for omitted branches/PRs. Full fixture tests are offline and do not qualify live upstream performance.

```js
async (input) => {
  let root=input.root; const skill=input.skill_dir;
  if(typeof root!=='string'||!(root.startsWith('/')||/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(root))||typeof skill!=='string'||!skill.startsWith('/'))throw Error('root must be an absolute checkout path or owner/repo; skill_dir must be absolute');
  for(const k of ['max_branches','pr_limit','max_probes'])if(input[k]!==undefined&&(!Number.isInteger(input[k])||input[k]<1))throw Error(k+' must be a positive integer');
  const q=s=>"'"+String(s).replace(/'/g,"'\\''")+"'";
  const max=Math.min(20,Math.max(1,input.max_branches??10));
  const limit=Math.min(10,Math.max(1,input.pr_limit??4));
  const failures=[];
  const safe=async(label,id,params)=>{try{return {ok:true,label,value:await callTool(id,params)}}catch(e){const f={ok:false,label,error:String(e).slice(0,600)};failures.push(f);return f}};
  const unwrap=v=>{if(typeof v==='string')return JSON.parse(v);const s=v?.stdout??v?.output??v?.content?.filter(x=>x.type==='text').map(x=>x.text).join('\n');return typeof s==='string'?JSON.parse(s):v};
  const command='python3 -c '+q("\nimport json,subprocess,sys,pathlib\nroot,skill,limit,branch=sys.argv[1:]\nimport re\n\ndef resolve_checkout(value, search_roots=None):\n if value.startswith('/'):\n  return str(pathlib.Path(value).resolve())\n if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+',value) or any(x in ('.','..') for x in value.split('/')):\n  raise ValueError('root must be an absolute checkout path or GitHub owner/repo')\n owner,name=value.split('/')\n roots=search_roots if search_roots is not None else [pathlib.Path.home()/'workspace',pathlib.Path.home()/'unraid']\n candidates=set()\n for parent in roots:\n  parent=pathlib.Path(parent)\n  for candidate in [parent/name,parent/owner/name]:\n   if candidate.is_dir(): candidates.add(candidate.resolve())\n  if parent.is_dir():\n   # Bounded second-level organization/product directories, not recursive worktree discovery.\n   children=sorted((x for x in parent.iterdir() if x.is_dir()),key=lambda x:x.name)\n   if len(children)>200: raise ValueError('Checkout discovery exceeds 200 directories; supply an absolute root')\n   for child in children:\n    candidate=child/name\n    if candidate.is_dir(): candidates.add(candidate.resolve())\n matches=[]\n for candidate in sorted(candidates):\n  top=subprocess.run(['git','-C',str(candidate),'rev-parse','--show-toplevel'],capture_output=True,text=True,timeout=5)\n  if top.returncode or pathlib.Path(top.stdout.strip()).resolve()!=candidate: continue\n  remote=subprocess.run(['git','-C',str(candidate),'remote','get-url','origin'],capture_output=True,text=True,timeout=5)\n  identity=re.search(r'github\\.com(?:/|:)([^/]+)/([^/]+?)(?:\\.git)?/?$',remote.stdout.strip())\n  if remote.returncode==0 and identity and '/'.join(identity.groups()).lower()==value.lower():matches.append(str(candidate))\n if not matches: raise ValueError('No verified local checkout for '+value+' under workspace or unraid; supply an absolute root. No repository was cloned.')\n if len(matches)>1: raise ValueError('Multiple verified checkouts for '+value+': '+', '.join(matches)+'; supply an absolute root')\n return matches[0]\ntry: root=resolve_checkout(root)\nexcept ValueError as e:\n print(json.dumps({'error':str(e)}));sys.exit(0)\nskill=pathlib.Path(skill)\nargs=['bash',str(skill/'scripts/repo_context.sh'),'--json','--no-fetch','--max-branches',limit]\nif branch: args+=['--branch',branch]\np=subprocess.run(args,cwd=root,capture_output=True,text=True,timeout=45)\nif p.returncode: raise RuntimeError(p.stderr[:500])\ns=json.loads(p.stdout)\ndef stdout(c): return c.get('stdout','') if c and c.get('exit')==0 else None\ndef clip(c,n=1200):\n if not c:return None\n out=dict(c);out['truncated']=len(out.get('stdout',''))>n;out['stdout']=out.get('stdout','')[:n];return out\ncommands={c['label']:c for c in s['commands']}\nrows=[]\nfor b in s['branches']:\n row={k:b.get(k) for k in ['name','sha','upstream','track','committerdate','worktreepath','base','base_rationale','limited','stale_evidence','risk_signals']}\n row['head']=subprocess.check_output(['git','rev-parse','--verify','refs/heads/'+b['name']],cwd=root,text=True).strip()\n row['ahead_behind_base']=stdout(b.get('ahead_behind'))\n row['changed_files']=stdout(b.get('diff_names'))\n row['diff_stat']=clip(b.get('diff_stat'))\n rows.append(row)\nwts=[]\nfor w in s['worktrees']:\n row={k:w.get(k) for k in ['path','branch','head','exists','locked','prunable','detached']}\n raw=stdout(w.get('status_porcelain_v2'));row['dirty']=None if raw is None else any(x and not x.startswith('#') for x in raw.splitlines());row['status']=clip(w.get('status_porcelain_v2'))\n wts.append(row)\nprint(json.dumps({'root':s['root'],'generated_at':s['generated_at'],'default_base':s['default_base'],'default_base_rationale':s['default_base_rationale'],'current_branch':subprocess.check_output(['git','branch','--show-current'],cwd=root,text=True).strip(),'remote':subprocess.check_output(['git','remote','get-url','origin'],cwd=root,text=True,stderr=subprocess.DEVNULL).strip() if subprocess.run(['git','remote','get-url','origin'],cwd=root,capture_output=True).returncode==0 else None,'branches':rows,'worktrees':wts,'branches_total':s['branches_total'],'branches_truncated':s['branches_truncated'],'command_failures':[{'label':c['label'],'exit':c['exit'],'stderr':c.get('stderr','')[:300]} for c in s['commands'] if c['exit']!=0],'remote_refs_refreshed':False}))\n")+' '+[root,skill,max,input.branch??''].map(q).join(' ');
  const localCall=await safe('local_snapshot','claude-macpoo::Bash',{command,timeout:55000,description:'Collect checkout branch and worktree evidence'});
  if(!localCall.ok)return {ok:false,classification:'unknown',failures};
  let local;try{local=unwrap(localCall.value)}catch(e){return {ok:false,classification:'unknown',error:'Local snapshot was unavailable or truncated',failures}}
  if(local.error)return {ok:false,classification:'unknown',error:local.error};
  root=local.root;
  const match=String(local.remote??'').match(/github\.com(?:/|:)([^/]+)\/([^/]+?)(?:\.git)?$/);
  const owner=input.owner??match?.[1],repo=input.repo??match?.[2];
  if(match&&((input.owner&&input.owner!==match[1])||(input.repo&&input.repo!==match[2])))return {ok:false,local,error:'Requested GitHub repository does not match origin'};
  let prs=[],total=null;
  if(owner&&repo){
    const search=await safe('open_prs','github::search_pull_requests',{owner,repo,query:'is:open',perPage:limit,page:1});
    if(search.ok){const v=unwrap(search.value);prs=v.items??[];total=v.total_count??null}
  }else failures.push({label:'forge',error:'No verified GitHub origin; CI is unverified'});
  const prEvidence=[];
  for(const pr of prs.slice(0,limit)){
    const number=pr.number;
    const calls=await codemode.batch([
      ()=>safe('pr_'+number,'github::pull_request_read',{owner,repo,pullNumber:number,method:'get'}),
      ()=>safe('checks_'+number,'github::pull_request_read',{owner,repo,pullNumber:number,method:'get_check_runs',perPage:100,page:1}),
      ()=>safe('reviews_'+number,'github::pull_request_read',{owner,repo,pullNumber:number,method:'get_reviews',perPage:100,page:1}),
      ()=>safe('threads_'+number,'github::pull_request_read',{owner,repo,pullNumber:number,method:'get_review_comments',perPage:100}),
    ]);
    const values=calls.ok.map(x=>x.value); const data=values[0]?.ok?unwrap(values[0].value):null;
    const checkData=values[1]?.ok?unwrap(values[1].value):null;
    const checks=Array.isArray(checkData)?checkData:checkData?.check_runs??[];
    const reviewData=values[2]?.ok?unwrap(values[2].value):null;
    const threads=values[3]?.ok?unwrap(values[3].value):null;
    const runsCall=data?.head?.ref?await safe('runs_'+number,'github-actions::actions_list',{owner,repo,method:'list_workflow_runs',perPage:30,page:1,workflow_runs_filter:{branch:data.head.ref}}):null;
    const runsData=runsCall?.ok?unwrap(runsCall.value):null;
    const allRuns=Array.isArray(runsData)?runsData:runsData?.workflow_runs??[];
    const runs=allRuns.filter(r=>r.head_sha===data?.head?.sha).map(r=>({id:r.id,name:r.name,head_sha:r.head_sha,status:r.status,conclusion:r.conclusion,url:r.html_url}));
    prEvidence.push({number,url:data?.html_url??pr.html_url,branch:data?.head?.ref,head:data?.head?.sha,base:data?.base?.ref,draft:data?.draft,mergeable:data?.mergeable,checks:checks.map(c=>({name:c.name,status:c.status,conclusion:c.conclusion,head_sha:c.head_sha,url:c.html_url})),workflow_runs:runs,workflow_runs_complete:runsCall?.ok===true&&allRuns.length<30&&(runsData?.total_count??allRuns.length)<=allRuns.length,checks_complete:values[1]?.ok===true&&checks.length<100&&(checkData?.total_count??checks.length)<=checks.length,reviews_preview:JSON.stringify(reviewData).slice(0,1800),threads_preview:JSON.stringify(threads).slice(0,1800),review_data_truncated:JSON.stringify(reviewData).length>1800||JSON.stringify(threads).length>1800,review_policy_verified:false});
  }
  const requests=local.branches.filter(b=>!b.limited&&b.name!==local.default_base?.replace(/^origin\//,'')&&!['main','master','trunk','develop'].includes(b.name)&&b.stale_evidence?.merged_into_base!==true).slice(0,Math.min(input.max_probes??1,10)).map(b=>{const pr=prEvidence.find(p=>p.branch===b.name);return {branch:b.name,base:pr?.base?'origin/'+pr.base:b.base}}).filter(r=>r.base);
  let probes=[];
  if(input.check_mergeability!==false&&requests.length){
    const r=await safe('merge_probes','claude-macpoo::Bash',{command:'python3 -c '+q("\nimport json,subprocess,sys\nroot,skill,requests=sys.argv[1:]; results=[]\nfor r in json.loads(requests):\n base=r['base'];branch=r['branch']\n # Prefer the PR base, retaining local fallback only when no PR base was supplied.\n ref=subprocess.run(['git','rev-parse','--verify','--end-of-options',base],cwd=root,capture_output=True,text=True)\n if ref.returncode:results.append({'branch':branch,'mergeable':'unknown','error':'Base ref unavailable: '+base});continue\n before=subprocess.check_output(['git','worktree','list','--porcelain'],cwd=root,text=True)\n p=subprocess.run(['bash',skill+'/scripts/check_mergeability.sh',base,branch],cwd=root,capture_output=True,text=True,timeout=45)\n after=subprocess.check_output(['git','worktree','list','--porcelain'],cwd=root,text=True)\n merged='yes' if 'mergeable: yes' in p.stdout else 'no' if 'failure_kind: conflicts' in p.stdout else 'unknown'\n results.append({'branch':branch,'base':base,'base_sha':ref.stdout.strip(),'mergeable':merged,'exit':p.returncode,'output':p.stdout[-2200:],'stderr':p.stderr[-300:],'cleanup_confirmed':before==after})\nprint(json.dumps({'probes':results}))\n")+' '+[root,skill,JSON.stringify(requests)].map(q).join(' '),timeout:55000,description:'Probe branch merges and verify temporary worktree cleanup'});
    if(r.ok)try{probes=unwrap(r.value).probes}catch(e){failures.push({label:'merge_probes',error:'Probe output unavailable'})}
  }
  const rows=local.branches.map(b=>{
    const wt=local.worktrees.filter(w=>w.branch===b.name),pr=prEvidence.find(p=>p.branch===b.name),probe=probes.find(p=>p.branch===b.name);
    const primary=['main','master','trunk','develop',local.default_base?.replace(/^origin\//,'')].includes(b.name);
    const next=[];let status='unknown';
    if(wt.some(w=>w.dirty===true))next.push('Preserve and commit or resolve dirty work before merge');
    if(wt.some(w=>w.locked))next.push('Review worktree lock ownership');
    if(pr?.draft)next.push('Complete draft PR');
    if(pr&&b.head!==pr.head)next.push('Reconcile local HEAD with PR head');
    if([...(pr?.checks??[]),...(pr?.workflow_runs??[])].some(c=>['failure','cancelled','timed_out','action_required'].includes(c.conclusion)))next.push('Resolve failing checks');
    if(pr?.checks.some(c=>c.status!=='completed'))next.push('Wait for running checks');
    if(pr?.base&&b.base?.replace(/^origin\//,'')!==pr.base)next.push('Inspect patch and risk signals against the PR base; initial snapshot used repository default');
    if(pr&&!pr.checks_complete)next.push('Complete check-run pagination');
    if(!probe||probe.mergeable==='unknown'||!probe.cleanup_confirmed)next.push('Obtain complete merge probe and cleanup evidence');
    const risks=Object.entries(b.risk_signals??{}).filter(([k,v])=>Array.isArray(v)?v.length:Boolean(v));
    if(risks.length)next.push('Review flagged patch and overlapping files');
    next.push('Confirm required tests and review policy; no local tests run');
    if(!primary){
      if(probe?.mergeable==='no'||pr?.mergeable===false)status='conflicted';
      else if(wt.some(w=>w.dirty===true)||pr?.draft||next.some(x=>x==='Resolve failing checks'||x==='Reconcile local HEAD with PR head'))status='needs_work';
      else if(b.stale_evidence?.merged_into_base===true&&wt.every(w=>w.dirty===false&&w.exists!==false&&!w.locked))status='stale_cleanup_candidate';
      else if(pr||risks.length)status='needs_work';
    }
    return {branch:b.name,primary,status,head:b.head,base:probe?.base??b.base,upstream:b.upstream,ahead_behind_base:b.ahead_behind_base,worktrees:wt,stale_evidence:b.stale_evidence,risk_signals:b.risk_signals,merge_probe:probe??null,pr:pr??null,next_actions:primary?[]:next,limited:b.limited===true};
  });
  const ready=rows.filter(r=>!r.primary&&r.status==='ready_to_merge');
  return {schema_version:2,snippet:'repo-status-gh-pulse',ok:failures.length===0,complete:false,observed_at:local.generated_at,repository:owner&&repo?owner+'/'+repo:null,root:local.root,current_branch:local.current_branch,branches:rows,detached_worktrees:local.worktrees.filter(w=>w.detached),merge_order:ready.map(r=>r.branch),merge_order_note:ready.length?'Review dependencies before applying this order':'None verified ready; review blockers and dependency/overlap evidence before proposing an order',coverage:{branches_total:local.branches_total,branches_collected:rows.filter(r=>!r.limited).length,branches_truncated:local.branches_truncated,open_prs_total:total,open_prs_inspected:prEvidence.length,pr_list_complete:total!==null&&total<=prEvidence.length,remote_refs_refreshed:false,tests_run:false,review_policy_verified:false},failures,local_command_failures:local.command_failures,limits:['Local refs were not fetched; remote base freshness is unverified','Risk markers are leads, not automatic blockers','Required review/test policy needs human or repository-specific interpretation','Missing or dirty worktrees never authorize deletion']};
}
```
