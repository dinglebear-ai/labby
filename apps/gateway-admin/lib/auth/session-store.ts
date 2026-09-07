import { invalidateAuthorityRequests } from './authority-context.ts'
import { MalformedAuthorityResponseError, authorityIdentity, parseAuthoritySnapshot, resetAuthorityOpaqueValues, selectAuthorityWorkspace, type AuthorityOwner, type AuthoritySnapshot } from './authority.ts'

export type SessionAuthority = AuthoritySnapshot
export type { AuthorityOwner, AuthoritySnapshot }

export type BrowserSessionState =
  | { status: 'loading' }
  | {
      status: 'authenticated'
      user: {
        sub: string
        email?: string | null
      }
      expiresAt: number
      csrfToken: string
      authority?: SessionAuthority
      /** Compatibility presentation flag derived only from server-projected capabilities. */
      isAdmin?: boolean
      projectId?: string
    }
  | { status: 'unauthenticated' }
  | {
      status: 'auth_error'
      kind?: string
      message: string
      requestId?: string
    }

type SessionPayload =
  | {
      authenticated: true
      user: {
        sub: string
        email?: string | null
      }
      expires_at: number
      csrf_token: string
      project_id?: string | null
      principal_id?: string | null
      active_owner?: { kind?: string; id?: string } | null
      active_team_id?: string | null
      active_project_id?: string | null
      capabilities?: unknown
      authority_generation?: number | null
      owner?: unknown
      organization_id?: unknown
      teams?: unknown
      projects?: unknown
      project?: unknown
    }
  | {
      authenticated: false
    }

type SessionErrorPayload = {
  kind?: string
  message?: string
}

let currentState: BrowserSessionState = { status: 'loading' }
export const AUTHORITY_WORKSPACE_CHANGED_EVENT = 'labby:authority-workspace-changed'
let sessionGeneration = 0
const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) {
    listener()
  }
}

function setState(next: BrowserSessionState) {
  const previousIdentity = sessionIdentity(currentState)
  const nextIdentity = sessionIdentity(next)
  if (previousIdentity !== nextIdentity) {
    sessionGeneration += 1
    invalidateAuthorityRequests(sessionGeneration)
  }
  currentState = next
  emit()
}

/**
 * The identity that decides whether a state change is an authority change.
 * Transport fields (CSRF token, expiry) are deliberately excluded: a session
 * refresh that only rotates them must neither abort in-flight requests nor
 * defeat the CSRF retry in `performServiceAction`.
 */
function sessionIdentity(state: BrowserSessionState) {
  if (state.status !== 'authenticated') return state.status
  return `authenticated:${state.user.sub}:${authorityIdentity(state.authority)}`
}

function normalizeAuthority(payload: Extract<SessionPayload, { authenticated: true }>): SessionAuthority | undefined {
  const hasProjection = payload.authority_generation !== undefined || payload.organization_id !== undefined || payload.owner !== undefined || payload.active_owner !== undefined
  return hasProjection ? parseAuthoritySnapshot(payload as unknown as Record<string, unknown>) : undefined
}

function normalizePayload(payload: SessionPayload): BrowserSessionState {
  if (!payload.authenticated) {
    return { status: 'unauthenticated' }
  }
  const authority = normalizeAuthority(payload)
  return {
    status: 'authenticated',
    user: payload.user,
    expiresAt: payload.expires_at,
    csrfToken: payload.csrf_token,
    authority,
    isAdmin: authority?.capabilities.includes('platform.manage') ?? false,
    projectId: authority?.activeProjectId,
  }
}

export function subscribeToBrowserSession(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function getBrowserSessionState() {
  return currentState
}

export function getSessionCsrfToken() {
  return currentState.status === 'authenticated' ? currentState.csrfToken : undefined
}

export function getSessionProjectId() {
  return currentState.status === 'authenticated' ? currentState.projectId : undefined
}

export function getSessionAuthority() {
  return currentState.status === 'authenticated' ? currentState.authority : undefined
}

export function sessionHasCapability(capability: string) {
  return getSessionAuthority()?.capabilities.includes(capability) ?? false
}

export function selectSessionWorkspace(selection: { teamId?: string | null; projectId?: string | null }) {
  if (currentState.status !== 'authenticated' || !currentState.authority) throw new Error('Authority is unavailable')
  const authority = selectAuthorityWorkspace(currentState.authority, selection)
  setState({ ...currentState, authority, projectId: authority.activeProjectId })
  if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent(AUTHORITY_WORKSPACE_CHANGED_EVENT))
  return authority
}

/** Authority-adjacent cache generation. Never expose the subject in cache keys. */
export function getBrowserSessionEpoch() {
  return sessionGeneration
}

export async function loadBrowserSession() {
  const generationAtStart = sessionGeneration
  let next: BrowserSessionState

  try {
    const response = await fetch('/auth/session', {
      cache: 'no-store',
      credentials: 'include',
    })

    if (response.ok) {
      const payload = (await response.json()) as SessionPayload
      next = normalizePayload(payload)
    } else if (response.status === 401 || response.status === 403) {
      next = { status: 'unauthenticated' }
    } else {
      const payload = (await response.json().catch(() => null)) as SessionErrorPayload | null
      next = {
        status: 'auth_error',
        kind: payload?.kind,
        message: payload?.message || SESSION_ERROR_MESSAGE,
        requestId: response.headers.get('x-request-id') ?? undefined,
      }
    }
  } catch (error) {
    if (error instanceof MalformedAuthorityResponseError) {
      next = { status: 'auth_error', kind: 'incompatible_authority', message: error.message }
    } else {
    next = {
      status: 'auth_error',
      kind: 'network_error',
      message: SESSION_ERROR_MESSAGE,
    }
    }
  }

  if (generationAtStart !== sessionGeneration) {
    return currentState
  }

  setState(next)
  return next
}

export class LogoutRevocationError extends Error {
  constructor(public readonly status?: number) {
    super(status === undefined
      ? 'The server could not be reached to revoke the session. Local sign-out completed; the server session may remain active until it expires.'
      : `The server did not confirm sign-out (HTTP ${status}). Local sign-out completed; the server session may remain active until it expires.`)
    this.name = 'LogoutRevocationError'
  }
}

/**
 * Sign the browser out. Local authority state is always cleared, even when the
 * server-side revocation fails: a browser that keeps rendering a workspace it
 * asked to leave is the worse failure. A failed revocation is still reported
 * to the caller as `LogoutRevocationError` so it can be surfaced separately.
 */
export async function logoutBrowserSession() {
  const csrfToken = getSessionCsrfToken()
  let failure: LogoutRevocationError | undefined
  try {
    const response = await fetch('/auth/logout', {
      method: 'POST',
      cache: 'no-store',
      credentials: 'include',
      headers: csrfToken
        ? {
            'x-csrf-token': csrfToken,
          }
        : undefined,
    })
    if (!response.ok) failure = new LogoutRevocationError(response.status)
  } catch {
    failure = new LogoutRevocationError()
  } finally {
    sessionGeneration += 1
    resetAuthorityOpaqueValues()
    setState({ status: 'unauthenticated' })
  }
  if (failure) throw failure
}

export function __setBrowserSessionStateForTests(state: BrowserSessionState) {
  sessionGeneration += 1
  currentState = state
}
const SESSION_ERROR_MESSAGE = 'Unable to reach the authentication service. Try again.'
