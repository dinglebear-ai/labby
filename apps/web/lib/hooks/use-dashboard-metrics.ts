'use client'

import useSWR from 'swr'
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { getBrowserSessionContextIdentity, getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { fetchDashboardChangeToken, MetricsApiError } from '@/lib/api/metrics-client'
import { createDashboardMetricsSampler } from '@/lib/api/dashboard-metrics-sampler'
import type { DashboardMetrics, MetricsWindow } from '@/lib/types/metrics'

export const dashboardMetricsKey = (window: MetricsWindow, base: string, context: string, epoch: number) =>
  ['dashboard-metrics', window, base, context, epoch] as const

const aggregateInterval: Record<MetricsWindow, number> = { '1h': 10_000, '24h': 30_000, '7d': 60_000, '30d': 60_000 }
const isActive = () => typeof document === 'undefined' || (document.visibilityState !== 'hidden' && navigator.onLine !== false)
const canRetryTelemetry = (error: unknown) => {
  if (!(error instanceof Error) || error.name === 'AbortError') return false
  const status = (error as Error & { status?: number }).status
  return status === undefined || status === 408 || status === 429 || status >= 500
}
const metricsSubscribers = new Map<string, number>()
const metricsRequests = new Map<string, Set<AbortController>>()

/** One sampler and one change-signal loop for the Overview's sole metrics consumer. */
export function useDashboardMetrics(window: MetricsWindow) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => 0)
  const context = getBrowserSessionContextIdentity()
  const base = normalizeGatewayApiBase()
  const scope = JSON.stringify([base, context, epoch])
  const authority = JSON.stringify([base, context, epoch, window])
  const sampler = useRef<{ scope: string; value: ReturnType<typeof createDashboardMetricsSampler> } | null>(null)
  if (sampler.current?.scope !== scope) sampler.current = { scope, value: createDashboardMetricsSampler() }
  const currentSampler = sampler.current.value
  const mounted = useRef(false)
  const flights = useRef(new Map<string, Promise<DashboardMetrics>>())
  const lastSuccess = useRef({ authority: '', at: 0 })
  const sample = useRef<{ authority: string; value: DashboardMetrics } | null>(null)
  const applied = useRef({ authority: '', token: '' })
  const running = useRef(false)
  const retry = useRef({ failures: 0, after: 0 })
  const permanentFailure = useRef<{ authority: string; error: unknown } | null>(null)
  const currentAuthority = useRef(authority)
  if (currentAuthority.current !== authority) {
    currentAuthority.current = authority
    permanentFailure.current = null
    running.current = false
    retry.current = { failures: 0, after: 0 }
  }
  const [tick, setTick] = useState(0)
  const [unsupported, setUnsupported] = useState<{ authority: string } | null>(null)
  const [signalDelay, setSignalDelay] = useState(5_000)
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])

  const request = useCallback(async <T,>(key: string, fetch: (signal: AbortSignal) => Promise<T>): Promise<T> => {
    const controller = new AbortController()
    const owned = metricsRequests.get(key) ?? new Set<AbortController>()
    owned.add(controller)
    metricsRequests.set(key, owned)
    try {
      const result = await fetch(controller.signal)
      if (controller.signal.aborted || epoch !== getBrowserSessionEpoch() || context !== getBrowserSessionContextIdentity() || base !== normalizeGatewayApiBase()) {
        throw new DOMException('Metrics authority or API target changed', 'AbortError')
      }
      return result
    } finally {
      owned.delete(controller)
      if (!owned.size && metricsRequests.get(key) === owned) metricsRequests.delete(key)
    }
  }, [base, context, epoch])

  useEffect(() => {
    const ownedFlights = flights.current
    metricsSubscribers.set(authority, (metricsSubscribers.get(authority) ?? 0) + 1)
    return () => {
      const remaining = (metricsSubscribers.get(authority) ?? 1) - 1
      if (remaining > 0) metricsSubscribers.set(authority, remaining)
      else {
        metricsSubscribers.delete(authority)
        // React replays effects synchronously in StrictMode. Defer final-owner
        // cleanup one microtask so the replay can reclaim the same request.
        queueMicrotask(() => {
          if (metricsSubscribers.has(authority)) return
          for (const controller of metricsRequests.get(authority) ?? []) controller.abort()
          metricsRequests.delete(authority)
          if (!mounted.current || sampler.current?.scope !== scope) currentSampler.clear()
          ownedFlights.delete(authority)
        })
      }
    }
  }, [authority, currentSampler, scope])

  const loadMetrics = useCallback(() => {
    if (permanentFailure.current?.authority === authority) return Promise.reject(permanentFailure.current.error)
    const existing = flights.current.get(authority)
    if (existing) return existing
    if (sample.current?.authority === authority && Date.now() - lastSuccess.current.at < aggregateInterval[window]) {
      return Promise.resolve(sample.current.value)
    }
    const flight = request(authority, signal => currentSampler.fetch(window, { baseUrl: base, signal }))
      .then(result => {
        lastSuccess.current = { authority, at: Date.now() }
        sample.current = { authority, value: result }
        return result
      }).catch(error => {
        if (currentAuthority.current === authority && !canRetryTelemetry(error) && error?.name !== 'AbortError') permanentFailure.current = { authority, error }
        throw error
      }).finally(() => {
        if (flights.current.get(authority) === flight) flights.current.delete(authority)
      })
    flights.current.set(authority, flight)
    return flight
  }, [authority, base, currentSampler, request, window])

  const metrics = useSWR<DashboardMetrics>(
    dashboardMetricsKey(window, base, context, epoch),
    loadMetrics,
    {
      revalidateOnFocus: false,
      revalidateOnReconnect: false,
      refreshInterval: 60_000, // Window aging and recovery after an initial failure.
      refreshWhenHidden: false,
      refreshWhenOffline: false,
      isPaused: () => !isActive(),
      keepPreviousData: false,
      shouldRetryOnError: false,
    },
  )

  const token = useSWR(
    unsupported?.authority === authority ? null : ['dashboard-change', window, base, context, epoch],
    async () => request(authority, signal => fetchDashboardChangeToken({ baseUrl: base, signal })),
    {
      refreshInterval: signalDelay,
      refreshWhenHidden: false,
      refreshWhenOffline: false,
      isPaused: () => !isActive(),
      revalidateOnFocus: false,
      revalidateOnReconnect: false,
      shouldRetryOnError: false,
      onSuccess: () => setSignalDelay(5_000),
      onError: error => {
        if (error instanceof MetricsApiError && (error.code === 'usage_change_token_unsupported' || error.status === 404)) setUnsupported({ authority })
        else setSignalDelay(delay => Math.min(delay * 2, 30_000))
      },
    },
  )

  const currentToken = token.data ? JSON.stringify([token.data.latestCallId, token.data.tsUnix]) : null
  const { data, isValidating, mutate, error: metricsError } = metrics
  const refreshToken = token.mutate
  const signalError = token.error

  // SWR skips interval revalidation while a cached error exists. Separate,
  // bounded retries are needed even before the first successful sample.
  useEffect(() => {
    if (!canRetryTelemetry(metricsError)) return
    const timer = globalThis.window.setTimeout(() => {
      if (mounted.current && currentAuthority.current === authority && isActive()) void mutate().catch(() => {})
    }, 60_000)
    return () => globalThis.window.clearTimeout(timer)
  }, [authority, metricsError, mutate])
  useEffect(() => {
    if (unsupported?.authority === authority || !canRetryTelemetry(signalError)) return
    const timer = globalThis.window.setTimeout(() => {
      if (mounted.current && currentAuthority.current === authority && isActive()) void refreshToken().catch(() => {})
    }, signalDelay)
    return () => globalThis.window.clearTimeout(timer)
  }, [authority, refreshToken, signalDelay, signalError, unsupported])

  useEffect(() => {
    if (permanentFailure.current?.authority === authority) return
    if (!currentToken || !data || isValidating || running.current || !isActive()) return
    if (applied.current.authority === authority && applied.current.token === currentToken) return
    const due = Math.max(
      lastSuccess.current.authority === authority ? lastSuccess.current.at + aggregateInterval[window] : 0,
      retry.current.after,
    )
    if (Date.now() < due) {
      const timer = globalThis.window.setTimeout(() => setTick(value => value + 1), due - Date.now())
      return () => globalThis.window.clearTimeout(timer)
    }
    running.current = true
    // Acknowledge only the token captured before a successful aggregate. A
    // newer row seen during a slow fetch stays dirty; failures retain the token.
    void mutate(() => loadMetrics(), { revalidate: false, throwOnError: true }).then(() => {
      if (currentAuthority.current !== authority) return
      applied.current = { authority, token: currentToken }
      retry.current = { failures: 0, after: 0 }
    }).catch(error => {
      if (currentAuthority.current !== authority || !canRetryTelemetry(error)) return
      const failures = Math.min(retry.current.failures + 1, 5)
      retry.current = { failures, after: Date.now() + Math.min(2_000 * 2 ** (failures - 1), 30_000) }
    }).finally(() => {
      if (!mounted.current || currentAuthority.current !== authority) return
      running.current = false
      setTick(value => value + 1)
    })
  }, [authority, currentToken, data, isValidating, loadMetrics, mutate, tick, window])

  useEffect(() => {
    const catchUp = () => {
      if (!isActive()) return
      if (unsupported?.authority !== authority) void refreshToken().catch(() => {})
      if (lastSuccess.current.authority !== authority || Date.now() - lastSuccess.current.at >= 60_000) void mutate().catch(() => {})
      setTick(value => value + 1)
    }
    document.addEventListener('visibilitychange', catchUp)
    globalThis.window.addEventListener('online', catchUp)
    return () => {
      document.removeEventListener('visibilitychange', catchUp)
      globalThis.window.removeEventListener('online', catchUp)
    }
  }, [authority, mutate, refreshToken, unsupported])

  const refresh = useCallback(() => {
    // Explicit operator refresh bypasses the automatic aggregate throttle,
    // but still joins an existing request and preserves the bounded log cache.
    sample.current = null
    permanentFailure.current = null
    retry.current = { failures: 0, after: 0 }
    return mutate()
  }, [mutate])

  return { ...metrics, refresh }
}
