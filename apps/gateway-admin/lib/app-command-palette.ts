import { parseStdioCommandLine } from '@/lib/stdio-command'
import type { CreateGatewayInput } from '@/lib/types/gateway'

export type AppCommandKind = 'destination' | 'action'

export type AppCommandGroupKey = 'best-match' | 'actions' | 'destinations'

export type AppCommandIconKey =
  | 'agents'
  | 'docs'
  | 'gateway'
  | 'logs'
  | 'overview'
  | 'settings'
  | 'snippets'
  | 'tasks'
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
    id: 'destination-sessions',
    kind: 'destination',
    title: 'Sessions · Mock',
    description: 'Open the visual-only agent sessions surface. All rows are explicitly marked as mock data.',
    keywords: ['sessions', 'agents', 'workspaces', 'mock'],
    group: 'destinations',
    icon: 'agents',
    href: '/sessions',
    actionHint: 'Open mock',
    priority: 84,
  },
  {
    id: 'destination-tasks',
    kind: 'destination',
    title: 'Tasks · Mock',
    description: 'Open the visual-only recurring tasks surface. All rows are explicitly marked as mock data.',
    keywords: ['tasks', 'schedules', 'recurring', 'agents', 'mock'],
    group: 'destinations',
    icon: 'tasks',
    href: '/tasks',
    actionHint: 'Open mock',
    priority: 83,
  },
  {
    id: 'destination-logs',
    kind: 'destination',
    title: 'Logs · Mock',
    description: 'Open the visual-only unified log stream. Every event is explicitly marked as mock data.',
    keywords: ['logs', 'events', 'observability', 'stream', 'mock'],
    group: 'destinations',
    icon: 'logs',
    href: '/logs',
    actionHint: 'Open mock',
    priority: 82,
  },
  {
    id: 'destination-discovery',
    kind: 'destination',
    title: 'Discovery · Mock',
    description: 'Open the visual-only Depot discovery surface. All artifacts are explicitly marked as mock data.',
    keywords: ['discovery', 'depot', 'bazaar', 'artifacts', 'mock'],
    group: 'destinations', icon: 'gateway', href: '/discovery', actionHint: 'Open mock', priority: 81,
  },
  {
    id: 'destination-create',
    kind: 'destination',
    title: 'Create · Mock',
    description: 'Open the visual-only artifact authoring surface. All workflow state is explicitly marked as mock data.',
    keywords: ['create', 'author', 'artifact', 'bundle', 'mock'],
    group: 'destinations', icon: 'snippets', href: '/create', actionHint: 'Open mock', priority: 80,
  },
  {
    id: 'destination-library',
    kind: 'destination',
    title: 'Library · Mock',
    description: 'Open the visual-only personal artifact library. All inventory is explicitly marked as mock data.',
    keywords: ['library', 'artifacts', 'loadouts', 'snippets', 'mock'],
    group: 'destinations', icon: 'docs', href: '/library', actionHint: 'Open mock', priority: 79,
  },
  {
    id: 'destination-agents',
    kind: 'destination',
    title: 'Agents · Mock',
    description: 'Open the visual-only agent launcher. All sessions are explicitly marked as mock data.',
    keywords: ['agents', 'sessions', 'workspace', 'mock'],
    group: 'destinations', icon: 'agents', href: '/agents', actionHint: 'Open mock', priority: 78,
  },
  {
    id: 'destination-stash',
    kind: 'destination',
    title: 'Stash · Mock',
    description: 'Open the visual-only agent file stash. Every file is explicitly marked as mock data.',
    keywords: ['stash', 'files', 'workspace', 'mock'],
    group: 'destinations', icon: 'docs', href: '/stash', actionHint: 'Open mock', priority: 77,
  },
  {
    id: 'destination-containers',
    kind: 'destination',
    title: 'Dev Containers · Mock',
    description: 'Open the visual-only development container inventory. All images are explicitly marked as mock data.',
    keywords: ['containers', 'incus', 'images', 'workspace', 'mock'],
    group: 'destinations', icon: 'logs', href: '/containers', actionHint: 'Open mock', priority: 76,
  },
  {
    id: 'destination-instance',
    kind: 'destination',
    title: 'Labby Instance · Mock',
    description: 'Open the visual-only hosted instance surface. All service state is explicitly marked as mock data.',
    keywords: ['labby', 'instance', 'hosted', 'region', 'mock'],
    group: 'destinations', icon: 'gateway', href: '/instance', actionHint: 'Open mock', priority: 75,
  },
  {
    id: 'destination-team-overview', kind: 'destination', title: 'Team Overview · Mock',
    description: 'Open the visual-only tootie.tv workspace overview. All team state is explicitly marked as mock data.',
    keywords: ['team', 'overview', 'tootie.tv', 'members', 'mock'], group: 'destinations', icon: 'agents', href: '/team', actionHint: 'Open mock', priority: 74,
  },
  {
    id: 'destination-team-library', kind: 'destination', title: 'Team Library · Mock',
    description: 'Open the visual-only shared artifact library. All submissions are explicitly marked as mock data.',
    keywords: ['team', 'library', 'artifacts', 'submissions', 'mock'], group: 'destinations', icon: 'docs', href: '/team/library', actionHint: 'Open mock', priority: 73,
  },
  {
    id: 'destination-team-projects', kind: 'destination', title: 'Projects · Mock',
    description: 'Open the visual-only team projects surface. All bindings are explicitly marked as mock data.',
    keywords: ['team', 'projects', 'repositories', 'loadouts', 'mock'], group: 'destinations', icon: 'gateway', href: '/team/projects', actionHint: 'Open mock', priority: 72,
  },
  {
    id: 'destination-team-activity', kind: 'destination', title: 'Activity · Mock',
    description: 'Open the visual-only team activity feed. Every event is explicitly marked as mock data.',
    keywords: ['team', 'activity', 'feed', 'announcements', 'mock'], group: 'destinations', icon: 'usage', href: '/team/activity', actionHint: 'Open mock', priority: 71,
  },
  {
    id: 'destination-team-stash', kind: 'destination', title: 'Team Stash · Mock',
    description: 'Open the visual-only shared working-file stash. Every file is explicitly marked as mock data.',
    keywords: ['team', 'stash', 'files', 'shared', 'mock'], group: 'destinations', icon: 'docs', href: '/team/stash', actionHint: 'Open mock', priority: 70,
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

// ── Server filters (mock parity: the palette's filter panel) ─────────────────

export type PaletteStatusFilter = 'healthy' | 'disconnected' | 'enabled' | 'disabled'
export type PaletteTransportFilter = 'stdio' | 'http'

export type PaletteServerFilters = {
  status: PaletteStatusFilter[]
  transport: PaletteTransportFilter[]
}

export const EMPTY_PALETTE_FILTERS: PaletteServerFilters = { status: [], transport: [] }

export const PALETTE_STATUS_FILTERS: Array<{ value: PaletteStatusFilter; label: string }> = [
  { value: 'healthy', label: 'Healthy' },
  { value: 'disconnected', label: 'Disconnected' },
  { value: 'enabled', label: 'Enabled' },
  { value: 'disabled', label: 'Disabled' },
]

/**
 * The mock also offers a `Source` group (Gateway / Registry). This console has
 * no equivalent taxonomy on `Gateway.source`, so that group is omitted.
 */
export const PALETTE_TRANSPORT_FILTERS: Array<{ value: PaletteTransportFilter; label: string }> = [
  { value: 'stdio', label: 'stdio' },
  { value: 'http', label: 'HTTP' },
]

export type PaletteFilterableGateway = GatewayConnectionInput & { transport: string }

function matchesStatusFilter(
  gateway: PaletteFilterableGateway,
  value: PaletteStatusFilter,
): boolean {
  switch (value) {
    case 'healthy':
      return describeGatewayConnection(gateway).tone === 'success'
    case 'disconnected':
      return !gateway.status.connected
    case 'enabled':
      return gateway.enabled !== false
    case 'disabled':
      return gateway.enabled === false
  }
}

/** OR within a filter group, AND across groups — the mock's behaviour. */
export function gatewayMatchesPaletteFilters(
  gateway: PaletteFilterableGateway,
  filters: PaletteServerFilters,
): boolean {
  if (filters.status.length && !filters.status.some((v) => matchesStatusFilter(gateway, v))) {
    return false
  }
  if (
    filters.transport.length &&
    !filters.transport.some((v) => gateway.transport === v || (v === 'http' && gateway.transport === 'in_process'))
  ) {
    return false
  }
  return true
}

/** Count of gateways a given pill would match, shown beside the pill label. */
export function countPaletteFilterMatches(
  gateways: ReadonlyArray<PaletteFilterableGateway>,
  group: 'status' | 'transport',
  value: string,
): number {
  return gateways.filter((gateway) =>
    group === 'status'
      ? matchesStatusFilter(gateway, value as PaletteStatusFilter)
      : gatewayMatchesPaletteFilters(gateway, {
          status: [],
          transport: [value as PaletteTransportFilter],
        }),
  ).length
}

export function paletteFiltersActive(filters: PaletteServerFilters): boolean {
  return filters.status.length > 0 || filters.transport.length > 0
}

/** Toggle a value inside one filter group, returning a new filters object. */
export function togglePaletteFilter(
  filters: PaletteServerFilters,
  group: 'status' | 'transport',
  value: string,
): PaletteServerFilters {
  const current = filters[group] as string[]
  const next = current.includes(value)
    ? current.filter((entry) => entry !== value)
    : [...current, value]
  return { ...filters, [group]: next } as PaletteServerFilters
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
  const fallbackName =
    transport === 'http' ? hostnameOf(target) : target.trim().split(/\s+/)[0]
  const name = form.name.trim() || fallbackName

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

  // Reuse the console's shared tokenizer so quoted args and leading env
  // assignments behave exactly as they do in the full add-server dialog.
  let command: string
  let args: string[]
  try {
    ;({ command, args } = parseStdioCommandLine(target))
  } catch {
    return null
  }
  if (!command) return null

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
