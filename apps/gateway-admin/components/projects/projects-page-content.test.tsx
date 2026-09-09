import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'

import type { AuthoritySnapshot } from '@/lib/auth/authority.ts'
import { __resetAuthorityContextForTests } from '@/lib/auth/authority-context.ts'
import { __setBrowserSessionStateForTests, selectSessionWorkspace } from '@/lib/auth/session-store.ts'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils.tsx'
import { ProjectsPageContent } from './projects-page-content.tsx'

installTestDom()

const authority: AuthoritySnapshot = {
  schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1',
  activeOwner: { kind: 'team', id: 'team-owner' }, activeTeamId: 'team-owner',
  teams: [
    { id: 'team-owner', role: 'owner', membershipEpoch: 1, policyEpoch: 1 },
    { id: 'team-member', role: 'member', membershipEpoch: 1, policyEpoch: 1 },
  ],
  projects: [], capabilities: ['scope.read'], generation: 7,
}

function authenticate(selection: Partial<AuthoritySnapshot> = {}) {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority: { ...authority, ...selection } })
}

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __resetAuthorityContextForTests()
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

const rows = [
  { project_id: 'owned', team_id: 'team-owner', name: 'Owned project', status: 'active', role: 'owner', policy_epoch: 1, can_manage: true },
  { project_id: 'read-only', team_id: 'team-member', name: 'Read-only project', status: 'active', role: 'member', policy_epoch: 1, can_manage: false },
  // The local team role says owner; the server says the lifecycle is locked. The server wins.
  { project_id: 'locked', team_id: 'team-owner', name: 'Locked project', status: 'archived', role: 'owner', policy_epoch: 2, can_manage: false },
]

test('archive controls follow the server-derived lifecycle authority, never the local team role', async () => {
  document.body.replaceChildren()
  authenticate()
  globalThis.fetch = async () => Response.json(rows)
  const view = await renderClient(<ProjectsPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /Locked project/))
  const archive = (name: string) => view.container.querySelector<HTMLButtonElement>(`button[aria-label="Archive ${name}"]`)
  assert.equal(archive('Owned project')?.disabled, false)
  assert.equal(archive('Read-only project')?.disabled, true)
  assert.equal(archive('Locked project')?.disabled, true, 'a local owner role must not unlock a row the server marked unmanageable')
  await view.unmount()
})

test('the create form follows the active team through the session subscription and reloads on workspace change', async () => {
  document.body.replaceChildren()
  authenticate({ activeOwner: { kind: 'personal', id: 'principal-1' }, activeTeamId: undefined })
  const listCalls: string[] = []
  globalThis.fetch = async (_input, init) => {
    listCalls.push(String((init?.headers as Headers | undefined)?.get?.('x-csrf-token') ?? ''))
    return Response.json([])
  }
  const view = await renderClient(<ProjectsPageContent />)
  await waitFor(() => assert.match(view.container.textContent || '', /No accessible Projects/))
  assert.equal(listCalls.length, 1)
  assert.match(view.container.textContent || '', /Select a Team workspace to create a Project/)
  assert.equal(view.container.querySelector('input[aria-label="Project ID"]') === null, true, 'a personal workspace has no create form')

  await act(async () => { selectSessionWorkspace({ teamId: 'team-owner' }) })
  await waitFor(() => assert.equal(listCalls.length, 2, 'a workspace switch must reload the list'))
  await waitFor(() => assert.ok(view.container.querySelector('input[aria-label="Project ID"]'), 'a team workspace exposes the create form'))
  await view.unmount()
})

test('an aborted request from a workspace switch is not rendered as a failure', async () => {
  document.body.replaceChildren()
  authenticate()
  let calls = 0
  globalThis.fetch = async (_input, init) => {
    calls += 1
    if (calls === 1) {
      return new Promise<Response>((_resolve, reject) => init?.signal?.addEventListener('abort', () => reject(init.signal?.reason), { once: true }))
    }
    return Response.json([])
  }
  const view = await renderClient(<ProjectsPageContent />)
  await waitFor(() => assert.equal(calls, 1))
  await act(async () => { selectSessionWorkspace({ teamId: 'team-member' }) })
  await waitFor(() => assert.match(view.container.textContent || '', /No accessible Projects/))
  assert.equal(view.container.querySelector('[role="alert"]') === null, true, 'AbortError must not surface as an operator-facing error')
  await view.unmount()
})
