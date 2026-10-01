import type { SettingsFieldSpec, SettingsState, SettingsUpdateEntry } from '@/lib/api/setup-client'

const INVALID_FIELD_INPUT = '__lab_invalid_settings_input__'

export interface InvalidFieldInput {
  readonly kind: typeof INVALID_FIELD_INPUT
  readonly raw: string
  readonly message: string
}

function invalidFieldInput(raw: string, message: string): InvalidFieldInput {
  return { kind: INVALID_FIELD_INPUT, raw, message }
}

export function isInvalidFieldInput(value: unknown): value is InvalidFieldInput {
  return typeof value === 'object'
    && value !== null
    && (value as { kind?: unknown }).kind === INVALID_FIELD_INPUT
}

export function fieldsForSection(schemaFields: SettingsFieldSpec[], section: string): SettingsFieldSpec[] {
  return schemaFields
    .filter((field) => field.section === section)
    .sort((a, b) => a.label.localeCompare(b.label))
}

export function editableFields(fields: SettingsFieldSpec[]): SettingsFieldSpec[] {
  return fields.filter((field) => field.write_policy === 'editable' && field.control !== 'read_only')
}

export function valueAsInputString(value: unknown): string {
  if (isInvalidFieldInput(value)) return value.raw
  if (value === null || value === undefined) return ''
  if (Array.isArray(value)) return value.join('\n')
  return String(value)
}

export function parseFieldInput(field: SettingsFieldSpec, raw: string | boolean): unknown {
  if (field.control === 'bool') return Boolean(raw)
  const text = String(raw)
  if (field.control === 'number') {
    if (text.trim() === '') return field.required || field.backend === 'env'
      ? invalidFieldInput(text, 'Enter a whole number.')
      : null
    const parsed = Number(text)
    if (!Number.isFinite(parsed) || !Number.isInteger(parsed)) return invalidFieldInput(text, 'Must be an integer.')
    if (field.min !== null && parsed < field.min) return invalidFieldInput(text, `Must be at least ${field.min}.`)
    if (field.max !== null && parsed > field.max) return invalidFieldInput(text, `Must be at most ${field.max}.`)
    return parsed
  }
  if (field.control === 'string_list') {
    const entries = text
      .split(/\r?\n|,/)
      .map((entry) => entry.trim())
      .filter(Boolean)
    if (field.required && entries.length === 0) return invalidFieldInput(text, 'Enter at least one value.')
    if (field.key === 'api.cors_origins' && entries.some((entry) => !isHttpOrigin(entry))) {
      return invalidFieldInput(text, 'Enter full HTTP or HTTPS origins without a path, credentials, query, or fragment.')
    }
    if (field.key === 'mcp.allowed_hosts' && entries.some((entry) => !isAllowedHost(entry))) {
      return invalidFieldInput(text, 'Enter host names or IP addresses, optionally with a port. Wildcards and URLs are not allowed.')
    }
    return entries
  }
  if (field.control === 'enum' && !field.options.some((option) => option.value === text)) {
    return invalidFieldInput(text, 'Select one of the available values.')
  }
  if (field.control === 'url' && text.trim()) {
    try {
      const parsed = new URL(text)
      if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password || parsed.search || parsed.hash) {
        return invalidFieldInput(text, 'Enter an HTTP or HTTPS URL without credentials, query, or fragment.')
      }
    } catch {
      return invalidFieldInput(text, 'Enter a valid HTTP or HTTPS URL.')
    }
  }
  if (['LABBY_MCP_HTTP_HOST', 'mcp.host'].includes(field.key)
    && (text.trim() || field.key === 'LABBY_MCP_HTTP_HOST')
    && !isBindHost(text.trim())) {
    return invalidFieldInput(text, 'Enter an IP address or DNS host name without a port or URL scheme.')
  }
  if (field.required && !text.trim()) return invalidFieldInput(text, 'This value is required.')
  return text
}

function isHttpOrigin(value: string): boolean {
  try {
    const parsed = new URL(value)
    return ['http:', 'https:'].includes(parsed.protocol) && Boolean(parsed.hostname)
      && !parsed.username && !parsed.password && parsed.pathname === '/'
      && !parsed.search && !parsed.hash
  } catch {
    return false
  }
}

function isAllowedHost(value: string): boolean {
  if (value === '*' || /[\s/@?#\\]/.test(value)) return false
  try {
    const parsed = new URL(`http://${value}/`)
    return Boolean(parsed.hostname) && !parsed.username && !parsed.password && parsed.pathname === '/'
  } catch {
    try {
      const parsed = new URL(`http://[${value}]/`)
      return Boolean(parsed.hostname) && parsed.pathname === '/'
    } catch {
      return false
    }
  }
}

function isBindHost(value: string): boolean {
  if (!value || /[\s/@?#\\]/.test(value)) return false
  // URL normalizes an explicit :80 to an empty port; inspect the raw host too.
  if (value.includes(':') && !value.startsWith('[') && value.split(':').length === 2) return false
  if (/\]:/.test(value)) return false
  try {
    const parsed = new URL(`http://${value}/`)
    return Boolean(parsed.hostname) && !parsed.port && parsed.pathname === '/'
  } catch {
    try {
      const parsed = new URL(`http://[${value}]/`)
      return Boolean(parsed.hostname) && parsed.pathname === '/'
    } catch {
      return false
    }
  }
}

export function collectFieldInputErrors(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
): Record<string, string> {
  const errors: Record<string, string> = {}
  for (const field of fields) {
    if (!changedKeys.has(field.key)) continue
    const value = values[field.key]
    if (isInvalidFieldInput(value)) errors[field.key] = value.message
  }
  return errors
}

export function buildDirtyEntries(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
  initialValues: Record<string, unknown>,
): SettingsUpdateEntry[] {
  return fields
    .filter((field) => changedKeys.has(field.key))
    .map((field) => {
      const value = values[field.key] ?? null
      if (isInvalidFieldInput(value)) {
        throw new Error(`invalid value for ${field.key}: ${value.message}`)
      }
      const previous = initialValues[field.key] ?? null
      const unset = field.backend === 'config_toml'
        && !field.required
        && (value === null || value === '' || (Array.isArray(value) && value.length === 0))
      return unset ? { key: field.key, value: null, previous, unset: true } : { key: field.key, value, previous }
    })
}

export function buildDirtyEntriesByBackend(
  fields: SettingsFieldSpec[],
  changedKeys: Set<string>,
  values: Record<string, unknown>,
  initialValues: Record<string, unknown>,
  sources: SettingsState['sources'] = {},
): { envEntries: SettingsUpdateEntry[]; configEntries: SettingsUpdateEntry[] } {
  // A config.toml field shadowed by an env var, or an env field the process
  // environment overrides, cannot be changed from here; never submit either.
  const editable = editableFields(fields).filter((field) => !sources[field.key]?.overridden_by_env)
  const backendByKey = new Map(editable.map((field) => [field.key, field.backend]))
  const entries = buildDirtyEntries(editable, changedKeys, values, initialValues)
  return {
    envEntries: entries.filter((entry) => backendByKey.get(entry.key) === 'env'),
    configEntries: entries.filter((entry) => backendByKey.get(entry.key) === 'config_toml'),
  }
}

export function hasEnvOverrideWarning(field: SettingsFieldSpec, state: SettingsState): boolean {
  return Boolean(state.sources[field.key]?.overridden_by_env)
}
