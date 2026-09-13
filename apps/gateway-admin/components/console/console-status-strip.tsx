'use client'

import * as React from 'react'
import Link from 'next/link'
import { useBrowserSession } from '@/lib/auth/session'
import { authorityIdentity } from '@/lib/auth/authority'

import { GatewayApiError, gatewayAction, gatewayApi } from '@/lib/api/gateway-client'
import { isAbortError } from '@/lib/api/service-action-client'
import type { GatewayNotification } from '@/lib/notification-acknowledgements'
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
  | { kind: 'ready'; snapshot: ConsoleStatusSnapshot; attention?: string[]; disconnectedOccurrences?: Record<string, string>; alerts?: GatewayNotification[] }
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
  const [runtimeResult, clientsResult, gatewayResult] = await Promise.allSettled([
    gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
    gatewayAction<GatewayClientView[]>('gateway.clients.list', {}, signal),
    gatewayApi.list(signal),
  ])
  if (runtimeResult.status !== 'fulfilled') return classifyStatusFailure(runtimeResult.reason)
  const snapshot = deriveConsoleStatus(runtimeResult.value, clientsResult.status === 'fulfilled' ? clientsResult.value : undefined)
  if (clientsResult.status === 'rejected' && !isAbortError(clientsResult.reason)) snapshot.sessionsUnavailable = failureReason(clientsResult.reason)
  const alerts = gatewayResult.status === 'fulfilled' ? gatewayResult.value.flatMap(gateway => (gateway.warnings ?? []).map(warning => ({ key: `gateway:${gateway.name}:warning:${warning.code}`, fingerprint: warning.occurrence_id ?? `${warning.code}:${warning.message}`, gatewayName: gateway.name, message: warning.message }))) : undefined
  if (alerts) for (const runtime of runtimeResult.value) if ((runtime.likely_stale_count ?? 0) > 0) alerts.push({ key: `gateway:${runtime.name}:stale`, fingerprint: runtime.notification_incidents?.stale ?? String(runtime.likely_stale_count), gatewayName: runtime.name, message: `${runtime.likely_stale_count} likely stale processes` })
  return { kind: 'ready', snapshot, alerts, disconnectedOccurrences: Object.fromEntries(runtimeResult.value.flatMap(row => row.notification_incidents?.tools ? [[row.name, row.notification_incidents.tools]] : [])), attention: runtimeResult.value.filter(row => row.enabled !== false && row.connected !== true).map(row => row.name) }
}

function Metric({
  value,
  label,
  color,
  title,
  href,
}: {
  value: React.ReactNode
  label: string
  color: string
  /** Explains an unavailable value; also exposed as the accessible name. */
  title?: string
  href: string
}) {
  return (
    <Link
      href={href}
      data-menurow="1"
      title={title}
      aria-label={title}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        height: 26,
        padding: '0 9px',
        border: '1px solid transparent',
        borderRadius: 8,
        textDecoration: 'none',
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
    </Link>
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
            href="/gateways"
            label="up"
            color={upstreamMetricColor(state.snapshot)}
          />
          {state.snapshot.sessions !== undefined ? (
            <Metric
              value={state.snapshot.sessions}
              href="/agents"
              label="sessions"
              color="var(--aurora-accent-pink)"
            />
          ) : state.snapshot.sessionsUnavailable ? (
            <Metric
              value="—"
              href="/agents"
              label="sessions"
              color="var(--aurora-text-muted)"
              title={`Session count is unavailable: ${state.snapshot.sessionsUnavailable}`}
            />
          ) : null}
          <Metric
            value={state.snapshot.tools}
            href="/tools"
            label="tools"
            color="var(--aurora-accent-strong)"
          />
        </>
      )
  }
}

export function useConsoleStatus() {
  const session = useBrowserSession()
  const identity = session.status === 'authenticated' ? authorityIdentity(session.authority) : session.status
  const [result, setResult] = React.useState<{ identity: string; state: ConsoleStatusState }>({ identity, state: { kind: 'loading' } })

  React.useEffect(() => {
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    const refresh = async () => {
      try {
        const state = await loadConsoleStatus(controller.signal)
        if (!controller.signal.aborted) setResult({ identity, state })
      } catch (error: unknown) {
        if (!controller.signal.aborted) setResult({ identity, state: classifyStatusFailure(error) })
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(() => { void refresh() }, 30_000)
      }
    }
    void refresh()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [identity])

  return result.identity === identity ? result.state : { kind: 'loading' } as ConsoleStatusState
}

export function ConsoleStatusStrip({ state }: { state: ConsoleStatusState }) {

  return (
    <div
      data-console-status-strip="1"
      className="max-[1040px]:!hidden"
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 0,
        minWidth: 0,
        marginLeft: 14,
        paddingLeft: 8,
        borderLeft: '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, transparent)',
        flexShrink: 0,
      }}
    >
      <ConsoleStatusContent state={state} />
    </div>
  )
}
