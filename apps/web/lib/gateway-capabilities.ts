import type { CapabilityKind, CapabilityObservationValue, GatewayStatus } from './types/gateway.ts'

const countKeys = { tools: 'tool', resources: 'resource', prompts: 'prompt', skills: 'skill' } as const
/** Compatibility counts are used only when a legacy backend supplies no observation contract. */
export function capabilityValue(status: GatewayStatus, kind: CapabilityKind): CapabilityObservationValue {
  const observation = status.capability_observation?.[kind]
  if (observation) return observation
  const key = countKeys[kind]
  return {
    state: status.catalog_warming ? 'unknown' : 'known',
    discovered: status[`discovered_${key}_count`] ?? 0,
    exposed: status[`exposed_${key}_count`] ?? 0,
  }
}
export function capabilityLabel(status: GatewayStatus, kind: CapabilityKind): string {
  const value = capabilityValue(status, kind)
  if (value.state === 'unknown') return 'Not discovered'
  if (value.state === 'failed') return 'Discovery failed'
  if (value.discovered === null || value.exposed === null) return 'Not discovered'
  return `${value.exposed}/${value.discovered}${value.state === 'stale' ? ' · stale' : ''}`
}
export function capabilityDescription(status: GatewayStatus, kind: CapabilityKind): string {
  const value = capabilityValue(status, kind)
  const error = value.error
  return `${capabilityLabel(status, kind)}${error ? ` — ${error}` : ''}`
}
export function capabilityScopeLabel(status: GatewayStatus): string | undefined {
  const scope = status.capability_observation?.scope
  return scope === 'credential' ? 'Credential catalog' : scope === 'global' ? 'Shared catalog' : undefined
}
export function summarizeCapabilities(statuses: GatewayStatus[], kind: CapabilityKind) {
  return statuses.reduce((sum, status) => {
    const value = capabilityValue(status, kind)
    if (value.state !== 'known' || value.discovered === null || value.exposed === null) {
      sum.incomplete++
    } else {
      sum.discovered += value.discovered
      sum.exposed += value.exposed
    }
    return sum
  }, { discovered: 0, exposed: 0, incomplete: 0 })
}
