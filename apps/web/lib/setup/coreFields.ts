// Single source of truth for the operator-level core config fields shown
// on /setup/core-config and /settings/core. Both pages render the same set;
// adding or renaming a core var must happen here only.

export interface CoreField {
  key: string
  label: string
  description: string
  example: string
}

export const CORE_FIELDS: readonly CoreField[] = [
  {
    key: 'LABBY_MCP_HTTP_HOST',
    label: 'Bind host',
    description: 'Host the Labby HTTP server binds to. Default 127.0.0.1 (loopback only).',
    example: '127.0.0.1',
  },
  {
    key: 'LABBY_MCP_HTTP_PORT',
    label: 'Bind port',
    description: 'TCP port for the Labby HTTP server.',
    example: '8765',
  },
  {
    key: 'LABBY_LOG',
    label: 'Log filter',
    description: 'tracing-subscriber filter directive.',
    example: 'labby=info,labby_apis=warn',
  },
  {
    key: 'LABBY_LOG_FORMAT',
    label: 'Log format',
    description: 'Set to "json" for structured logs in production.',
    example: 'text',
  },
] as const
