import test from 'node:test'
import assert from 'node:assert/strict'

import type { BrowserSessionState } from '@/lib/auth/session-store'
import { settingsRailEntries } from './SettingsRail'

function authenticated(flags: { isAdmin?: boolean; isConfiguredAdmin?: boolean }): BrowserSessionState {
  return {
    status: 'authenticated',
    user: { sub: 'user' },
    expiresAt: 1,
    csrfToken: 'csrf',
    authorityState: 'ready',
    ...flags,
  }
}

test('settings rail offers Authentication only to a configured admin', () => {
  const labels = (session: BrowserSessionState) => settingsRailEntries(session).map((entry) => entry.label)

  assert.equal(labels({ status: 'unauthenticated' }).includes('Authentication'), false)
  assert.equal(labels(authenticated({ isAdmin: false })).includes('Authentication'), false)
  // platform.manage alone: the server refuses the page's writes and its
  // allowlist reads, so the entry is not offered.
  const platformAdmin = labels(authenticated({ isAdmin: true, isConfiguredAdmin: false }))
  assert.equal(platformAdmin.includes('Authentication'), false)
  assert.equal(platformAdmin.includes('Notifications'), true)
  assert.equal(platformAdmin.includes('Depot'), true)
  const configuredAdmin = labels(authenticated({ isAdmin: true, isConfiguredAdmin: true }))
  assert.equal(configuredAdmin.includes('Authentication'), true)
  assert.equal(configuredAdmin.includes('Notifications'), true)
  assert.equal(configuredAdmin.includes('Depot'), true)
})
