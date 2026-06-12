'use client'

import { useEffect, useMemo, useState } from 'react'
import { Loader2 } from 'lucide-react'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { setupApi } from '@/lib/api/setup-client'
import { buildDirtyEntries, editableFields } from '@/lib/settings/schema'
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
  const [saving, setSaving] = useState(false)
  const [errors, setErrors] = useState<Record<string, string>>({})

  useEffect(() => {
    setValues(initialValues)
    setChangedKeys(new Set())
    setErrors({})
  }, [initialValues])

  async function save(): Promise<void> {
    setSaving(true)
    setErrors({})
    try {
      const editable = editableFields(fields)
      const entries = buildDirtyEntries(editable, changedKeys, values)
      const envEntries = entries.filter((entry) => editable.find((field) => field.key === entry.key)?.backend === 'env')
      const configEntries = entries.filter((entry) => editable.find((field) => field.key === entry.key)?.backend === 'config_toml')
      let next = state
      if (envEntries.length > 0) next = await setupApi.settingsEnvUpdate(section, envEntries, true)
      if (configEntries.length > 0) next = (await setupApi.settingsConfigUpdate(section, configEntries, true)).state
      onSaved(next)
    } catch (err) {
      setErrors({ _form: err instanceof Error ? err.message : 'save failed' })
    } finally {
      setSaving(false)
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
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
            }}
          />
        ))}
        {errors._form ? <p className="text-sm text-destructive">{errors._form}</p> : null}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" disabled={saving || changedKeys.size === 0} onClick={() => { setValues(initialValues); setChangedKeys(new Set()) }}>
            Reset
          </Button>
          <Button type="button" disabled={saving || changedKeys.size === 0} onClick={() => void save()}>
            {saving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            Save changes
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
