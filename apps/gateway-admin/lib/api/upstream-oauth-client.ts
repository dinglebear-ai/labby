import type { ProbeResponse, StartResponse, UpstreamEntry, UpstreamOauthStatus } from '@/lib/types/upstream-oauth'
import { normalizeGatewayApiBase } from './gateway-config'
import { gatewayHeaders } from './gateway-request'

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const base = normalizeGatewayApiBase()

  const res = await fetch(`${base}${path}`, {
    credentials: 'include',
    ...init,
    headers: { ...gatewayHeaders(), ...init?.headers },
  })
  if (!res.ok) {
    let body: { kind?: string; message?: string } = {}
    try { body = await res.json() } catch { /* ignore */ }
    throw Object.assign(new Error(body.message ?? `HTTP ${res.status}`), { kind: body.kind })
  }
  return res.json() as Promise<T>
}

export const upstreamOauthApi = {
  listUpstreams(signal?: AbortSignal): Promise<UpstreamEntry[]> {
    return apiFetch('/gateway/oauth/upstreams', { signal })
  },

  status(upstream: string, signal?: AbortSignal): Promise<UpstreamOauthStatus> {
    return apiFetch(`/gateway/oauth/status?upstream=${encodeURIComponent(upstream)}`, { signal })
  },

  start(upstream: string, signal?: AbortSignal): Promise<StartResponse> {
    return apiFetch('/gateway/oauth/start', {
      method: 'POST',
      body: JSON.stringify({ upstream }),
      signal,
    })
  },

  clear(upstream: string, signal?: AbortSignal): Promise<{ ok: boolean }> {
    return apiFetch(
      `/gateway/oauth/clear?upstream=${encodeURIComponent(upstream)}&confirm=true`,
      { method: 'POST', signal },
    )
  },

  probe(url: string, signal?: AbortSignal, upstream?: string): Promise<ProbeResponse> {
    return apiFetch('/gateway/oauth/probe', {
      method: 'POST',
      body: JSON.stringify({ url, upstream, confirm: true }),
      headers: { 'Content-Type': 'application/json' },
      signal,
    })
  },
}
