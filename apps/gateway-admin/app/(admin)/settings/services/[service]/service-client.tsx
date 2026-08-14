'use client'

// Client component for one service's config form. Fetches the schema +
// current draft values, renders <ServiceForm>, and on save fires
// setup.draft.set + setup.draft.commit (per-section apply-on-save).

import { useEffect, useRef, useState } from 'react'
import { useSearchParams } from 'next/navigation'
import Link from 'next/link'
import { ArrowLeft, Loader2 } from 'lucide-react'

import { SettingsCard, SettingsRowStrip } from '@/components/settings/SettingsChrome'
import { ServiceForm, type ProbeOutcome } from '@/components/setup/ServiceForm'
import { PluginToggle } from '@/components/setup/PluginToggle'
import { setupApi, type ServiceSchema } from '@/lib/api/setup-client'
import { doctorApi } from '@/lib/api/doctor-client'
import { type FieldView } from '@/lib/setup/schemaBuilder'
import { STORED_SECRET_MARKER, buildServiceFormDefaults, draftEntriesToMap } from '@/lib/setup/draft'
import { isKnownService } from '@/lib/setup/buildServiceSlugs'

interface PageState {
  schema: ServiceSchema
  fields: FieldView[]
  defaults: Record<string, string>
}

export default function ServicePage({
  service,
}: {
  service: string
}): React.ReactElement {
  const searchParams = useSearchParams()
  const [state, setState] = useState<PageState | undefined>()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()
  const [saved, setSaved] = useState(false)
  const [pluginInstalled, setPluginInstalled] = useState(false)
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // Clear any pending "saved" auto-reset on unmount.
  useEffect(() => () => {
    if (savedTimerRef.current) clearTimeout(savedTimerRef.current)
  }, [])

  useEffect(() => {
    if (!isKnownService(service)) {
      setError(`Unknown service: ${service}`)
      setLoading(false)
      return
    }
    const controller = new AbortController()
    Promise.all([
      setupApi.schemaGet([service], controller.signal),
      setupApi.draftGet(controller.signal),
      setupApi.servicesStatus(controller.signal),
    ])
      .then(([schemaResponse, draft, statusResponse]) => {
        if (controller.signal.aborted) return
        const schema = schemaResponse.services[service]
        if (!schema) {
          setError(`Service ${service} not present in schema response`)
          return
        }
        const draftMap = draftEntriesToMap(draft.entries)
        const { fields, defaults } = buildServiceFormDefaults(schema.env, draftMap)
        setState({ schema, fields, defaults })
        setPluginInstalled(
          statusResponse.services.find((status) => status.name === service)?.plugin_installed ?? false,
        )
      })
      .catch((err) => {
        if (controller.signal.aborted) return
        setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [service])

  async function save(values: Record<string, string>): Promise<void> {
    const fieldByName = new Map(state?.fields.map((field) => [field.name, field]) ?? [])
    const entries = Object.entries(values)
      .filter(([key, value]) => {
        const field = fieldByName.get(key)
        if (!field?.secret) return true
        return value !== '' && value !== STORED_SECRET_MARKER && value !== '********'
      })
      .map(([key, value]) => ({ key, value }))
    if (entries.length === 0) return
    await setupApi.draftSet(entries, { force: true })
    await setupApi.draftCommit({ force: true })
    setSaved(true)
    if (savedTimerRef.current) clearTimeout(savedTimerRef.current)
    savedTimerRef.current = setTimeout(() => setSaved(false), 2000)
  }

  async function probe(_values: Record<string, string>, signal: AbortSignal): Promise<ProbeOutcome> {
    try {
      const finding = await doctorApi.serviceProbe(service, undefined, signal)
      return {
        status: finding.severity === 'ok' ? 'ok' : 'fail',
        message: finding.message,
      }
    } catch (err) {
      return {
        status: 'fail',
        message: err instanceof Error ? err.message : 'probe failed',
      }
    }
  }

  // ?focus=KEY pre-scrolls to the named field — used for Doctor → Service
  // deep-linking when the user clicks a failing service.
  const focusKey = searchParams?.get('focus') ?? null

  return (
    <SettingsCard
      title={
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
          <Link
            href="/settings/services/"
            className="text-aurora-text-muted hover:text-aurora-text-primary"
            aria-label="Back to services"
            style={{ display: 'grid' }}
          >
            <ArrowLeft className="h-3.5 w-3.5" />
          </Link>
          {state?.schema.display_name ?? service}
        </span>
      }
      action={
        state ? (
          <PluginToggle
            service={service}
            installed={pluginInstalled}
            onChanged={setPluginInstalled}
          />
        ) : null
      }
      description={
        <>
          {state?.schema.description ?? `Configure ${service} env vars`}
          {focusKey ? (
            <span style={{ display: 'block', marginTop: 2, color: 'var(--aurora-warn)' }}>
              Focused field: {focusKey}
            </span>
          ) : null}
        </>
      }
    >
      {loading ? (
        <SettingsRowStrip>
          <span className="flex items-center gap-2 text-[11.5px] text-aurora-text-muted">
            <Loader2 className="h-4 w-4 animate-spin" /> loading
          </span>
        </SettingsRowStrip>
      ) : null}
      {error ? (
        <SettingsRowStrip>
          <span className="text-[11.5px] text-destructive">{error}</span>
        </SettingsRowStrip>
      ) : null}
      {state ? (
        <SettingsRowStrip style={{ display: 'block' }}>
          <ServiceForm
            fields={state.fields}
            defaultValues={state.defaults}
            onSave={save}
            onProbe={probe}
            submitLabel="Save changes"
          />
          {saved ? (
            <p style={{ marginTop: 10, fontSize: 11, color: 'var(--aurora-success)' }}>
              ✓ Saved to ~/.labby/.env
            </p>
          ) : null}
        </SettingsRowStrip>
      ) : null}
    </SettingsCard>
  )
}
