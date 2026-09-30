import { normalizeGatewayApiBase } from './gateway-config.ts'
import { gatewayHeaders } from './gateway-request.ts'

export type AllowedEmailRole = 'member' | 'admin'

export interface AllowedEmailEntry {
  email: string
  added_by: string
  created_at: string
  role: AllowedEmailRole
}

export class AuthAdminApiError extends Error {
  status: number
  kind?: string

  constructor(message: string, status: number, kind?: string) {
    super(message)
    this.name = 'AuthAdminApiError'
    this.status = status
    this.kind = kind
  }
}

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const base = normalizeGatewayApiBase()
  const res = await fetch(`${base}${path}`, {
    credentials: 'include',
    ...init,
    headers: { ...gatewayHeaders(), ...init?.headers },
  })

  if (res.status === 204) {
    return undefined as T
  }

  if (!res.ok) {
    let body: { kind?: string; message?: string } = {}
    try {
      body = (await res.json()) as { kind?: string; message?: string }
    } catch {
      /* ignore parse errors */
    }
    throw new AuthAdminApiError(
      body.message ?? `HTTP ${res.status}`,
      res.status,
      body.kind,
    )
  }

  return res.json() as Promise<T>
}

export const authAdminApi = {
  async listAllowedEmails(signal?: AbortSignal): Promise<AllowedEmailEntry[]> {
    const data = await apiFetch<{ entries: AllowedEmailEntry[] }>(
      '/auth/allowed-emails',
      { signal },
    )
    return data.entries
  },

  async addAllowedEmail(
    email: string,
    role: AllowedEmailRole,
    signal?: AbortSignal,
  ): Promise<AllowedEmailEntry> {
    const data = await apiFetch<{ entry: AllowedEmailEntry }>(
      '/auth/allowed-emails',
      {
        method: 'POST',
        body: JSON.stringify({ email, role }),
        signal,
      },
    )
    return data.entry
  },

  async removeAllowedEmail(email: string, signal?: AbortSignal): Promise<void> {
    await apiFetch<undefined>(
      `/auth/allowed-emails/${encodeURIComponent(email)}`,
      { method: 'DELETE', signal },
    )
  },
}
