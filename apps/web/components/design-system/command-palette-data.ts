export type CommandPaletteItemType = 'destination' | 'action' | 'recent'

export type CommandPaletteGroupKey =
  | 'best-match'
  | 'suggested-actions'
  | 'recent-context'
  | 'destinations'

export type CommandPaletteIconKey =
  | 'gateway'
  | 'logs'
  | 'settings'
  | 'reload'
  | 'token'
  | 'setup'
  | 'recent'

export type CommandPaletteItem = {
  id: string
  type: CommandPaletteItemType
  title: string
  description: string
  keywords: string[]
  group: CommandPaletteGroupKey
  icon: CommandPaletteIconKey
  actionHint: string
  priority: number
  service?: string
  entityId?: string
  recentTimestamp?: string
  dangerous?: boolean
}

export const commandPaletteItems: CommandPaletteItem[] = [
  {
    id: 'destination-gateway-admin',
    type: 'destination',
    title: 'Gateway Admin',
    description: 'Open the primary control plane for routes, upstream servers, and policies.',
    keywords: ['gateway', 'gateways', 'admin', 'routes', 'policies', 'upstreams'],
    group: 'destinations',
    icon: 'gateway',
    actionHint: 'Open',
    priority: 100,
  },
  {
    id: 'destination-logs',
    type: 'destination',
    title: 'Logs',
    description: 'Jump into the local-master log stream with dense operational filtering.',
    keywords: ['logs', 'tail', 'stream', 'events', 'observability'],
    group: 'destinations',
    icon: 'logs',
    actionHint: 'Open',
    priority: 95,
  },
  {
    id: 'destination-settings',
    type: 'destination',
    title: 'Settings',
    description: 'Review environment configuration, auth posture, and instance-level preferences.',
    keywords: ['settings', 'preferences', 'config', 'auth'],
    group: 'destinations',
    icon: 'settings',
    actionHint: 'Open',
    priority: 82,
  },
  {
    id: 'action-reload-gateways',
    type: 'action',
    title: 'Reload gateways',
    description: 'Apply the active gateway runtime configuration without leaving the current context.',
    keywords: ['reload', 'gateways', 'gateway', 'apply', 'runtime'],
    group: 'suggested-actions',
    icon: 'reload',
    actionHint: 'Run',
    priority: 96,
  },
  {
    id: 'action-rotate-token',
    type: 'action',
    title: 'Rotate MCP token',
    description: 'Issue a fresh bearer token and update the environment-backed control-plane configuration.',
    keywords: ['rotate', 'token', 'mcp', 'security', 'auth'],
    group: 'suggested-actions',
    icon: 'token',
    actionHint: 'Secure',
    priority: 90,
    dangerous: true,
  },
  {
    id: 'action-tail-local-logs',
    type: 'action',
    title: 'Tail local logs',
    description: 'Resume the live log stream with the current filter stack and time window preserved.',
    keywords: ['logs', 'tail', 'stream', 'follow', 'events'],
    group: 'suggested-actions',
    icon: 'logs',
    actionHint: 'Run',
    priority: 91,
  },
  {
    id: 'action-open-setup',
    type: 'action',
    title: 'Open setup',
    description: 'Return to onboarding and environment validation without leaving the admin shell.',
    keywords: ['setup', 'onboarding', 'env', 'doctor'],
    group: 'suggested-actions',
    icon: 'setup',
    actionHint: 'Open',
    priority: 78,
  },
  {
    id: 'recent-edge-proxy-prod',
    type: 'recent',
    title: 'edge-proxy-prod',
    description: 'Resume the last inspected gateway entity with its inspector state restored.',
    keywords: ['edge', 'proxy', 'prod', 'gateway', 'recent'],
    group: 'recent-context',
    icon: 'recent',
    actionHint: 'Jump',
    priority: 92,
    service: 'gateway',
    entityId: 'edge-proxy-prod',
    recentTimestamp: '2026-04-23T15:56:00Z',
  },
  {
    id: 'recent-logs-errors',
    type: 'recent',
    title: 'Errors in local logs',
    description: 'Reopen the recent logs view scoped to warn and error events.',
    keywords: ['logs', 'errors', 'warn', 'recent', 'filters'],
    group: 'recent-context',
    icon: 'recent',
    actionHint: 'Jump',
    priority: 72,
    service: 'logs',
    entityId: 'logs-errors',
    recentTimestamp: '2026-04-23T15:48:00Z',
  },
]
