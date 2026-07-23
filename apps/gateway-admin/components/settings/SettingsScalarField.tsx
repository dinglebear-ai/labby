'use client'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Badge } from '@/components/ui/badge'
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
  const errorId = `${id}-error`
  const inputValue = valueAsInputString(value)
  const source = state.sources[field.key]
  const envOverride = source?.overridden_by_env
  const isEnvShadowedConfig = field.backend === 'config_toml' && Boolean(envOverride)
  const disabled = field.write_policy !== 'editable' || isEnvShadowedConfig
  const sourceLabel = source?.source ?? 'default'
  const backendLabel = field.backend === 'env' ? '.env' : 'config.toml'
  const describedBy = error ? errorId : undefined
  const controlProps = {
    id,
    disabled,
    'aria-invalid': Boolean(error),
    'aria-describedby': describedBy,
  }

  function renderControl(): React.ReactNode {
    switch (field.control) {
      case 'bool':
        return (
          <Switch
            {...controlProps}
            className="h-6 w-11 border-2 border-transparent bg-[#e5e5e5] shadow-none data-[state=checked]:bg-[#ff6600] [&_[data-slot=switch-thumb]]:size-5 [&_[data-slot=switch-thumb]]:bg-white [&_[data-slot=switch-thumb]]:shadow-md [&_[data-slot=switch-thumb]]:data-[state=checked]:translate-x-5"
            checked={Boolean(value)}
            onCheckedChange={(checked) => onChange(field.key, checked)}
          />
        )
      case 'enum':
        return (
          <Select value={inputValue} disabled={disabled} onValueChange={(next) => onChange(field.key, next)}>
            <SelectTrigger
              {...controlProps}
              className="h-10 w-full border-[#d4d4d4] bg-white text-[#1c1b1b] shadow-none hover:bg-[#fafafa]"
            >
              <SelectValue placeholder={field.example ?? 'Select'} />
            </SelectTrigger>
            <SelectContent className="border-[#d4d4d4] bg-white text-[#1c1b1b]">
              {field.options.map((option) => (
                <SelectItem className="focus:bg-[#fff7ed] focus:text-[#1c1b1b]" key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )
      case 'string_list':
        return <Textarea {...controlProps} value={inputValue} className="min-h-24 border-[#d4d4d4] bg-white font-mono text-xs text-[#1c1b1b] shadow-none" onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
      case 'read_only':
        return <pre className="max-h-64 overflow-auto rounded-md bg-[#f5f5f5] p-3 text-xs text-[#1c1b1b]">{JSON.stringify(value ?? null, null, 2)}</pre>
      default:
        return <Input {...controlProps} className="h-10 border-[#d4d4d4] bg-white text-[#1c1b1b] shadow-none placeholder:text-[#a3a3a3]" type={field.control === 'number' ? 'number' : 'text'} value={inputValue} onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))} />
    }
  }

  return (
    <div
      data-unraid-settings-row="true"
      className="grid grid-cols-1 items-start gap-3 border-b border-[#f0f0f0] px-5 py-4 last:border-b-0 md:grid-cols-[35%_minmax(0,1fr)] md:gap-x-6"
    >
      <div className="min-w-0 pt-1 md:text-right">
        <Label htmlFor={id} className="text-[13px] font-semibold text-[#1c1b1b]">
          {field.label}
        </Label>
        <p className="mt-1 text-[11px] leading-4 text-[#737373]">{field.description}</p>
        <p className="mt-1 truncate font-mono text-[10px] text-[#a3a3a3]">{field.key}</p>
      </div>
      <div className="min-w-0 space-y-2">
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge className="border-0 bg-[#e5e7eb] text-[#1f2937]" variant="secondary">{backendLabel}</Badge>
          <Badge className="border-[#e5e5e5] bg-white text-[#737373]" variant="outline">source: {sourceLabel}</Badge>
          <Badge className="border-[#e5e5e5] bg-white text-[#737373]" variant="outline">risk: {field.risk}</Badge>
          <span className="rounded bg-[#f5f5f5] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-[0.08em] text-[#737373]">
            {field.apply_mode}
          </span>
          {field.write_policy !== 'editable' ? <Badge className="border-[#e9bf41] bg-[#fff7d6] text-[#8a6914]" variant="outline">{field.write_policy}</Badge> : null}
          {field.env_override ? <Badge className="border-[#e5e5e5] bg-white text-[#737373]" variant="outline">env: {field.env_override}</Badge> : null}
        </div>
        {hasEnvOverrideWarning(field, state) ? (
          <p className="text-xs text-[#8a6914]">{envOverride} currently overrides this config.toml value. Edit the env var or remove the override first.</p>
        ) : null}
        {renderControl()}
        {error ? <p id={errorId} className="text-xs text-[#bd1818]">{error}</p> : null}
      </div>
    </div>
  )
}
