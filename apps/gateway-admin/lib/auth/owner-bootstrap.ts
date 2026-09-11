import { getSessionCsrfToken, loadBrowserSession } from './session-store.ts'

export type OwnerBootstrapInput = {
  organizationName: string
  projectName: string
}

export type OwnerBootstrapOutcome = 'created' | 'already_applied'

/**
 * Gateway statuses from a reverse proxy or CDN edge that never reached Labby
 * (for example Cloudflare's 522 origin timeout). Owner bootstrap is idempotent,
 * so a repeat returns `already_applied` rather than creating anything twice.
 */
const EDGE_RETRY_STATUSES = new Set([502, 503, 504, 520, 521, 522, 523, 524])
const DEFAULT_RETRY_DELAYS_MS: readonly number[] = [1000, 2500]

async function postWithEdgeRetries(init: RequestInit, retryDelaysMs: readonly number[]): Promise<Response> {
  for (let attempt = 0; ; attempt += 1) {
    const canRetry = attempt < retryDelaysMs.length
    try {
      const response = await fetch('/v1/access/bootstrap-owner', init)
      if (!canRetry || !EDGE_RETRY_STATUSES.has(response.status)) return response
    } catch (error) {
      if (!canRetry) throw error
    }
    await new Promise((resolve) => setTimeout(resolve, retryDelaysMs[attempt]))
  }
}

export class OwnerBootstrapError extends Error {
  readonly status: number
  readonly kind?: string

  constructor(status: number, message: string, kind?: string) {
    super(message)
    this.name = 'OwnerBootstrapError'
    this.status = status
    this.kind = kind
  }
}

/**
 * Browser client for `POST /v1/access/bootstrap-owner`. The server derives the
 * owner identity from the session and enforces every eligibility gate; this
 * only sends the two display names. On success the browser session is
 * reloaded so the new durable authority replaces the pending state.
 */
export async function bootstrapOwner(
  input: OwnerBootstrapInput,
  options: { retryDelaysMs?: readonly number[] } = {},
): Promise<OwnerBootstrapOutcome> {
  const csrfToken = getSessionCsrfToken()
  if (!csrfToken) {
    throw new OwnerBootstrapError(0, 'Your browser session has no CSRF token. Sign in again.', 'auth_failed')
  }
  const response = await postWithEdgeRetries(
    {
      method: 'POST',
      credentials: 'include',
      cache: 'no-store',
      headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken },
      body: JSON.stringify({
        organization_name: input.organizationName.trim(),
        project_name: input.projectName.trim(),
      }),
    },
    options.retryDelaysMs ?? DEFAULT_RETRY_DELAYS_MS,
  )
  const payload = (await response.json().catch(() => null)) as
    | { status?: unknown; kind?: unknown; message?: unknown }
    | null
  if (!response.ok) {
    const kind = typeof payload?.kind === 'string' ? payload.kind : undefined
    const message =
      typeof payload?.message === 'string' && payload.message
        ? payload.message
        : `Owner bootstrap failed (HTTP ${response.status}).`
    if (response.status === 409) {
      // A conflict still opens the access store, and this identity may already
      // hold owner authority through another linked identity. Reload so a
      // session that is now ready replaces the setup screen.
      await loadBrowserSession()
    }
    throw new OwnerBootstrapError(response.status, message, kind)
  }
  if (payload?.status !== 'created' && payload?.status !== 'already_applied') {
    throw new OwnerBootstrapError(response.status, 'Owner bootstrap returned an unexpected response.')
  }
  await loadBrowserSession()
  return payload.status
}

/** Actionable copy for the owner setup screen; falls back to the server message. */
export function describeOwnerBootstrapError(error: unknown): string {
  if (!(error instanceof OwnerBootstrapError)) {
    return 'Labby could not reach the server to complete owner bootstrap. Try again.'
  }
  if (EDGE_RETRY_STATUSES.has(error.status)) {
    return (
      `Labby's public address could not reach the server (HTTP ${error.status}). ` +
      'This is a network problem between the edge proxy and your server, not your input. ' +
      'Try again; repeating owner bootstrap is safe.'
    )
  }
  switch (error.kind) {
    case 'conflict':
      return (
        'This Labby already has an owner with different setup names or a different sign-in identity. ' +
        'Enter the organization and project names from the original setup, signed in as the original owner.'
      )
    case 'forbidden':
      return 'Only the configured Labby admin account can complete owner bootstrap. Sign in with that account.'
    case 'validation_failed':
      return 'Enter an organization name and a project name of 128 characters or fewer.'
    default:
      return error.message
  }
}
