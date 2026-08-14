'use client'

import useSWR from 'swr'
import { fetchDashboardMetrics } from '@/lib/api/metrics-client'
import type { DashboardMetrics, MetricsWindow } from '@/lib/types/metrics'
import { shouldRetryMetrics } from '@/lib/dashboard/dashboard-load-state'

export const dashboardMetricsKey = (window: MetricsWindow) =>
  `/dashboard-metrics/${window}`

/** Windowed gateway activity metrics. Polls every 15s; pauses off-focus. */
export function useDashboardMetrics(window: MetricsWindow) {
  return useSWR<DashboardMetrics>(
    dashboardMetricsKey(window),
    () => fetchDashboardMetrics(window),
    {
      revalidateOnFocus: false,
      refreshInterval: (data) => data ? 15_000 : 0,
      keepPreviousData: true,
      shouldRetryOnError: shouldRetryMetrics,
      onErrorRetry: (error, _key, _config, revalidate, context) => {
        if (!shouldRetryMetrics(error)) return
        if (context.retryCount >= 2) return
        setTimeout(() => revalidate({ retryCount: context.retryCount + 1 }), 2_000)
      },
    },
  )
}
