export interface GatewayMetricItem {
  key: 'discovered' | 'exposed' | 'filtered'
  label: string
  value: number
  tone: 'muted' | 'success'
}

export function buildGatewayMetricItems(
  discoveredCount: number,
  exposedCount: number,
  showFiltered = true,
): GatewayMetricItem[] {
  const items: GatewayMetricItem[] = [
    {
      key: 'discovered',
      label: 'tools',
      value: discoveredCount,
      tone: 'muted',
    },
    {
      key: 'exposed',
      label: 'exposed',
      value: exposedCount,
      tone: 'success',
    },
  ]

  const filteredCount = discoveredCount - exposedCount
  if (showFiltered && filteredCount > 0) {
    items.push({
      key: 'filtered',
      label: 'filtered',
      value: filteredCount,
      tone: 'muted',
    })
  }

  return items
}
