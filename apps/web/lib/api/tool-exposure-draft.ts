import type { DiscoveredTool, ExposurePolicy } from '../types/gateway.ts'

export const EXPOSE_NONE_PATTERN = '__labby_expose_none__'
export type ExposureViewFilter = 'all' | 'enabled' | 'hidden'

function sortUnique(values: string[]): string[] {
  return [...new Set(values)].sort((left, right) => left.localeCompare(right))
}

export function createExposureDraftFromTools(tools: DiscoveredTool[]): string[] {
  return sortUnique(tools.filter((tool) => tool.exposed).map((tool) => tool.name))
}

export function applyBulkExposureToDraft(
  draftSelectedToolNames: string[],
  targetToolNames: string[],
  shouldEnable: boolean,
): string[] {
  const draftSet = new Set(draftSelectedToolNames)

  for (const toolName of targetToolNames) {
    if (shouldEnable) {
      draftSet.add(toolName)
    } else {
      draftSet.delete(toolName)
    }
  }

  return sortUnique([...draftSet])
}

export function buildExposurePolicyFromDraft(
  allToolNames: string[],
  draftSelectedToolNames: string[],
): ExposurePolicy {
  const totalTools = sortUnique(allToolNames)
  const totalToolSet = new Set(totalTools)
  const selectedTools = sortUnique(draftSelectedToolNames).filter((toolName) =>
    totalToolSet.has(toolName),
  )
  const allToolsSelected =
    totalTools.length > 0 &&
    totalTools.every((toolName) => selectedTools.includes(toolName)) &&
    selectedTools.every((toolName) => totalToolSet.has(toolName))

  if (allToolsSelected) {
    return { mode: 'expose_all', patterns: [] }
  }

  if (selectedTools.length === 0) {
    // A genuinely empty allowlist. Each write path encodes this for its own
    // wire contract: custom gateways persist `expose_tools: []`, which the
    // backend compiles to an allowlist matching nothing; virtual servers still
    // need EXPOSE_NONE_PATTERN because their `allowed_actions: []` means
    // expose-ALL. See `gatewayApi.setExposurePolicy`.
    return { mode: 'allowlist', patterns: [] }
  }

  return {
    mode: 'allowlist',
    patterns: selectedTools,
  }
}

export function getDraftExposureSummary(
  allToolNames: string[],
  draftSelectedToolNames: string[],
): { exposedCount: number; totalCount: number; label: string } {
  const totalCount = sortUnique(allToolNames).length
  const totalToolSet = new Set(sortUnique(allToolNames))
  const exposedCount = sortUnique(draftSelectedToolNames).filter((toolName) =>
    totalToolSet.has(toolName),
  ).length

  return {
    exposedCount,
    totalCount,
    label: `${exposedCount}/${totalCount}`,
  }
}

export function stripExposeNonePattern(patterns: string[]): string[] {
  return patterns.filter((pattern) => pattern !== EXPOSE_NONE_PATTERN)
}

export function getExposureFilterCounts(
  tools: DiscoveredTool[],
): Record<ExposureViewFilter, number> {
  const enabled = tools.filter((tool) => tool.exposed).length

  return {
    all: tools.length,
    enabled,
    hidden: tools.length - enabled,
  }
}

export function filterToolsForExposureView(
  tools: DiscoveredTool[],
  filter: ExposureViewFilter,
  search: string,
): DiscoveredTool[] {
  const normalizedSearch = search.trim().toLowerCase()

  return tools.filter((tool) => {
    const matchesFilter =
      filter === 'all'
        ? true
        : filter === 'enabled'
          ? tool.exposed
          : !tool.exposed

    const matchesSearch =
      normalizedSearch.length === 0 ||
      tool.name.toLowerCase().includes(normalizedSearch) ||
      tool.description?.toLowerCase().includes(normalizedSearch)

    return matchesFilter && matchesSearch
  })
}

export function getDraftChangeDescription(
  currentExposedToolNames: string[],
  draftSelectedToolNames: string[],
): string {
  const current = new Set(currentExposedToolNames)
  const draft = new Set(draftSelectedToolNames)

  const enabled = draftSelectedToolNames.filter((toolName) => !current.has(toolName)).length
  const hidden = currentExposedToolNames.filter((toolName) => !draft.has(toolName)).length

  if (enabled === 0 && hidden === 0) {
    return 'No unsaved exposure changes.'
  }

  const parts: string[] = []
  if (enabled > 0) {
    parts.push(`${enabled} tool${enabled === 1 ? '' : 's'} will be enabled`)
  }
  if (hidden > 0) {
    parts.push(`${hidden} tool${hidden === 1 ? '' : 's'} will be hidden`)
  }

  return `${parts.join(', ')}.`
}
