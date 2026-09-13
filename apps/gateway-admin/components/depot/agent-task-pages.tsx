'use client'

import { useEffect, useState, type ReactNode } from 'react'
import { AlertCircle, ChevronRight, Play, RefreshCw } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { authorityIdentity, useBrowserSession } from '@/lib/auth/session'
import {
  listAgentHarnesses,
  listAgents,
  listVisibleAgentSessions,
  type AgentHarnessView,
  type AgentRunResult,
  type AgentSessionView,
  type AgentView,
  type TaskView,
} from '@/lib/agent-tasks/client'
import { AgentSessionLedger, AgentSessionViewer } from './agent-session-panel'
import { NewAgentSessionWizard } from './new-agent-session-wizard'

export const SESSION_UNAVAILABLE = 'Direct replay is not available from this legacy task ledger.'
const PANEL = 'min-w-0 overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium)]'
const ROW = 'flex min-w-0 items-center gap-2.5 px-4 py-[9px]'
const LABEL = 'text-[9.5px] font-bold uppercase tracking-[.1em] text-aurora-text-muted'
const FILTER = 'h-6 rounded-full border border-aurora-border-strong px-2.5 text-[10.5px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg'

function WorkspaceFrame({ page, children }: { page: string; children: ReactNode }) {
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: page }]} /><div className={`${AURORA_PAGE_FRAME} ${AURORA_PAGE_SHELL} !gap-[14px]`}>{children}</div></>
}

function Status({ state }: { state: string }) {
  const tone = ['running', 'active', 'succeeded'].includes(state) ? 'var(--aurora-success)' : state === 'failed' ? 'var(--aurora-error)' : ['suspended', 'queued', 'created'].includes(state) ? 'var(--aurora-warn)' : 'var(--aurora-text-muted)'
  return <span className="inline-flex min-w-0 items-center gap-1.5 text-[10.5px] capitalize" style={{ color: tone }}><span className="size-1.5 shrink-0 rounded-full bg-current" /><span className="truncate">{state}</span></span>
}

function RecordFields({ fields }: { fields: Array<[string, string | number | null | undefined]> }) {
  return <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 rounded-lg border border-aurora-border-subtle bg-aurora-control-surface px-[11px] py-[9px] text-[11.5px]">{fields.map(([label, value]) => <div key={label} className="contents"><dt className="text-aurora-text-muted">{label}</dt><dd className="min-w-0 break-all text-aurora-text-primary">{value ?? 'Not reported'}</dd></div>)}</dl>
}

export function TaskRecords({ rows }: { rows: TaskView[] }) {
  const [filter, setFilter] = useState('All')
  const [open, setOpen] = useState<string>()
  const shown = rows.filter(row => filter === 'All' || (filter === 'Active' ? ['created', 'queued', 'running'].includes(row.state) : filter === 'Finished' ? ['succeeded', 'cancelled', 'expired'].includes(row.state) : row.state === 'failed'))
  return <section className={PANEL} aria-label="Task records">
    <header className="flex flex-wrap items-center gap-2 border-b border-aurora-border-subtle bg-aurora-control-surface px-[15px] py-2.5"><h2 className={`${LABEL} mr-auto`}>Task ledger</h2>{['All', 'Active', 'Finished', 'Failed'].map(item => <button key={item} type="button" aria-pressed={filter === item} onClick={() => setFilter(item)} className={FILTER}>{item}</button>)}</header>
    <div className={`${ROW} border-b border-aurora-border-subtle !py-2 ${LABEL}`} aria-hidden="true"><span className="w-[84px] shrink-0">State</span><span className="min-w-0 flex-1">Task</span><span className="hidden w-[132px] shrink-0 min-[1180px]:block">Owner</span><span className="hidden w-[150px] shrink-0 min-[1400px]:block">Agent</span><span className="w-[66px] shrink-0 text-right">Attempt</span><span className="w-7" /></div>
    {shown.map(row => <article key={row.task_id} className="border-t border-aurora-border-subtle first:border-t-0">
      <button type="button" aria-expanded={open === row.task_id} onClick={() => setOpen(open === row.task_id ? undefined : row.task_id)} className={`${ROW} w-full text-left hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary`}><span className="w-[84px] shrink-0"><Status state={row.state} /></span><span className="min-w-0 flex-1"><strong className="block truncate text-[12.5px] font-semibold text-aurora-text-primary">{row.task_id}</strong><span className="block truncate text-[10.5px] text-aurora-text-muted">Agent revision {row.agent_version}</span></span><span className="hidden w-[132px] shrink-0 truncate text-[11px] text-aurora-text-muted min-[1180px]:block">{row.owner_kind} · {row.owner_id}</span><span className="hidden w-[150px] shrink-0 truncate text-[11px] text-aurora-text-primary min-[1400px]:block">{row.agent_id}</span><span className="w-[66px] shrink-0 text-right text-[10.5px] tabular-nums text-aurora-text-muted">{row.attempt}</span><ChevronRight aria-hidden="true" className={`size-[13px] shrink-0 text-aurora-text-muted ${open === row.task_id ? 'rotate-90' : ''}`} /></button>
      {open === row.task_id ? <div className="grid gap-[14px] border-t border-aurora-border-subtle bg-aurora-page-bg px-4 pb-4 pt-3 lg:grid-cols-[minmax(0,1.3fr)_minmax(220px,.7fr)] lg:pl-[68px]"><div><h3 className={`${LABEL} mb-2`}>Result</h3><RecordFields fields={[[ 'State', row.state ], ['Output digest', row.output_digest], ['Error code', row.error_code]]} /><p className="mt-2 text-[11px] text-aurora-text-muted">Run history and session transcripts are not available from this server.</p></div><div><h3 className={`${LABEL} mb-2`}>Definition</h3><RecordFields fields={[[ 'Owner', `${row.owner_kind} · ${row.owner_id}` ], ['Agent', row.agent_id], ['Agent revision', row.agent_version], ['Attempt', row.attempt]]} /><span title={SESSION_UNAVAILABLE}><Button disabled size="sm" variant="outline" className="mt-2"><Play />Run Now</Button></span></div></div> : null}
    </article>)}
    {!shown.length ? <div className="px-5 py-9 text-center"><p className="font-display text-[15px] font-bold text-aurora-text-primary">No {filter === 'All' ? '' : `${filter.toLowerCase()} `}tasks.</p><p className="mt-1 text-xs text-aurora-text-muted">Durable task records appear here when submitted to this workspace.</p></div> : null}
  </section>
}

export { TaskSchedulesPage as TasksPage } from './task-schedules-page'

export function AgentRecords({ rows, runnableAgentIds, onStart, onCreate }: { rows: AgentView[]; runnableAgentIds?: ReadonlySet<string>; onStart?: (agentId: string) => void; onCreate?: () => void }) {
  const [filter, setFilter] = useState('All')
  const [open, setOpen] = useState<string>()
  const shown = rows.filter(row => filter === 'All' || row.state === filter.toLowerCase())
  return <section className={PANEL} aria-label="Agent definitions"><header className="flex items-center gap-2 border-b border-aurora-border-subtle bg-aurora-control-surface px-[15px] py-2.5"><h2 className={`${LABEL} mr-auto`}>Definitions</h2>{['All', 'Active', 'Suspended'].map(item => <button key={item} type="button" aria-pressed={filter === item} onClick={() => setFilter(item)} className={FILTER}>{item}</button>)}</header><div className={`${ROW} border-b border-aurora-border-subtle !py-2 ${LABEL}`} aria-hidden="true"><span className="w-[84px] shrink-0">State</span><span className="min-w-0 flex-1">Agent</span><span className="hidden w-[128px] shrink-0 min-[1180px]:block">Owner</span><span className="w-[66px] shrink-0 text-right">Revision</span><span className="w-7" /></div>{shown.map(row => <article key={row.agent_id} className="border-t border-aurora-border-subtle"><button type="button" aria-expanded={open === row.agent_id} onClick={() => setOpen(open === row.agent_id ? undefined : row.agent_id)} className={`${ROW} w-full text-left hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary`}><span className="w-[84px] shrink-0"><Status state={row.state} /></span><span className="min-w-0 flex-1"><strong className="block truncate text-[12.5px] font-semibold text-aurora-text-primary">{row.agent_id}</strong><span className="block truncate text-[10.5px] text-aurora-text-muted">{row.harness_id ? `Harness ${row.harness_id}` : 'No matching configured harness'}</span></span><span className="hidden w-[128px] shrink-0 truncate text-[11px] text-aurora-text-muted min-[1180px]:block">{row.owner_kind} · {row.owner_id}</span><span className="w-[66px] shrink-0 text-right text-[10.5px] tabular-nums text-aurora-text-muted">v{row.version}</span><ChevronRight aria-hidden="true" className={`size-[13px] shrink-0 text-aurora-text-muted ${open === row.agent_id ? 'rotate-90' : ''}`} /></button>{open === row.agent_id ? <div className="border-t border-aurora-border-subtle bg-aurora-page-bg p-4"><RecordFields fields={[[ 'Agent', row.agent_id ], ['Owner', `${row.owner_kind} · ${row.owner_id}`], ['Revision', row.version], ['Harness bundle', row.harness_id], ['Harness digest', row.harness_digest], ['Repository reference', row.repository_digest], ['Base image reference', row.image_digest], ['Loadout pin', row.loadout_digest], ['Content digest', row.content_digest], ['Catalog generation', row.catalog_generation], ['State', row.state]]} /><p className="mt-2 text-[11px] text-aurora-text-muted">The server resolves this immutable definition only when every pin matches an operator-provisioned host harness bundle.</p>{onStart && row.state === 'active' ? <Button size="sm" className="mt-3" onClick={() => onStart(row.agent_id)} disabled={!runnableAgentIds?.has(row.agent_id)}><Play />New Session</Button> : null}</div> : null}</article>)}{!shown.length ? <div className="px-5 py-[38px] text-center"><p className="font-display text-[15px] font-bold text-aurora-text-primary">No {filter === 'All' ? '' : `${filter.toLowerCase()} `}agent definitions.</p><p className="mt-1 text-xs text-aurora-text-muted">{filter === 'All' ? 'Create the first definition from an operator-approved runtime bundle.' : 'Definitions belong to the selected workspace.'}</p>{filter === 'All' && onCreate ? <Button size="sm" className="mt-space-4" onClick={onCreate}><Play />Create First Agent</Button> : null}</div> : null}</section>
}

export function AgentsPage() {
  const session = useBrowserSession()
  const identity = authorityIdentity(session.status === 'authenticated' ? session.authority : undefined)
  const capabilities = session.status === 'authenticated' && session.authority ? session.authority.capabilities : []
  return <AgentWorkspace key={identity} capabilities={capabilities} />
}

export function AgentWorkspace({ capabilities }: { capabilities: readonly string[] }) {
  const [records, setRecords] = useState<{ agents: AgentView[]; harnesses: AgentHarnessView[]; sessions: AgentSessionView[] }>({ agents: [], harnesses: [], sessions: [] })
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()
  const [refresh, setRefresh] = useState(0)
  const [wizard, setWizard] = useState<{ open: boolean; agentId?: string }>({ open: false })
  const [selected, setSelected] = useState<Pick<AgentSessionView, 'agent_id' | 'session_id'>>()

  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    void Promise.all([listAgents(controller.signal), listAgentHarnesses(controller.signal)])
      .then(async ([agents, harnesses]) => ({ agents, harnesses, sessions: await listVisibleAgentSessions(agents, controller.signal) }))
      .then(value => { if (!controller.signal.aborted) { setRecords(value); setError(undefined) } })
      .catch(cause => { if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : 'Agent workspace data could not be loaded.') })
      .finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [refresh])

  useEffect(() => {
    if (!records.sessions.some(item => ['admitted', 'running', 'cancelling'].includes(item.status))) return
    const timer = window.setInterval(() => setRefresh(value => value + 1), 2000)
    return () => window.clearInterval(timer)
  }, [records.sessions])

  function openStarted(started: AgentRunResult) {
    setRefresh(value => value + 1)
    setSelected({ agent_id: started.agent_id, session_id: started.session_id })
  }

  const hasAvailableHarness = records.harnesses.some(harness => harness.available)
  const runnableAgentIds = new Set(records.agents.filter(agent => agent.state === 'active' && records.harnesses.some(harness => harness.available && harness.id === agent.harness_id && harness.digest === agent.harness_digest)).map(agent => agent.agent_id))
  const canOperate = capabilities.includes('scope.operate')
  const canCreate = capabilities.includes('scope.create')
  const canStart = canOperate && (runnableAgentIds.size > 0 || (canCreate && hasAvailableHarness))
  return <WorkspaceFrame page="Agents">
    <ConsoleHero eyebrow="Workspace · Agents" title="Agents" description="Pinned Agent definitions, configured harness bundles, and retained session evidence." actions={<><Button size="icon-sm" variant="ghost" title="Refresh agents" aria-label="Refresh agents" onClick={() => setRefresh(value => value + 1)} disabled={loading}><RefreshCw /></Button><Button onClick={() => setWizard({ open: true })} disabled={loading || !canStart}><Play />New Session</Button></>} stats={[{ label: 'Definitions', value: loading && !records.agents.length ? '—' : records.agents.length }, { label: 'Active', value: loading && !records.agents.length ? '—' : records.agents.filter(row => row.state === 'active').length, tone: 'var(--aurora-success)' }, { label: 'Running', value: records.sessions.filter(row => ['admitted', 'running', 'cancelling'].includes(row.status)).length, tone: 'var(--aurora-accent-strong)' }, { label: 'Retained sessions', value: records.sessions.length }]} />
    {error ? <Alert variant="error"><AlertCircle /><AlertTitle>Agents unavailable</AlertTitle><AlertDescription>{error}</AlertDescription></Alert> : null}
    {loading && !records.agents.length && !error ? <p role="status" className="py-8 text-center text-xs text-aurora-text-muted">Loading Agent workspace…</p> : null}
    {records.agents.length ? <AgentRecords rows={records.agents} runnableAgentIds={runnableAgentIds} onStart={canOperate ? agentId => setWizard({ open: true, agentId }) : undefined} /> : null}
    {!loading && !error && !records.agents.length ? <AgentRecords rows={[]} onCreate={canOperate && canCreate && hasAvailableHarness ? () => setWizard({ open: true }) : undefined} /> : null}
    <AgentSessionLedger sessions={records.sessions} loading={loading && Boolean(records.agents.length)} error={records.agents.length ? error : undefined} onRefresh={() => setRefresh(value => value + 1)} onOpen={setSelected} />
    <NewAgentSessionWizard open={wizard.open} onOpenChange={open => { setWizard(current => ({ ...current, open })); if (!open) setRefresh(value => value + 1) }} agents={records.agents} harnesses={records.harnesses} initialAgentId={wizard.agentId} canCreate={canCreate} onStarted={openStarted} />
    <AgentSessionViewer selected={selected} onClose={() => setSelected(undefined)} onChanged={() => setRefresh(value => value + 1)} onResumed={openStarted} />
  </WorkspaceFrame>
}
