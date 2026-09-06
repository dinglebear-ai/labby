export type AuthorityOwner =
  | { kind: 'installation'; id: string }
  | { kind: 'team'; id: string }
  | { kind: 'project'; id: string }
  | { kind: 'personal'; id: string }

export type SessionAuthority = {
  principalId: string
  activeOwner: AuthorityOwner
  activeTeamId?: string
  activeProjectId?: string
  capabilities: readonly string[]
  generation: number
}

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
    }
  | {
      authenticated: false
    }

type SessionErrorPayload = {
  kind?: string
  message?: string
}

let currentState: BrowserSessionState = { status: 'loading' }
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
  if (previousIdentity !== nextIdentity) sessionGeneration += 1
  currentState = next
  emit()
}

function sessionIdentity(state: BrowserSessionState) {
  if (state.status !== 'authenticated') return state.status
  const authority = state.authority
  const authorityIdentity = authority
    ? [
        authority.principalId,
        authority.activeOwner.kind,
        authority.activeOwner.id,
        authority.activeTeamId ?? '',
        authority.activeProjectId ?? '',
        [...authority.capabilities].sort().join(','),
        authority.generation,
      ].join(':')
    : 'authority-unavailable'
  return `authenticated:${state.user.sub}:${authorityIdentity}:${state.csrfToken}:${state.expiresAt}`
}

function normalizeAuthority(payload: Extract<SessionPayload, { authenticated: true }>): SessionAuthority | undefined {
  const principalId = nonEmpty(payload.principal_id)
  const generation = payload.authority_generation
  if (!principalId || !Number.isSafeInteger(generation) || Number(generation) < 0) return undefined

  const capabilities = Array.isArray(payload.capabilities)
    ? [...new Set(payload.capabilities.filter((value): value is string => nonEmpty(value) !== undefined))].sort()
    : []
  const projectedOwner = payload.active_owner
  const kind = projectedOwner?.kind
  const id = nonEmpty(projectedOwner?.id)
  const activeOwner = id && isOwnerKind(kind)
    ? { kind, id } as AuthorityOwner
    : { kind: 'personal' as const, id: principalId }

  return {
    principalId,
    activeOwner,
    activeTeamId: nonEmpty(payload.active_team_id),
    activeProjectId: nonEmpty(payload.active_project_id ?? payload.project_id),
    capabilities,
    generation: Number(generation),
  }
}

function nonEmpty(value: unknown) {
  return typeof value === 'string' && value.trim() ? value : undefined
}

function isOwnerKind(value: unknown): value is AuthorityOwner['kind'] {
  return value === 'installation' || value === 'team' || value === 'project' || value === 'personal'
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
  } catch {
    next = {
      status: 'auth_error',
      kind: 'network_error',
      message: SESSION_ERROR_MESSAGE,
    }
  }

  if (generationAtStart !== sessionGeneration) {
    return currentState
  }

  setState(next)
  return next
}

export async function logoutBrowserSession() {
  const csrfToken = getSessionCsrfToken()
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

  if (!response.ok) {
    throw new Error('Failed to logout browser session')
  }

  sessionGeneration += 1
  setState({ status: 'unauthenticated' })
}

export function __setBrowserSessionStateForTests(state: BrowserSessionState) {
  sessionGeneration += 1
  currentState = state
}
const SESSION_ERROR_MESSAGE = 'Unable to reach the authentication service. Try again.'
