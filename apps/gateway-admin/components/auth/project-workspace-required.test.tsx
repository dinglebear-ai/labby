import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '../../lib/testing/dom-test-utils.tsx'
import { __setBrowserSessionStateForTests, getSessionAuthority, getSessionProjectId, type BrowserSessionState } from '../../lib/auth/session-store.ts'
import type { AuthorityProject, AuthoritySnapshot } from '../../lib/auth/authority.ts'

installTestDom()
let ProjectWorkspaceRequired: typeof import('./project-workspace-required.tsx').ProjectWorkspaceRequired
let Toaster: typeof import('@/components/ui/sonner').Toaster
test.before(async () => {
  ;({ ProjectWorkspaceRequired } = await import('./project-workspace-required.tsx'))
  ;({ Toaster } = await import('@/components/ui/sonner'))
})
test.afterEach(() => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

const DESCRIPTION = 'The Library is project-scoped. Select an eligible project workspace to continue.'
const authenticated = (overrides: Partial<Extract<BrowserSessionState, { status: 'authenticated' }>> = {}): BrowserSessionState => ({ status: 'authenticated', user: { sub: 'operator' }, expiresAt: Date.now() + 60_000, csrfToken: 'csrf', ...overrides })
const authority = (projects: AuthorityProject[]): AuthoritySnapshot => ({
  schemaVersion: 1,
  compatibilityGeneration: 1,
  principalId: 'principal-1',
  organizationId: 'org-1',
  activeOwner: { kind: 'personal', id: 'principal-1' },
  teams: [],
  projects,
  capabilities: ['scope.read'],
  generation: 1,
})
const gate = (session: BrowserSessionState) => <ProjectWorkspaceRequired session={session} description={DESCRIPTION} />

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw lastError
}

test('the states AuthBootstrap owns render nothing', async () => {
  const sessions: BrowserSessionState[] = [
    { status: 'loading' },
    { status: 'unauthenticated' },
    { status: 'auth_error', message: 'Unable to reach the authentication service. Try again.' },
  ]
  for (const session of sessions) {
    const view = await renderClient(gate(session))
    try { assert.equal(view.container.textContent, '', `${session.status} must render no panel of its own`) } finally { await view.unmount() }
  }
})

test('mock data mode explains that no project can be projected', async () => {
  const previous = process.env.NEXT_PUBLIC_MOCK_DATA
  process.env.NEXT_PUBLIC_MOCK_DATA = 'true'
  const view = await renderClient(gate({ status: 'loading' }))
  try {
    assert.match(view.container.textContent ?? '', /Project required/)
    assert.match(view.container.textContent ?? '', /Mock data mode does not project an authenticated project/)
    assert.equal(view.container.querySelector('button'), null)
  } finally {
    await view.unmount()
    if (previous === undefined) delete process.env.NEXT_PUBLIC_MOCK_DATA
    else process.env.NEXT_PUBLIC_MOCK_DATA = previous
  }
})

test('a session without an authority projection asks for a new sign-in instead of an empty chooser', async () => {
  const view = await renderClient(gate(authenticated()))
  try {
    assert.match(view.container.textContent ?? '', /Project required/)
    assert.match(view.container.textContent ?? '', /no workspace authority projection/)
    assert.match(view.container.textContent ?? '', /Sign in again/)
    assert.doesNotMatch(view.container.textContent ?? '', /No eligible project/)
    assert.equal(view.container.querySelector('button'), null)
  } finally { await view.unmount() }
})

test('the chooser offers only the server-projected projects and binds the chosen one', async () => {
  const session = authenticated({ authority: authority([{ id: 'project-1', role: 'owner', name: 'Project One' }, { id: 'project-2', role: 'member' }]) })
  __setBrowserSessionStateForTests(session)
  const view = await renderClient(gate(session))
  try {
    assert.ok(view.container.textContent?.includes(DESCRIPTION))
    const buttons = [...view.container.querySelectorAll('button')]
    assert.deepEqual(buttons.map(button => button.textContent), ['Project One', 'project-2'])
    await act(async () => buttons[0].click())
    assert.equal(getSessionProjectId(), 'project-1')
    assert.deepEqual(getSessionAuthority()?.activeOwner, { kind: 'project', id: 'project-1' })
  } finally { await view.unmount() }
})

test('a session with no eligible project names the remedy', async () => {
  const view = await renderClient(gate(authenticated({ authority: authority([]) })))
  try {
    assert.match(view.container.textContent ?? '', /No eligible project is available for this session/)
    assert.match(view.container.textContent ?? '', /reload the page/)
    assert.equal(view.container.querySelector('button'), null)
  } finally { await view.unmount() }
})

test('a selection the current authority cannot satisfy reports through a toast instead of escaping', async () => {
  const rendered = authenticated({ authority: authority([{ id: 'project-1', role: 'owner', name: 'Project One' }]) })
  // The store has since lost its projection; the test hook does not notify
  // subscribers, so the chooser rendered from `rendered` is stale on purpose.
  __setBrowserSessionStateForTests(authenticated())
  const view = await renderClient(<>{gate(rendered)}<Toaster /></>)
  try {
    const choose = [...view.container.querySelectorAll('button')].find(button => button.textContent === 'Project One')
    assert.ok(choose)
    await act(async () => choose.click())
    await waitFor(() => assert.match(document.body.textContent ?? '', /Authority is unavailable/))
    assert.equal(getSessionProjectId(), undefined, 'a rejected selection binds nothing')
  } finally { await view.unmount() }
})
