'use client'

import { useSyncExternalStore } from 'react'

export {
  __setBrowserSessionStateForTests,
  getBrowserSessionState,
  getSessionCsrfToken,
  getSessionProjectId,
  loadBrowserSession,
  logoutBrowserSession,
  subscribeToBrowserSession,
  type BrowserSessionState,
} from './session-store.ts'
import { getBrowserSessionState, subscribeToBrowserSession } from './session-store.ts'

export function useBrowserSession() {
  return useSyncExternalStore(
    subscribeToBrowserSession,
    getBrowserSessionState,
    getBrowserSessionState,
  )
}
