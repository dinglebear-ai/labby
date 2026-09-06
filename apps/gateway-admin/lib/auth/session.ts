'use client'

import { useSyncExternalStore } from 'react'

export {
  __setBrowserSessionStateForTests,
  getBrowserSessionState,
  getSessionCsrfToken,
  getSessionAuthority,
  getSessionProjectId,
  loadBrowserSession,
  logoutBrowserSession,
  subscribeToBrowserSession,
  sessionHasCapability,
  type AuthorityOwner,
  type BrowserSessionState,
  type SessionAuthority,
} from './session-store.ts'
import { getBrowserSessionState, subscribeToBrowserSession } from './session-store.ts'

export function useBrowserSession() {
  return useSyncExternalStore(
    subscribeToBrowserSession,
    getBrowserSessionState,
    getBrowserSessionState,
  )
}
