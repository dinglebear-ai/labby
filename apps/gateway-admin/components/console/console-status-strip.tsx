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

declare global {
  interface Window {
    __LABBY_CONSOLE_STATUS_FIXTURE__?: ConsoleStatusSnapshot
  }
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

async function loadConsoleStatus(signal: AbortSignal): Promise<ConsoleStatusSnapshot | null> {
  if (typeof window !== 'undefined' && window.__LABBY_CONSOLE_STATUS_FIXTURE__) {
    return window.__LABBY_CONSOLE_STATUS_FIXTURE__
  }

  const [runtimeResult, clientsResult] = await Promise.allSettled([
    gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
    gatewayAction<GatewayClientView[]>('gateway.clients.list', {}, signal),
  ])
  if (runtimeResult.status !== 'fulfilled') return null

  return deriveConsoleStatus(
    runtimeResult.value,
    clientsResult.status === 'fulfilled' ? clientsResult.value : undefined,
  )
}

function EnvironmentBadge() {
  const environment = process.env.NODE_ENV === 'production' ? 'PROD' : 'DEV'
  return (
    <span
      title="Gateway environment"
      data-console-environment="1"
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 4,
        height: 20,
        padding: '0 7px',
        borderRadius: 5,
        border: '1px solid color-mix(in srgb, var(--aurora-success) 30%, transparent)',
        background: 'color-mix(in srgb, var(--aurora-success) 11%, transparent)',
        color: 'var(--aurora-success)',
        fontSize: 8,
        lineHeight: 'normal',
        fontWeight: 700,
        letterSpacing: '0.1em',
        flexShrink: 0,
      }}
    >
      <span
        aria-hidden="true"
        style={{
          width: 4,
          height: 4,
          borderRadius: 999,
          background: 'currentColor',
          boxShadow: '0 0 4px currentColor',
          flexShrink: 0,
        }}
      />
      {environment}
    </span>
  )
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
      <span style={{ color, fontWeight: 700, fontVariantNumeric: "tabular-nums" }}>{value}</span>
      <span>{label}</span>
    </span>
  )
}

export function ConsoleStatusStrip() {
  const [snapshot, setSnapshot] = React.useState<ConsoleStatusSnapshot | null>(() => (
    typeof window !== 'undefined' ? (window.__LABBY_CONSOLE_STATUS_FIXTURE__ ?? null) : null
  ))

  React.useEffect(() => {
    if (snapshot) return
    const controller = new AbortController()
    loadConsoleStatus(controller.signal)
      .then((next) => {
        if (!controller.signal.aborted) setSnapshot(next)
      })
      .catch(() => {
        // Status chrome is best-effort. The console remains fully usable if
        // this lightweight snapshot is unavailable or the viewer lacks admin scope.
      })
    return () => controller.abort()
  }, [snapshot])

  return (
    <div
      data-console-status-strip="1"
      className="max-[1040px]:!hidden"
      style={{
        display: 'flex',
        alignItems: 'center',
        minWidth: 0,
        marginLeft: 6,
        flexShrink: 0,
      }}
    >
      <EnvironmentBadge />
      {snapshot ? (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 20,
            marginLeft: 57,
            minWidth: 0,
          }}
        >
          <Metric
            value={`${snapshot.connected}/${snapshot.total}`}
            label="up"
            color="var(--aurora-warn)"
          />
          {snapshot.sessions !== undefined ? (
            <Metric
              value={snapshot.sessions}
              label="sessions"
              color="var(--aurora-accent-pink)"
            />
          ) : null}
          <Metric
            value={snapshot.tools}
            label="tools"
            color="var(--aurora-accent-strong)"
          />
        </div>
      ) : null}
    </div>
  )
}
