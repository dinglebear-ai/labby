import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'

import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'

installTestDom()

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

async function waitFor(check: () => void): Promise<void> {
  let lastError: unknown
  for (let attempt = 0; attempt < 100; attempt++) {
    try { check(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
  }
  throw lastError
}

for (const scenario of [
  { name: 'success', response: () => Response.json({ agent_id: 'starter-agent', agent_version: 1, session_id: 'session-1', status: 'completed', output_digest: 'digest', output: 'Labby Agent ready.', authority_expires_at: 123 }), expected: /Agent test completed.*Labby Agent ready/ },
  { name: 'provider failure', response: () => Response.json({ message: 'Provider unavailable' }, { status: 503 }), expected: /Provider unavailable/ },
]) test(`starter Agent ${scenario.name} remains truthful after save`, async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'person-1' },
    expiresAt: Date.now() + 100_000,
    csrfToken: 'csrf',
    authority: {
      schemaVersion: 1, compatibilityGeneration: 1,
      principalId: 'person-1', organizationId: 'org-1',
      activeOwner: { kind: 'personal', id: 'person-1' },
      teams: [], projects: [], capabilities: ['scope.read', 'scope.create', 'scope.operate'], generation: 1,
    },
  })
  let created = false
  let runCalls = 0
  globalThis.fetch = async (input, init) => {
    if (init?.method === 'GET') return Response.json({ services: [] })
    const request = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    switch (request.action) {
      case 'agents.list': return Response.json({ agents: created ? [{ agent_id: 'starter-agent', owner_kind: 'personal', owner_id: 'person-1', state: 'active', version: 1, catalog_generation: 'g1' }] : [] })
      case 'agents.models.list': return Response.json({ models: ['model-1'] })
      case 'access.team.list': return Response.json({ teams: [] })
      case 'projects.list': return Response.json([])
      case 'agents.create':
        assert.equal(request.params.owner_id, 'person-1')
        assert.equal(request.params.model, 'model-1')
        created = true
        return Response.json({ agent_id: 'starter-agent', owner_kind: 'personal', owner_id: 'person-1', state: 'active', version: 1, catalog_generation: 'g1' })
      case 'agents.run':
        runCalls += 1
        return scenario.response()
      default: throw new Error(`Unexpected action ${request.action} at ${String(input)}`)
    }
  }
  const { AgentsPage } = await import('./depot-workspace-pages')
  const view = await renderClient(<AgentsPage />)
  try {
    await waitFor(() => assert.ok([...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Create a starter Agent'))))
    await act(async () => [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Create a starter Agent'))!.click())
    await waitFor(() => assert.ok([...document.querySelectorAll('button')].find((button) => button.textContent?.includes('Create and test Agent') && !button.hasAttribute('disabled'))))
    await act(async () => [...document.querySelectorAll('button')].find((button) => button.textContent?.includes('Create and test Agent'))!.click())
    await waitFor(() => assert.match(view.container.textContent ?? '', scenario.expected))
    assert.equal(created, true)
    assert.equal(runCalls, 1)
  } finally {
    await view.unmount()
  }
})
