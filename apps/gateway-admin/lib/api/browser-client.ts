import { browserActionUrl } from './gateway-config'
import { performServiceAction, type ServiceActionError } from './service-action-client'
import type {
  BrowserIdentity,
  BrowserListResponse,
  BrowserPairingListResponse,
  BrowserSession,
  BrowserSessionListResponse,
  BrowserStatusResponse,
} from '@/lib/types/browser'

export class BrowserApiError extends Error implements ServiceActionError {
  status: number
  code?: string
  param?: string

  constructor(message: string, status: number, code?: string, param?: string) {
    super(message)
    this.name = 'BrowserApiError'
    this.status = status
    this.code = code
    this.param = param
  }
}

async function browserAction<T>(action: string, params: object, signal?: AbortSignal): Promise<T> {
  return performServiceAction<T, BrowserApiError>({
    action,
    params,
    signal,
    serviceLabel: 'Browser bridge',
    url: browserActionUrl(),
    createError: (message, status, code, param) => new BrowserApiError(message, status, code, param),
  })
}

export const browserApi = {
  status: (signal?: AbortSignal) => browserAction<BrowserStatusResponse>('browser.status', {}, signal),
  async list(signal?: AbortSignal) {
    return (await browserAction<BrowserListResponse>('browser.list', {}, signal)).browsers
  },
  async pairings(signal?: AbortSignal) {
    return (await browserAction<BrowserPairingListResponse>('browser.pairing.list', {}, signal)).pairings
  },
  approvePairing: (pairingId: string, signal?: AbortSignal) =>
    browserAction<BrowserIdentity>('browser.pairing.approve', { pairing_id: pairingId }, signal),
  revoke: (browserId: string, signal?: AbortSignal) =>
    browserAction<BrowserIdentity>('browser.revoke', { browser_id: browserId }, signal),
  async sessions(signal?: AbortSignal, cursor?: string) {
    const page = await browserAction<BrowserSessionListResponse>('browser.sessions', cursor ? { cursor } : {}, signal)
    const sessions: BrowserSession[] = []
    // Lists intentionally omit catalogs. Fetch the reviewed detail in bounded batches.
    for (let offset = 0; offset < page.sessions.length; offset += 4) {
      const details = await Promise.all(page.sessions.slice(offset, offset + 4).map((session) =>
        browserAction<BrowserSession>('browser.session.get', { session_id: session.id }, signal)))
      sessions.push(...details)
    }
    return { sessions, next_cursor: page.next_cursor ?? null }
  },
  setSessionEnabled: (sessionId: string, enabled: boolean, catalogDigest: string, signal?: AbortSignal) =>
    browserAction<BrowserSession>('browser.session.enable', { session_id: sessionId, enabled, catalog_digest: catalogDigest }, signal),
}