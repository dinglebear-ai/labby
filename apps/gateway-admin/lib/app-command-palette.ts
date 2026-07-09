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
