'use client'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { hasEnvOverrideWarning, parseFieldInput, valueAsInputString } from '@/lib/settings/schema'

export function SettingsScalarField({
  field,
  value,
  state,
  error,
  onChange,
}: {
  field: SettingsFieldSpec
  value: unknown
  state: SettingsState
  error?: string
  onChange: (key: string, value: unknown) => void
}): React.ReactElement {
  const id = `settings-${field.key.replaceAll('.', '-')}`
  const disabled = field.write_policy !== 'editable'
  const inputValue = valueAsInputString(value)
  const envOverride = state.sources[field.key]?.overridden_by_env

  return (
    <div className="grid gap-2 rounded-md border p-3">
      <div className="flex items-start justify-between gap-3">
        <div>
          <Label htmlFor={id}>{field.label}</Label>
          <p className="mt-1 text-xs text-muted-foreground">{field.description}</p>
          <p className="mt-1 font-mono text-[11px] text-muted-foreground">{field.key}</p>
        </div>
        <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase text-muted-foreground">
          {field.apply_mode}
        </span>
      </div>
      {hasEnvOverrideWarning(field, state) ? (
        <p className="text-xs text-amber-600">{envOverride} currently overrides this config.toml value.</p>
      ) : null}
      {field.control === 'bool' ? (
        <Switch id={id} checked={Boolean(value)} disabled={disabled} onCheckedChange={(checked) => onChange(field.key, checked)} />
      ) : field.control === 'enum' ? (
        <Select value={inputValue} disabled={disabled} onValueChange={(next) => onChange(field.key, next)}>
          <SelectTrigger id={id}>
            <SelectValue placeholder={field.example ?? 'Select'} />
          </SelectTrigger>
          <SelectContent>
            {field.options.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : field.control === 'string_list' ? (
        <Textarea id={id} value={inputValue} disabled={disabled} className="min-h-24 font-mono text-xs" onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
      ) : field.control === 'read_only' ? (
        <pre className="max-h-64 overflow-auto rounded-md bg-muted p-3 text-xs">{JSON.stringify(value ?? null, null, 2)}</pre>
      ) : (
        <Input id={id} type={field.control === 'number' ? 'number' : 'text'} value={inputValue} disabled={disabled} onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
      )}
      {error ? <p className="text-xs text-destructive">{error}</p> : null}
    </div>
  )
}
