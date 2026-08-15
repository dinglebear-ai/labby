'use client'

import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { hasEnvOverrideWarning, parseFieldInput, valueAsInputString } from '@/lib/settings/schema'
import {
  SettingsMetaPill,
  SettingsRow,
  SettingsToggle,
  SettingsValue,
  SETTINGS_CONTROL_STYLE,
  SETTINGS_MULTILINE_CONTROL_STYLE,
} from './SettingsChrome'

/**
 * One field as a mock settings row: label + description on the left, control
 * parked on the right. Wide controls (free text, list editors, JSON dumps)
 * fall back to the stacked variant, which keeps the same hairline and padding
 * but drops the control onto its own line.
 */
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

  // Read-only primitives render as the mock's muted `code` value; structured
  // values still need the JSON dump.
  const isPrimitive =
    value === null ||
    value === undefined ||
    typeof value === 'string' ||
    typeof value === 'number' ||
    typeof value === 'boolean'
  const stacked =
    field.control === 'text' ||
    field.control === 'string_list' ||
    (field.control === 'read_only' && !isPrimitive)

  function renderControl(): React.ReactNode {
    switch (field.control) {
      case 'bool':
        return (
          <SettingsToggle
            id={id}
            label={field.label}
            checked={Boolean(value)}
            disabled={disabled}
            invalid={Boolean(error)}
            describedBy={describedBy}
            onChange={(checked) => onChange(field.key, checked)}
          />
        )
      case 'enum':
        return (
          <Select value={inputValue} disabled={disabled} onValueChange={(next) => onChange(field.key, next)}>
            <SelectTrigger {...controlProps} style={{ ...SETTINGS_CONTROL_STYLE, minWidth: 150 }}>
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
        )
      case 'string_list':
        return (
          <Textarea
            {...controlProps}
            value={inputValue}
            className="w-full font-mono"
            style={SETTINGS_MULTILINE_CONTROL_STYLE}
            onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))}
          />
        )
      case 'read_only':
        return isPrimitive ? (
          <SettingsValue>{inputValue === '' ? '—' : inputValue}</SettingsValue>
        ) : (
          <pre
            className="max-h-64 overflow-auto"
            style={{ ...SETTINGS_MULTILINE_CONTROL_STYLE, fontSize: 11, margin: 0 }}
          >
            {JSON.stringify(value ?? null, null, 2)}
          </pre>
        )
      default:
        return (
          <Input
            {...controlProps}
            type={field.control === 'number' ? 'number' : 'text'}
            value={inputValue}
            className={stacked ? 'w-full' : undefined}
            style={
              stacked
                ? { ...SETTINGS_CONTROL_STYLE, width: '100%' }
                : { ...SETTINGS_CONTROL_STYLE, width: 150 }
            }
            onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))}
          />
        )
    }
  }

  const meta = (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4, alignItems: 'center' }}>
      <code style={{ fontSize: 10.5, color: 'var(--aurora-text-muted)' }}>{field.key}</code>
      <SettingsMetaPill>{backendLabel}</SettingsMetaPill>
      <SettingsMetaPill>source: {sourceLabel}</SettingsMetaPill>
      <SettingsMetaPill>risk: {field.risk}</SettingsMetaPill>
      <SettingsMetaPill>{field.apply_mode}</SettingsMetaPill>
      {field.write_policy !== 'editable' ? (
        <SettingsMetaPill tone="warn">{field.write_policy}</SettingsMetaPill>
      ) : null}
      {field.env_override ? <SettingsMetaPill>env: {field.env_override}</SettingsMetaPill> : null}
    </div>
  )

  const description = (
    <>
      {field.description}
      {hasEnvOverrideWarning(field, state) ? (
        <span style={{ display: 'block', marginTop: 4, color: 'var(--aurora-warn)' }}>
          {envOverride} currently overrides this config.toml value. Edit the env var or remove the
          override first.
        </span>
      ) : null}
      {error ? (
        <span
          id={errorId}
          style={{ display: 'block', marginTop: 4, color: 'var(--aurora-error)' }}
        >
          {error}
        </span>
      ) : null}
    </>
  )

  return (
    <SettingsRow
      layout={stacked ? 'stacked' : 'inline'}
      htmlFor={field.control === 'bool' ? undefined : id}
      label={field.label}
      description={description}
      meta={meta}
      control={renderControl()}
    />
  )
}
