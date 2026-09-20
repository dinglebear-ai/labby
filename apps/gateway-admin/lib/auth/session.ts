'use client'

import { useSyncExternalStore } from 'react'

export {
  __setBrowserSessionStateForTests,
  AUTHORITY_WORKSPACE_CHANGED_EVENT,
  LogoutRevocationError,
  exchangeBearerBrowserSession,
  getBrowserSessionContextIdentity,
  getBrowserSessionState,
  getProjectBoundSessionScope,
  getSessionCsrfToken,
  getSessionAuthority,
  getSessionProjectId,
  isProjectBoundSession,
  loadBrowserSession,
  logoutBrowserSession,
  subscribeToBrowserSession,
  sessionHasCapability,
  selectSessionWorkspace,
  type AuthorityOwner,
  type BrowserSessionState,
  type ProjectBoundSession,
  type SessionAuthority,
  type SessionAuthorityState,
} from './session-store.ts'
export {
  AUTHORITY_COMPATIBILITY_GENERATION,
  AUTHORITY_SCHEMA_VERSION,
  MalformedAuthorityResponseError,
  WorkspaceSelectionError,
  authorityCacheKey,
  authorityIdentity,
  parseAuthoritySnapshot,
  selectAuthorityWorkspace,
  type AuthorityCacheKey,
  type AuthorityProject,
  type AuthoritySnapshot,
  type AuthorityTeam,
} from './authority.ts'
import { getBrowserSessionState, getProjectBoundSessionScope, subscribeToBrowserSession } from './session-store.ts'

export function useBrowserSession() {
  return useSyncExternalStore(
    subscribeToBrowserSession,
    getBrowserSessionState,
    getBrowserSessionState,
  )
}

/**
 * The project-bound session scope, or `''` while the session has no project.
 * Re-renders only when that scope changes, unlike `useBrowserSession`, which
 * re-renders on every session emit.
 */
export function useProjectBoundSessionScope() {
  return useSyncExternalStore(
    subscribeToBrowserSession,
    getProjectBoundSessionScope,
    getProjectBoundSessionScope,
  )
}
