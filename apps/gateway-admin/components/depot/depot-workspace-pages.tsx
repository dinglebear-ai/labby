'use client'

import { useEffect, useState } from 'react'
import {
  Archive, Bot, Box, CheckCircle2, CirclePlus, Clock3,
  FileCode2, FileText, Layers3,
  Pause, Play, Search, ChevronDown,
} from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription,
  AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { ArtifactComposer } from './artifact-composer'
import { DevContainersPageContent } from './dev-containers-page-content'
import {
  cancelTask, createAgent, createTask, deleteAgent, getTaskResult, listAgents, listTasks,
  queueTask, runAgent, suspendAgent, updateAgent,
  type AgentRunResult, type AgentView, type OwnerKind, type TaskResult, type TaskView,
} from '@/lib/agent-tasks/client'

const demoArtifacts = [
  ['Skill', 'repo-triage', 'Cluster open PRs and issues, then draft a triage note.', '#review · #github'],
  ['Agent', 'rust-reviewer', 'Review Rust changes and flag unsafe blocks with rationale.', '#rust · #review'],
  ['MCP', 'labby', 'Gateway control plane exposing scoped upstream MCP capabilities.', '#gateway · #mcp'],
  ['Command', '/ship', 'Run release checks and prepare a release draft.', '#release'],
  ['Loadout', 'operator-console', 'Operational tools for logs, services, and infrastructure.', '#ops · #homelab'],
  ['Snippet', 'gateway-reconcile', 'Probe disconnected servers and summarize the delta.', '#gateway'],
]

export type LibrarySection = 'artifacts' | 'loadouts' | 'snippets' | 'tools'

const LIBRARY_TABS = [
  ['artifacts', '/library', 'Artifacts', Box],
  ['loadouts', '/loadouts', 'Loadouts', Archive],
  ['snippets', '/snippets', 'Snippets', FileText],
  ['tools', '/tools', 'Tools', Search],
] as const

/**
 * The one Library section nav shared by Library, Loadouts, and Snippets.
 * `attached` renders it as a hero footer; a count badge appears only for
 * sections whose caller supplied a count, so nothing is shown as unknown.
 */
export function LibraryTabs({ active, attached = false, counts = {} }: { active: LibrarySection; attached?: boolean; counts?: Partial<Record<LibrarySection, number>> }) {
  if (attached) return <nav aria-label="Library sections" data-library-tabs="1" className="aurora-scrollbar flex max-w-full gap-0.5 overflow-x-auto rounded-b-aurora-3 border-t border-aurora-border-subtle bg-aurora-control-surface px-5">
    {LIBRARY_TABS.map(([id, href, label, Icon]) => <a key={id} href={href} aria-current={active === id ? 'page' : undefined} className="inline-flex h-[38px] shrink-0 items-center gap-2 whitespace-nowrap border-b-2 border-transparent px-3.5 text-[12.5px] font-[650] text-aurora-text-muted transition-colors hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary">
      <Icon aria-hidden="true" className="size-3.5" />{label}
      {counts[id] !== undefined ? <span className={`inline-flex h-[19px] min-w-5 items-center justify-center rounded-[5px] border px-[5px] text-[10.5px] font-bold tabular-nums ${active === id ? 'border-aurora-accent-primary bg-aurora-selected-bg text-aurora-accent-strong' : 'border-aurora-border-default bg-aurora-page-bg text-aurora-text-muted'}`}>{counts[id]}</span> : null}
    </a>)}
  </nav>
  return <nav aria-label="Library sections" className="flex max-w-full gap-5 overflow-x-auto border-b border-aurora-border-subtle px-1 sm:gap-6 sm:px-3">
    {LIBRARY_TABS.map(([id, href, label]) => <a key={id} href={href} aria-current={active === id ? 'page' : undefined} className="shrink-0 border-b-2 border-transparent px-2 py-3 text-sm font-semibold text-aurora-text-muted transition-colors hover:text-aurora-text-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-text-primary">{label}</a>)}
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

export function AgentsPage() {
  const [agents, setAgents] = useState<AgentView[]>([])
  const [loadError, setLoadError] = useState<string | null>(null)
  const [selected, setSelected] = useState<AgentView | null>(null)
  const [creating, setCreating] = useState(false)
  const [agentId, setAgentId] = useState('')
  const [ownerKind, setOwnerKind] = useState<OwnerKind>('personal')
  const [ownerId, setOwnerId] = useState('')
  const [instructions, setInstructions] = useState('')
  const [model, setModel] = useState('chatgpt-browser')
  const [mutating, setMutating] = useState(false)

  const applyAgents = (items: AgentView[]) => {
    setAgents(items)
    setLoadError(null)
    setSelected(current => current ? items.find(item => item.agent_id === current.agent_id) ?? null : null)
    if (!ownerId && items[0]) {
      const kind = toOwnerKind(items[0].owner_kind)
      if (kind) setOwnerKind(kind)
      setOwnerId(items[0].owner_id)
    }
  }
  const refresh = async () => {
    try { applyAgents(await listAgents()) }
    catch (error) { setLoadError(errorMessage(error, 'Agent service unavailable')) }
  }
  useEffect(() => {
    const controller = new AbortController()
    void listAgents(controller.signal).then(items => {
      setAgents(items)
      setLoadError(null)
      setSelected(current => current ? items.find(item => item.agent_id === current.agent_id) ?? null : null)
    }).catch(error => {
      if (!controller.signal.aborted) setLoadError(errorMessage(error, 'Agent service unavailable'))
    })
    return () => controller.abort()
  }, [])

  const create = async () => {
    setMutating(true)
    setLoadError(null)
    try {
      const created = await createAgent({ agentId: agentId.trim(), ownerKind, ownerId: ownerId.trim(), instructions, model })
      setCreating(false)
      setSelected(created)
      setAgentId('')
      setInstructions('')
      await refresh()
    } catch (error) {
      setLoadError(errorMessage(error, 'Unable to create Agent'))
    } finally {
      setMutating(false)
    }
  }

  const active = agents.filter(agent => agent.state === 'active').length
  return <>
    <AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Agents' }]} />
    <PageFrame>
      <ConsoleHero eyebrow="Workspace · Agents" title="Agents" description="Immutable Agent definitions executed through Labby’s shared Assistant LLM provider." pulse={{ color: 'var(--aurora-success)' }} actions={<Button onClick={() => setCreating(true)}><CirclePlus/>New Agent</Button>} stats={[
        {label:'Active',value:active,icon:<Play size={12}/>,tone:'var(--aurora-success)'},
        {label:'Suspended',value:agents.filter(agent=>agent.state==='suspended').length,icon:<Pause size={12}/>},
        {label:'Definitions',value:agents.length,icon:<Bot size={12}/>},
        {label:'Runtime',value:'Assistant LLM',icon:<CheckCircle2 size={12}/>},
      ]}/>
      {loadError?<InlineError message={loadError}/>:null}
      <AgentsCollection agents={agents} onSelect={setSelected}/>
    </PageFrame>
    <AgentSessionSheet agent={selected} onOpenChange={open => !open && setSelected(null)} onChanged={refresh} />
    <Dialog open={creating} onOpenChange={setCreating}>
      <DialogContent className="border-aurora-border-strong bg-aurora-panel-medium">
        <DialogTitle>New Agent</DialogTitle>
        <DialogDescription>Create a pinned LLM Agent definition. Provider identity is captured in the immutable harness digest.</DialogDescription>
        <label className="text-xs font-semibold text-aurora-text-muted">Agent ID<input autoFocus value={agentId} onChange={event=>setAgentId(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="release-reviewer"/></label>
        <div className="grid grid-cols-2 gap-3">
          <SelectField label="Owner" value={ownerKind} onChange={value=>setOwnerKind(value as OwnerKind)}><option value="personal">Personal</option><option value="team">Team</option><option value="project">Project</option></SelectField>
          <label className="text-xs text-aurora-text-muted">Owner ID<input value={ownerId} onChange={event=>setOwnerId(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="principal / team / project ID"/></label>
        </div>
        <label className="text-xs font-semibold text-aurora-text-muted">Model<input value={model} onChange={event=>setModel(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="chatgpt-browser"/></label>
        <label className="text-xs font-semibold text-aurora-text-muted">Instructions<textarea value={instructions} onChange={event=>setInstructions(event.target.value)} rows={7} className="mt-2 w-full resize-y rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-3 text-sm text-aurora-text-primary" placeholder="Describe the Agent’s role, constraints, and expected output."/></label>
        <Button onClick={()=>void create()} disabled={mutating||!agentId.trim()||!ownerId.trim()||!instructions.trim()}><CirclePlus/>{mutating?'Creating…':'Create Agent'}</Button>
      </DialogContent>
    </Dialog>
  </>
}

function AgentSessionSheet({ agent, onOpenChange, onChanged }: { agent: AgentView | null; onOpenChange: (open: boolean) => void; onChanged: () => Promise<void> }) {
  const [input, setInput] = useState('')
  const [revisionInstructions, setRevisionInstructions] = useState('')
  const [revisionModel, setRevisionModel] = useState('')
  const [result, setResult] = useState<AgentRunResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)

  useEffect(() => {
    setInput('')
    setRevisionInstructions('')
    setRevisionModel('')
    setResult(null)
    setError(null)
    setConfirmDelete(false)
  }, [agent?.agent_id, agent?.version])

  const run = async () => {
    if (!agent) return
    setBusy(true); setError(null); setResult(null)
    try { setResult(await runAgent(agent.agent_id, input)) }
    catch (failure) { setError(errorMessage(failure, 'Agent run failed')) }
    finally { setBusy(false) }
  }
  const publishRevision = async () => {
    if (!agent) return
    setBusy(true); setError(null)
    try {
      await updateAgent({ agentId: agent.agent_id, instructions: revisionInstructions || undefined, model: revisionModel || undefined })
      setRevisionInstructions(''); setRevisionModel('')
      await onChanged()
    } catch (failure) { setError(errorMessage(failure, 'Agent revision update failed')) }
    finally { setBusy(false) }
  }
  const suspend = async () => {
    if (!agent) return
    setBusy(true); setError(null)
    try { await suspendAgent(agent.agent_id); await onChanged(); onOpenChange(false) }
    catch (failure) { setError(errorMessage(failure, 'Unable to suspend Agent')) }
    finally { setBusy(false) }
  }
  const remove = async () => {
    if (!agent) return
    setBusy(true); setError(null)
    try { await deleteAgent(agent.agent_id); setConfirmDelete(false); onOpenChange(false); await onChanged() }
    catch (failure) { setConfirmDelete(false); setError(errorMessage(failure, 'Unable to delete Agent')) }
    finally { setBusy(false) }
  }

  return <>
  <Sheet open={Boolean(agent)} onOpenChange={onOpenChange}>
    <SheetContent className="!w-[min(96vw,760px)] border-aurora-border-strong bg-aurora-panel-medium p-0 sm:!max-w-[760px]">
      <SheetHeader className="border-b border-aurora-border-subtle bg-aurora-panel-strong px-6 py-5">
        <SheetTitle className="text-xl text-aurora-text-primary">{agent?.agent_id ?? 'Agent definition'}</SheetTitle>
        <SheetDescription className="text-aurora-text-muted">Run the pinned definition or publish a new immutable revision.</SheetDescription>
      </SheetHeader>
      <div className="grid grid-cols-2 border-b border-aurora-border-subtle bg-aurora-panel-low sm:grid-cols-4">
        {[
          ['Owner',agent ? agent.owner_kind + ':' + agent.owner_id : '—'],
          ['Revision',agent ? 'v' + agent.version : '—'],
          ['Runtime','Assistant LLM'],
          ['State',agent?.state ?? '—'],
        ].map(([label,value])=><div key={label} className="border-r border-aurora-border-subtle px-5 py-4 last:border-r-0"><span className="block text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{label}</span><strong className="mt-1 block truncate text-xs text-aurora-text-primary">{value}</strong></div>)}
      </div>
      <div className="space-y-5 overflow-y-auto px-6 py-5">
        {error?<InlineError message={error}/>:null}
        <section className="space-y-3">
          <div><h3 className="text-sm font-semibold text-aurora-text-primary">Run Agent</h3><p className="text-xs text-aurora-text-muted">Input is bound to this run; the immutable Agent instructions remain pinned to revision {agent?.version ?? '—'}.</p></div>
          <textarea value={input} onChange={event=>setInput(event.target.value)} rows={5} className="w-full resize-y rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-3 text-sm text-aurora-text-primary" placeholder="Optional run input…"/>
          <Button onClick={()=>void run()} disabled={busy||!agent||agent.state!=='active'}><Play/>{busy?'Running…':'Run Agent'}</Button>
          {result?<div className="rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-4"><div className="mb-2 text-xs text-aurora-text-muted">Session {result.session_id} · {result.status} · {result.output_digest}{result.output_truncated ? ' · inline output truncated' : ''}</div><pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words text-sm text-aurora-text-primary">{result.output ?? 'The provider returned no materialized text output.'}</pre></div>:null}
        </section>
        <section className="space-y-3 border-t border-aurora-border-subtle pt-5">
          <div><h3 className="text-sm font-semibold text-aurora-text-primary">Publish revision</h3><p className="text-xs text-aurora-text-muted">Provide new instructions, a new model, or both. Omitted values inherit from the prior revision.</p></div>
          <input value={revisionModel} onChange={event=>setRevisionModel(event.target.value)} className="h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="New model (optional)"/>
          <textarea value={revisionInstructions} onChange={event=>setRevisionInstructions(event.target.value)} rows={5} className="w-full resize-y rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-3 text-sm text-aurora-text-primary" placeholder="New instructions (optional)"/>
          <Button variant="outline" onClick={()=>void publishRevision()} disabled={busy||(!revisionInstructions.trim()&&!revisionModel.trim())}>Publish new revision</Button>
        </section>
        <section className="flex flex-wrap gap-2 border-t border-aurora-border-subtle pt-5">
          {agent?.state==='active'?<Button variant="outline" onClick={()=>void suspend()} disabled={busy}><Pause/>Suspend</Button>:null}
          <Button variant="outline" onClick={()=>setConfirmDelete(true)} disabled={busy||!agent}>Delete Agent</Button>
        </section>
      </div>
    </SheetContent>
  </Sheet>
  <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle>Delete Agent definition?</AlertDialogTitle>
        <AlertDialogDescription>Delete {agent?.agent_id ?? 'this Agent'} from the active Agent catalog. Existing durable Task records remain immutable.</AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
        <AlertDialogAction onClick={()=>void remove()} disabled={busy}>{busy?'Deleting…':'Delete Agent'}</AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
  </>
}

export function TasksPage() {
  const [tasks, setTasks] = useState<TaskView[]>([])
  const [agents, setAgents] = useState<AgentView[]>([])
  const [loadError, setLoadError] = useState<string | null>(null)
  const [selected, setSelected] = useState<TaskView | null>(null)
  const [creating, setCreating] = useState(false)
  const [taskId, setTaskId] = useState('')
  const [agentId, setAgentId] = useState('')
  const [input, setInput] = useState('')
  const [mutating, setMutating] = useState(false)

  const applyTasks = (items: TaskView[]) => {
    setTasks(items)
    setSelected(current => current ? items.find(item => item.task_id === current.task_id) ?? current : null)
  }
  const refresh = async () => {
    try { applyTasks(await listTasks()); setLoadError(null) }
    catch (error) { setLoadError(errorMessage(error, 'Task service unavailable')) }
  }
  useEffect(() => {
    const controller = new AbortController()
    void Promise.all([listTasks(controller.signal), listAgents(controller.signal)]).then(([taskItems, agentItems]) => {
      applyTasks(taskItems)
      setAgents(agentItems)
      setAgentId(current => current || agentItems.find(agent=>agent.state==='active')?.agent_id || '')
      setLoadError(null)
    }).catch(error => {
      if (!controller.signal.aborted) setLoadError(errorMessage(error, 'Agent Task services unavailable'))
    })
    return () => controller.abort()
  }, [])

  const hasLiveTasks = tasks.some(task=>['queued','running','cancelling'].includes(task.state))
  useEffect(() => {
    if (!hasLiveTasks) return
    const controller = new AbortController()
    const poll = () => {
      void listTasks(controller.signal).then(items => {
        setTasks(items)
        setSelected(current => current ? items.find(item => item.task_id === current.task_id) ?? current : null)
      }).catch(error => {
        if (!controller.signal.aborted) setLoadError(errorMessage(error, 'Task refresh failed'))
      })
    }
    const interval = globalThis.setInterval(poll, 1000)
    return () => { controller.abort(); globalThis.clearInterval(interval) }
  }, [hasLiveTasks])

  const createAndQueue = async () => {
    const agent = agents.find(candidate => candidate.agent_id === agentId)
    const ownerKind = agent ? toOwnerKind(agent.owner_kind) : null
    if (!agent || !ownerKind) { setLoadError('Select an active Agent with a supported owner scope.'); return }
    setMutating(true); setLoadError(null)
    try {
      await createTask({
        taskId: taskId.trim(),
        // Task ID is immutable and owner-scoped, so using it as the UI
        // idempotency key makes a retry after create-success/queue-failure exact.
        idempotencyKey: taskId.trim(),
        ownerKind,
        ownerId: agent.owner_id,
        agentId: agent.agent_id,
        input,
      })
      await queueTask(taskId.trim())
      setCreating(false); setTaskId(''); setInput('')
      await refresh()
    } catch (error) {
      setLoadError(errorMessage(error, 'Unable to create and queue Task'))
    } finally {
      setMutating(false)
    }
  }

  return <>
    <AppHeader breadcrumbs={[{label:'Workspace'},{label:'Tasks'}]}/>
    <PageFrame>
      <ConsoleHero eyebrow="Workspace · Agent Tasks" title="Tasks" description="Durable, owner-scoped LLM work queued through Labby’s fenced Task runtime." actions={<Button onClick={()=>{if(!taskId)setTaskId('task-'+globalThis.crypto.randomUUID());setCreating(true)}} disabled={!agents.some(agent=>agent.state==='active')}><CirclePlus/>New Task</Button>} stats={[
        {label:'Tasks',value:tasks.length,icon:<Clock3 size={12}/>},
        {label:'Queued',value:tasks.filter(task=>task.state==='queued').length,icon:<CheckCircle2 size={12}/>,tone:'var(--aurora-success)'},
        {label:'Running',value:tasks.filter(task=>task.state==='running').length,icon:<Play size={12}/>},
        {label:'Failed',value:tasks.filter(task=>task.state==='failed').length,icon:<Clock3 size={12}/>,tone:'var(--aurora-error)'},
      ]}/>
      {loadError?<InlineError message={loadError}/>:null}
      <TasksCollection tasks={tasks} onSelect={setSelected}/>
    </PageFrame>
    <TaskDialog task={selected} onOpenChange={open=>!open&&setSelected(null)} onChanged={refresh}/>
    <Dialog open={creating} onOpenChange={setCreating}>
      <DialogContent className="border-aurora-border-strong bg-aurora-panel-medium">
        <DialogTitle>New Task</DialogTitle>
        <DialogDescription>Create an immutable input bound to an Agent revision, then queue it immediately.</DialogDescription>
        <label className="text-xs font-semibold text-aurora-text-muted">Task ID<input autoFocus value={taskId} onChange={event=>setTaskId(event.target.value)} className="mt-2 h-10 w-full rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 text-sm text-aurora-text-primary" placeholder="task-release-review"/></label>
        <SelectField label="Agent" value={agentId} onChange={setAgentId}>{agents.filter(agent=>agent.state==='active').map(agent=><option key={agent.agent_id} value={agent.agent_id}>{agent.agent_id} · {agent.owner_kind}:{agent.owner_id} · v{agent.version}</option>)}</SelectField>
        <label className="text-xs font-semibold text-aurora-text-muted">Task input<textarea value={input} onChange={event=>setInput(event.target.value)} rows={7} className="mt-2 w-full resize-y rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-3 text-sm text-aurora-text-primary" placeholder="Describe the work for this Task attempt."/></label>
        <Button onClick={()=>void createAndQueue()} disabled={mutating||!taskId.trim()||!agentId||!input.trim()}><Play/>{mutating?'Queueing…':'Create & Queue'}</Button>
      </DialogContent>
    </Dialog>
  </>
}

function AgentsCollection({agents,onSelect}:{agents:AgentView[];onSelect:(agent:AgentView)=>void}) {
  const [filter,setFilter]=useState('All')
  const shown=agents.filter(agent=>filter==='All'||agent.state===filter)
  return <DashboardPanel title="Definitions" action={<div className="flex gap-1">{['All','active','suspended'].map(item=><button key={item} type="button" onClick={()=>setFilter(item)} aria-pressed={filter===item} className="rounded-full border border-aurora-border-subtle px-3 py-1 text-[10px] font-semibold text-aurora-text-muted aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg">{item}</button>)}</div>}>
    <div className="overflow-x-auto"><table className="w-full text-sm"><thead><tr className="border-b border-aurora-border-subtle">{['Status','Agent','Owner','Revision','Runtime','Catalog'].map(head=><th key={head} className="px-3 py-2 text-left text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{head}</th>)}</tr></thead><tbody>{shown.length?shown.map(agent=><tr key={agent.agent_id} onClick={()=>onSelect(agent)} className="cursor-pointer border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg"><td className="px-3 py-3"><StatusDot status={agent.state}/></td><td className="px-3 py-3"><button type="button" onClick={event=>{event.stopPropagation();onSelect(agent)}} className="font-semibold text-aurora-text-primary underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{agent.agent_id}</button></td><td className="px-3 py-3"><Badge variant="outline" className="text-aurora-accent-primary">{agent.owner_kind}:{agent.owner_id}</Badge></td><td className="px-3 py-3 text-aurora-text-muted">v{agent.version}</td><td className="px-3 py-3 text-aurora-text-muted">Assistant LLM</td><td className="px-3 py-3 text-aurora-text-muted">{agent.catalog_generation}</td></tr>):<tr><td colSpan={6} className="px-3 py-8 text-center text-sm text-aurora-text-muted">No Agent definitions match this filter.</td></tr>}</tbody></table></div>
  </DashboardPanel>
}

function TasksCollection({tasks,onSelect}:{tasks:TaskView[];onSelect:(task:TaskView)=>void}) {
  const [filter,setFilter]=useState('All')
  const shown=tasks.filter(task=>filter==='All'||task.state===filter)
  return <DashboardPanel title="Task ledger" action={<div className="flex max-w-full gap-1 overflow-x-auto">{['All','created','queued','running','succeeded','failed','cancelled','expired'].map(item=><button key={item} type="button" onClick={()=>setFilter(item)} aria-pressed={filter===item} className="shrink-0 rounded-full border border-aurora-border-subtle px-3 py-1 text-[10px] font-semibold text-aurora-text-muted aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg">{item}</button>)}</div>}>
    <div className="overflow-x-auto"><table className="w-full text-sm"><thead><tr className="border-b border-aurora-border-subtle">{['State','Task','Attempt','Owner','Agent','Result'].map(head=><th key={head} className="px-3 py-2 text-left text-[9px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">{head}</th>)}</tr></thead><tbody>{shown.length?shown.map(task=><tr key={task.task_id} className="cursor-pointer border-b border-aurora-border-subtle/70 last:border-0 hover:bg-aurora-hover-bg" onClick={()=>onSelect(task)}><td className="px-3 py-2"><StatusDot status={task.state}/></td><td className="px-3 py-2"><button type="button" onClick={event=>{event.stopPropagation();onSelect(task)}} className="font-semibold text-aurora-text-primary underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">{task.task_id}</button></td><td className="px-3 py-2 text-aurora-text-muted">{task.attempt}</td><td className="px-3 py-2"><Badge variant="outline">{task.owner_kind}:{task.owner_id}</Badge></td><td className="px-3 py-2 text-aurora-text-muted">{task.agent_id} · v{task.agent_version}</td><td className="px-3 py-2 text-aurora-text-muted">{isTerminalTaskState(task.state)?(task.state==='succeeded'?'view output':'view details'):'pending'}</td></tr>):<tr><td colSpan={6} className="px-3 py-8 text-center text-sm text-aurora-text-muted">No Tasks match this filter.</td></tr>}</tbody></table></div>
  </DashboardPanel>
}

function TaskDialog({task,onOpenChange,onChanged}:{task:TaskView|null;onOpenChange:(open:boolean)=>void;onChanged:()=>Promise<void>}) {
  const [result,setResult]=useState<TaskResult|null>(null)
  const [error,setError]=useState<string|null>(null)
  const [busy,setBusy]=useState(false)
  const taskId=task?.task_id
  const taskState=task?.state
  const terminal=Boolean(taskState&&isTerminalTaskState(taskState))
  useEffect(()=>{
    setResult(null); setError(null)
    if(!taskId||!taskState||!isTerminalTaskState(taskState)) return
    const controller=new AbortController()
    void getTaskResult(taskId,controller.signal).then(setResult).catch(failure=>{if(!controller.signal.aborted)setError(errorMessage(failure,'Unable to read Task result'))})
    return()=>controller.abort()
  },[taskId,taskState])
  const queue=async()=>{if(!task)return;setBusy(true);setError(null);try{await queueTask(task.task_id);await onChanged()}catch(failure){setError(errorMessage(failure,'Unable to queue Task'))}finally{setBusy(false)}}
  const cancel=async()=>{if(!task)return;setBusy(true);setError(null);try{await cancelTask(task.task_id);await onChanged()}catch(failure){setError(errorMessage(failure,'Unable to cancel Task'))}finally{setBusy(false)}}
  return <Dialog open={Boolean(task)} onOpenChange={onOpenChange}><DialogContent className="border-aurora-border-strong bg-aurora-panel-medium"><DialogTitle>{task?.task_id??'Task'}</DialogTitle><DialogDescription>Authoritative durable Agent Task record and terminal result.</DialogDescription>{error?<InlineError message={error}/>:null}<dl className="divide-y divide-aurora-border-subtle rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low px-4">{[
    ['State',task?.state],['Attempt',task?String(task.attempt):undefined],['Owner',task?task.owner_kind+':'+task.owner_id:undefined],['Agent',task?task.agent_id+' · v'+task.agent_version:undefined],['Output digest',result?.output_digest??task?.output_digest??'—'],['Error',result?.error_code??task?.error_code??'—']
  ].map(([label,value])=><div key={label} className="flex justify-between gap-4 py-3 text-sm"><dt className="text-aurora-text-muted">{label}</dt><dd className="break-all text-right font-medium text-aurora-text-primary">{value}</dd></div>)}</dl>{result?.output?<pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-aurora-1 border border-aurora-border-subtle bg-aurora-panel-low p-4 text-sm text-aurora-text-primary">{result.output}</pre>:null}{result?.output_truncated?<p className="text-xs text-aurora-text-muted">Inline output is truncated at 256 KiB; the output digest keys the full stored text.</p>:null}<div className="flex gap-2">{task&&['created','queued'].includes(task.state)?<Button onClick={()=>void queue()} disabled={busy}><Play/>{task.state==='queued'?'Resume queue':'Queue'}</Button>:null}{task&&['queued','running'].includes(task.state)?<Button variant="outline" onClick={()=>void cancel()} disabled={busy}><Pause/>Cancel</Button>:null}{terminal&&!result?<span className="text-xs text-aurora-text-muted">Loading terminal result…</span>:null}</div></DialogContent></Dialog>
}

function SelectField({label,value,onChange,children}:{label:string;value:string;onChange:(value:string)=>void;children:React.ReactNode}) { return <label className="text-xs text-aurora-text-muted">{label}<span className="relative mt-2 block"><select value={value} onChange={event=>onChange(event.target.value)} className="h-10 w-full appearance-none rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface pl-3 pr-10 text-sm text-aurora-text-primary">{children}</select><ChevronDown className="pointer-events-none absolute right-3 top-1/2 size-4 -translate-y-1/2 text-aurora-text-muted"/></span></label> }

function StatusDot({status}:{status:string}) { const normalized=status.toLowerCase(); const color=['active','running','succeeded'].includes(normalized)?'bg-aurora-success':['failed','expired'].includes(normalized)?'bg-aurora-error':['suspended','cancelling'].includes(normalized)?'bg-aurora-warn':'bg-aurora-text-muted'; return <span role="img" aria-label={status} title={status} className={'block size-2 rounded-full '+color}/> }
function InlineError({message}:{message:string}) { return <div role="alert" className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/5 p-3 text-sm text-aurora-error">{message}</div> }
function errorMessage(error:unknown,fallback:string) { return error instanceof Error && error.message ? error.message : fallback }
function toOwnerKind(value:string):OwnerKind|null { return value==='personal'||value==='team'||value==='project'?value:null }
function isTerminalTaskState(value:string) { return ['succeeded','failed','cancelled','expired'].includes(value) }

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
