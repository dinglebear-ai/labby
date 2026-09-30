'use client'

import useSWR from 'swr'
import { useSyncExternalStore } from 'react'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { fetchDashboardMetrics } from '@/lib/api/metrics-client'
import type { DashboardMetrics, MetricsWindow } from '@/lib/types/metrics'
import { shouldRetryMetrics } from '@/lib/dashboard/dashboard-load-state'

export const dashboardMetricsKey = (window: MetricsWindow, base: string, context: string, epoch: number) =>
  ['dashboard-metrics', window, base, context, epoch] as const

/** Windowed gateway activity metrics. Polls every 60s; pauses off-focus. */
export function useDashboardMetrics(window: MetricsWindow) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  const context = getBrowserSessionContextIdentity()
  const base = normalizeGatewayApiBase()
  return useSWR<DashboardMetrics>(
    dashboardMetricsKey(window, base, context, epoch),
    async () => {
      const metrics = await fetchDashboardMetrics(window)
      if (epoch !== getBrowserSessionEpoch() || context !== getBrowserSessionContextIdentity() || base !== normalizeGatewayApiBase()) {
        throw new DOMException('Authority or API target changed', 'AbortError')
      }
      return metrics
    },
    {
      revalidateOnFocus: false,
      refreshInterval: (data) => data ? 60_000 : 0,
      keepPreviousData: false,
      shouldRetryOnError: shouldRetryMetrics,
      onErrorRetry: (error, _key, _config, revalidate, context) => {
        if (!shouldRetryMetrics(error)) return
        if (context.retryCount >= 2) return
        setTimeout(() => revalidate({ retryCount: context.retryCount + 1 }), 2_000)
      },
    },
  )
}
