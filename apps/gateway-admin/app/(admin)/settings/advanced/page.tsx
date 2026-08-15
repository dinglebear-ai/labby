'use client'

import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'

import { AdvancedReadOnlyBlock } from '@/components/settings/AdvancedReadOnlyBlock'
import { SettingsScalarSection } from '@/components/settings/SettingsScalarSection'
import { SettingsCard, SETTINGS_CONTROL_STYLE } from '@/components/settings/SettingsChrome'
import { Input } from '@/components/ui/input'
import { setupApi, type EnvSettingSpec, type SettingsSchemaResponse, type SettingsState } from '@/lib/api/setup-client'
import { fieldsForSection } from '@/lib/settings/schema'

export default function AdvancedPage(): React.ReactElement {
  const [schema, setSchema] = useState<SettingsSchemaResponse | undefined>()
  const [settings, setSettings] = useState<SettingsState | undefined>()
  const [envSchema, setEnvSchema] = useState<EnvSettingSpec[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | undefined>()

  useEffect(() => {
    const controller = new AbortController()
    Promise.all([
      setupApi.settingsSchema(controller.signal),
      setupApi.settingsState('advanced', controller.signal),
      setupApi.settingsEnvSchema(controller.signal),
    ])
      .then(([schemaResponse, stateResponse, envResponse]) => {
        if (controller.signal.aborted) return
        setSchema(schemaResponse)
        setSettings(stateResponse)
        setEnvSchema(envResponse)
      })
      .catch((err) => {
        if (!controller.signal.aborted) setError(err instanceof Error ? err.message : 'load failed')
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false)
      })
    return () => controller.abort()
  }, [])

  const fields = schema ? fieldsForSection(schema.fields, 'advanced') : []
  const readonlyFields = fields.filter((field) => field.write_policy !== 'editable')
  const scalarFields = fields.filter((field) => field.write_policy === 'editable')

  return (
    <>
      <h2 className="sr-only">Advanced settings</h2>
      {loading ? (
        <div className="flex items-center gap-2 text-[11.5px] text-aurora-text-muted">
          <Loader2 className="h-4 w-4 animate-spin" /> loading advanced settings
        </div>
      ) : null}
      {error ? <p className="text-[11.5px] text-destructive">{error}</p> : null}
      {settings ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <SettingsScalarSection
            title="Advanced Scalars"
            description="Low-risk advanced scalar limits and paths."
            section="advanced"
            state={settings}
            fields={scalarFields}
            onSaved={setSettings}
          />
          <AdvancedReadOnlyBlock state={settings} fields={readonlyFields} />
          <EnvInventoryTable entries={envSchema} />
        </div>
      ) : null}
    </>
  )
}

function EnvInventoryTable({ entries }: { entries: EnvSettingSpec[] }): React.ReactElement {
  const [query, setQuery] = useState('')
  const filtered = entries.filter((entry) =>
    `${entry.key} ${entry.service} ${entry.description}`.toLowerCase().includes(query.toLowerCase()),
  )
  return (
    <SettingsCard
      title="Environment Inventory"
      description="Known env keys from generated docs and service metadata. Only low-risk core env keys are editable in this epic."
      action={
        <Input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Filter env keys"
          aria-label="Filter env keys"
          style={{ ...SETTINGS_CONTROL_STYLE, width: 200 }}
        />
      }
    >
      <ul className="max-h-[520px] overflow-auto">
        {filtered.map((entry) => (
          <li
            key={entry.key}
            className="grid gap-1 md:grid-cols-[220px_1fr_auto]"
            style={{
              padding: '9px 16px',
              borderTop:
                '1px solid color-mix(in srgb, var(--aurora-border-default) 35%, var(--aurora-page-bg))',
            }}
          >
            <code style={{ fontSize: 11, color: 'var(--aurora-text-primary)' }}>{entry.key}</code>
            <p style={{ margin: 0, fontSize: 11.5, lineHeight: 1.5, color: 'var(--aurora-text-muted)' }}>
              {entry.description}
            </p>
            <p style={{ margin: 0, fontSize: 11, color: 'var(--aurora-text-muted)' }}>
              {entry.service}
              {entry.secret ? ' secret' : ''}
              {entry.editable ? ' editable' : ''}
            </p>
          </li>
        ))}
      </ul>
    </SettingsCard>
  )
}
