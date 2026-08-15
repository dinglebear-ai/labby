'use client'

// Service catalog overview — lists every service from setup.schema.get
// with a click-through to its dedicated /settings/services/[slug]/ page.
// The "Configured" indicator reflects whether all required env vars are
// present in the live ~/.labby/.env (via setup.state's missing array).

import { useEffect, useMemo, useState } from 'react'
import Link from 'next/link'
import { ChevronRight, Loader2, CircleAlert, CircleCheck } from 'lucide-react'

import {
  SettingsCard,
  SettingsRow,
  SettingsRowStrip,
} from '@/components/settings/SettingsChrome'
import { PluginToggle } from '@/components/setup/PluginToggle'
import { setupApi, type ServiceSchema, type SetupSnapshot, type ServiceStatus, type SettingsState } from '@/lib/api/setup-client'

interface ServiceRow {
  schema: ServiceSchema
  configured: boolean
  pluginInstalled: boolean
}

export default function ServicesIndex(): React.ReactElement {
  const [services, setServices] = useState<ServiceSchema[]>([])
  const [snapshot, setSnapshot] = useState<SetupSnapshot | undefined>()
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [statuses, setStatuses] = useState<ServiceStatus[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.schemaGet(undefined, controller.signal),
      setupApi.state(controller.signal),
      setupApi.servicesStatus(controller.signal),
      setupApi.settingsState('features', controller.signal),
    ])
      .then(([schemaResponse, snap, statusResponse, settingsResponse]) => {
        if (controller.signal.aborted) return
        setServices(
          Object.values(schemaResponse.services).sort((a, b) =>
            a.display_name.localeCompare(b.display_name),
          ),
        )
        setSnapshot(snap)
        setStatuses(statusResponse.services)
        setSettings(settingsResponse)
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const rows = useMemo<ServiceRow[]>(() => {
    const missing = new Set<string>(
      snapshot?.state.kind === 'partially_configured'
        ? snapshot.state.missing ?? []
        : snapshot?.state.kind === 'config_missing'
          ? snapshot.state.envars ?? []
          : [],
    )
    const statusByName = new Map(statuses.map((status) => [status.name, status]))
    return services.map((schema) => ({
      schema,
      configured: statusByName.get(schema.name)?.configured ?? schema.env
        .filter((e) => e.required)
        .every((e) => !missing.has(e.name)),
      pluginInstalled: statusByName.get(schema.name)?.plugin_installed ?? false,
    }))
  }, [services, snapshot, statuses])
  const builtInsEnabled = settings?.values['services.built_in_upstream_apis_enabled']

  return (
    <>
      <h2 className="sr-only">Service settings</h2>
      <SettingsCard
        title="Services"
        description={
          <>
            Configure connection details for every Bootstrap service. Click a
            row to edit its env vars; saves commit immediately to{' '}
            <code>~/.labby/.env</code>.
          </>
        }
      >
        {settings ? (
          <SettingsRow
            layout="stacked"
            label="Built-in upstream API services"
            description={
              <>
                {builtInsEnabled === true
                  ? 'Enabled from Features settings.'
                  : 'Disabled from Features settings; saved credentials are preserved.'}
                <span style={{ display: 'block', marginTop: 2 }}>
                  Service credentials remain managed on individual service pages; the full env
                  inventory is available in Advanced.
                </span>
              </>
            }
          />
        ) : null}
        {loading ? (
          <SettingsRowStrip>
            <span className="flex items-center gap-2 text-[11.5px] text-aurora-text-muted">
              <Loader2 className="h-4 w-4 animate-spin" /> loading catalog
            </span>
          </SettingsRowStrip>
        ) : null}
        {error ? (
          <SettingsRowStrip>
            <span className="text-[11.5px] text-destructive">{error}</span>
          </SettingsRowStrip>
        ) : null}
        {!loading && !error
          ? rows.map(({ schema, configured, pluginInstalled }) => (
              <SettingsRow
                key={schema.name}
                label={
                  <Link
                    href={`/settings/services/${schema.name}/`}
                    style={{ color: 'inherit', textDecoration: 'none' }}
                  >
                    {schema.display_name}
                  </Link>
                }
                description={schema.description ?? undefined}
                control={
                  <div
                    className="flex items-center gap-2"
                    style={{ fontSize: 11, color: 'var(--aurora-text-muted)' }}
                  >
                    <PluginToggle service={schema.name} installed={pluginInstalled} disabled={!configured} />
                    {configured ? (
                      <span
                        className="inline-flex items-center gap-1"
                        style={{ color: 'var(--aurora-success)' }}
                      >
                        <CircleCheck className="h-3 w-3" /> configured
                      </span>
                    ) : (
                      <span
                        className="inline-flex items-center gap-1"
                        style={{ color: 'var(--aurora-warn)' }}
                      >
                        <CircleAlert className="h-3 w-3" /> incomplete
                      </span>
                    )}
                    <Link
                      href={`/settings/services/${schema.name}/`}
                      aria-label={`Open ${schema.display_name} settings`}
                      style={{ display: 'grid', color: 'var(--aurora-text-muted)' }}
                    >
                      <ChevronRight className="h-4 w-4" />
                    </Link>
                  </div>
                }
              />
            ))
          : null}
      </SettingsCard>
    </>
  )
}
