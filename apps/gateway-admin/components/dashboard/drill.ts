import type {
  ActorDrillTarget,
  ActorKind,
  AttributionDrillFilter,
  MetricsWindow,
  UsageAttribution,
} from '@/lib/types/metrics'

export type { ActorDrillTarget, AttributionDrillFilter } from '@/lib/types/metrics'

/** Which entity a dashboard drill-down targets. UI state, not server data. */
export type DrillTarget =
  | { type: 'tool'; name: string }
  | ActorDrillTarget

export function actorDrillTarget(source: {
  id: string
  filter_id?: string
  label: string
  kind: ActorKind
  attribution?: UsageAttribution | null
}): ActorDrillTarget {
  const attribution = source.attribution
  const filter: AttributionDrillFilter = {
    actor: source.filter_id ?? source.id,
  }
  if (source.kind === 'client') {
    if (attribution?.client_name) filter.client_name = attribution.client_name
    if (attribution?.client_version) filter.client_version = attribution.client_version
  } else if (source.kind === 'agent' && attribution?.agent_id) {
    filter.agent_id = attribution.agent_id
  }
  return { type: 'agent', filter, label: source.label, kind: source.kind }
}

export function actorUsageHref(target: ActorDrillTarget, window: MetricsWindow): string {
  const params = new URLSearchParams({ window, agent: target.filter.actor })
  if (target.filter.client_name) params.set('client_name', target.filter.client_name)
  if (target.filter.client_version) params.set('client_version', target.filter.client_version)
  if (target.filter.agent_id) params.set('agent_id', target.filter.agent_id)
  return `/usage?${params.toString()}`
}

export function sameAttributionDrillFilter(
  left: AttributionDrillFilter,
  right: AttributionDrillFilter,
): boolean {
  return left.actor === right.actor
    && left.client_name === right.client_name
    && left.client_version === right.client_version
    && left.agent_id === right.agent_id
}
