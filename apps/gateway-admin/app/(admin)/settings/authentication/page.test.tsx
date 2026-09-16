import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { installTestDom } from '@/lib/testing/dom-install'

type SessionModule = typeof import('@/lib/auth/session-store')

async function withPage(
  sessionState: Parameters<SessionModule['__setBrowserSessionStateForTests']>[0],
  run: (view: { container: HTMLElement; unmount: () => Promise<void> }, calls: string[]) => Promise<void>,
) {
  installTestDom()
  const session: SessionModule = await import('@/lib/auth/session-store')
  const { setupApi } = await import('@/lib/api/setup-client')
  const { authAdminApi } = await import('@/lib/api/auth-admin-client')
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const calls: string[] = []
  const originals = {
    settingsSchema: setupApi.settingsSchema,
    settingsState: setupApi.settingsState,
    listAllowedEmails: authAdminApi.listAllowedEmails,
  }
  setupApi.settingsSchema = async () => {
    calls.push('settings.schema')
    return { schema_version: 1, sections: [], fields: [] }
  }
  setupApi.settingsState = async (section) => {
    calls.push(`settings.state:${section}`)
    return {
      schema_version: 1,
      config_path: '/tmp/config.toml',
      env_path: '/tmp/.env',
      section: 'authentication',
      values: {},
      sources: {},
    }
  }
  authAdminApi.listAllowedEmails = async () => {
    calls.push('auth.allowed_user.list')
    return []
  }
  session.__setBrowserSessionStateForTests(sessionState)
  const { default: AuthenticationPage } = await import('./page')
  const view = await renderClient(<AuthenticationPage />)
  try {
    await run(view, calls)
  } finally {
    await view.unmount()
    setupApi.settingsSchema = originals.settingsSchema
    setupApi.settingsState = originals.settingsState
    authAdminApi.listAllowedEmails = originals.listAllowedEmails
    session.__setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
}

async function settle() {
  const { act } = await import('react')
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 20))
  })
}

test('hides operator-only controls for a platform admin who is not a configured admin', async () => {
  await withPage(
    {
      status: 'authenticated',
      user: { sub: 'colleague', email: 'colleague@example.com' },
      expiresAt: Date.now() + 60_000,
      csrfToken: 'csrf',
      authorityState: 'ready',
      isAdmin: true,
      isConfiguredAdmin: false,
    },
    async (view, calls) => {
      await settle()
      const text = view.container.textContent ?? ''
      // The server refuses both the administrator list and the allowlist for
      // this session, so neither editor is offered and no request is made.
      assert.doesNotMatch(text, /Administrators/)
      assert.doesNotMatch(text, /Allowed users/)
      assert.match(
        view.container.querySelector('[role="alert"]')?.textContent ?? '',
        /configured administrator/i,
      )
      assert.deepEqual(calls, [])
    },
  )
})

test('shows the administrator list and allowed users to a configured admin', async () => {
  await withPage(
    {
      status: 'authenticated',
      user: { sub: 'owner', email: 'owner@example.com' },
      expiresAt: Date.now() + 60_000,
      csrfToken: 'csrf',
      authorityState: 'ready',
      isAdmin: true,
      isConfiguredAdmin: true,
    },
    async (view, calls) => {
      await settle()
      const text = view.container.textContent ?? ''
      assert.match(text, /Administrators/)
      assert.match(text, /Allowed users/)
      assert.equal(view.container.querySelector('[role="alert"]'), null)
      assert.deepEqual(
        [...calls].sort(),
        ['auth.allowed_user.list', 'settings.schema', 'settings.state:authentication'],
      )
    },
  )
})
