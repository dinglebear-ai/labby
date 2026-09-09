import type { AuthoritySnapshot } from './authority.ts'
import { authorityCacheKey } from './authority.ts'

/**
 * In-flight request controllers keyed by the browser-session generation they
 * were started under. `invalidateAuthorityRequests` aborts and drops every
 * bucket except the current generation, so this map never holds more than
 * one live generation plus whatever a caller started before its invalidation
 * ran.
 */
const controllers = new Map<number, Set<AbortController>>()

export function beginAuthorityRequest(snapshot: AuthoritySnapshot, contextGeneration: number, connectionId = 'local', callerSignal?: AbortSignal) {
  const controller = new AbortController()
  const generation = contextGeneration
  let bucket = controllers.get(generation)
  if (!bucket) { bucket = new Set(); controllers.set(generation, bucket) }
  bucket.add(controller)
  if (callerSignal) {
    if (callerSignal.aborted) controller.abort(callerSignal.reason)
    else callerSignal.addEventListener('abort', () => controller.abort(callerSignal.reason), { once: true })
  }
  const finish = () => {
    bucket?.delete(controller)
    if (bucket?.size === 0 && controllers.get(generation) === bucket) controllers.delete(generation)
  }
  return { generation, cacheKey: authorityCacheKey(snapshot, connectionId), signal: controller.signal, finish }
}

export function invalidateAuthorityRequests(currentGeneration: number) {
  for (const [generation, bucket] of controllers) {
    if (generation !== currentGeneration) {
      for (const controller of bucket) controller.abort(new DOMException('Authority context changed', 'AbortError'))
      controllers.delete(generation)
    }
  }
}

export function __resetAuthorityContextForTests() {
  for (const bucket of controllers.values()) for (const controller of bucket) controller.abort()
  controllers.clear()
}

export function __authorityContextStatsForTests() {
  let inFlight = 0
  for (const bucket of controllers.values()) inFlight += bucket.size
  return { activeGenerations: controllers.size, inFlight }
}
