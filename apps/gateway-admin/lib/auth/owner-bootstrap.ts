import { getSessionCsrfToken, loadBrowserSession } from './session-store.ts'

export type OwnerBootstrapInput = {
  organizationName: string
  projectName: string
}

export type OwnerBootstrapOutcome = 'created' | 'already_applied'

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
export async function bootstrapOwner(input: OwnerBootstrapInput): Promise<OwnerBootstrapOutcome> {
  const csrfToken = getSessionCsrfToken()
  if (!csrfToken) {
    throw new OwnerBootstrapError(0, 'Your browser session has no CSRF token. Sign in again.', 'auth_failed')
  }
  const response = await fetch('/v1/access/bootstrap-owner', {
    method: 'POST',
    credentials: 'include',
    cache: 'no-store',
    headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken },
    body: JSON.stringify({
      organization_name: input.organizationName.trim(),
      project_name: input.projectName.trim(),
    }),
  })
  const payload = (await response.json().catch(() => null)) as
    | { status?: unknown; kind?: unknown; message?: unknown }
    | null
  if (!response.ok) {
    const kind = typeof payload?.kind === 'string' ? payload.kind : undefined
    const message =
      typeof payload?.message === 'string' && payload.message
        ? payload.message
        : `Owner bootstrap failed (HTTP ${response.status}).`
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
