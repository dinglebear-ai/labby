'use client'

import useSWR from 'swr'
import { useSyncExternalStore } from 'react'
import {
  fetchAgentDetail,
  fetchToolCalls,
  fetchToolDetail,
} from '@/lib/api/metrics-client'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import {
  getBrowserSessionContextIdentity,
  getBrowserSessionEpoch,
  subscribeToBrowserSession,
} from '@/lib/auth/session-store'
import type {
  ActorDrillTarget,
  AgentDetail,
  MetricsWindow,
  ToolCallPage,
  ToolCallQuery,
  ToolDetail,
} from '@/lib/types/metrics'

type UsageRequestContext = {
  base: string
  identity: string
  epoch: number
}

function useUsageRequestContext(): UsageRequestContext {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  return {
    base: normalizeGatewayApiBase(),
    identity: getBrowserSessionContextIdentity(),
    epoch,
  }
}

function assertUsageRequestContextCurrent(context: UsageRequestContext) {
  if (
    context.epoch !== getBrowserSessionEpoch()
    || context.identity !== getBrowserSessionContextIdentity()
    || context.base !== normalizeGatewayApiBase()
  ) {
    throw new DOMException('Authority or API target changed', 'AbortError')
  }
}

export const agentDetailKey = (
  target: ActorDrillTarget,
  window: MetricsWindow,
  context: UsageRequestContext,
) => ['agent-detail', target, window, context.base, context.identity, context.epoch] as const

export const toolDetailKey = (
  name: string,
  window: MetricsWindow,
  context: UsageRequestContext,
) => ['tool-detail', name, window, context.base, context.identity, context.epoch] as const

export const toolCallsKey = (query: ToolCallQuery, context: UsageRequestContext) =>
  ['tool-calls', query, context.base, context.identity, context.epoch] as const

/** Single-tool drill-down. Pass `null` name to disable (closed drawer). */
export function useToolDetail(name: string | null, window: MetricsWindow) {
  const context = useUsageRequestContext()
  return useSWR<ToolDetail>(
    name ? toolDetailKey(name, window, context) : null,
    name ? async () => {
      const detail = await fetchToolDetail(name, window, { baseUrl: context.base })
      assertUsageRequestContextCurrent(context)
      return detail
    } : null,
    { revalidateOnFocus: false, keepPreviousData: false },
  )
}

/** Single-agent/device drill-down. Pass `null` id to disable. */
export function useAgentDetail(target: ActorDrillTarget | null, window: MetricsWindow) {
  const context = useUsageRequestContext()
  return useSWR<AgentDetail>(
    target ? agentDetailKey(target, window, context) : null,
    target ? async () => {
      const detail = await fetchAgentDetail(target, window, { baseUrl: context.base })
      assertUsageRequestContextCurrent(context)
      return detail
    } : null,
    { revalidateOnFocus: false, keepPreviousData: false },
  )
}

/** Filterable tool-call log for the explorer page. */
export function useToolCalls(query: ToolCallQuery) {
  const context = useUsageRequestContext()
  return useSWR<ToolCallPage>(
    toolCallsKey(query, context),
    async () => {
      const page = await fetchToolCalls(query, { baseUrl: context.base })
      assertUsageRequestContextCurrent(context)
      return page
    },
    { revalidateOnFocus: false, keepPreviousData: false },
  )
}
