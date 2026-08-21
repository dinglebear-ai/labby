'use client'

import { OutcomeDot } from './recent-calls'
import { Skeleton } from '@/components/ui/skeleton'
import type { ToolCallRecord } from '@/lib/types/metrics'
import { formatDuration, formatRelativeTime } from '@/lib/dashboard/dashboard-metrics'

function formatBytes(value: number | null | undefined) {
  if (value === null || value === undefined) return '—'
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

export function UsageCallCards({
  calls,
  isLoading,
  error,
  onRetry,
}: {
  calls: ToolCallRecord[] | undefined
  isLoading: boolean
  error: unknown
  onRetry: () => void
}) {
  if (error && !calls) {
    return (
      <div className="py-8 text-center">
        <span className="text-sm text-aurora-error">Couldn&apos;t load calls. </span>
        <button
          type="button"
          onClick={onRetry}
          className="text-sm font-medium text-aurora-accent-primary underline-offset-4 hover:underline"
        >
          Retry
        </button>
      </div>
    )
  }

  if (isLoading && !calls) {
    return <div className="space-y-2">{Array.from({ length: 6 }, (_, index) => <Skeleton key={index} className="h-28 w-full rounded-lg" />)}</div>
  }

  if (!calls || calls.length === 0) {
    return <div className="py-10 text-center text-sm text-aurora-text-muted">No calls match these filters.</div>
  }

  return (
    <div className="space-y-2">
      {calls.map((call) => (
        <article
          key={call.id}
          className="rounded-lg border border-aurora-border-subtle bg-aurora-control-surface/10 p-3 shadow-[inset_0_1px_0_rgba(255,255,255,0.025)]"
        >
          <div className="flex min-w-0 items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="truncate font-mono text-[12px] font-semibold text-aurora-text-primary">{call.tool}</p>
              {call.action || (call.capability && call.capability !== 'tools') ? (
                <p className="truncate font-mono text-[11px] text-aurora-text-muted">
                  {[call.capability && call.capability !== 'tools' ? call.capability : null, call.action].filter(Boolean).join(' · ')}
                </p>
              ) : null}
            </div>
            <span className="shrink-0 text-[11px] text-aurora-text-muted">{formatRelativeTime(call.ts)}</span>
          </div>
          <div className="mt-3 grid grid-cols-2 gap-x-3 gap-y-2 text-[11px]">
            <div className="min-w-0">
              <div className="uppercase tracking-[0.08em] text-aurora-text-muted">Agent</div>
              <div className="truncate text-aurora-text-primary">{call.agent_label === 'unattributed' ? 'Not attributed' : call.agent_label}</div>
            </div>
            <div>
              <div className="uppercase tracking-[0.08em] text-aurora-text-muted">Outcome</div>
              <span className="inline-flex items-center gap-1.5 text-aurora-text-primary">
                <OutcomeDot outcome={call.outcome} />
                {call.outcome === 'failed' ? (call.error_kind ?? 'failed') : 'ok'}
              </span>
            </div>
            <div>
              <div className="uppercase tracking-[0.08em] text-aurora-text-muted">Latency</div>
              <div className="font-mono text-aurora-text-primary">{formatDuration(call.elapsed_ms)}</div>
            </div>
            <div>
              <div className="uppercase tracking-[0.08em] text-aurora-text-muted">Response</div>
              <div className="font-mono text-aurora-text-primary">{formatBytes(call.response_bytes)}</div>
            </div>
          </div>
        </article>
      ))}
    </div>
  )
}
