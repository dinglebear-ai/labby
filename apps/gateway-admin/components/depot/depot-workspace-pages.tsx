'use client'

import { useState } from 'react'
import {
  Bot, Box, CheckCircle2, CirclePlus, Clock3, Code2, Download, ExternalLink,
  FileArchive, FileCode2, FileJson, FileText, Grid2X2, Inbox, Layers3, List,
  Pause, Play, Search, Square, Table2, Upload, X, ChevronDown, ArrowUpDown, ScrollText,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { ArtifactComposer } from './artifact-composer'
import { DevContainersPageContent } from './dev-containers-page-content'
import { NewAgentSessionWizard } from './new-agent-session-wizard'
import { AlpineMark, CodexMark, DebianMark, UbuntuMark } from './brand-marks'

const demoArtifacts = [
  ['Skill', 'repo-triage', 'Cluster open PRs and issues, then draft a triage note.', '#review · #github'],
  ['Agent', 'rust-reviewer', 'Review Rust changes and flag unsafe blocks with rationale.', '#rust · #review'],
  ['MCP', 'labby', 'Gateway control plane exposing scoped upstream MCP capabilities.', '#gateway · #mcp'],
  ['Command', '/ship', 'Run release checks and prepare a release draft.', '#release'],
  ['Loadout', 'operator-console', 'Operational tools for logs, services, and infrastructure.', '#ops · #homelab'],
  ['Snippet', 'gateway-reconcile', 'Probe disconnected servers and summarize the delta.', '#gateway'],
]

export function LibraryTabs({ active }: { active: 'artifacts' | 'loadouts' | 'snippets' }) {
  const tabs = [
    ['artifacts', '/library', 'Artifacts'],
    ['loadouts', '/loadouts', 'Loadouts'],
    ['snippets', '/snippets', 'Snippets'],
  ] as const
  return <nav aria-label="Library sections" className="flex gap-6 border-b border-aurora-border-subtle px-3">
    {tabs.map(([id, href, label]) => <a key={id} href={href} aria-current={active === id ? 'page' : undefined} className="border-b-2 border-transparent px-1 py-3 text-sm font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary">{label}</a>)}
  </nav>
}

function PageFrame({ children }: { children: React.ReactNode }) {
  return <div className={`${AURORA_PAGE_SHELL} flex-1`}><div className={`${AURORA_PAGE_FRAME} space-y-4`}>{children}</div></div>
}

export function LibraryPage() {
  return <><AppHeader breadcrumbs={[{ label: 'Depot' }, { label: 'Library' }]} /><PageFrame>
    <LibraryTabs active="artifacts" />
    <ConsoleHero eyebrow="Depot · Library" title="Library" pulse={{ color: 'var(--aurora-warn)', label: 'preview layout' }} actions={<div className="flex gap-2"><Button variant="outline">Backup all</Button><Button><CirclePlus />New loadout</Button></div>} stats={[
      { label: 'Artifacts', value: '102,745', icon: <Box size={12}/> },
      { label: 'Loadouts', value: '4', icon: <Layers3 size={12}/> },
      { label: 'Snippets', value: '6', icon: <FileCode2 size={12}/> },
      { label: 'Authority', value: 'Read only', icon: <CheckCircle2 size={12}/> },
    ]}/>
    <div className="grid gap-4 lg:grid-cols-[210px_1fr]">
      <aside className="space-y-3"><DashboardPanel title="Views"><div className="space-y-1 text-sm"><button className="w-full rounded-aurora-1 bg-aurora-surface-muted px-3 py-2 text-left text-aurora-text-primary">All artifacts</button>{['Published', 'MCP servers', 'Skills', 'Agents', 'Commands'].map(x => <button key={x} className="w-full px-3 py-2 text-left text-aurora-text-muted hover:text-aurora-text-primary">{x}</button>)}</div></DashboardPanel></aside>
      <DashboardPanel title="Artifacts" action={<div className="relative"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input aria-label="Filter library" className="h-9 rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low pl-9 pr-3 text-sm" placeholder="Filter artifacts…"/></div>}>
        <div className="divide-y divide-aurora-border-subtle">{demoArtifacts.map(([kind, name, description, tags]) => <div key={name} className="grid gap-2 px-2 py-3 sm:grid-cols-[100px_1fr_180px] sm:items-center"><Badge variant="outline">{kind}</Badge><div><div className="font-semibold text-aurora-text-primary">{name}</div><div className="text-xs text-aurora-text-muted">{description}</div></div><div className="text-xs text-aurora-accent-primary">{tags}</div></div>)}</div>
      </DashboardPanel>
    </div>
  </PageFrame></>
}

export function CreatePage() {
  return <ArtifactComposer />
}

const agents = [
  ['Running', 'Reconcile the gateway catalog', 'operator-console', 'platform-base', 'Claude Code', '12m'],
  ['Running', 'Research Depot publishing contracts', 'research-workbench', 'rust-heavy', 'Codex', '4m'],
  ['Completed', 'Verify Skills build matrix', 'operator-console', 'platform-base', 'Codex', '2m'],
  ['Failed', 'Draft plugin manifest', 'research-workbench', 'edge-minimal', 'Claude Code', '41s'],
]

export function AgentsPage() {
  const [selected, setSelected] = useState<string[] | null>(null)
  const [creating, setCreating] = useState(false)
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Agents' }]} /><PageFrame><ConsoleHero eyebrow="Workspace · Agents" title="Agents" pulse={{ color: 'var(--aurora-success)' }} actions={<Button onClick={() => setCreating(true)}><CirclePlus/>New session</Button>} stats={[{label:'Running',value:2,icon:<Play size={12}/>,tone:'var(--aurora-success)'},{label:'Completed',value:1,icon:<CheckCircle2 size={12}/>},{label:'Failed',value:1,icon:<Clock3 size={12}/>,tone:'var(--aurora-error)'},{label:'Median',value:<><span>4m 12s</span><small className="ml-1 text-[10px] font-normal text-aurora-text-muted">per session</small></>,icon:<Clock3 size={12}/>} ]}/><AgentsCollection rows={agents} onSelect={setSelected}/></PageFrame>
    <AgentSessionSheet session={selected} onOpenChange={(open) => !open && setSelected(null)} />
    <NewAgentSessionWizard open={creating} onOpenChange={setCreating} />
  </>
}

function AgentSessionSheet({ session, onOpenChange }: { session: string[] | null; onOpenChange: (open: boolean) => void }) {
  const running = session?.[0] === 'Running'
  const events: Array<[typeof FileText, string, string, string]> = [
    [FileText,'Read','src/gateway/reconcile.rs','412 lines'],
    [Search,'Searched','callers of reconcile_all','7 matches · 38ms'],
    [Bot,'Agent update','Splitting the fleet scan into a dirty-set pass while retaining a periodic full sweep.',''],
    [FileCode2,'Wrote','src/gateway/reconcile.rs','+148 −62'],
    [Play,'Ran','cargo test reconcile','24 passed · 3.1s'],
  ]
  return <Sheet open={Boolean(session)} onOpenChange={onOpenChange}>
    <SheetContent className="!w-[min(92vw,680px)] border-aurora-border-strong bg-aurora-panel-medium p-0 sm:!max-w-[680px]">
      <SheetHeader className="border-b border-aurora-border-subtle bg-aurora-panel-strong px-6 py-5">
        <div className="flex items-center gap-2"><span className={`size-2 rounded-full ${running ? 'bg-aurora-success shadow-[0_0_8px_var(--aurora-success)]' : session?.[0] === 'Failed' ? 'bg-aurora-error' : 'bg-aurora-text-muted'}`} /><SheetTitle className="text-xl text-aurora-text-primary">{session?.[1] ?? 'Agent session'}</SheetTitle></div>
        <SheetDescription className="pl-4 text-aurora-text-muted">tootie-tv/labby · jmagar · {session?.[5]}</SheetDescription>
      </SheetHeader>
      <div className="grid grid-cols-2 border-b border-aurora-border-subtle bg-aurora-panel-low sm:grid-cols-4">
        {[['Loadout',session?.[2]],['Container',session?.[3]],['Harness',session?.[4]],['Status',session?.[0]]].map(([label,value])=><div key={label} className="border-r border-aurora-border-subtle px-5 py-4 last:border-r-0"><span className="block text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{label}</span><strong className="mt-1 block text-xs text-aurora-text-primary">{value}</strong></div>)}
      </div>
      <div className="flex-1 overflow-y-auto px-6 py-5">
        <div className="mb-6 rounded-aurora-2 border border-aurora-error/25 bg-aurora-error/5 px-4 py-3 text-sm leading-6 text-aurora-text-primary">The reconcile loop re-probes every server on each tick. Keep the error semantics while making the pass incremental.</div>
        <ol className="relative ml-3 border-l border-aurora-border-strong pl-6">
          {events.map(([Icon,verb,target,meta],index)=><li key={`${verb}-${target}`} className="relative pb-6 last:pb-0"><span className="absolute -left-[39px] grid size-7 place-items-center rounded-full border border-aurora-accent-primary/50 bg-aurora-panel-medium text-aurora-accent-primary"><Icon className="size-3.5" /></span><div className="flex items-start justify-between gap-4"><p className={index===2?'font-semibold leading-6 text-aurora-text-primary':'text-sm text-aurora-text-primary'}><span className="text-aurora-text-muted">{verb}</span> {target}</p>{meta?<span className="shrink-0 text-xs text-aurora-text-muted">{meta}</span>:null}</div></li>)}
          {running ? <li className="relative text-sm font-semibold text-aurora-error"><span className="absolute -left-[39px] grid size-7 place-items-center rounded-full border border-aurora-error/50 bg-aurora-panel-medium"><ActivitySpinner /></span>Working…</li> : null}
        </ol>
      </div>
      <SheetFooter className="flex-row items-center justify-between border-t border-aurora-border-subtle bg-aurora-panel-strong px-6 py-4"><span className="truncate text-xs text-aurora-text-muted">{session?.[2]} · {session?.[3]} · {session?.[4]}</span><div className="flex gap-2"><Button variant="outline"><ExternalLink />Copy transcript</Button>{running?<Button variant="outline" className="border-aurora-error/40 text-aurora-error"><Square />Stop session</Button>:<Button><Play />Run again</Button>}</div></SheetFooter>
    </SheetContent>
  </Sheet>
}

function ActivitySpinner() { return <span className="size-3 animate-pulse rounded-full bg-aurora-error" /> }

const tasks = [
  ['Armed', 'Loadout Scope Audit', 'Daily · 02:00', 'project-a', 'in 6h', 'Flag write-capable tools added since the last run.', 'passed'],
  ['Armed', 'Error Forensics Digest', 'Mon, Thu · 07:00', 'platform', 'in 2d', 'Cluster gateway errors and post a concise digest to Activity.', 'passed'],
  ['Armed', 'Upstream Drift Sweep', 'Weekly · Sun 03:00', 'shared', 'in 4d', 'Merge clean upstream updates and open reviewable diffs for the rest.', 'partial'],
  ['Armed', 'Container Rebuild', 'Weekly · Sat 01:00', 'platform', 'in 3d', 'Rebuild every image against the pinned toolchain set.', 'passed'],
  ['Paused', 'Dependency Bump PR', 'Daily · 05:00', 'project-b', 'paused', 'Open one safe dependency update pull request per repository.', 'failed'],
]
export function TasksPage() {
  const [rows, setRows] = useState(tasks)
  const [selected, setSelected] = useState<string[] | null>(null)
  const [creating, setCreating] = useState(false)
  const [name,setName]=useState(''),[definition,setDefinition]=useState(''),[schedule,setSchedule]=useState('Daily · 09:00'),[loadout,setLoadout]=useState('operator-console')
  const create=()=>{if(!name.trim()||!definition.trim())return;setRows(c=>[['Armed',name.trim(),schedule,loadout,'tomorrow',definition.trim(),'pending'],...c]);setName('');setDefinition('');setCreating(false)}
  return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Tasks'}]}/><PageFrame><ConsoleHero eyebrow="Team · Schedules" title="Tasks" description="Recurring agent runs. Each task carries its own loadout, container and repository, and reports back into Activity when it finishes." actions={<Button onClick={()=>setCreating(true)}><CirclePlus/>New Task</Button>} stats={[{label:'Scheduled',value:rows.length,icon:<Clock3 size={12}/>},{label:'Armed',value:rows.filter(row=>row[0]==='Armed').length,icon:<CheckCircle2 size={12}/>,tone:'var(--aurora-success)'},{label:'Next run',value:<><span className="text-aurora-accent-primary">02:00</span><small className="ml-1 text-[10px] font-normal text-aurora-text-muted">Scope Audit</small></>,icon:<Play size={12}/>},{label:'Failures',value:<><span>1</span><small className="ml-1 text-[10px] font-normal text-aurora-text-muted">last 7 days</small></>,icon:<Clock3 size={12}/>,tone:'var(--aurora-error)'}]}/><TasksCollection rows={rows} setRows={setRows} onSelect={setSelected}/></PageFrame>
    <TaskDialog row={selected} onOpenChange={open=>!open&&setSelected(null)} onSave={updated=>{setRows(c=>c.map(row=>row===selected?updated:row));setSelected(updated)}}/>
    <Dialog open={creating} onOpenChange={setCreating}><DialogContent className="border-aurora-border-strong bg-aurora-panel-medium"><DialogTitle>New task</DialogTitle><DialogDescription>Schedule a reusable agent run.</DialogDescription><TaskFields name={name} setName={setName} definition={definition} setDefinition={setDefinition} schedule={schedule} setSchedule={setSchedule} loadout={loadout} setLoadout={setLoadout}/><Button onClick={create} disabled={!name.trim()||!definition.trim()}><CirclePlus/>Create task</Button></DialogContent></Dialog>
  </>
}

function SortHead({children,onClick}:{children:React.ReactNode;onClick:()=>void}){return <th className="px-3 py-2 text-left"><button type="button" onClick={onClick} className="flex items-center gap-1 text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted hover:text-aurora-text-primary">{children}<ArrowUpDown className="size-3 opacity-45"/></button></th>}

function AgentsCollection({rows,onSelect}:{rows:string[][];onSelect:(row:string[])=>void}){
  const [filter,setFilter]=useState('All'),[sort,setSort]=useState(1),[view,setView]=useState<ViewMode>('table')
  const shown=[...rows].filter(row=>filter==='All'||row[0]===filter).sort((a,b)=>a[sort].localeCompare(b[sort]))
  return <DashboardPanel title="Sessions" action={<div className="flex items-center gap-3"><div className="flex gap-1">{['All','Running','Completed','Failed'].map(item=><button key={item} type="button" onClick={()=>setFilter(item)} aria-pressed={filter===item} className="rounded-full border border-aurora-border-subtle px-3 py-1 text-[10px] font-semibold text-aurora-text-muted aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg">{item}</button>)}</div><ViewModes value={view} onChange={setView}/></div>}>
    {view==='table'?<div className="overflow-x-auto"><table className="w-full text-sm"><thead><tr className="border-b border-aurora-border-subtle">{['Status','Session','Loadout','Container','Harness','Elapsed'].map((head,index)=><SortHead key={head} onClick={()=>setSort(index)}>{head}</SortHead>)}</tr></thead><tbody>{shown.map(row=><tr key={row[1]} tabIndex={0} onClick={()=>onSelect(row)} onKeyDown={event=>event.key==='Enter'&&onSelect(row)} className="cursor-pointer border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg"><td className="px-3 py-3"><StatusDot status={row[0]}/></td><td className="px-3 py-3 font-semibold text-aurora-text-primary">{row[1]}</td><td className="px-3 py-3"><Badge variant="outline" className="text-aurora-accent-primary">{row[2]}</Badge></td><td className="px-3 py-3 text-aurora-text-muted"><span className="flex items-center gap-2"><ProductMark kind={row[3]}/>{row[3]}</span></td><td className="px-3 py-3 text-aurora-text-muted"><span className="flex items-center gap-2"><ProductMark kind={row[4]}/>{row[4]}</span></td><td className="px-3 py-3 text-aurora-text-muted">{row[5]}</td></tr>)}</tbody></table></div>:<div className={view==='cards'?'grid gap-3 md:grid-cols-2 xl:grid-cols-3':'divide-y divide-aurora-border-subtle'}>{shown.map(row=><button key={row[1]} onClick={()=>onSelect(row)} className="w-full rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-left"><StatusDot status={row[0]}/><strong className="mt-2 block text-aurora-text-primary">{row[1]}</strong><span className="mt-1 block text-xs text-aurora-text-muted">{row.slice(2).join(' · ')}</span></button>)}</div>}
  </DashboardPanel>
}

function StatusDot({status}:{status:string}){const color=status==='Running'||status==='Armed'?'bg-aurora-success':status==='Failed'?'bg-aurora-error':status==='Paused'?'bg-aurora-warn':'bg-aurora-text-muted';return <span role="img" aria-label={status} title={status} className={`block size-2 rounded-full ${color}`}/>}

function ProductMark({kind}:{kind:string}){
  if(kind==='platform-base')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><UbuntuMark className="size-3.5 fill-current"/></span>
  if(kind==='rust-heavy')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><DebianMark className="size-3.5 fill-current"/></span>
  if(kind==='edge-minimal')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><AlpineMark className="size-3.5 fill-current"/></span>
  if(kind==='Claude Code')return <span aria-label="Claude" className="grid size-5 place-items-center rounded bg-white/5 text-[10px] font-black text-aurora-text-primary">AI</span>
  if(kind==='Codex')return <span className="grid size-5 place-items-center rounded bg-white/5 text-aurora-text-primary"><CodexMark className="size-3.5 fill-current"/></span>
  return <span aria-label="Gemini" className="grid size-5 place-items-center rounded bg-white/5 text-sm text-aurora-text-primary">✦</span>
}

function TasksCollection({rows,setRows,onSelect}:{rows:string[][];setRows:React.Dispatch<React.SetStateAction<string[][]>>;onSelect:(row:string[])=>void}){
  const [filter,setFilter]=useState('All')
  const shown=rows.filter(row=>filter==='All'||row[0]===filter)
  const toggle=(target:string[])=>setRows(current=>current.map(row=>row===target?[row[0]==='Armed'?'Paused':'Armed',...row.slice(1)]:row))
  return <DashboardPanel title="Scheduled" action={<div className="flex gap-1">{['All','Armed','Paused'].map(item=><button key={item} type="button" onClick={()=>setFilter(item)} aria-pressed={filter===item} className="rounded-full border border-aurora-border-subtle px-3 py-1 text-[10px] font-semibold text-aurora-text-muted aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg">{item}</button>)}</div>}><div className="overflow-x-auto"><table className="w-full text-sm"><thead><tr className="border-b border-aurora-border-subtle">{['On','Task','Schedule','Loadout','Last run','Next',''].map(head=><th key={head} className="px-3 py-2 text-left text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{head}</th>)}</tr></thead><tbody>{shown.map(row=><tr key={row[1]} className="border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg"><td className="px-3 py-2"><button type="button" role="switch" aria-checked={row[0]==='Armed'} aria-label={`${row[0]==='Armed'?'Pause':'Arm'} ${row[1]}`} onClick={()=>toggle(row)} className="relative h-5 w-9 rounded-full border border-aurora-border-strong bg-aurora-control-surface aria-checked:border-aurora-accent-primary aria-checked:bg-aurora-accent-primary"><span className="absolute left-0.5 top-0.5 size-3.5 rounded-full bg-aurora-text-muted transition-transform [[aria-checked=true]_&]:translate-x-4 [[aria-checked=true]_&]:bg-aurora-page-bg"/></button></td><td className="px-3 py-2"><button onClick={()=>onSelect(row)} className="text-left"><strong className="block text-xs text-aurora-text-primary">{row[1]}</strong><span className="block max-w-[30rem] truncate text-[10px] text-aurora-text-muted">{row[5]}</span></button></td><td className="px-3 py-2 text-xs text-aurora-text-muted">◷ {row[2]}</td><td className="px-3 py-2"><Badge variant="outline" className="text-aurora-accent-primary">#{row[3]}</Badge></td><td className="px-3 py-2"><span className={`inline-flex items-center gap-1.5 text-[10px] font-semibold ${row[6]==='failed'?'text-aurora-error':row[6]==='partial'?'text-aurora-warn':'text-aurora-success'}`}><span className="size-1.5 rounded-full bg-current"/>{row[6]}</span></td><td className="px-3 py-2 text-xs text-aurora-text-muted">{row[4]}</td><td className="px-3 py-2"><Button variant="ghost" size="icon-sm" aria-label={`View logs for ${row[1]}`} asChild><a href="/logs"><ScrollText className="size-4"/></a></Button></td></tr>)}</tbody></table></div></DashboardPanel>
}

function SelectField({label,value,onChange,children}:{label:string;value:string;onChange:(value:string)=>void;children:React.ReactNode}){return <label className="text-xs text-aurora-text-muted">{label}<span className="relative mt-2 block"><select value={value} onChange={event=>onChange(event.target.value)} className="h-10 w-full appearance-none rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface pl-3 pr-10 text-sm text-aurora-text-primary">{children}</select><ChevronDown className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/></span></label>}
function TaskFields({name,setName,definition,setDefinition,schedule,setSchedule,loadout,setLoadout}:{name:string;setName:(v:string)=>void;definition:string;setDefinition:(v:string)=>void;schedule:string;setSchedule:(v:string)=>void;loadout:string;setLoadout:(v:string)=>void}){return <><label className="text-xs font-semibold text-aurora-text-muted">Task name<input autoFocus value={name} onChange={event=>setName(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="Weekly gateway review"/></label><label className="text-xs font-semibold text-aurora-text-muted">Define the task<textarea value={definition} onChange={event=>setDefinition(event.target.value)} rows={4} className="mt-2 w-full resize-none rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-3 text-sm text-aurora-text-primary" placeholder="Describe exactly what the agent should do and what a successful run produces."/></label><div className="grid grid-cols-2 gap-3"><SelectField label="Schedule" value={schedule} onChange={setSchedule}><option>Daily · 09:00</option><option>Daily · 02:00</option><option>Weekly · Monday</option><option>Weekly · Sun 03:00</option></SelectField><SelectField label="Loadout" value={loadout} onChange={setLoadout}><option>operator-console</option><option>research-workbench</option><option>project-a</option><option>project-b</option><option>platform</option><option>shared</option></SelectField></div></>}

function TaskDialog({row,onOpenChange,onSave}:{row:string[]|null;onOpenChange:(open:boolean)=>void;onSave:(row:string[])=>void}){const [editing,setEditing]=useState(false),[name,setName]=useState(''),[definition,setDefinition]=useState(''),[schedule,setSchedule]=useState('Daily · 09:00'),[loadout,setLoadout]=useState('operator-console');const begin=()=>{if(!row)return;setName(row[1]);setSchedule(row[2]);setLoadout(row[3]);setDefinition(row[5]);setEditing(true)};return <Dialog open={Boolean(row)} onOpenChange={open=>{onOpenChange(open);if(!open)setEditing(false)}}><DialogContent className="border-aurora-border-strong bg-aurora-panel-medium"><DialogTitle>{editing?'Edit task':row?.[1]??'Task'}</DialogTitle><DialogDescription>{editing?'Change the task definition, schedule, or loadout.':'Workspace details and controls.'}</DialogDescription>{editing?<TaskFields name={name} setName={setName} definition={definition} setDefinition={setDefinition} schedule={schedule} setSchedule={setSchedule} loadout={loadout} setLoadout={setLoadout}/>:<><p className="rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-3 text-sm leading-6 text-aurora-text-primary">{row?.[5]}</p><dl className="divide-y divide-aurora-border-subtle rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low px-4">{[['State',row?.[0]],['Schedule',row?.[2]],['Loadout',row?.[3]],['Next run',row?.[4]]].map(([label,value])=><div key={label} className="flex justify-between gap-4 py-3 text-sm"><dt className="text-aurora-text-muted">{label}</dt><dd className="font-medium text-aurora-text-primary">{value}</dd></div>)}</dl></>}<div className="flex flex-wrap justify-end gap-2">{editing?<><Button variant="outline" onClick={()=>setEditing(false)}>Cancel</Button><Button onClick={()=>{if(row)onSave([row[0],name,schedule,loadout,row[4],definition,row[6]]);setEditing(false)}}>Save changes</Button></>:<><Button variant="outline" onClick={begin}>Edit task</Button><Button variant="outline"><Pause/>Pause task</Button><Button><Play/>Run now</Button><Button variant="outline" asChild><a href="/logs"><ScrollText/>View last run logs</a></Button></>}</div></DialogContent></Dialog>}

const files = [
  ['Data','fleet-snapshot.json','stash://me/fleet-snapshot.json','412 KB','2h ago'],
  ['Doc','reconcile-notes.md','stash://me/reconcile-notes.md','18 KB','5h ago'],
  ['Archive','gateway-trace.log','stash://me/gateway-trace.log','96 MB','1d ago'],
  ['Code','schema.prisma','stash://me/schema.prisma','11 KB','1d ago'],
]
export function StashPage() { return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Stash'}]}/><PageFrame><ConsoleHero eyebrow="Workspace · Stash" title="Stash" pulse={{color:'var(--aurora-success)'}} actions={<Button><Upload/>Upload</Button>} stats={[{label:'Files',value:4,icon:<Inbox size={12}/>},{label:'Size',value:'96.4 MB',icon:<FileText size={12}/>},{label:'Shared',value:2,icon:<Bot size={12}/>} ]}/><button className="group w-full rounded-aurora-2 border border-dashed border-aurora-accent-primary/50 bg-[linear-gradient(135deg,color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent),color-mix(in_srgb,var(--aurora-success)_7%,transparent))] p-7 text-center text-sm text-aurora-text-muted transition-colors hover:border-aurora-accent-primary"><Upload className="mx-auto mb-2 size-6 text-aurora-accent-primary"/><strong className="block text-aurora-text-primary">Drop files here or browse</strong>Available to agents through <code className="text-aurora-success">stash://</code></button><StashCollection/></PageFrame></> }

export function DevContainersPage() { return <><AppHeader breadcrumbs={[{label:'Workspace'},{label:'Dev Containers'}]}/><PageFrame><DevContainersPageContent /></PageFrame></> }

const logRows = [
  ['Info','gateway','catalog reconciled · 2 healthy upstreams'],
  ['Info','context7','tools/call completed · 200 in 1.7s'],
  ['Warn','claude-macpoo','SSH session reconnected'],
  ['Debug','depot','catalog search returned 50 of 102745'],
  ['Info','labby','skills catalog refreshed'],
]
export function LogsPage() { return <><AppHeader breadcrumbs={[{label:'Logs'}]}/><PageFrame><ConsoleHero eyebrow="Observability" title="Logs" pulse={{color:'var(--aurora-warn)',label:'preview data'}}/><DashboardPanel title="Event stream" action={<div className="flex gap-2"><Button variant="outline">Follow</Button><Button variant="outline">Download</Button></div>}><div className="mb-3 flex gap-2"><div className="relative flex-1"><Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/><input className="h-9 w-full rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low pl-9 text-sm" placeholder="Filter lines…"/></div><Badge variant="outline">All sources</Badge></div><DataTable headings={['Level','Source','Message']} rows={logRows}/></DashboardPanel></PageFrame></> }

function DataTable({ headings, rows }: { headings: string[]; rows: string[][] }) {
  return <div className="overflow-x-auto"><table className="w-full text-left text-sm"><thead><tr className="border-b border-aurora-border-subtle">{headings.map(h=><th key={h} className="px-3 py-2 text-[11px] uppercase tracking-[.14em] text-aurora-text-muted">{h}</th>)}</tr></thead><tbody>{rows.map((row,index)=><tr key={index} className="border-b border-aurora-border-subtle/70 last:border-0">{row.map((cell,i)=><td key={i} className={`px-3 py-3 ${i === 1 ? 'font-semibold text-aurora-text-primary' : 'text-aurora-text-muted'}`}>{i === 0 ? <Badge variant="outline">{cell}</Badge> : cell}</td>)}</tr>)}</tbody></table></div>
}

type ViewMode = 'table'|'list'|'cards'
function ViewModes({value,onChange}:{value:ViewMode;onChange:(value:ViewMode)=>void}) { return <div className="flex rounded-aurora-1 border border-aurora-border-subtle bg-aurora-control-surface p-0.5">{([[Table2,'Table','table'],[List,'List','list'],[Grid2X2,'Cards','cards']] as const).map(([Icon,label,mode])=><button key={mode} type="button" onClick={()=>onChange(mode)} aria-pressed={value===mode} aria-label={`${label} view`} title={`${label} view`} className={`rounded p-1.5 ${value===mode?'bg-aurora-selected-bg text-aurora-accent-primary':'text-aurora-text-muted hover:text-aurora-text-primary'}`}><Icon className="size-3.5"/></button>)}</div> }

function StashCollection(){const [view,setView]=useState<ViewMode>('table');return <DashboardPanel title="Files" action={<ViewModes value={view} onChange={setView}/>}><div className={view==='cards'?'grid gap-3 md:grid-cols-2 xl:grid-cols-3':view==='list'?'divide-y divide-aurora-border-subtle':'divide-y divide-aurora-border-subtle'}>{files.map((file,index)=><div key={file[1]} className={view==='cards'?'rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-low p-4':'grid grid-cols-[34px_minmax(0,1fr)_120px_90px] items-center gap-3 px-3 py-3 hover:bg-aurora-hover-bg'}><span className={`grid size-8 place-items-center rounded-aurora-1 ${index===0?'bg-aurora-accent-primary/10 text-aurora-accent-primary':index===1?'bg-aurora-success/10 text-aurora-success':index===2?'bg-aurora-error/10 text-aurora-error':'bg-aurora-warn/10 text-aurora-warn'}`}>{index===0?<FileJson/>:index===1?<FileText/>:index===2?<FileArchive/>:<Code2/>}</span><div className={view==='cards'?'mt-3':''}><strong className="text-sm text-aurora-text-primary">{file[1]}</strong><code className="block truncate text-xs text-aurora-text-muted">{file[2]}</code></div><span className="text-xs text-aurora-text-muted">{file[3]}</span><div className="flex items-center justify-end gap-1"><Button size="icon-sm" variant="ghost" aria-label="Download"><Download/></Button><Button size="icon-sm" variant="ghost" aria-label="Remove"><X/></Button></div></div>)}</div></DashboardPanel>}
