'use client'

import { useCallback, useEffect, useState } from 'react'
import { AlertCircle, Copy, RefreshCw, RotateCcw, Square } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import {
  getAgentSession,
  getAgentTranscript,
  resumeAgentSession,
  stopAgentSession,
  type AgentRunResult,
  type AgentSessionView,
  type AgentTranscriptView,
} from '@/lib/agent-tasks/client'

const PANEL = 'min-w-0 overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium)]'
const LABEL = 'text-[9.5px] font-bold uppercase tracking-[.1em] text-aurora-text-muted'
const ACTIVE_STATES = new Set(['admitted', 'running', 'cancelling'])
const RESUMABLE_STATES = new Set(['completed', 'failed', 'cancelled', 'revoked', 'interrupted'])

function statusColor(status: string) {
  if (status === 'completed') return 'var(--aurora-success)'
  if (status === 'failed' || status === 'revoked') return 'var(--aurora-error)'
  if (ACTIVE_STATES.has(status)) return 'var(--aurora-accent-strong)'
  return 'var(--aurora-warn)'
}

function shortId(value?: string | null) {
  if (!value) return '—'
  return value.length > 28 ? `${value.slice(0, 18)}…${value.slice(-7)}` : value
}

function timestamp(value?: number | null) {
  if (!value) return '—'
  return new Date(value).toLocaleString()
}

export function AgentSessionLedger({
  sessions,
  loading,
  error,
  onRefresh,
  onOpen,
}: {
  sessions: AgentSessionView[]
  loading: boolean
  error?: string
  onRefresh: () => void
  onOpen: (session: AgentSessionView) => void
}) {
  return <section className={PANEL} aria-label="Agent sessions">
    <header className="flex items-center gap-space-2 border-b border-aurora-border-subtle bg-aurora-control-surface px-space-5 py-space-3">
      <h2 className={`${LABEL} mr-auto`}>Retained sessions</h2>
      {loading ? <span role="status" className="text-[10.5px] text-aurora-text-muted">Refreshing…</span> : null}
      <Button size="icon-sm" variant="ghost" aria-label="Refresh sessions" title="Refresh sessions" onClick={onRefresh} disabled={loading}><RefreshCw /></Button>
    </header>
    {error ? <div className="p-space-5"><Alert variant="error"><AlertCircle /><AlertTitle>Sessions unavailable</AlertTitle><AlertDescription>{error}</AlertDescription></Alert></div> : null}
    {!error && sessions.length ? <div>
      <div className={`hidden grid-cols-[100px_minmax(0,1fr)_minmax(0,1fr)_130px] gap-space-4 border-b border-aurora-border-subtle px-space-5 py-space-2 md:grid ${LABEL}`} aria-hidden="true"><span>Status</span><span>Session</span><span>Agent</span><span>Updated</span></div>
      {sessions.map(session => <button key={session.session_id} type="button" onClick={() => onOpen(session)} className="grid w-full gap-space-2 border-t border-aurora-border-subtle px-space-5 py-space-4 text-left first:border-t-0 hover:bg-aurora-hover-bg focus-visible:outline-2 focus-visible:outline-aurora-accent-primary md:grid-cols-[100px_minmax(0,1fr)_minmax(0,1fr)_130px] md:items-center md:gap-space-4">
        <span className="inline-flex items-center gap-space-2 text-[11px] capitalize" style={{ color: statusColor(session.status) }}><span className="size-1.5 rounded-full bg-current" />{session.status}</span>
        <span className="min-w-0"><strong className="block truncate text-[12px] font-semibold text-aurora-text-primary">{shortId(session.session_id)}</strong>{session.resumed_from_session_id ? <span className="block truncate text-[10px] text-aurora-text-muted">Resumed from {shortId(session.resumed_from_session_id)}</span> : null}</span>
        <span className="truncate text-[11px] text-aurora-text-primary">{session.agent_id} · v{session.agent_version}</span>
        <time dateTime={new Date(session.updated_at).toISOString()} title={timestamp(session.updated_at)} className="text-[10.5px] tabular-nums text-aurora-text-muted">{timestamp(session.updated_at)}</time>
      </button>)}
    </div> : null}
    {!error && !sessions.length && !loading ? <div className="px-space-5 py-10 text-center"><p className="font-display text-[15px] font-bold text-aurora-text-primary">No retained sessions</p><p className="mt-space-1 text-xs text-aurora-text-muted">Start a session from an active Agent definition.</p></div> : null}
  </section>
}

export function AgentSessionViewer({
  selected,
  onClose,
  onChanged,
  onResumed,
}: {
  selected?: Pick<AgentSessionView, 'agent_id' | 'session_id'>
  onClose: () => void
  onChanged: () => void
  onResumed: (session: AgentRunResult) => void
}) {
  const [session, setSession] = useState<AgentSessionView>()
  const [evidence, setEvidence] = useState<AgentTranscriptView>()
  const [error, setError] = useState<string>()
  const [loading, setLoading] = useState(false)
  const [mutation, setMutation] = useState<'stop' | 'resume'>()
  const [copied, setCopied] = useState(false)

  const load = useCallback(async (signal?: AbortSignal) => {
    if (!selected) return
    setLoading(true)
    try {
      const [nextSession, nextEvidence] = await Promise.all([
        getAgentSession(selected.agent_id, selected.session_id, signal),
        getAgentTranscript(selected.agent_id, selected.session_id, signal),
      ])
      setSession(nextSession)
      setEvidence(nextEvidence)
      setError(undefined)
    } catch (cause) {
      if (!(cause instanceof DOMException && cause.name === 'AbortError')) setError(cause instanceof Error ? cause.message : 'Session evidence could not be loaded.')
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [selected])

  useEffect(() => {
    if (!selected) {
      setSession(undefined)
      setEvidence(undefined)
      setError(undefined)
      return
    }
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [load, selected])

  useEffect(() => {
    if (!selected || !session || !ACTIVE_STATES.has(session.status)) return
    const timer = window.setInterval(() => void load(), 1500)
    return () => window.clearInterval(timer)
  }, [load, selected, session])

  async function stop() {
    if (!selected) return
    setMutation('stop')
    setError(undefined)
    try {
      const next = await stopAgentSession(selected.agent_id, selected.session_id)
      setSession(current => current ? { ...current, ...next } : current)
      onChanged()
      window.setTimeout(() => void load(), 300)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The session could not be stopped.')
    } finally {
      setMutation(undefined)
    }
  }

  async function resume() {
    if (!selected) return
    setMutation('resume')
    setError(undefined)
    try {
      const next = await resumeAgentSession(selected.agent_id, selected.session_id)
      onResumed(next)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The session could not be resumed.')
    } finally {
      setMutation(undefined)
    }
  }

  async function copyTranscript() {
    if (!evidence) return
    await navigator.clipboard.writeText(`Input\n-----\n${evidence.input}\n\nTranscript\n----------\n${evidence.transcript ?? ''}`)
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1200)
  }

  return <Sheet open={Boolean(selected)} onOpenChange={open => { if (!open) onClose() }}>
    <SheetContent className="w-full border-aurora-border-default bg-aurora-panel-strong sm:max-w-[680px]">
      <SheetHeader className="border-b border-aurora-border-subtle">
        <SheetTitle className="font-display text-aurora-text-primary">Session evidence</SheetTitle>
        <SheetDescription>{selected ? `${selected.agent_id} · ${shortId(selected.session_id)}` : 'Retained Agent session'}</SheetDescription>
      </SheetHeader>
      <div className="grid min-h-0 flex-1 gap-space-5 overflow-y-auto px-space-5 pb-space-6">
        {error ? <Alert variant="error"><AlertCircle /><AlertTitle>Session action failed</AlertTitle><AlertDescription>{error}</AlertDescription></Alert> : null}
        {loading && !session ? <p role="status" className="py-10 text-center text-xs text-aurora-text-muted">Loading retained evidence…</p> : null}
        {session ? <>
          <div className="flex flex-wrap items-center gap-space-2">
            <span className="inline-flex items-center gap-space-2 text-xs capitalize" style={{ color: statusColor(session.status) }}><span className="size-2 rounded-full bg-current" />{session.status}</span>
            <span className="text-[11px] text-aurora-text-muted">Updated {timestamp(session.updated_at)}</span>
            <div className="ml-auto flex gap-space-2">
              {ACTIVE_STATES.has(session.status) ? <Button size="sm" variant="outline" onClick={() => void stop()} disabled={Boolean(mutation)}><Square />{mutation === 'stop' ? 'Stopping…' : 'Stop'}</Button> : null}
              {RESUMABLE_STATES.has(session.status) ? <Button size="sm" onClick={() => void resume()} disabled={Boolean(mutation)}><RotateCcw />{mutation === 'resume' ? 'Resuming…' : 'Resume'}</Button> : null}
            </div>
          </div>
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-space-4 gap-y-space-2 rounded-aurora-2 border border-aurora-border-subtle bg-aurora-control-surface p-space-4 text-[11px]">
            {[['Session', session.session_id], ['Agent revision', session.agent_version], ['Input digest', session.input_digest], ['Output digest', session.output_digest], ['Error code', session.error_code], ['Started', timestamp(session.created_at)], ['Completed', timestamp(session.completed_at)], ['Resumed from', session.resumed_from_session_id]].map(([label, value]) => <div className="contents" key={String(label)}><dt className="text-aurora-text-muted">{label}</dt><dd className="min-w-0 break-all text-aurora-text-primary">{value ?? '—'}</dd></div>)}
          </dl>
          <section className="grid gap-space-2"><h3 className={LABEL}>Input</h3><pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-aurora-2 border border-aurora-border-subtle bg-aurora-page-bg p-space-4 text-[12px] leading-relaxed text-aurora-text-primary">{evidence?.input ?? 'Loading…'}</pre></section>
          <section className="grid gap-space-2">
            <div className="flex items-center gap-space-2"><h3 className={`${LABEL} mr-auto`}>Transcript</h3>{evidence?.truncated ? <span className="text-[10px] text-aurora-warn">Retained limit reached</span> : null}<Button size="sm" variant="ghost" onClick={() => void copyTranscript()} disabled={!evidence}><Copy />{copied ? 'Copied' : 'Copy'}</Button></div>
            <pre aria-live={ACTIVE_STATES.has(session.status) ? 'polite' : 'off'} className="min-h-40 overflow-auto whitespace-pre-wrap break-words rounded-aurora-2 border border-aurora-border-subtle bg-aurora-page-bg p-space-4 text-[12px] leading-relaxed text-aurora-text-primary">{evidence?.transcript || (ACTIVE_STATES.has(session.status) ? 'Waiting for harness output…' : 'No output was retained.')}</pre>
          </section>
        </> : null}
      </div>
    </SheetContent>
  </Sheet>
}
