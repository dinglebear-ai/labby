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
  /** Undefined until the Features section has been read successfully. */
  codeModeEnabled?: boolean
  /** The explicitly configured public MCP endpoint; undefined when none is configured. */
  gatewayEndpoint?: string
  /** Undefined until the Core section has been read successfully. */
  configPath?: string
  /** Sections that could not be read, each with the reason the server reported. */
  errors: string[]
}

const EMPTY_SNAPSHOT: SettingsOverviewSnapshot = { errors: [] }

function firstNonEmptyString(...values: unknown[]): string | undefined {
  return values.find((value): value is string => typeof value === 'string' && value.trim().length > 0)
}

/**
 * Resolves the public MCP endpoint from the explicitly configured MCP gateway
 * URL only. The app origin is a separate public entrypoint and is never used
 * to guess where MCP clients should connect.
 */
export function resolveGatewayEndpoint(values: Record<string, unknown>): string | undefined {
  const raw = firstNonEmptyString(values.LABBY_MCP_GATEWAY_URL, values['public_urls.mcp_gateway'])
  if (!raw) return undefined
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

function describeFailure(section: string, result: PromiseSettledResult<unknown>): string | undefined {
  if (result.status === 'fulfilled') return undefined
  const reason: unknown = result.reason
  return `${section}: ${reason instanceof Error ? reason.message : String(reason)}`
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
      {snapshot.errors.length ? (
        <p role="alert" className="text-[11.5px] text-destructive">
          Some settings could not be read. {snapshot.errors.join(' · ')}
        </p>
      ) : null}
      <SettingsCard title="Gateway">
        <SettingsRow
          label="Code Mode"
          description="Expose the catalog as a single code-execution tool instead of individual tool schemas."
          control={
            snapshot.codeModeEnabled === undefined
              ? <SettingsValue>Not reported</SettingsValue>
              : <SettingsToggle checked={snapshot.codeModeEnabled} readOnly label="Code Mode" />
          }
        />
        <SettingsRow
          label="Gateway Endpoint"
          description="Public MCP endpoint downstream clients connect to."
          control={<SettingsValue>{snapshot.gatewayEndpoint ?? 'Not configured'}</SettingsValue>}
        />
      </SettingsCard>

      {console}

      <SettingsCard title="Diagnostics">
        <SettingsRow
          label="Config Path"
          description="Gateway configuration file on disk."
          control={<SettingsValue>{snapshot.configPath ?? 'Not reported'}</SettingsValue>}
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
      const codeMode = features.status === 'fulfilled' ? features.value.values['code_mode.enabled'] : undefined
      setSnapshot({
        codeModeEnabled: typeof codeMode === 'boolean' ? codeMode : undefined,
        gatewayEndpoint: surfaces.status === 'fulfilled' ? resolveGatewayEndpoint(surfaces.value.values) : undefined,
        configPath: core.status === 'fulfilled' ? core.value.config_path || undefined : undefined,
        errors: [
          describeFailure('Features', features),
          describeFailure('Surfaces', surfaces),
          describeFailure('Core', core),
        ].filter((entry): entry is string => Boolean(entry)),
      })
    })
    return () => controller.abort()
  }, [])

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <SettingsOverviewCards snapshot={snapshot} console={console} />
    </div>
  )
}
