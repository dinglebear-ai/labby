'use client'

import * as React from 'react'

import { GatewayApiError, gatewayAction } from '@/lib/api/gateway-client'
import { isAbortError } from '@/lib/api/service-action-client'
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
  /** Set when the client inventory request failed, so the sessions metric reads as unavailable rather than vanishing. */
  sessionsUnavailable?: string
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

export type ConsoleStatusState =
  | { kind: 'loading' }
  | { kind: 'ready'; snapshot: ConsoleStatusSnapshot }
  /** The viewer lacks gateway admin scope: expected, so no chrome is shown. */
  | { kind: 'unauthorized' }
  | { kind: 'unavailable'; reason: string }

function failureReason(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/**
 * Missing admin scope is an expected outcome for ordinary viewers; an aborted
 * request (unmount or authority change) is not a failure; anything else is surfaced.
 */
export function classifyStatusFailure(error: unknown): ConsoleStatusState {
  if (isAbortError(error)) return { kind: 'loading' }
  if (error instanceof GatewayApiError && (error.status === 401 || error.status === 403)) return { kind: 'unauthorized' }
  return { kind: 'unavailable', reason: failureReason(error) }
}

async function loadConsoleStatus(signal: AbortSignal): Promise<ConsoleStatusState> {
  const [runtimeResult, clientsResult] = await Promise.allSettled([
    gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
    gatewayAction<GatewayClientView[]>('gateway.clients.list', {}, signal),
  ])
  if (runtimeResult.status !== 'fulfilled') return classifyStatusFailure(runtimeResult.reason)
  const snapshot = deriveConsoleStatus(runtimeResult.value, clientsResult.status === 'fulfilled' ? clientsResult.value : undefined)
  if (clientsResult.status === 'rejected' && !isAbortError(clientsResult.reason)) snapshot.sessionsUnavailable = failureReason(clientsResult.reason)
  return { kind: 'ready', snapshot }
}

function Metric({
  value,
  label,
  color,
  title,
}: {
  value: React.ReactNode
  label: string
  color: string
  /** Explains an unavailable value; also exposed as the accessible name. */
  title?: string
}) {
  return (
    <span
      title={title}
      aria-label={title}
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
export function ConsoleStatusContent({ state }: { state: ConsoleStatusState }): React.ReactElement | null {
  switch (state.kind) {
    case 'loading':
    case 'unauthorized':
      return null
    case 'unavailable':
      return (
        <span
          data-console-status-unavailable="1"
          role="status"
          aria-label={`Gateway status is unavailable: ${state.reason}`}
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
          ) : state.snapshot.sessionsUnavailable ? (
            <Metric
              value="—"
              label="sessions"
              color="var(--aurora-text-muted)"
              title={`Session count is unavailable: ${state.snapshot.sessionsUnavailable}`}
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
  const [state, setState] = React.useState<ConsoleStatusState>({ kind: 'loading' })

  React.useEffect(() => {
    const controller = new AbortController()
    loadConsoleStatus(controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) setState(next)
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return
        setState(classifyStatusFailure(error))
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
