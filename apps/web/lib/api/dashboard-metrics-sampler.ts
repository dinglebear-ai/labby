import { getBrowserSessionContextIdentity, getBrowserSessionEpoch } from '../auth/session-store.ts'
import { normalizeGatewayApiBase } from './gateway-config.ts'
import { queryServerLogs } from './server-logs-client.ts'
import {
  fetchDashboardMetrics,
  type DashboardLogObservation,
  type MetricsRequestOptions,
} from './metrics-client.ts'
import type { DashboardMetrics, MetricsWindow } from '../types/metrics.ts'

const LOG_REFRESH_MS = 60_000
const LOG_QUERY = { limit: 500, max_scan_bytes: 2 * 1024 * 1024, stop_after_limit: true }

/** Create one sampler per mounted hook. No cache or in-flight request is shared
 * across hooks, authorities, API bases, or bearer modes. */
export function createDashboardMetricsSampler() {
  // Credential equality is kept private to this hook instance. It is never
  // serialized into a cache key, logged, or shared across callers.
  let context: { epoch: number; identity: string; base: string; standalone: boolean; token?: string } | null = null
  let generation = 0
  let logs: DashboardLogObservation | null = null

  function clear() {
    generation += 1
    context = null
    logs = null
  }

  async function fetch(window: MetricsWindow, options?: MetricsRequestOptions): Promise<DashboardMetrics> {
    const next = {
      epoch: getBrowserSessionEpoch(),
      identity: getBrowserSessionContextIdentity(),
      base: normalizeGatewayApiBase(options?.baseUrl),
      standalone: options?.standaloneBearerAuth === true,
      token: options?.token,
    }
    if (!context || context.epoch !== next.epoch || context.identity !== next.identity || context.base !== next.base || context.standalone !== next.standalone || context.token !== next.token) {
      clear()
      context = next
    }
    const requestGeneration = generation
    const reusable = !options?.token && !next.standalone
    let observation = reusable && logs && Date.now() - logs.observedAt < LOG_REFRESH_MS ? logs : null
    // The log endpoint uses the browser session, not an explicit bearer. Never
    // combine that authority's observations with standalone bearer analytics.
    let error: string | undefined = next.standalone ? 'retained logs are unavailable in standalone bearer mode' : undefined
    if (!observation && !next.standalone) {
      try {
        const result = await queryServerLogs(LOG_QUERY, { baseUrl: options?.baseUrl, signal: options?.signal })
        observation = { observedAt: Date.now(), entries: result.entries, truncated: result.truncated }
        if (generation === requestGeneration && getBrowserSessionEpoch() === next.epoch && getBrowserSessionContextIdentity() === next.identity && reusable && !options?.signal?.aborted) {
          if (!logs || observation.observedAt >= logs.observedAt) logs = observation
        }
      } catch (cause) {
        if (options?.signal?.aborted) throw cause
        error = cause instanceof Error ? cause.message : 'server log query failed'
      }
    }
    if (generation !== requestGeneration || getBrowserSessionEpoch() !== next.epoch || getBrowserSessionContextIdentity() !== next.identity || options?.signal?.aborted) {
      throw new DOMException('Metrics authority or request changed', 'AbortError')
    }
    const result = await fetchDashboardMetrics(window, options, { logs: observation, error })
    if (generation !== requestGeneration || getBrowserSessionEpoch() !== next.epoch || getBrowserSessionContextIdentity() !== next.identity || options?.signal?.aborted) {
      throw new DOMException('Metrics authority or request changed', 'AbortError')
    }
    return result
  }

  return { fetch, clear }
}
