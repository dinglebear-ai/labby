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
  const descriptionId = `${id}-description`
  const secretConfigured = field.secret && typeof value === 'object' && value !== null && (value as { configured?: unknown }).configured === true
  const inputValue = field.secret ? (typeof value === 'string' ? value : '') : valueAsInputString(value)
  const source = state.sources[field.key]
  const envOverride = source?.overridden_by_env
  const isEnvShadowedConfig = field.backend === 'config_toml' && Boolean(envOverride)
  // The server reports an env-backed field as overridden when the process
  // environment (service manager, container) supplies it: `.env` edits would
  // never take effect, and the server refuses them.
  const isProcessEnvOverride = field.backend === 'env' && Boolean(envOverride)
  const disabled = field.write_policy !== 'editable' || isEnvShadowedConfig || isProcessEnvOverride
  const sourceLabel = source?.source === 'env' ? 'service environment'
    : source?.source === 'config_toml' ? 'saved gateway configuration'
    : 'built-in default'
  const applyLabel = field.apply_mode === 'restart' ? 'Restart required'
    : field.apply_mode === 'partial' ? 'Some effects require restart'
    : field.apply_mode === 'immediate' ? 'Applies immediately'
    : 'View only'
  const describedBy = error ? `${descriptionId} ${errorId}` : descriptionId
  const logFilter = ['LABBY_LOG', 'log.filter'].includes(field.key)
  const logLevels = ['off', 'error', 'warn', 'info', 'debug', 'trace']
  const controlProps = {
    id,
    name: field.key,
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
  const stacked = field.section === 'agents' || field.control === 'url' || field.secret ||
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
            <SelectTrigger {...controlProps} style={{ ...SETTINGS_CONTROL_STYLE, minWidth: stacked ? 0 : 150, width: stacked ? '100%' : undefined }}>
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
            className="max-h-64 overflow-auto aurora-scrollbar"
            style={{ ...SETTINGS_MULTILINE_CONTROL_STYLE, fontSize: 11, margin: 0 }}
          >
            {JSON.stringify(value ?? null, null, 2)}
          </pre>
        )
      default:
        return (
          <>
          <Input
            {...controlProps}
            type={field.secret ? 'password' : field.control === 'number' ? 'number' : field.control === 'url' ? 'url' : 'text'}
            min={field.control === 'number' ? field.min ?? undefined : undefined}
            max={field.control === 'number' ? field.max ?? undefined : undefined}
            step={field.control === 'number' ? 1 : undefined}
            list={logFilter ? `${id}-levels` : undefined}
            required={field.required}
            value={inputValue}
            placeholder={field.secret && secretConfigured ? 'Configured ••••••••' : field.example ?? undefined}
            className={stacked ? 'w-full' : undefined}
            style={
              stacked
                ? { ...SETTINGS_CONTROL_STYLE, width: '100%' }
                : { ...SETTINGS_CONTROL_STYLE, width: 150 }
            }
            onChange={(event) => onChange(field.key, parseFieldInput(field, event.target.value))}
          />
          {logFilter ? <datalist id={`${id}-levels`}>{logLevels.map((level) => <option key={level} value={level} />)}</datalist> : null}
          </>
        )
    }
  }

  const meta = (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 4, alignItems: 'center' }}>
      <SettingsMetaPill>{applyLabel}</SettingsMetaPill>
      <SettingsMetaPill>Current value: {sourceLabel}</SettingsMetaPill>
      {field.secret && secretConfigured ? <SettingsMetaPill>configured</SettingsMetaPill> : null}
      {field.write_policy !== 'editable' ? (
        <SettingsMetaPill tone="warn">{field.write_policy === 'dangerous_flow_required' ? 'Requires a dedicated security flow' : 'View only'}</SettingsMetaPill>
      ) : null}
      <details className="text-[10.5px] text-aurora-text-muted">
        <summary className="cursor-pointer">Technical key</summary>
        <code>{field.key}</code>
        {field.env_override ? <span> · Environment override: {field.env_override}</span> : null}
      </details>
    </div>
  )

  const description = (
    <span id={descriptionId}>
      {field.description}
      {logFilter ? <span style={{ display: 'block', marginTop: 4 }}>Levels: {logLevels.join(', ')}. Set one level for everything, or use component=level entries separated by commas.</span> : null}
      {field.control === 'number' && (field.min !== null || field.max !== null) ? (
        <span style={{ display: 'block', marginTop: 4 }}>
          Valid range: {field.min ?? 'any'} to {field.max ?? 'any'}.
        </span>
      ) : null}
      {field.control === 'enum' ? (
        <span style={{ display: 'block', marginTop: 4 }}>
          Choose from: {field.options.map((option) => option.label).join(', ')}.
        </span>
      ) : null}
      {hasEnvOverrideWarning(field, state) ? (
        <span style={{ display: 'block', marginTop: 4, color: 'var(--aurora-warn)' }}>
          {isProcessEnvOverride
            ? `${envOverride} is set in the server's process environment, which takes precedence over .env. Change it where the server is started; edits here would have no effect.`
            : `${envOverride} currently overrides this config.toml value. Edit the env var or remove the override first.`}
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
    </span>
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
