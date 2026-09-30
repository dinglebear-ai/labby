import {
  type CommandPaletteGroupKey,
  type CommandPaletteItem,
  commandPaletteItems,
} from './command-palette-data'

export type CommandPaletteGroup = {
  key: CommandPaletteGroupKey
  label: string
  items: CommandPaletteItem[]
}

export type CommandPaletteState = {
  items: CommandPaletteItem[]
  groups: CommandPaletteGroup[]
  activeItemId: string | null
}

const GROUP_LABELS: Record<CommandPaletteGroupKey, string> = {
  'best-match': 'Best match',
  'suggested-actions': 'Suggested actions',
  'recent-context': 'Recent context',
  destinations: 'Destinations',
}

function normalize(value: string): string {
  return value.trim().toLowerCase()
}

function scoreItem(item: CommandPaletteItem, query: string): { baseScore: number; totalScore: number } {
  if (!query) {
    return { baseScore: 0, totalScore: item.priority }
  }

  const normalizedTitle = item.title.toLowerCase()
  const normalizedDescription = item.description.toLowerCase()
  let baseScore = 0
  let matched = false

  // Cumulative title scoring is intentional: exact and prefix hits should
  // outrank plain substring matches, so do not collapse these into `else if`.
  if (normalizedTitle === query) {
    baseScore += 200
    matched = true
  }
  if (normalizedTitle.startsWith(query)) {
    baseScore += 120
    matched = true
  }
  if (normalizedTitle.includes(query)) {
    baseScore += 75
    matched = true
  }
  if (normalizedDescription.includes(query)) {
    baseScore += 20
    matched = true
  }

  // Keywords use the same cumulative ranking model as titles on purpose.
  for (const keyword of item.keywords) {
    const normalizedKeyword = keyword.toLowerCase()
    if (normalizedKeyword === query) {
      baseScore += 95
      matched = true
    } else if (normalizedKeyword.startsWith(query)) {
      baseScore += 55
      matched = true
    } else if (normalizedKeyword.includes(query)) {
      baseScore += 30
      matched = true
    }
  }

  if (!matched) {
    return { baseScore: 0, totalScore: 0 }
  }

  let totalScore = baseScore + item.priority
  if (item.type === 'destination') totalScore += 6
  if (item.type === 'action') totalScore += 3

  return { baseScore, totalScore }
}

function filterItems(query: string, items: CommandPaletteItem[]): CommandPaletteItem[] {
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

export function buildCommandPaletteState(
  query: string,
  items: CommandPaletteItem[] = commandPaletteItems,
): CommandPaletteState {
  const ranked = filterItems(query, items)
  if (!ranked.length) {
    return {
      items: [],
      groups: [],
      activeItemId: null,
    }
  }

  const [bestMatch, ...rest] = ranked
  const grouped = new Map<CommandPaletteGroupKey, CommandPaletteItem[]>([
    ['best-match', [bestMatch]],
    ['suggested-actions', []],
    ['recent-context', []],
    ['destinations', []],
  ])

  for (const item of rest) {
    if (item.type === 'action') grouped.get('suggested-actions')?.push(item)
    else if (item.type === 'recent') grouped.get('recent-context')?.push(item)
    else grouped.get('destinations')?.push(item)
  }

  const groups: CommandPaletteGroup[] = [...grouped.entries()]
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

export function findItemById(
  itemId: string | null,
  items: CommandPaletteItem[],
): CommandPaletteItem | null {
  if (!itemId) return null
  return items.find((item) => item.id === itemId) ?? null
}

export function getNextActiveItemId(
  currentItemId: string | null,
  items: CommandPaletteItem[],
  direction: 'next' | 'previous',
): string | null {
  if (!items.length) return null
  if (!currentItemId) return items[0]?.id ?? null

  const currentIndex = items.findIndex((item) => item.id === currentItemId)
  if (currentIndex === -1) return items[0]?.id ?? null

  if (direction === 'next') {
    return items[(currentIndex + 1) % items.length]?.id ?? items[0]?.id ?? null
  }

  return items[(currentIndex - 1 + items.length) % items.length]?.id ?? items[0]?.id ?? null
}
