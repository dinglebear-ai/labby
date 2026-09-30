'use client'

import { useEffect, useMemo, useRef, useState } from 'react'
import { Loader2 } from 'lucide-react'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { setupApi, SetupApiError } from '@/lib/api/setup-client'
import { buildDirtyEntriesByBackend, collectFieldInputErrors } from '@/lib/settings/schema'
import { SettingsCard, SettingsRowStrip } from './SettingsChrome'
import { SettingsScalarField } from './SettingsScalarField'

export function SettingsScalarSection({
  title,
  description,
  section,
  state,
  fields,
  onSaved,
}: {
  title: string
  description: string
  section: string
  state: SettingsState
  fields: SettingsFieldSpec[]
  onSaved: (state: SettingsState) => void
}): React.ReactElement {
  const initialValues = useMemo(
    () => Object.fromEntries(fields.map((field) => [field.key, state.values[field.key] ?? null])),
    [fields, state.values],
  )
  const [values, setValues] = useState<Record<string, unknown>>(initialValues)
  const [changedKeys, setChangedKeys] = useState<Set<string>>(new Set())
  const [confirmed, setConfirmed] = useState(false)
  const [saving, setSaving] = useState(false)
  const [errors, setErrors] = useState<Record<string, string>>({})
  const savingRef = useRef(false)

  useEffect(() => {
    setValues(initialValues)
    setChangedKeys(new Set())
    setConfirmed(false)
    setErrors({})
  }, [initialValues])

  async function save(): Promise<void> {
    if (savingRef.current) return
    savingRef.current = true
    setSaving(true)
    setErrors({})
    try {
      const inputErrors = collectFieldInputErrors(fields, changedKeys, values)
      if (Object.keys(inputErrors).length > 0) {
        setErrors(inputErrors)
        return
      }
      const { envEntries, configEntries } = buildDirtyEntriesByBackend(fields, changedKeys, values, initialValues, state.sources)
      if (envEntries.length > 0 && configEntries.length > 0) {
        setErrors({ _form: 'Save .env and config.toml settings separately.' })
        return
      }
      if (!confirmed) {
        setErrors({ _form: 'Confirm the settings write before saving.' })
        return
      }
      let next = state
      if (envEntries.length > 0) next = await setupApi.settingsEnvUpdate(section, envEntries, confirmed)
      if (configEntries.length > 0) next = (await setupApi.settingsConfigUpdate(section, configEntries, confirmed)).state
      onSaved(next)
    } catch (err) {
      const message = err instanceof Error ? err.message : 'save failed'
      const param = err instanceof SetupApiError ? err.param : undefined
      if (param && fields.some((field) => field.key === param)) {
        setErrors({ [param]: message })
      } else {
        setErrors({ _form: message })
      }
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }

  return (
    <SettingsCard title={title} description={description}>
      {fields.map((field) => (
        <SettingsScalarField
          key={field.key}
          field={field}
          value={values[field.key]}
          state={state}
          error={errors[field.key]}
          onChange={(key, value) => {
            setValues((prev) => ({ ...prev, [key]: value }))
            setChangedKeys((prev) => new Set(prev).add(key))
            setConfirmed(false)
          }}
        />
      ))}
      <SettingsRowStrip style={{ flexWrap: 'wrap', justifyContent: 'flex-end', gap: 10 }}>
        {errors._form ? (
          <p
            style={{
              flex: '1 1 0%',
              minWidth: 0,
              margin: 0,
              fontSize: 11.5,
              color: 'var(--aurora-error)',
            }}
          >
            {errors._form}
          </p>
        ) : null}
        {changedKeys.size > 0 ? (
          <label
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              fontSize: 11.5,
              color: 'var(--aurora-text-muted)',
            }}
          >
            <Checkbox checked={confirmed} onCheckedChange={(checked) => setConfirmed(checked === true)} />
            Confirm settings write
          </label>
        ) : null}
        <Button type="button" size="sm" variant="outline" disabled={saving || changedKeys.size === 0} onClick={() => { setValues(initialValues); setChangedKeys(new Set()); setConfirmed(false) }}>
          Reset
        </Button>
        <Button type="button" size="sm" disabled={saving || changedKeys.size === 0 || !confirmed} onClick={() => void save()}>
          {saving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
          Save changes
        </Button>
      </SettingsRowStrip>
    </SettingsCard>
  )
}
