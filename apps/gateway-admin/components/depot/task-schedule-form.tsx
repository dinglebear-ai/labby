'use client'

import { useId, useState, type FormEvent } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import type { AgentView } from '@/lib/agent-tasks/client'
import type { RetryPolicy, ScheduleSpec, TaskScheduleDetail } from '@/lib/agent-tasks/schedules'

export type ScheduleDraft = { name: string; agentId: string; input: string; kind: ScheduleSpec['kind']; time: string; timezone: string; weekdays: number[]; expression: string; intervalMinutes: string; date: string; armed: boolean; retryEnabled: boolean; maxRetries: string; backoffMinutes: string }
export function newScheduleDraft(detail?: TaskScheduleDetail): ScheduleDraft {
  const spec = detail?.schedule
  const at = spec?.kind === 'once' ? new Date(spec.at) : undefined
  const localDate = at ? `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, '0')}-${String(at.getDate()).padStart(2, '0')}T${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}` : ''
  return { name: detail?.name ?? '', agentId: detail?.agent_id ?? '', input: detail?.task_template.input ?? '', kind: spec?.kind ?? 'daily', time: spec && 'hour' in spec ? `${String(spec.hour).padStart(2, '0')}:${String(spec.minute).padStart(2, '0')}` : '02:00', timezone: spec && 'timezone' in spec ? spec.timezone : Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC', weekdays: spec?.kind === 'weekly' ? spec.weekdays : [1], expression: spec?.kind === 'cron' ? spec.expression : '0 3 * * *', intervalMinutes: spec?.kind === 'interval' ? String(spec.every_ms / 60_000) : '60', date: localDate, armed: detail?.armed ?? false, retryEnabled: (detail?.retry_policy?.max_retries ?? 0) > 0, maxRetries: String(detail?.retry_policy?.max_retries || 2), backoffMinutes: String((detail?.retry_policy?.backoff_ms ?? 300000) / 60000) }
}
export function validateRetryPolicy(draft: ScheduleDraft): RetryPolicy {
  if (!draft.retryEnabled) return { max_retries: 0, backoff_ms: 300000 }
  const max_retries = Number(draft.maxRetries), backoff_ms = Number(draft.backoffMinutes) * 60000
  if (!Number.isInteger(max_retries) || max_retries < 1 || max_retries > 10) throw new Error('Choose 1–10 retries.')
  if (!Number.isSafeInteger(backoff_ms) || backoff_ms < 60000 || backoff_ms > 86400000) throw new Error('Choose a retry delay from 1 minute to 24 hours.')
  return { max_retries, backoff_ms }
}
export function validateScheduleDraft(draft: ScheduleDraft, now = Date.now()): ScheduleSpec {
  if (!draft.name.trim() || draft.name.trim().length > 200) throw new Error('Enter a task name of 1–200 characters.')
  if (!draft.agentId) throw new Error('Choose an active agent in this workspace.')
  if (!draft.input.trim() || new TextEncoder().encode(draft.input).length > 1024 * 1024) throw new Error('Enter a prompt of at most 1 MiB.')
  if (draft.kind === 'once') { const at = new Date(draft.date).getTime(); if (!Number.isFinite(at) || at <= now) throw new Error('Choose a future date and time.'); return { kind: 'once', at } }
  if (draft.kind === 'interval') { const every_ms = Number(draft.intervalMinutes) * 60_000; if (!Number.isSafeInteger(every_ms) || every_ms < 60_000 || every_ms > 366 * 86400_000) throw new Error('Choose an interval from 1 minute to 366 days.'); return { kind: 'interval', every_ms } }
  try { new Intl.DateTimeFormat('en', { timeZone: draft.timezone }).format() } catch { throw new Error('Enter a valid IANA timezone, such as America/New_York.') }
  if (draft.kind === 'cron') { if (draft.expression.trim().split(/\s+/).length !== 5) throw new Error('Enter a five-field cron expression: minute hour day month weekday.'); return { kind: 'cron', expression: draft.expression.trim(), timezone: draft.timezone } }
  const [hour, minute] = draft.time.split(':').map(Number)
  if (!/^\d{2}:\d{2}$/.test(draft.time) || hour > 23 || minute > 59) throw new Error('Enter a valid time.')
  if (draft.kind === 'daily') return { kind: 'daily', hour, minute, timezone: draft.timezone }
  if (!draft.weekdays.length) throw new Error('Choose at least one weekday.')
  return { kind: 'weekly', weekdays: [...draft.weekdays].sort(), hour, minute, timezone: draft.timezone }
}

export function TaskScheduleForm({ detail, agents, pending, error, onClose, onSave }: { detail?: TaskScheduleDetail; agents: AgentView[]; pending: boolean; error?: string; onClose: () => void; onSave: (draft: ScheduleDraft, spec: ScheduleSpec) => Promise<void> }) {
  const [draft, setDraft] = useState(() => newScheduleDraft(detail))
  const [invalid, setInvalid] = useState<string>()
  const id = useId()
  const update = <K extends keyof ScheduleDraft>(key: K, value: ScheduleDraft[K]) => { setDraft(current => ({ ...current, [key]: value })); setInvalid(undefined) }
  const submit = (event: FormEvent) => { event.preventDefault(); try { const spec = validateScheduleDraft(draft); validateRetryPolicy(draft); void onSave(draft, spec) } catch (reason) { setInvalid(reason instanceof Error ? reason.message : 'Check the schedule fields.') } }
  const field = 'grid gap-1.5 text-xs text-aurora-text-muted'
  return <Dialog open onOpenChange={open => { if (!open && !pending) onClose() }}><DialogContent className="sm:max-w-[580px] border-aurora-border-strong bg-aurora-panel-strong text-aurora-text-primary" showCloseButton={!pending}><DialogHeader><DialogTitle>{detail ? 'Edit Task' : 'New Task'}</DialogTitle><DialogDescription className="text-aurora-text-muted">Schedule an active agent in this workspace. Its pinned revision and the shared Assistant LLM provider determine execution.</DialogDescription></DialogHeader>
    <form onSubmit={submit} className="aurora-scrollbar min-h-0 space-y-4 overflow-y-auto pr-1">
      <fieldset disabled={pending} className="space-y-4 disabled:opacity-60">
        <label className={field}>Task name<Input autoFocus maxLength={200} value={draft.name} onChange={event => update('name', event.target.value)} /></label>
        <div className={field}><label htmlFor={`${id}-agent`}>Agent</label><Select value={draft.agentId} onValueChange={value => update('agentId', value)}><SelectTrigger id={`${id}-agent`} className="w-full"><SelectValue placeholder="Choose an active agent" /></SelectTrigger><SelectContent>{agents.filter(agent => agent.state === 'active').map(agent => <SelectItem key={agent.agent_id} value={agent.agent_id}>{agent.agent_id} · v{agent.version}</SelectItem>)}</SelectContent></Select>{!agents.some(agent => agent.state === 'active') ? <p>No active agents in this workspace. Create an agent before scheduling a task.</p> : null}</div>
        <label className={field}>Prompt<Textarea value={draft.input} rows={4} onChange={event => update('input', event.target.value)} placeholder="What should this agent do each run?" /></label>
        <div className="grid gap-3 sm:grid-cols-2"><div className={field}><label htmlFor={`${id}-cadence`}>Cadence</label><Select value={draft.kind} onValueChange={value => update('kind', value as ScheduleSpec['kind'])}><SelectTrigger id={`${id}-cadence`} className="w-full"><SelectValue /></SelectTrigger><SelectContent>{(['once', 'interval', 'daily', 'weekly', 'cron'] as const).map(kind => <SelectItem key={kind} value={kind}>{({ once: 'Once', interval: 'Interval', daily: 'Daily', weekly: 'Selected weekdays', cron: 'Cron' })[kind]}</SelectItem>)}</SelectContent></Select></div>
          {draft.kind === 'once' ? <label className={field}>Date and time (browser local)<Input type="datetime-local" value={draft.date} onChange={event => update('date', event.target.value)} /></label> : draft.kind === 'interval' ? <label className={field}>Every (minutes)<Input type="number" min={1} max={527040} step="any" value={draft.intervalMinutes} onChange={event => update('intervalMinutes', event.target.value)} /></label> : draft.kind === 'cron' ? <label className={field}>Cron expression<Input value={draft.expression} onChange={event => update('expression', event.target.value)} placeholder="0 3 * * *" /></label> : <label className={field}>Time<Input type="time" value={draft.time} onChange={event => update('time', event.target.value)} /></label>}</div>
        {draft.kind === 'weekly' ? <div className={field}><span>Weekdays</span><div className="flex flex-wrap gap-1.5">{['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'].map((day, index) => <Button key={day} type="button" data-visible-label size="sm" variant={draft.weekdays.includes(index) ? 'secondary' : 'outline'} aria-pressed={draft.weekdays.includes(index)} onClick={() => update('weekdays', draft.weekdays.includes(index) ? draft.weekdays.filter(value => value !== index) : [...draft.weekdays, index])}>{day}</Button>)}</div></div> : null}
        {!['once', 'interval'].includes(draft.kind) ? <label className={field}>Timezone<Input value={draft.timezone} onChange={event => update('timezone', event.target.value)} placeholder="America/New_York" /></label> : null}
        {!detail ? <label className="flex items-center gap-2 text-xs"><Switch checked={draft.armed} onCheckedChange={value => update('armed', value)} />Arm after creating</label> : null}
        <label className="flex items-center gap-2 text-xs"><Switch checked={draft.retryEnabled} onCheckedChange={value => update('retryEnabled', value)} />Retry execution failures</label>
        {draft.retryEnabled ? <div className="grid grid-cols-2 gap-3"><label className={field}>Maximum retries<Input type="number" min={1} max={10} value={draft.maxRetries} onChange={event => update('maxRetries', event.target.value)} /></label><label className={field}>Retry delay (minutes)<Input type="number" min={1} max={1440} step="any" value={draft.backoffMinutes} onChange={event => update('backoffMinutes', event.target.value)} /></label></div> : null}
      </fieldset>
      <p className="text-[11px] leading-relaxed text-aurora-text-muted">Missed runs coalesce into one run. Nonexistent daylight-saving times are skipped; repeated times run twice. Retries apply only to execution failures, with a fixed delay after the failure is observed. Cancellation, access loss, and configuration failures are never retried. Scheduling delegates future runs beyond this browser session; current access is checked before every admission.</p>
      {invalid || error ? <p role="alert" className="text-xs text-aurora-error">{invalid ?? error}</p> : null}
      <div className="flex justify-end gap-2"><Button type="button" data-visible-label variant="outline" disabled={pending} onClick={onClose}>Cancel</Button><Button type="submit" data-visible-label disabled={pending || !agents.some(agent => agent.state === 'active')}>{pending ? 'Saving…' : detail ? 'Save changes' : 'Create Task'}</Button></div>
    </form>
  </DialogContent></Dialog>
}
