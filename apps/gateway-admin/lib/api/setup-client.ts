// TypeScript wrapper over the Labby setup dispatch service.
//
// Mirrors crates/labby/src/dispatch/setup/catalog.rs. All actions go through
// POST /v1/setup with { action, params } shape (same as MCP).
//
// Each action returns the typed response directly; transport errors throw
// SetupApiError with the stable kind tag from docs/dev/ERRORS.md.

import { setupActionUrl } from './gateway-config.ts'
import { performServiceAction, type ServiceActionError } from './service-action-client.ts'

const USE_MOCK_DATA = process.env.NEXT_PUBLIC_MOCK_DATA === 'true'

export class SetupApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'SetupApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

async function setupAction<T>(
  action: string,
  params: Record<string, unknown> = {},
  signal?: AbortSignal,
): Promise<T> {
  return performServiceAction<T, SetupApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Setup',
    url: setupActionUrl(),
    createError: (message, status, code, param) => new SetupApiError(message, status, code, param),
  })
}

// ─── State machine ──────────────────────────────────────────────────────

export type SetupStateKind =
  | 'uninitialized'
  | 'config_missing'
  | 'partially_configured'
  | 'health_checking'
  | 'ready'

export interface SetupState {
  kind: SetupStateKind
  envars?: string[]
  missing?: string[]
  services?: string[]
}

export interface SetupSnapshot {
  first_run: boolean
  env_path: string
  draft_path: string
  last_completed_step: number
  draft_stale: boolean
  has_draft: boolean
  draft_entry_count: number
  env_mtime_unix_seconds: number | null
  draft_mtime_unix_seconds: number | null
  state: SetupState
}

// ─── UiSchema projection ────────────────────────────────────────────────

export type FieldKindKey =
  | 'text'
  | 'secret'
  | 'url'
  | 'bool'
  | 'number'
  | 'file_path'
  | 'enum'

export interface UiValidation {
  required: boolean
  min_length: number | null
  max_length: number | null
  pattern: string | null
}

export interface UiFieldSchema {
  kind: FieldKindKey
  enum_values: string[] | null
  advanced: boolean
  help_url: string | null
  depends_on: string | null
  validation: UiValidation
}

export interface ServiceEnvVar {
  name: string
  description: string
  example: string
  secret: boolean
  required: boolean
  ui?: UiFieldSchema
}

export interface ServiceSchema {
  name: string
  display_name: string
  description: string
  category: string
  supports_multi_instance: boolean
  default_port: number | null
  built_in_upstream_api?: boolean
  env: ServiceEnvVar[]
}

export interface SchemaGetResponse {
  services: Record<string, ServiceSchema>
}

const BASE_VALIDATION: UiValidation = {
  required: false,
  min_length: null,
  max_length: null,
  pattern: null,
}

function textUi(required = false, kind: FieldKindKey = 'text'): UiFieldSchema {
  return {
    kind,
    enum_values: null,
    advanced: false,
    help_url: null,
    depends_on: null,
    validation: {
      ...BASE_VALIDATION,
      required,
    },
  }
}

const MOCK_SERVICES: Record<string, ServiceSchema> = {
  unifi: {
    name: 'unifi',
    display_name: 'UniFi',
    description: 'Network controller.',
    category: 'Network',
    supports_multi_instance: true,
    default_port: 8443,
    env: [
      {
        name: 'UNIFI_URL',
        description: 'Base URL of the UniFi controller',
        example: 'https://unifi.example.com',
        secret: false,
        required: true,
        ui: textUi(true, 'url'),
      },
      {
        name: 'UNIFI_API_KEY',
        description: 'UniFi API key',
        example: '',
        secret: true,
        required: true,
        ui: textUi(true, 'secret'),
      },
    ],
  },
  apprise: {
    name: 'apprise',
    display_name: 'Apprise',
    description: 'Notification gateway.',
    category: 'Notifications',
    supports_multi_instance: true,
    default_port: 8000,
    env: [
      {
        name: 'APPRISE_URL',
        description: 'Base URL of the Apprise API',
        example: 'https://apprise.example.com',
        secret: false,
        required: true,
        ui: textUi(true, 'url'),
      },
      {
        name: 'APPRISE_TOKEN',
        description: 'Apprise API token',
        example: '',
        secret: true,
        required: true,
        ui: textUi(true, 'secret'),
      },
    ],
  },
}

const MOCK_DRAFT_ENTRIES: DraftEntry[] = [
  { key: "LABBY_MCP_HTTP_HOST", value: "127.0.0.1" },
  { key: "LABBY_MCP_HTTP_PORT", value: "3101" },
  { key: "LABBY_LOG", value: "labby=info" },
  { key: "LABBY_LOG_FORMAT", value: "json" },
  { key: 'UNIFI_URL', value: 'https://unifi.example.com' },
  { key: 'UNIFI_API_KEY', value: '***' },
  { key: 'APPRISE_URL', value: 'https://apprise.example.com' },
]

function mockSetupSnapshot(): SetupSnapshot {
  return {
    first_run: false,
    env_path: '~/.labby/.env',
    draft_path: '~/.labby/.env.draft',
    last_completed_step: 3,
    draft_stale: false,
    has_draft: true,
    draft_entry_count: MOCK_DRAFT_ENTRIES.length,
    env_mtime_unix_seconds: null,
    draft_mtime_unix_seconds: null,
    state: {
      kind: 'partially_configured',
      missing: ['APPRISE_TOKEN'],
      services: Object.keys(MOCK_SERVICES),
    },
  }
}

// ─── Drafts ─────────────────────────────────────────────────────────────

export interface DraftEntry {
  key: string
  value: string
}

export interface DraftSetOutcome {
  written: number
  skipped: string[]
  backup_path: string | null
}

export interface DraftDiscardOutcome {
  removed: boolean
}

export interface CommitOutcome {
  written: number
  skipped: string[]
  backup_path: string | null
  audit_pass_count: number
  audit_total_count: number
  // Present when the gate failed; the caller can render the audit body inline.
  ok?: false
  audit?: unknown
}

export interface InstalledPlugin {
  id: string
  service: string | null
}

export interface PluginLifecycleOutcome {
  service: string
  package_id: string
  status: string
  message: string
}

export interface ServiceStatus {
  name: string
  display_name: string
  description: string
  configured: boolean
  plugin_installed: boolean
  plugin_package_id: string | null
  required_env: string[]
}

export interface ServicesStatusResponse {
  services: ServiceStatus[]
  plugins: InstalledPlugin[]
}

export type SettingsBackend = 'env' | 'config_toml'
export type SettingsControl = 'text' | 'url' | 'bool' | 'number' | 'enum' | 'string_list' | 'read_only'
export type SettingsRisk = 'low' | 'restart' | 'security_sensitive' | 'dangerous'
export type SettingsWritePolicy = 'editable' | 'read_only' | 'dangerous_flow_required' | 'secret_write_only_future'
export type SettingsApplyMode = 'immediate' | 'restart' | 'partial' | 'read_only'
export type SettingsSourceKind = 'env' | 'config_toml' | 'default'

export interface SettingsOption {
  value: string
  label: string
}

export interface SettingsFieldSpec {
  key: string
  label: string
  description: string
  section: string
  backend: SettingsBackend
  control: SettingsControl
  risk: SettingsRisk
  write_policy: SettingsWritePolicy
  apply_mode: SettingsApplyMode
  secret: boolean
  required: boolean
  env_override: string | null
  min: number | null
  max: number | null
  options: SettingsOption[]
  example: string | null
}

export interface SettingsSectionSpec {
  id: string
  label: string
  description: string
  advanced: boolean
}

export interface SettingsSchemaResponse {
  schema_version: number
  sections: SettingsSectionSpec[]
  fields: SettingsFieldSpec[]
}

export interface SettingsValueSource {
  source: SettingsSourceKind
  overridden_by_env: string | null
}

export interface SettingsState {
  schema_version: number
  config_path: string
  env_path: string
  section: string
  values: Record<string, unknown>
  sources: Record<string, SettingsValueSource>
}

export interface SettingsUpdateEntry {
  key: string
  value: unknown
  previous: unknown
  unset?: boolean
}

export interface SettingsMutationOutcome {
  state: SettingsState
  backup_path: string | null
}

export interface EnvSettingSpec {
  service: string
  key: string
  required: boolean
  secret: boolean
  description: string
  example: string
  editable: boolean
}

export interface SettingsUpdate {
  services: {
    built_in_upstream_apis_enabled: boolean
  }
}

export const MOCK_SETTINGS_SCHEMA: SettingsSchemaResponse = {
  schema_version: 1,
  sections: [
    { id: 'core', label: 'Core', description: 'Env-backed process defaults.', advanced: false },
    { id: 'features', label: 'Features', description: 'Runtime feature gates.', advanced: false },
    { id: 'advanced', label: 'Advanced', description: 'Advanced settings.', advanced: true },
  ],
  fields: [
    { key: 'LABBY_LOG', label: 'Log filter', description: 'Tracing filter directive.', section: 'core', backend: 'env', control: 'text', risk: 'restart', write_policy: 'editable', apply_mode: 'restart', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: 'labby=info' },
    { key: 'services.built_in_upstream_apis_enabled', label: 'Built-in upstream API services', description: 'Enable bundled external API integrations.', section: 'features', backend: 'config_toml', control: 'bool', risk: 'low', write_policy: 'editable', apply_mode: 'immediate', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: 'true' },
    { key: 'auth', label: 'Auth config', description: 'Redacted auth settings.', section: 'advanced', backend: 'config_toml', control: 'read_only', risk: 'security_sensitive', write_policy: 'secret_write_only_future', apply_mode: 'read_only', secret: true, required: false, env_override: null, min: null, max: null, options: [], example: null },
  ],
}

export const MOCK_ENV_SCHEMA: EnvSettingSpec[] = [
  { service: 'labby', key: 'LABBY_LOG', required: false, secret: false, description: 'Tracing filter directive.', example: 'labby=info', editable: true },
  { service: 'setup', key: 'LABBY_MCP_HTTP_TOKEN', required: true, secret: true, description: 'Bearer token.', example: '<token>', editable: false },
]

function mockSettingsState(section: string, updates: SettingsUpdateEntry[] = []): SettingsState {
  const values: Record<string, unknown> = {
    LABBY_LOG: 'labby=info,labby_apis=warn',
    'services.built_in_upstream_apis_enabled': true,
    auth: { google_client_secret: { has_value: true } },
  }
  for (const update of updates) values[update.key] = update.value
  return {
    schema_version: 1,
    config_path: '~/.config/labby/config.toml',
    env_path: '~/.labby/.env',
    section,
    values,
    sources: {
      LABBY_LOG: { source: 'env', overridden_by_env: null },
      'services.built_in_upstream_apis_enabled': { source: 'config_toml', overridden_by_env: null },
      auth: { source: 'config_toml', overridden_by_env: null },
    },
  }
}

// ─── Public API ─────────────────────────────────────────────────────────

export const setupApi = {
  state(signal?: AbortSignal): Promise<SetupSnapshot> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(structuredClone(mockSetupSnapshot()))
    }
    return setupAction<SetupSnapshot>('state', {}, signal)
  },

  schemaGet(services?: string[], signal?: AbortSignal): Promise<SchemaGetResponse> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      const selected = services?.length ? services : Object.keys(MOCK_SERVICES)
      return Promise.resolve({
        services: Object.fromEntries(
          selected
            .map((service) => [service, MOCK_SERVICES[service]] as const)
            .filter(([, schema]) => schema !== undefined),
        ) as Record<string, ServiceSchema>,
      })
    }
    return setupAction<SchemaGetResponse>('schema.get', services ? { services } : {}, signal)
  },

  settingsSchema(signal?: AbortSignal): Promise<SettingsSchemaResponse> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(structuredClone(MOCK_SETTINGS_SCHEMA))
    }
    return setupAction<SettingsSchemaResponse>('settings.schema', {}, signal)
  },

  settingsState(section = 'core', signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(mockSettingsState(section))
    }
    return setupAction<SettingsState>('settings.state', { section }, signal)
  },

  settingsConfigUpdate(section: string, entries: SettingsUpdateEntry[], confirm: boolean, signal?: AbortSignal): Promise<SettingsMutationOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ state: mockSettingsState(section, entries), backup_path: '~/.config/labby/config.toml.bak.mock' })
    }
    return setupAction<SettingsMutationOutcome>('settings.config.update', { section, entries, confirm }, signal)
  },

  settingsEnvUpdate(section: string, entries: SettingsUpdateEntry[], confirm: boolean, signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(mockSettingsState(section, entries))
    }
    return setupAction<SettingsState>('settings.env.update', { section, entries, confirm }, signal)
  },

  settingsEnvSchema(signal?: AbortSignal): Promise<EnvSettingSpec[]> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(structuredClone(MOCK_ENV_SCHEMA))
    }
    return setupAction<EnvSettingSpec[]>('settings.env_schema', {}, signal)
  },

  settingsUpdate(patch: SettingsUpdate, signal?: AbortSignal): Promise<SettingsState> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve(mockSettingsState('features', [
        {
          key: 'services.built_in_upstream_apis_enabled',
          value: patch.services.built_in_upstream_apis_enabled,
          previous: null,
        },
      ]))
    }
    return setupAction<SettingsState>('settings.update', { ...patch, confirm: true }, signal)
  },

  draftGet(signal?: AbortSignal): Promise<{ entries: DraftEntry[] }> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ entries: structuredClone(MOCK_DRAFT_ENTRIES) })
    }
    return setupAction<{ entries: DraftEntry[] }>('draft.get', {}, signal)
  },

  draftSet(
    entries: DraftEntry[],
    options?: { force?: boolean },
    signal?: AbortSignal,
  ): Promise<DraftSetOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ written: entries.length, skipped: [], backup_path: null })
    }
    return setupAction<DraftSetOutcome>(
      'draft.set',
      { entries, force: options?.force ?? false },
      signal,
    )
  },

  draftDiscard(signal?: AbortSignal): Promise<DraftDiscardOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({ removed: true })
    }
    return setupAction<DraftDiscardOutcome>('draft.discard', {}, signal)
  },

  draftCommit(
    options?: { force?: boolean },
    signal?: AbortSignal,
  ): Promise<CommitOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        written: 4,
        skipped: [],
        backup_path: null,
        audit_pass_count: 3,
        audit_total_count: 3,
      })
    }
    return setupAction<CommitOutcome>(
      'draft.commit',
      { force: options?.force ?? false, confirm: true },
      signal,
    )
  },

  finalize(signal?: AbortSignal): Promise<CommitOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        written: 4,
        skipped: [],
        backup_path: null,
        audit_pass_count: 3,
        audit_total_count: 3,
      })
    }
    return setupAction<CommitOutcome>('finalize', { confirm: true }, signal)
  },

  installedPlugins(signal?: AbortSignal): Promise<InstalledPlugin[]> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve([{ id: 'lab-unifi@lab', service: 'unifi' }])
    }
    return setupAction<InstalledPlugin[]>('installed_plugins', {}, signal)
  },

  servicesStatus(signal?: AbortSignal): Promise<ServicesStatusResponse> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        plugins: [{ id: 'lab-unifi@lab', service: 'unifi' }],
        services: Object.values(MOCK_SERVICES).map((schema) => ({
          name: schema.name,
          display_name: schema.display_name,
          description: schema.description,
          configured: schema.name === 'unifi',
          plugin_installed: schema.name === 'unifi',
          plugin_package_id: `lab-${schema.name}@lab`,
          required_env: schema.env.filter((env) => env.required).map((env) => env.name),
        })),
      })
    }
    return setupAction<ServicesStatusResponse>('services_status', {}, signal)
  },

  installPlugin(service: string, signal?: AbortSignal): Promise<PluginLifecycleOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        service,
        package_id: `lab-${service}@lab`,
        status: 'install',
        message: 'mock install complete',
      })
    }
    return setupAction<PluginLifecycleOutcome>('install_plugin', { service, confirm: true }, signal)
  },

  uninstallPlugin(service: string, signal?: AbortSignal): Promise<PluginLifecycleOutcome> {
    if (USE_MOCK_DATA) {
      signal?.throwIfAborted?.()
      return Promise.resolve({
        service,
        package_id: `lab-${service}@lab`,
        status: 'uninstall',
        message: 'mock uninstall complete',
      })
    }
    return setupAction<PluginLifecycleOutcome>('uninstall_plugin', { service, confirm: true }, signal)
  },
}
