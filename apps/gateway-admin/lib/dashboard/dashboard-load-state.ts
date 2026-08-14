export type DashboardMetricsLoadState = 'loading' | 'ready' | 'unavailable' | 'error'

type MetricsError = Error & { status?: number; code?: string }

export function isMetricsUnsupported(error: unknown): boolean {
  if (!(error instanceof Error)) return false
  const candidate = error as MetricsError
  return candidate.status === 404 || candidate.code === 'unknown_action' || candidate.code === 'method_not_found'
}

export function metricsLoadState(
  data: unknown,
  error: unknown,
  isLoading: boolean,
): DashboardMetricsLoadState {
  if (data) return 'ready'
  if (isMetricsUnsupported(error)) return 'unavailable'
  if (error) return 'error'
  return isLoading ? 'loading' : 'loading'
}

export function shouldRetryMetrics(error: unknown): boolean {
  return !isMetricsUnsupported(error)
}
