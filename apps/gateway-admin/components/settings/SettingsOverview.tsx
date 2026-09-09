'use client'

import type { ReactNode } from 'react'
import { useEffect, useState } from 'react'

import { setupApi } from '@/lib/api/setup-client'
import {
  SettingsCard,
  SettingsRow,
  SettingsToggle,
  SettingsValue,
} from './SettingsChrome'

export interface SettingsOverviewSnapshot {
  codeModeEnabled: boolean
  gatewayEndpoint: string
  configPath: string
}

const EMPTY_SNAPSHOT: SettingsOverviewSnapshot = {
  codeModeEnabled: false,
  gatewayEndpoint: 'Unavailable',
  configPath: 'Unavailable',
}

function firstNonEmptyString(...values: unknown[]): string | undefined {
  return values.find((value): value is string => typeof value === 'string' && value.trim().length > 0)
}

export function resolveGatewayEndpoint(values: Record<string, unknown>): string {
  const raw = firstNonEmptyString(
    values.LABBY_MCP_GATEWAY_URL,
    values['public_urls.mcp_gateway'],
    values.LABBY_PUBLIC_URL,
    values['public_urls.app'],
  )
  if (!raw) return 'Unavailable'
  try {
    const url = new URL(raw)
    const path = url.pathname.replace(/\/+$/, '')
    if (!path.endsWith('/mcp')) url.pathname = `${path}/mcp`
    url.search = ''
    url.hash = ''
    return url.toString().replace(/\/$/, '')
  } catch {
    return raw
  }
}

export function SettingsOverviewCards({
  snapshot,
  console,
}: {
  snapshot: SettingsOverviewSnapshot
  console: ReactNode
}): React.ReactElement {
  return (
    <>
      <SettingsCard title="Gateway">
        <SettingsRow
          label="Code Mode"
          description="Expose the catalog as a single code-execution tool instead of individual tool schemas."
          control={<SettingsToggle checked={snapshot.codeModeEnabled} readOnly label="Code Mode" />}
        />
        <SettingsRow
          label="Auto-Reconnect"
          description="Retry disconnected upstream servers with exponential backoff."
          control={<SettingsToggle checked readOnly label="Auto-Reconnect" />}
        />
        <SettingsRow
          label="Gateway Endpoint"
          description="Base URL downstream clients connect to."
          control={<SettingsValue>{snapshot.gatewayEndpoint}</SettingsValue>}
        />
      </SettingsCard>

      {console}

      <SettingsCard title="Diagnostics">
        <SettingsRow
          label="Usage Telemetry"
          description="Record per-tool call metrics (total_calls, error_calls, avg_elapsed_ms)."
          control={<SettingsToggle checked readOnly label="Usage Telemetry" />}
        />
        <SettingsRow
          label="Config Path"
          description="Gateway configuration file on disk."
          control={<SettingsValue>{snapshot.configPath}</SettingsValue>}
        />
      </SettingsCard>
    </>
  )
}

export function SettingsOverview({ console }: { console: ReactNode }): React.ReactElement {
  const [snapshot, setSnapshot] = useState<SettingsOverviewSnapshot>(EMPTY_SNAPSHOT)

  useEffect(() => {
    const controller = new AbortController()
    Promise.allSettled([
      setupApi.settingsState('features', controller.signal),
      setupApi.settingsState('surfaces', controller.signal),
      setupApi.settingsState('core', controller.signal),
    ]).then(([features, surfaces, core]) => {
      if (controller.signal.aborted) return
      setSnapshot({
        codeModeEnabled:
          features.status === 'fulfilled' && features.value.values['code_mode.enabled'] === true,
        gatewayEndpoint:
          surfaces.status === 'fulfilled'
            ? resolveGatewayEndpoint(surfaces.value.values)
            : EMPTY_SNAPSHOT.gatewayEndpoint,
        configPath:
          core.status === 'fulfilled' && core.value.config_path
            ? core.value.config_path
            : EMPTY_SNAPSHOT.configPath,
      })
    })
    return () => controller.abort()
  }, [])

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: 4 }}>
      <SettingsOverviewCards snapshot={snapshot} console={console} />
    </div>
  )
}

