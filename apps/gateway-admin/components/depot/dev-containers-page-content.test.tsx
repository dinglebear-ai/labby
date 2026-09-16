import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'

installTestDom()

const originalFetch = globalThis.fetch
const authority = (principalId: string) => ({
  schemaVersion: 1 as const,
  compatibilityGeneration: 1 as const,
  principalId,
  organizationId: 'org',
  activeOwner: { kind: 'personal' as const, id: principalId },
  teams: [],
  projects: [],
  capabilities: ['scope.read', 'scope.create', 'scope.operate', 'scope.delete'],
  generation: 1,
})

function authenticate(principalId: string) {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: principalId },
    expiresAt: Date.now() + 60_000,
    csrfToken: `csrf-${principalId}`,
    authority: authority(principalId),
  })
}

async function waitFor(check: () => void) {
  let error: unknown
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try { check(); return } catch (reason) { error = reason }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw error
}

test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

test('authority changes clear a destructive container target before rendering the new workspace', async () => {
  authenticate('operator-a')
  let listCalls = 0
  globalThis.fetch = (async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    if (body.action !== 'dev_containers.list') throw new Error(`Unexpected action ${body.action}`)
    listCalls += 1
    return Response.json({
      instances: listCalls === 1 ? [{
        instance_id: 'prior-authority-container',
        owner_kind: 'personal',
        owner_id: 'operator-a',
        desired_state: 'running',
        observed_state: 'running',
      }] : [],
    })
  }) as typeof fetch

  const { DevContainersPageContent } = await import('./dev-containers-page-content.tsx')
  const view = await renderClient(<DevContainersPageContent />)
  try {
    await waitFor(() => assert.ok(view.container.querySelector('[aria-label="Destroy prior-authority-container"]')))
    await act(async () => view.container.querySelector<HTMLButtonElement>('[aria-label="Destroy prior-authority-container"]')!.click())
    assert.match(document.body.textContent ?? '', /permanently destroys prior-authority-container/)

    authenticate('operator-b')
    await act(async () => {
      await view.rerender(<DevContainersPageContent />)
      await new Promise(resolve => setTimeout(resolve, 0))
    })

    assert.doesNotMatch(
      document.body.textContent ?? '',
      /prior-authority-container/,
      'a destructive target selected by the prior authority must not survive a workspace switch',
    )
  } finally {
    await view.unmount()
  }
})
