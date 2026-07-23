'use client'

import { useEffect, useMemo, useRef, useState } from 'react'
import { Loader2 } from 'lucide-react'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { setupApi, SetupApiError } from '@/lib/api/setup-client'
import { buildDirtyEntriesByBackend, collectFieldInputErrors } from '@/lib/settings/schema'
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
    <Card
      data-unraid-settings-card="true"
      className="gap-0 overflow-hidden rounded-[6px] border-2 border-[#f5f5f5] bg-white text-[#1c1b1b] shadow-[0_4px_6px_-1px_rgba(0,0,0,0.08)]"
    >
      <CardHeader className="gap-1 border-b border-[#f0f0f0] px-5 py-4">
        <CardTitle className="text-base font-semibold tracking-normal text-[#1c1b1b]">{title}</CardTitle>
        <CardDescription className="text-xs text-[#737373]">{description}</CardDescription>
      </CardHeader>
      <CardContent className="p-0">
        <div>
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
        </div>
        <div className="flex flex-wrap items-center gap-3 border-t border-[#f0f0f0] bg-[#fafafa] px-5 py-3">
          <div className="min-w-0 flex-1">
            {errors._form ? <p className="text-xs text-[#bd1818]">{errors._form}</p> : null}
            {changedKeys.size > 0 ? (
              <label className="flex items-center gap-2 text-xs text-[#737373]">
                <Checkbox
                  className="border-[#d4d4d4] data-[state=checked]:border-[#ff6600] data-[state=checked]:bg-[#ff6600]"
                  checked={confirmed}
                  onCheckedChange={(checked) => setConfirmed(checked === true)}
                />
                Confirm backup-first write of {changedKeys.size} changed {changedKeys.size === 1 ? 'setting' : 'settings'}
              </label>
            ) : (
              <p className="text-[11px] text-[#a3a3a3]">No unsaved changes</p>
            )}
          </div>
          <Button
            type="button"
            variant="outline"
            className="border-[#d4d4d4] bg-white text-[#1c1b1b] hover:bg-[#f5f5f5]"
            disabled={saving || changedKeys.size === 0}
            onClick={() => { setValues(initialValues); setChangedKeys(new Set()); setConfirmed(false) }}
          >
            Reset
          </Button>
          <Button
            type="button"
            className="border-0 bg-[linear-gradient(90deg,#e22828,#ff8c2f)] text-white hover:opacity-90"
            disabled={saving || changedKeys.size === 0 || !confirmed}
            onClick={() => void save()}
          >
            {saving ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
            Save changes
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
