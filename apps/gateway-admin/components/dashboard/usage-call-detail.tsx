'use client'

import Link from 'next/link'
import { Activity, ExternalLink } from 'lucide-react'

import { Button } from '@/components/ui/button'
import {
  DetailDrawer,
  DrawerSection,
  DrawerStatGrid,
} from '@/components/dashboard/detail-drawer'
import { OutcomeDot, SurfaceTag } from '@/components/dashboard/recent-calls'
import {
  formatCompactNumber,
  formatDuration,
  formatRelativeTime,
} from '@/lib/dashboard/dashboard-metrics'
import { usageTraceHref } from '@/lib/observability/usage-trace-link'
import type { ToolCallRecord } from '@/lib/types/metrics'

function formatBytes(value: number | null | undefined) {
  if (value === null || value === undefined) return '—'
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

function DetailRow({
  label,
  children,
  mono = false,
}: {
  label: string
  children: React.ReactNode
  mono?: boolean
}) {
  return (
    <div className="flex items-start justify-between gap-5 border-b border-aurora-border-subtle py-2.5 last:border-b-0">
      <span className="shrink-0 text-xs text-aurora-text-muted">{label}</span>
      <span className={`min-w-0 text-right text-sm text-aurora-text-primary ${mono ? 'font-mono text-[12px]' : ''}`}>
        {children}
      </span>
    </div>
  )
}

export function UsageCallDetailContent({
  call,
  tokensCollected,
  ipsCollected,
  surfacesCollected,
}: {
  call: ToolCallRecord
  tokensCollected: boolean
  ipsCollected: boolean
  surfacesCollected: boolean
}) {
  const outcomeLabel = call.outcome === 'failed' ? (call.error_kind ?? 'failed') : 'ok'
  const totalTokens = call.input_tokens + call.output_tokens

  return (
    <>
      <DrawerStatGrid
        items={[
          {
            label: 'Outcome',
            value: (
              <span className="inline-flex items-center gap-2">
                <OutcomeDot outcome={call.outcome} />
                {outcomeLabel}
              </span>
            ),
            tone: call.outcome === 'failed' ? 'error' : 'success',
          },
          { label: 'Latency', value: formatDuration(call.elapsed_ms) },
          { label: 'Response', value: formatBytes(call.response_bytes) },
          ...(surfacesCollected ? [{ label: 'Surface', value: <SurfaceTag surface={call.surface} /> }] : []),
        ]}
      />

      <DrawerSection
        title="Call details"
        action={(
          <Button asChild size="sm" variant="outline" className="h-8 text-xs">
            <Link href={usageTraceHref(call.tool)}>
              View traces <ExternalLink className="size-3.5" />
            </Link>
          </Button>
        )}
      >
        <div className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium px-4">
          <DetailRow label="Target" mono>{call.tool}</DetailRow>
          <DetailRow label="Operation" mono>{call.action ?? '—'}</DetailRow>
          <DetailRow label="Capability" mono>{call.capability ?? '—'}</DetailRow>
          <DetailRow label="Agent">
            {call.agent_label === 'unattributed' ? 'Not attributed' : call.agent_label}
          </DetailRow>
          <DetailRow label="OAuth scope">
            {call.subject_scoped ? 'Subject-scoped' : 'Shared / not subject-scoped'}
          </DetailRow>
          {ipsCollected && call.ip ? <DetailRow label="Source IP" mono>{call.ip}</DetailRow> : null}
          {call.error_kind ? <DetailRow label="Failure kind" mono>{call.error_kind}</DetailRow> : null}
        </div>
      </DrawerSection>

      {tokensCollected ? (
        <DrawerSection title="Tokens">
          <div className="grid grid-cols-3 gap-3">
            {[
              ['Input', call.input_tokens],
              ['Output', call.output_tokens],
              ['Total', totalTokens],
            ].map(([label, value]) => (
              <div key={label} className="rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium px-3 py-3">
                <div className="text-lg font-bold tabular-nums text-aurora-text-primary">
                  {formatCompactNumber(value as number)}
                </div>
                <div className="mt-1 text-[11px] uppercase tracking-[0.08em] text-aurora-text-muted">
                  {label}
                </div>
              </div>
            ))}
          </div>
        </DrawerSection>
      ) : null}
    </>
  )
}

export function UsageCallDetail({
  call,
  onClose,
  tokensCollected,
  ipsCollected,
  surfacesCollected,
}: {
  call: ToolCallRecord | null
  onClose: () => void
  tokensCollected: boolean
  ipsCollected: boolean
  surfacesCollected: boolean
}) {
  if (!call) {
    return (
      <DetailDrawer
        open={false}
        onClose={onClose}
        icon={<Activity className="size-4" />}
        title="Upstream call"
        subtitle=""
      >
        <span />
      </DetailDrawer>
    )
  }

  const operation = [call.tool, call.action].filter(Boolean).join('.')

  return (
    <DetailDrawer
      open
      onClose={onClose}
      icon={<Activity className="size-4" />}
      title={operation}
      subtitle={`${formatRelativeTime(call.ts)} · ${call.agent_label === 'unattributed' ? 'Not attributed' : call.agent_label}`}
    >
      <UsageCallDetailContent
        call={call}
        tokensCollected={tokensCollected}
        ipsCollected={ipsCollected}
        surfacesCollected={surfacesCollected}
      />
    </DetailDrawer>
  )
}
