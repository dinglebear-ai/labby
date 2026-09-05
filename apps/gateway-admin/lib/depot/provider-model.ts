import { getBrowserSessionEpoch } from '../auth/session-store.ts'

export const artifactKey = (providerId: string, artifactId: string) => JSON.stringify([providerId, artifactId])
export function parseArtifactKey(value: string): [string, string] | null {
  try {
    const parsed: unknown = JSON.parse(value)
    return Array.isArray(parsed) && parsed.length === 2 && parsed.every(item => typeof item === 'string') ? parsed as [string, string] : null
  } catch { return null }
}

export function discoveryUrl(input: { provider?: string; query?: string; artifactProvider?: string; artifact?: string }) {
  const params = new URLSearchParams()
  if (input.provider) params.set('provider', input.provider)
  if (input.query) params.set('query', input.query)
  if (input.artifactProvider !== undefined && input.artifact !== undefined) {
    params.set('artifactProvider', input.artifactProvider)
    params.set('artifact', input.artifact)
  }
  return params.toString()
}

let requestToken = 0
export function newDiscoveryContext() { return { token: ++requestToken, sessionEpoch: getBrowserSessionEpoch() } }
export function isCurrentDiscoveryContext(current: { token: number; sessionEpoch: number }, candidate: { token: number; sessionEpoch: number }) {
  return current.token === candidate.token && candidate.sessionEpoch === getBrowserSessionEpoch()
}
