'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { ChevronRight, Clock3, Pencil, Play, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { authorityIdentity, useBrowserSession, type AuthorityOwner } from '@/lib/auth/session'
import { isAbortError } from '@/lib/api/service-action-client'
import type { AgentView, TaskView } from '@/lib/agent-tasks/client'
import { armTaskSchedule, createTaskSchedule, deleteTaskSchedule, editTaskSchedule, getScheduledTask, getScheduledTaskResult, getTaskSchedule, listScheduleAgents, listTaskSchedules, pauseTaskSchedule, runTaskSchedule, scheduleLabel, scheduleTimezone, type ScheduleSpec, type TaskSchedule, type TaskScheduleDetail } from '@/lib/agent-tasks/schedules'
import { TaskScheduleForm, validateRetryPolicy, type ScheduleDraft } from './task-schedule-form'

const PANEL = 'min-w-0 overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium)]'
const LABEL = 'text-[9.5px] font-bold uppercase tracking-[.1em] text-aurora-text-muted'
const ROW = 'flex min-w-0 items-center gap-2.5 px-4 py-[9px]'
const FILTER = 'h-6 rounded-full border border-aurora-border-strong px-2.5 text-[10.5px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary aria-pressed:border-aurora-accent-primary aria-pressed:bg-aurora-accent-primary aria-pressed:text-aurora-page-bg'
const message = (error: unknown) => error instanceof Error ? error.message : 'Task request failed.'
function nextLabel(row: TaskSchedule) {
  if (!row.armed) return 'paused'
  if (row.next_run_at === null) return '—'
  const minutes = Math.ceil((row.next_run_at - Date.now()) / 60_000)
  return minutes <= 0 ? 'due' : minutes < 60 ? `in ${minutes}m` : minutes < 1440 ? `in ${Math.ceil(minutes / 60)}h` : `in ${Math.ceil(minutes / 1440)}d`
}
function RunState({ state }: { state?: string }) {
  const tone = state === 'succeeded' ? 'var(--aurora-success)' : state === 'failed' ? 'var(--aurora-error)' : ['running', 'queued', 'pending'].includes(state ?? '') ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)'
  return <span className="inline-flex min-w-0 items-center gap-1.5 truncate text-[10.5px] font-semibold" style={{ color: tone }}><span className="size-[5px] shrink-0 rounded-full bg-current" />{state ?? '—'}</span>
}
function Definition({ fields }: { fields: Array<[string, string | number | null | undefined]> }) {
  return <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1.5 rounded-lg border border-aurora-border-subtle bg-aurora-control-surface px-[11px] py-[9px] text-[11.5px]">{fields.map(([label, value]) => <div className="contents" key={label}><dt className="text-aurora-text-muted">{label}</dt><dd title={String(value ?? '')} className="min-w-0 truncate text-aurora-text-primary">{value ?? 'Not reported'}</dd></div>)}</dl>
}

export function TaskScheduleRows({ rows, agents, states, busy, canOperate, canDelete, onToggle, onRun, onEdit, onDelete }: { rows: TaskSchedule[]; agents: AgentView[]; states: Record<string, TaskView>; busy?: string; canOperate: boolean; canDelete: boolean; onToggle: (row: TaskSchedule) => void; onRun: (row: TaskSchedule) => void; onEdit: (row: TaskSchedule) => void; onDelete: (row: TaskSchedule) => void }) {
  const [filter, setFilter] = useState('All')
  const [open, setOpen] = useState<string>()
  const shown = rows.filter(row => filter === 'All' || (filter === 'Armed' ? row.armed : !row.armed))
  return <section aria-label="Scheduled tasks" className={PANEL}>
    <header className="flex items-center gap-2 border-b border-aurora-border-subtle bg-aurora-control-surface px-[15px] py-2.5"><h2 className={`${LABEL} mr-auto`}>Scheduled</h2>{['All', 'Armed', 'Paused'].map(item => <button type="button" key={item} className={FILTER} aria-pressed={filter === item} onClick={() => setFilter(item)}>{item}</button>)}</header>
    <div className={`${ROW} border-b border-aurora-border-subtle !py-2 ${LABEL}`} aria-hidden="true"><span className="w-[42px] shrink-0">On</span><span className="min-w-[96px] flex-1">Task</span><span className="hidden w-[132px] shrink-0 sm:block">Schedule</span><span className="hidden w-[150px] shrink-0 min-[1180px]:block">Catalog</span><span className="hidden w-[96px] shrink-0 min-[1000px]:block">Last run</span><span className="w-[86px] shrink-0 text-right">Next</span><span className="w-[51px] shrink-0" /></div>
    {shown.map(row => {
      const agent = agents.find(value => value.agent_id === row.agent_id)
      const expanded = open === row.schedule_id
      return <article key={row.schedule_id} className="border-t border-aurora-border-subtle first:border-t-0 even:bg-aurora-control-surface/40">
        <div className={`${ROW} hover:bg-aurora-hover-bg`}>
          <span className="w-[42px] shrink-0"><Switch aria-label={`${row.armed ? 'Pause' : 'Arm'} ${row.name}`} checked={row.armed} disabled={!canOperate || Boolean(busy)} onCheckedChange={() => onToggle(row)} className="!h-[18px] !w-8 border-aurora-border-strong [&_[data-slot=switch-thumb]]:size-3 [&_[data-slot=switch-thumb]]:data-[state=checked]:translate-x-4 [&_[data-slot=switch-thumb]]:data-[state=unchecked]:translate-x-0.5" /></span>
          <button type="button" aria-expanded={expanded} aria-label={`Inspect ${row.name}`} onClick={() => setOpen(expanded ? undefined : row.schedule_id)} className="min-w-[96px] flex-1 text-left focus-visible:outline-2 focus-visible:outline-aurora-accent-primary"><strong className={`block truncate text-[12.5px] font-semibold ${row.armed ? 'text-aurora-text-primary' : 'text-aurora-text-muted'}`}>{row.name}</strong><span className="block truncate text-[10.5px] text-aurora-text-muted">{row.agent_id}</span></button>
          <span title={`${scheduleLabel(row.schedule)} · ${scheduleTimezone(row.schedule)}`} className="hidden w-[132px] shrink-0 items-center gap-1.5 text-[11px] font-semibold text-aurora-text-primary sm:inline-flex"><Clock3 className="size-[11px] shrink-0 text-aurora-text-muted" /><span className="truncate">{scheduleLabel(row.schedule)}</span></span>
          <span className="hidden w-[150px] shrink-0 min-[1180px]:block"><span title={agent ? `Agent v${agent.version} · catalog ${agent.catalog_generation}` : 'The server did not report the pinned Agent.'} className="block truncate rounded-full border border-aurora-accent-primary/20 bg-aurora-accent-primary/10 px-2 py-0.5 font-mono text-[9.5px] text-aurora-accent-strong">{agent?.catalog_generation ?? 'Not reported'}</span></span>
          <span className="hidden w-[96px] shrink-0 min-[1000px]:block"><RunState state={row.last_error_kind ? 'failed' : row.last_task_id ? states[row.last_task_id]?.state : undefined} /></span>
          <time dateTime={row.next_run_at ? new Date(row.next_run_at).toISOString() : undefined} title={row.next_run_at ? new Date(row.next_run_at).toLocaleString() : undefined} className="w-[86px] shrink-0 text-right text-[10.5px] tabular-nums text-aurora-text-muted">{nextLabel(row)}</time>
          <Button data-visible-label type="button" variant="ghost" size="icon-sm" className="!size-7 !min-w-7 !p-0" title={`Run ${row.name} now`} aria-label={`Run ${row.name} now`} disabled={!canOperate || Boolean(busy)} onClick={() => onRun(row)}><Play className="!size-3" /></Button>
          <button type="button" aria-label={`${expanded ? 'Collapse' : 'Expand'} ${row.name}`} aria-expanded={expanded} onClick={() => setOpen(expanded ? undefined : row.schedule_id)} className="shrink-0 text-aurora-text-muted"><ChevronRight className={`size-[13px] ${expanded ? 'rotate-90' : ''}`} /></button>
        </div>
        {expanded ? <div className="grid gap-[14px] border-t border-aurora-border-subtle bg-aurora-page-bg/25 px-4 pb-4 pt-3 lg:grid-cols-[minmax(0,1.3fr)_minmax(220px,.7fr)] lg:pl-[68px]"><div><h3 className={`${LABEL} mb-2`}>Latest run</h3>{row.last_task_id ? <TaskRunDetails key={row.last_task_id} taskId={row.last_task_id} known={states[row.last_task_id]} /> : <p className="text-xs text-aurora-text-muted">No admitted runs yet.</p>}{row.last_error_kind ? <p className="mt-2 text-xs text-aurora-error">Admission failed: {row.last_error_kind}</p> : null}<p className="mt-3 text-[11px] leading-relaxed text-aurora-text-muted">Missed runs coalesce into one. Retries use fresh access checks and never replay cancelled tasks. Already admitted tasks have their own lifecycle.</p></div><div><h3 className={`${LABEL} mb-2`}>Definition</h3><Definition fields={[[ 'Schedule', `${scheduleLabel(row.schedule)} · ${scheduleTimezone(row.schedule)}` ], ['Agent', row.agent_id], ['Revision', agent ? `v${agent.version}` : undefined], ['Runtime', agent ? 'Assistant LLM' : undefined], ['Catalog', agent?.catalog_generation], ['Retries', row.retry_policy?.max_retries ? `${row.retry_policy.max_retries} retries · ${row.retry_policy.backoff_ms / 60000}m delay` : 'No automatic retries'], ['Next retry', row.next_retry_at ? new Date(row.next_retry_at).toLocaleString() : 'None pending'], ['Revision', row.revision]]} /><div className="mt-2 flex justify-end gap-2"><Button data-visible-label size="sm" variant="outline" disabled={!canOperate || Boolean(busy)} onClick={() => onEdit(row)}><Pencil />Edit</Button><Button data-visible-label size="sm" variant="outline" disabled={!canDelete || Boolean(busy)} onClick={() => onDelete(row)}><Trash2 />Delete</Button></div></div></div> : null}
      </article>
    })}
    {!shown.length ? <div className="px-5 py-10 text-center"><p className="font-display text-[15px] font-bold text-aurora-text-primary">No {filter === 'All' ? '' : filter.toLowerCase() + ' '}scheduled tasks.</p><p className="mt-1 text-xs text-aurora-text-muted">Create a task from an active agent in this workspace.</p></div> : null}
  </section>
}
function TaskRunDetails({ taskId, known }: { taskId: string; known?: TaskView }) {
  const [record, setRecord] = useState(known)
  const [error, setError] = useState<string>()
  useEffect(() => {
    const controller = new AbortController()
    void getScheduledTask(taskId, controller.signal).then(async task => {
      if (!controller.signal.aborted) setRecord(task)
      if (['succeeded', 'failed', 'cancelled', 'expired'].includes(task.state)) { const result = await getScheduledTaskResult(taskId, controller.signal); if (!controller.signal.aborted) setRecord(result) }
    }).catch(reason => { if (!isAbortError(reason)) setError(message(reason)) })
    return () => controller.abort()
  }, [taskId, known?.state])
  return <div className="space-y-2"><Definition fields={[[ 'Task', taskId ], ['State', record?.state], ['Attempt', record?.attempt], ['Output digest', record?.output_digest], ['Error', record?.error_code]]} />{error ? <p role="alert" className="text-xs text-aurora-error">{error}</p> : null}</div>
}

export function TaskWorkspace({ owner, capabilities }: { owner: AuthorityOwner; capabilities: readonly string[] }) {
  const [rows, setRows] = useState<TaskSchedule[]>([])
  const [agents, setAgents] = useState<AgentView[]>([])
  const [states, setStates] = useState<Record<string, TaskView>>({})
  const [loading, setLoading] = useState(true)
  const [inventoryError, setInventoryError] = useState<string>()
  const [agentError, setAgentError] = useState<string>()
  const [error, setError] = useState<string>()
  const [notice, setNotice] = useState<string>()
  const [busy, setBusy] = useState<string>()
  const busyRef = useRef(false)
  const runKeys = useRef(new Map<string, string>())
  const newScheduleId = useRef<string | undefined>(undefined)
  const [form, setForm] = useState<'new' | TaskScheduleDetail>()
  const [deleting, setDeleting] = useState<TaskSchedule>()
  const controller = useRef(new AbortController())
  const refreshing = useRef<AbortSignal | null>(null)
  const canCreate = capabilities.includes('scope.create') && capabilities.includes('scope.operate') && owner.kind !== 'installation'
  const canOperate = capabilities.includes('scope.operate')
  const canDelete = capabilities.includes('scope.delete')
  const load = useCallback(async () => {
    const signal = controller.current.signal
    if (refreshing.current === signal || signal.aborted) return
    refreshing.current = signal
    const [inventory, definitions] = await Promise.allSettled([listTaskSchedules(signal), listScheduleAgents(signal)])
    if (!signal.aborted) {
      if (inventory.status === 'fulfilled') {
        const visible = inventory.value.filter(row => row.owner_kind === owner.kind && row.owner_id === owner.id)
        setRows(visible); setInventoryError(undefined)
        const result: Record<string, TaskView> = {}
        const ids = [...new Set(visible.map(row => row.last_task_id).filter((id): id is string => Boolean(id)))]
        for (let i = 0; i < ids.length; i += 4) {
          const batch = await Promise.allSettled(ids.slice(i, i + 4).map(id => getScheduledTask(id, signal)))
          for (const item of batch) if (item.status === 'fulfilled') result[item.value.task_id] = item.value
        }
        if (!signal.aborted) setStates(result)
      } else if (!isAbortError(inventory.reason)) setInventoryError(message(inventory.reason))
      if (definitions.status === 'fulfilled') { setAgents(definitions.value.filter(row => row.owner_kind === owner.kind && row.owner_id === owner.id)); setAgentError(undefined) } else if (!isAbortError(definitions.reason)) setAgentError(message(definitions.reason))
      setLoading(false)
    }
    if (refreshing.current === signal) refreshing.current = null
  }, [controller, owner.id, owner.kind])
  useEffect(() => { if (controller.current.signal.aborted) controller.current = new AbortController(); void load(); const timer = setInterval(() => void load(), 10_000); return () => { clearInterval(timer); controller.current.abort() } }, [load])
  const mutate = async (key: string, action: () => Promise<unknown>, success?: string) => {
    if (busyRef.current) return
    busyRef.current = true
    setBusy(key); setError(undefined); setNotice(undefined)
    try { await action(); if (!controller.current.signal.aborted) { setNotice(success); await load() } }
    catch (reason) { if (!isAbortError(reason)) setError(message(reason)); throw reason }
    finally { busyRef.current = false; if (!controller.current.signal.aborted) setBusy(undefined) }
  }
  const safe = (operation: Promise<unknown>) => { void operation.catch(() => undefined) }
  const save = async (draft: ScheduleDraft, schedule: ScheduleSpec) => {
    const retry_policy = validateRetryPolicy(draft)
    await mutate('save', async () => {
      if (form && form !== 'new') await editTaskSchedule({ schedule_id: form.schedule_id, name: draft.name.trim(), agent_id: draft.agentId, input: draft.input, schedule, retry_policy }, controller.current.signal)
      else await createTaskSchedule({ schedule_id: newScheduleId.current ?? (newScheduleId.current = crypto.randomUUID()), name: draft.name.trim(), owner_kind: owner.kind, owner_id: owner.id, ...(owner.kind === 'project' ? { project_id: owner.id } : {}), agent_id: draft.agentId, input: draft.input, schedule, armed: draft.armed, retry_policy }, controller.current.signal)
      if (!controller.current.signal.aborted) setForm(undefined)
    }, form === 'new' ? 'Task created.' : 'Task updated.').catch(() => undefined)
  }
  const edit = (row: TaskSchedule) => safe(mutate(row.schedule_id, async () => { const detail = await getTaskSchedule(row.schedule_id, controller.current.signal); if (!controller.current.signal.aborted) setForm(detail) }))
  const unknown = loading || inventoryError
  const next = rows.filter(row => row.armed && row.next_run_at !== null).sort((left, right) => left.next_run_at! - right.next_run_at!)[0]
  const failedUnknown = rows.some(row => row.last_task_id && !row.last_error_kind && !states[row.last_task_id])
  return <><ConsoleHero eyebrow={`${owner.kind} · Schedules`} title="Tasks" description="Recurring agent runs. Each task executes an active agent’s pinned revision through Labby’s shared Assistant LLM provider and retains its task result." actions={<><Button data-visible-label size="icon-sm" variant="ghost" aria-label="Refresh tasks" onClick={() => void load()} disabled={loading || Boolean(busy)}><RefreshCw /></Button><Button data-visible-label variant="outline" disabled={!canCreate || Boolean(busy)} onClick={() => { setError(undefined); newScheduleId.current = crypto.randomUUID(); setForm('new') }}><Plus />New Task</Button></>} stats={[{ label: 'Scheduled', value: unknown ? '—' : rows.length, suffix: 'tasks' }, { label: 'Armed', value: unknown ? '—' : rows.filter(row => row.armed).length, suffix: 'live', tone: 'var(--aurora-success)' }, { label: 'Next Run', value: unknown || !next ? '—' : new Date(next.next_run_at!).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', ...('timezone' in next.schedule ? { timeZone: next.schedule.timezone } : {}) }), suffix: next?.name, tone: 'var(--aurora-accent-strong)' }, { label: 'Failures', value: unknown || failedUnknown ? '—' : rows.filter(row => row.last_error_kind || (row.last_task_id && states[row.last_task_id]?.state === 'failed')).length, suffix: 'latest runs', tone: 'var(--aurora-error)' }]} />
    {inventoryError ? <p role="alert" className="text-xs text-aurora-error">{inventoryError}</p> : null}{agentError ? <p role="alert" className="text-xs text-aurora-error">Agent inventory: {agentError}</p> : null}{error && !form && !deleting ? <p role="alert" className="text-xs text-aurora-error">{error}</p> : null}{notice ? <p role="status" className="text-xs text-aurora-text-muted">{notice}</p> : null}
    {loading ? <p role="status" className="py-10 text-center text-xs text-aurora-text-muted">Loading scheduled tasks…</p> : inventoryError && !rows.length ? null : <TaskScheduleRows rows={rows} agents={agents} states={states} busy={busy} canOperate={canOperate} canDelete={canDelete} onToggle={row => safe(mutate(row.schedule_id, () => row.armed ? pauseTaskSchedule(row.schedule_id, controller.current.signal) : armTaskSchedule(row.schedule_id, controller.current.signal), row.armed ? 'Task paused.' : 'Task armed.'))} onRun={row => { const key = runKeys.current.get(row.schedule_id) ?? crypto.randomUUID(); runKeys.current.set(row.schedule_id, key); safe(mutate(row.schedule_id, async () => { const response = await runTaskSchedule(row.schedule_id, key, controller.current.signal); runKeys.current.delete(row.schedule_id); return response }, 'Run accepted by the scheduler. Its result will appear after admission.')) }} onEdit={edit} onDelete={row => { setError(undefined); setDeleting(row) }} />}
    {form ? <TaskScheduleForm key={form === 'new' ? 'new' : form.schedule_id} detail={form === 'new' ? undefined : form} agents={agents} pending={busy === 'save'} error={error} onClose={() => { setForm(undefined); setError(undefined) }} onSave={save} /> : null}
    <ActionConfirmationDialog open={Boolean(deleting)} title="Delete this task schedule?" description={`Delete ${deleting?.name ?? 'this schedule'} and its schedule history. Already admitted task runs remain.`} confirmLabel="Delete schedule" busy={busy === deleting?.schedule_id} error={error ? { title: 'Delete failed', detail: error } : undefined} onOpenChange={open => { if (!open) { setDeleting(undefined); setError(undefined) } }} onConfirm={() => { if (deleting) safe(mutate(deleting.schedule_id, async () => { await deleteTaskSchedule(deleting.schedule_id, controller.current.signal); if (!controller.current.signal.aborted) setDeleting(undefined) }, 'Task schedule deleted.')) }} />
  </>
}
export function TaskSchedulesPage() {
  const session = useBrowserSession()
  const authority = session.status === 'authenticated' ? session.authority : undefined
  return <><AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Tasks' }]} /><div className={`${AURORA_PAGE_FRAME} ${AURORA_PAGE_SHELL} !gap-[14px]`}>{authority ? <TaskWorkspace key={authorityIdentity(authority)} owner={authority.activeOwner} capabilities={authority.capabilities} /> : <p className="py-10 text-center text-xs text-aurora-text-muted">Select an authenticated workspace to manage tasks.</p>}</div></>
}
