import type { DepotProviderOption } from '@/lib/api/depot-client'

export type SourceOrigin = 'mcp-registry' | 'acp-registry' | 'ard'
export type SourceSupport = { state: 'available' | 'unknown' | 'unavailable'; reason: string }

/** Federation requires every selected enabled backend to support the filter. */
export function discoverSourceSupport(providers: readonly DepotProviderOption[], selectedProvider: string, origin: SourceOrigin): SourceSupport {
  const selected = providers.filter(provider => selectedProvider === 'all' || provider.id === selectedProvider)
  if (!selected.length) return { state: 'unknown', reason: 'Backend support has not been reported yet' }
  const enabled = selected.filter(provider => provider.enabled)
  if (!enabled.length) return { state: 'unavailable', reason: 'No selected backend is enabled' }
  if (enabled.some(provider => provider.sourceOrigins && !provider.sourceOrigins.includes(origin))) {
    return { state: 'unavailable', reason: 'A selected backend does not support this source filter' }
  }
  if (enabled.some(provider => provider.sourceOrigins == null)) {
    return { state: 'unknown', reason: 'Source-filter support has not been reported by every selected backend' }
  }
  return { state: 'available', reason: 'Filter by this source' }
}
