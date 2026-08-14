import type { CreateGatewayInput } from '@/lib/types/gateway'

export type AppCommandKind = 'destination' | 'action'

export type AppCommandGroupKey = 'best-match' | 'actions' | 'destinations'

export type AppCommandIconKey =
  | 'docs'
  | 'gateway'
  | 'overview'
  | 'settings'
  | 'snippets'
  | 'usage'

export type AppCommandItem = {
  id: string
  kind: AppCommandKind
  title: string
  description: string
  keywords: string[]
  group: AppCommandGroupKey
  icon: AppCommandIconKey
  href: string
  actionHint: string
  priority: number
}

export type AppCommandGroup = {
  key: AppCommandGroupKey
  label: string
  items: AppCommandItem[]
}

export type AppCommandState = {
  items: AppCommandItem[]
  groups: AppCommandGroup[]
  activeItemId: string | null
}

const GROUP_LABELS: Record<AppCommandGroupKey, string> = {
  'best-match': 'Best match',
  actions: 'Actions',
  destinations: 'Destinations',
}

export const appCommandItems: AppCommandItem[] = [
  {
    id: 'destination-overview',
    kind: 'destination',
    title: 'Overview',
    description: 'Open the Labby dashboard with server health, activity, and quick actions.',
    keywords: ['home', 'dashboard', 'overview', 'summary'],
    group: 'destinations',
    icon: 'overview',
    href: '/',
    actionHint: 'Open',
    priority: 100,
  },
  {
    id: 'destination-gateways',
    kind: 'destination',
    title: 'Gateway',
    description: 'Open the gateway that hosts upstream servers, policies, and runtime exposure.',
    keywords: ['server', 'servers', 'gateway', 'gateways', 'routes', 'upstream', 'policy'],
    group: 'destinations',
    icon: 'gateway',
    href: '/gateways',
    actionHint: 'Open',
    priority: 98,
  },
  {
    id: 'destination-snippets',
    kind: 'destination',
    title: 'Snippets',
    description: 'Open executable Code Mode snippets with typed inputs, validation, and smoke checks.',
    keywords: ['snippets', 'snippet', 'code mode', 'workflow', 'workflows', 'execute', 'validate', 'test'],
    group: 'destinations',
    icon: 'snippets',
    href: '/snippets',
    actionHint: 'Open',
    priority: 87,
  },
  {
    id: 'destination-usage',
    kind: 'destination',
    title: 'Usage',
    description: 'Open the gateway usage explorer with tool-call volume, tokens, and per-tool detail.',
    keywords: ['usage', 'telemetry', 'metrics', 'tool calls', 'tokens', 'analytics'],
    group: 'destinations',
    icon: 'usage',
    href: '/usage',
    actionHint: 'Open',
    priority: 86,
  },
  {
    id: 'destination-settings',
    kind: 'destination',
    title: 'Settings',
    description: 'Review auth mode, environment configuration, and control-plane defaults.',
    keywords: ['settings', 'config', 'configuration', 'auth', 'preferences'],
    group: 'destinations',
    icon: 'settings',
    href: '/settings',
    actionHint: 'Open',
    priority: 80,
  },
  {
    id: 'destination-docs',
    kind: 'destination',
    title: 'Documentation',
    description: 'Read Labby docs, setup guidance, conventions, and operator references.',
    keywords: ['docs', 'documentation', 'help', 'reference', 'guide'],
    group: 'destinations',
    icon: 'docs',
    href: '/docs',
    actionHint: 'Open',
    priority: 78,
  },
  {
    id: 'action-review-gateways',
    kind: 'action',
    title: 'Review gateway',
    description: 'Inspect gateway-hosted servers, upstreams, and exposure state.',
    keywords: ['review', 'server', 'servers', 'gateway', 'gateways', 'health', 'runtime'],
    group: 'actions',
    icon: 'gateway',
    href: '/gateways',
    actionHint: 'Run',
    priority: 87,
  },
]

function normalize(value: string): string {
  return value.trim().toLowerCase()
}

function scoreItem(item: AppCommandItem, query: string): { baseScore: number; totalScore: number } {
  if (!query) {
    return { baseScore: 0, totalScore: item.priority }
  }

  const normalizedTitle = item.title.toLowerCase()
  const normalizedDescription = item.description.toLowerCase()
  let baseScore = 0
  let matched = false

  if (normalizedTitle === query) {
    baseScore += 220
    matched = true
  }
  if (normalizedTitle.startsWith(query)) {
    baseScore += 130
    matched = true
  }
  if (normalizedTitle.includes(query)) {
    baseScore += 80
    matched = true
  }
  if (normalizedDescription.includes(query)) {
    baseScore += 20
    matched = true
  }

  for (const keyword of item.keywords) {
    const normalizedKeyword = keyword.toLowerCase()
    if (normalizedKeyword === query) {
      baseScore += 100
      matched = true
    } else if (normalizedKeyword.startsWith(query)) {
      baseScore += 58
      matched = true
    } else if (normalizedKeyword.includes(query)) {
      baseScore += 32
      matched = true
    }
  }

  if (!matched) return { baseScore: 0, totalScore: 0 }

  let totalScore = baseScore + item.priority
  if (item.kind === 'destination') totalScore += 6
  if (item.kind === 'action') totalScore += 3

  return { baseScore, totalScore }
}

function filterItems(query: string, items: AppCommandItem[]): AppCommandItem[] {
  const normalizedQuery = normalize(query)
  if (!normalizedQuery) {
    return [...items].sort((a, b) => b.priority - a.priority)
  }

  return [...items]
    .map((item) => ({ item, ...scoreItem(item, normalizedQuery) }))
    .filter(({ baseScore }) => baseScore > 40)
    .sort((a, b) => b.totalScore - a.totalScore)
    .map(({ item }) => item)
}

export function buildAppCommandState(
  query: string,
  items: AppCommandItem[] = appCommandItems,
): AppCommandState {
  const ranked = filterItems(query, items)
  if (!ranked.length) {
    return {
      items: [],
      groups: [],
      activeItemId: null,
    }
  }

  const [bestMatch, ...rest] = ranked
  const grouped = new Map<AppCommandGroupKey, AppCommandItem[]>([
    ['best-match', [bestMatch]],
    ['actions', []],
    ['destinations', []],
  ])

  for (const item of rest) {
    grouped.get(item.group)?.push(item)
  }

  const groups = [...grouped.entries()]
    .filter(([, groupItems]) => groupItems.length > 0)
    .map(([key, groupItems]) => ({
      key,
      label: GROUP_LABELS[key],
      items: groupItems,
    }))

  return {
    items: ranked,
    groups,
    activeItemId: bestMatch.id,
  }
}

export function findAppCommandItemById(
  itemId: string | null,
  items: AppCommandItem[],
): AppCommandItem | null {
  if (!itemId) return null
  return items.find((item) => item.id === itemId) ?? null
}

// ── Catalog browse helpers (pure — no React/SWR imports) ─────────────────────

export type CatalogBrowseItem = {
  kind: 'catalog-service' | 'catalog-action'
  id: string
  /** Display name: service name or dotted action name. */
  title: string
  description: string
  /** Service name (both service and action items carry this). */
  service: string
  /** Action name for `catalog-action` items; empty for `catalog-service`. */
  actionName: string
  /** True when the action is destructive (only set for `catalog-action`). */
  destructive: boolean
}

/**
 * Transform a flat list of CatalogService entries into CatalogBrowseItems.
 * Returns service-level items for the root browse page.
 * Pure function — safe to call from node:test context.
 */
export function buildCatalogServiceItems(
  services: ReadonlyArray<{ name: string; description: string }>,
): CatalogBrowseItem[] {
  return services.map((svc) => ({
    kind: 'catalog-service' as const,
    id: `catalog-svc:${svc.name}`,
    title: svc.name,
    description: svc.description,
    service: svc.name,
    actionName: '',
    destructive: false,
  }))
}

/**
 * Transform a service's actions into CatalogBrowseItems for the action page.
 * Pure function — safe to call from node:test context.
 */
export function buildCatalogActionItems(
  service: string,
  actions: ReadonlyArray<{ action: string; description: string; destructive: boolean }>,
): CatalogBrowseItem[] {
  return actions.map((a) => ({
    kind: 'catalog-action' as const,
    id: `catalog-act:${service}:${a.action}`,
    title: a.action,
    description: a.description,
    service,
    actionName: a.action,
    destructive: a.destructive,
  }))
}

// ── Scoped-prefix parsing (mock parity: `>` actions · `#` servers · `/` pages) ─

export type PaletteScope = 'actions' | 'servers' | 'pages'

export type PaletteScopeState = {
  /** Active scope, or null when the query carries no recognised prefix. */
  scope: PaletteScope | null
  /** The query with the scope prefix stripped and trimmed. */
  query: string
}

const SCOPE_PREFIXES: Record<string, PaletteScope> = {
  '>': 'actions',
  '#': 'servers',
  '/': 'pages',
}

/**
 * Hint rendered in the counts strip. The mock also advertises `@ sessions`;
 * this console has no agent-session surface, so that scope is omitted.
 */
export const PALETTE_SCOPE_HINT = '> actions · # servers · / pages'

export const PALETTE_SCOPE_LABELS: Record<PaletteScope, string> = {
  actions: 'Actions only',
  servers: 'Servers only',
  pages: 'Pages only',
}

/** Split a raw palette query into its scope prefix and the residual query. */
export function parsePaletteScope(raw: string): PaletteScopeState {
  const trimmed = raw.trim()
  const scope = SCOPE_PREFIXES[trimmed.charAt(0)] ?? null
  return {
    scope,
    query: (scope ? trimmed.slice(1) : trimmed).trim(),
  }
}

/** True when `kind` should be rendered under the active scope. */
export function paletteScopeShows(scope: PaletteScope | null, kind: PaletteScope): boolean {
  return scope === null || scope === kind
}

// ── Counts strip + footer label ───────────────────────────────────────────────

export type PaletteCountsInput = {
  servers: number
  actions: number
  pages: number
  alerts: number
}

export type PaletteCount = { key: string; value: number }

/** Counts shown in the palette meta strip. Zero-valued buckets are dropped. */
export function buildPaletteCounts(input: PaletteCountsInput): PaletteCount[] {
  return (
    [
      { key: 'Servers', value: input.servers },
      { key: 'Actions', value: input.actions },
      { key: 'Pages', value: input.pages },
      { key: 'Alerts', value: input.alerts },
    ] as PaletteCount[]
  ).filter((count) => count.value > 0)
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`
}

/** Footer summary line, e.g. `5 servers · 2 actions match`. */
export function buildPaletteFooterLabel(input: PaletteCountsInput): string {
  const parts: string[] = []
  if (input.servers) parts.push(plural(input.servers, 'server', 'servers'))
  if (input.actions) parts.push(plural(input.actions, 'action', 'actions'))
  if (input.pages) parts.push(plural(input.pages, 'page', 'pages'))
  return parts.length ? `${parts.join(' · ')} match` : 'No matches'
}

// ── Gateway connection status (mock parity: the per-service detail header) ────

export type PaletteTone = 'success' | 'warn' | 'error' | 'muted'

export type GatewayConnection = {
  /** Lowercase status word, matching the mock's `healthy` / `needs auth` copy. */
  label: string
  tone: PaletteTone
}

/** Minimal structural shape needed to describe a gateway's connection state. */
export type GatewayConnectionInput = {
  enabled?: boolean
  status: { healthy: boolean; connected: boolean; last_error?: string }
  warnings?: ReadonlyArray<{ code: string; message: string }>
}

function mentionsAuth(gateway: GatewayConnectionInput): boolean {
  const haystack = [
    gateway.status.last_error ?? '',
    ...(gateway.warnings ?? []).flatMap((w) => [w.code, w.message]),
  ]
    .join(' ')
    .toLowerCase()
  return /\bauth|unauthori[sz]ed|401\b/.test(haystack)
}

/**
 * Map a gateway to the mock's connection vocabulary.
 * The mock distinguishes `token expired` from `needs auth`; this console has no
 * token-expiry field, so an auth-flavoured failure collapses to `needs auth`.
 */
export function describeGatewayConnection(gateway: GatewayConnectionInput): GatewayConnection {
  if (gateway.enabled === false) return { label: 'disabled', tone: 'muted' }
  if (!gateway.status.connected) {
    return mentionsAuth(gateway)
      ? { label: 'needs auth', tone: 'warn' }
      : { label: 'disconnected', tone: 'error' }
  }
  if (!gateway.status.healthy) {
    return mentionsAuth(gateway)
      ? { label: 'needs auth', tone: 'warn' }
      : { label: 'degraded', tone: 'warn' }
  }
  return { label: 'healthy', tone: 'success' }
}

// ── Inline "Add Server" flow (mock parity: the palette's add-server sheet) ────

export type PaletteAddAuth = 'none' | 'bearer' | 'oauth'

export type PaletteAddForm = {
  name: string
  /** Endpoint URL or stdio command line. */
  target: string
  auth: PaletteAddAuth
  tokenEnv: string
  /** `KEY=value, KEY=value` for stdio upstreams. */
  env: string
  proxyResources: boolean
  proxyPrompts: boolean
}

/** `https://…` targets are HTTP upstreams; anything else is a stdio command. */
export function detectPaletteAddTransport(target: string): 'http' | 'stdio' | null {
  const value = target.trim()
  if (!value) return null
  return /^https?:\/\//i.test(value) ? 'http' : 'stdio'
}

/** Parse `KEY=value, KEY=value` into an env map. Malformed entries are dropped. */
export function parsePaletteEnvPairs(raw: string): Record<string, string> {
  const env: Record<string, string> = {}
  for (const pair of raw.split(',')) {
    const eq = pair.indexOf('=')
    if (eq <= 0) continue
    const key = pair.slice(0, eq).trim()
    const value = pair.slice(eq + 1).trim()
    if (key) env[key] = value
  }
  return env
}

/**
 * Turn the inline add-server form into a `CreateGatewayInput`.
 * Returns null when there is no target to add — the caller surfaces the error.
 */
export function buildAddServerInput(form: PaletteAddForm): CreateGatewayInput | null {
  const transport = detectPaletteAddTransport(form.target)
  if (!transport) return null

  const target = form.target.trim()
  const name = form.name.trim() || (transport === 'http' ? hostnameOf(target) : target.split(/\s+/)[0])

  if (transport === 'http') {
    const tokenEnv = form.tokenEnv.trim()
    return {
      name,
      transport: 'http',
      config: {
        url: target,
        ...(form.auth === 'bearer' && tokenEnv ? { bearer_token_env: tokenEnv } : {}),
        ...(form.auth === 'oauth' ? { oauth_enabled: true } : {}),
        proxy_resources: form.proxyResources,
        proxy_prompts: form.proxyPrompts,
      },
    }
  }

  const [command, ...args] = target.split(/\s+/)
  const env = parsePaletteEnvPairs(form.env)
  return {
    name,
    transport: 'stdio',
    config: {
      command,
      ...(args.length ? { args } : {}),
      ...(Object.keys(env).length ? { env } : {}),
      proxy_resources: form.proxyResources,
      proxy_prompts: form.proxyPrompts,
    },
  }
}

function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname
  } catch {
    return url
  }
}

export type PaletteAlert = {
  id: string
  gatewayId: string
  label: string
  tone: PaletteTone
}

/**
 * Derive the "Needs Attention" rows from live gateway state.
 * Only enabled gateways that are not healthy produce an alert; the mock caps
 * the section at three rows.
 */
export function buildGatewayAlerts(
  gateways: ReadonlyArray<GatewayConnectionInput & { id: string; name: string }>,
  limit = 3,
): PaletteAlert[] {
  return gateways
    .filter((gateway) => gateway.enabled !== false)
    .map((gateway) => ({ gateway, connection: describeGatewayConnection(gateway) }))
    .filter(({ connection }) => connection.tone === 'error' || connection.tone === 'warn')
    .slice(0, limit)
    .map(({ gateway, connection }) => ({
      id: `alert-${gateway.id}`,
      gatewayId: gateway.id,
      label: `${gateway.name} ${connection.label}`,
      tone: connection.tone,
    }))
}
