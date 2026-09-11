'use client'

import * as React from 'react'

import { gatewayAction } from '@/lib/api/gateway-client'
import type { BackendGatewayMcpRuntimeView } from '@/lib/server/gateway-adapter'

export interface GatewayClientView {
  subject?: string | null
  client_name?: string | null
  client_version?: string | null
  transport: string
  connected_at: string
}

export interface ConsoleStatusSnapshot {
  connected: number
  total: number
  sessions?: number
  tools: number
}

export function deriveConsoleStatus(
  runtime: readonly BackendGatewayMcpRuntimeView[],
  clients?: readonly GatewayClientView[],
): ConsoleStatusSnapshot {
  const enabled = runtime.filter((row) => row.enabled !== false)
  return {
    connected: enabled.filter((row) => row.connected === true).length,
    total: enabled.length,
    sessions: clients?.length,
    tools: enabled.reduce((sum, row) => sum + (row.exposed_tool_count ?? 0), 0),
  }
}

/** Colour for the "up" metric: healthy only when every enabled upstream is connected. */
export function upstreamMetricColor(snapshot: Pick<ConsoleStatusSnapshot, 'connected' | 'total'>): string {
  if (snapshot.total === 0) return 'var(--aurora-text-muted)'
  return snapshot.connected === snapshot.total ? 'var(--aurora-success)' : 'var(--aurora-warn)'
}

type StatusState =
  | { kind: 'loading' }
  | { kind: 'ready'; snapshot: ConsoleStatusSnapshot }
  | { kind: 'unavailable'; reason: string }

function failureReason(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

async function loadConsoleStatus(signal: AbortSignal): Promise<StatusState> {
  const [runtimeResult, clientsResult] = await Promise.allSettled([
    gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
    gatewayAction<GatewayClientView[]>('gateway.clients.list', {}, signal),
  ])
  if (runtimeResult.status !== 'fulfilled') {
    return { kind: 'unavailable', reason: failureReason(runtimeResult.reason) }
  }
  return {
    kind: 'ready',
    snapshot: deriveConsoleStatus(
      runtimeResult.value,
      clientsResult.status === 'fulfilled' ? clientsResult.value : undefined,
    ),
  }
}

function Metric({
  value,
  label,
  color,
}: {
  value: React.ReactNode
  label: string
  color: string
}) {
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'baseline',
        gap: 5,
        color: 'var(--aurora-text-muted)',
        fontSize: 12.5,
        lineHeight: 'normal',
        fontWeight: 600,
        whiteSpace: 'nowrap',
      }}
    >
      <span style={{ color, fontWeight: 700, fontVariantNumeric: 'tabular-nums' }}>{value}</span>
      <span>{label}</span>
    </span>
  )
}

// Status chrome is best-effort: the console stays fully usable when the
// snapshot is unavailable (for example when the viewer lacks admin scope),
// but the failure is still shown rather than rendered as "nothing to report".
function ConsoleStatusContent({ state }: { state: StatusState }): React.ReactElement | null {
  switch (state.kind) {
    case 'loading':
      return null
    case 'unavailable':
      return (
        <span
          data-console-status-unavailable="1"
          title={`Gateway status is unavailable: ${state.reason}`}
          style={{ color: 'var(--aurora-text-muted)', fontSize: 11.5, lineHeight: 'normal', whiteSpace: 'nowrap' }}
        >
          status unavailable
        </span>
      )
    case 'ready':
      return (
        <>
          <Metric
            value={`${state.snapshot.connected}/${state.snapshot.total}`}
            label="up"
            color={upstreamMetricColor(state.snapshot)}
          />
          {state.snapshot.sessions !== undefined ? (
            <Metric
              value={state.snapshot.sessions}
              label="sessions"
              color="var(--aurora-accent-pink)"
            />
          ) : null}
          <Metric
            value={state.snapshot.tools}
            label="tools"
            color="var(--aurora-accent-strong)"
          />
        </>
      )
  }
}

export function ConsoleStatusStrip() {
  const [state, setState] = React.useState<StatusState>({ kind: 'loading' })

  React.useEffect(() => {
    const controller = new AbortController()
    loadConsoleStatus(controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) setState(next)
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return
        setState({ kind: 'unavailable', reason: failureReason(error) })
      })
    return () => controller.abort()
  }, [])

  return (
    <div
      data-console-status-strip="1"
      className="max-[1040px]:!hidden"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 20,
        minWidth: 0,
        marginLeft: 6,
        flexShrink: 0,
      }}
    >
      <ConsoleStatusContent state={state} />
    </div>
  )
}
