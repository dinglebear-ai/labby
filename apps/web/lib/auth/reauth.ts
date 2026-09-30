import { getBrowserSessionEpoch } from './session-store.ts'

export interface ReauthPurpose {
  action: string
  resource: string
  version: string
  operation: string
  scope: string
  payload: unknown
}

export interface ReauthStarted {
  authorizationUrl: string
  interaction: string
  expiresAt: number
}

export type ReauthPoll =
  | { status: 'Pending' }
  | { status: 'Completed'; proof: string }
  | { status: 'Expired' }

async function errorMessage(response: Response) {
  const body = await response.json().catch(() => null) as { message?: unknown } | null
  return typeof body?.message === 'string' ? body.message : `Reauthentication failed (${response.status})`
}

export async function startReauth(
  purpose: ReauthPurpose,
  csrfToken: string,
  fetcher: typeof fetch = fetch,
): Promise<ReauthStarted> {
  const response = await fetcher('/auth/reauth', {
    method: 'POST', cache: 'no-store', credentials: 'include',
    headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken },
    body: JSON.stringify(purpose),
  })
  if (!response.ok) throw new Error(await errorMessage(response))
  return await response.json() as ReauthStarted
}

export async function pollReauth(interaction: string, fetcher: typeof fetch = fetch): Promise<ReauthPoll> {
  const response = await fetcher(`/auth/reauth/${encodeURIComponent(interaction)}`, {
    cache: 'no-store', credentials: 'include',
  })
  if (!response.ok) throw new Error(await errorMessage(response))
  return await response.json() as ReauthPoll
}

export async function cancelReauth(
  interaction: string,
  csrfToken: string,
  fetcher: typeof fetch = fetch,
) {
  const response = await fetcher(`/auth/reauth/${encodeURIComponent(interaction)}`, {
    method: 'DELETE', cache: 'no-store', credentials: 'include',
    headers: { 'x-csrf-token': csrfToken },
  })
  if (!response.ok && response.status !== 404) throw new Error(await errorMessage(response))
}

export async function waitForReauthProof(
  interaction: string,
  initialEpoch = getBrowserSessionEpoch(),
  dependencies: {
    fetcher?: typeof fetch
    epoch?: () => number
    delay?: () => Promise<void>
    attempts?: number
  } = {},
) {
  const epoch = dependencies.epoch ?? getBrowserSessionEpoch
  const delay = dependencies.delay ?? (() => new Promise<void>(resolve => setTimeout(resolve, 750)))
  for (let attempt = 0; attempt < (dependencies.attempts ?? 400); attempt += 1) {
    if (epoch() !== initialEpoch) throw new Error('Browser session changed during reauthentication')
    const result = await pollReauth(interaction, dependencies.fetcher)
    if (result.status === 'Completed') return result.proof
    if (result.status === 'Expired') throw new Error('Reauthentication expired. Try again.')
    await delay()
  }
  throw new Error('Reauthentication timed out. Try again.')
}
