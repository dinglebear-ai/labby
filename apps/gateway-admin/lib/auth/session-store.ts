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
      is_admin: boolean
      project_id?: string | null
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
  return state.status === 'authenticated'
    ? `authenticated:${state.user.sub}:${state.isAdmin ? 'admin' : 'user'}:${state.projectId ?? 'unbound'}:${state.csrfToken}:${state.expiresAt}`
    : state.status
}

function normalizePayload(payload: SessionPayload): BrowserSessionState {
  if (!payload.authenticated) {
    return { status: 'unauthenticated' }
  }
  return {
    status: 'authenticated',
    user: payload.user,
    expiresAt: payload.expires_at,
    csrfToken: payload.csrf_token,
    isAdmin: payload.is_admin ?? false,
    projectId: payload.project_id ?? undefined,
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
