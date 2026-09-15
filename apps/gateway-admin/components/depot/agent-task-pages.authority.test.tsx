import assert from 'node:assert/strict'
import test from 'node:test'
import { installTestDom } from '@/lib/testing/dom-install'
import { __setBrowserSessionStateForTests, selectSessionWorkspace } from '@/lib/auth/session-store'

const browser = installTestDom()
Object.defineProperty(globalThis, 'DocumentFragment', { value: browser.DocumentFragment, configurable: true })
Object.defineProperty(globalThis, 'NodeFilter', { value: browser.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: browser.HTMLInputElement, configurable: true })
Object.defineProperty(globalThis, 'HTMLTextAreaElement', { value: browser.HTMLTextAreaElement, configurable: true })

const agent = { agent_id: 'old-agent', owner_kind: 'team', owner_id: 'team-1', version: 1, state: 'active', content_digest: 'sha256:content', repository_digest: 'sha256:repository', image_digest: 'sha256:image', harness_digest: 'sha256:harness', harness_id: 'codex', loadout_digest: 'sha256:loadout', catalog_generation: 'catalog-1' }
const harness = { id: 'codex', digest: 'sha256:harness', available: true, content_digest: 'sha256:content', repository_digest: 'sha256:repository', image_digest: 'sha256:image', loadout_digest: 'sha256:loadout', catalog_generation: 'catalog-1' }
const originalFetch = globalThis.fetch

function authenticateTeams() {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority: { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal', organizationId: 'org', activeOwner: { kind: 'team', id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }, { id: 'team-2', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read', 'scope.create', 'scope.operate'], generation: 1 } })
}

async function waitFor(check: () => void) {
  const { act } = await import('react')
  let error: unknown
  for (let attempt = 0; attempt < 100; attempt++) {
    try { check(); return } catch (reason) { error = reason }
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 5)) })
  }
  throw error
}

test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

test('authority changes immediately discard old Agent rows, viewer, and wizard state', async () => {
  document.body.replaceChildren()
  authenticateTeams()
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    const team = new Headers(init?.headers).get('x-labby-team-id')
    if (team === 'team-2') {
      return new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), { once: true })
      })
    }
    if (body.action === 'agents.list') return Response.json({ agents: [agent], next_cursor: null })
    if (body.action === 'agents.harnesses') return Response.json({ harnesses: [harness] })
    if (body.action === 'agents.sessions.list') return Response.json({ sessions: [{ agent_id: 'old-agent', agent_version: 1, session_id: 'old-session', status: 'completed', input_digest: 'sha256:input', authority_expires_at: 100, created_at: 10, updated_at: 20, completed_at: 20 }], next_cursor: null })
    if (body.action === 'agents.session.get') return Response.json({ agent_id: 'old-agent', agent_version: 1, session_id: 'old-session', status: 'completed', input_digest: 'sha256:input', authority_expires_at: 100, created_at: 10, updated_at: 20, completed_at: 20 })
    if (body.action === 'agents.session.transcript') return Response.json({ agent_id: 'old-agent', session_id: 'old-session', status: 'completed', input: 'Old authority input', transcript: 'Old authority output', truncated: false })
    throw new Error(`Unexpected action ${body.action}`)
  }
  const [{ act, createElement }, { renderClient }, { AgentsPage }] = await Promise.all([
    import('react'),
    import('@/lib/testing/dom-test-utils'),
    import('./agent-task-pages'),
  ])
  const view = await renderClient(createElement(AgentsPage))
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /old-agent/))
    const sessionRow = [...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('old-session'))!
    await act(async () => { sessionRow.click(); await Promise.resolve() })
    await waitFor(() => assert.match(document.body.textContent ?? '', /Old authority input/))
    const newSession = [...view.container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.trim() === 'New Session')!
    await act(async () => { newSession.click(); await Promise.resolve() })
    await waitFor(() => assert.match(document.body.textContent ?? '', /New Agent Session/))

    await act(async () => { selectSessionWorkspace({ teamId: 'team-2' }); await Promise.resolve() })

    await waitFor(() => {
      assert.doesNotMatch(document.body.textContent ?? '', /old-agent|old-session|Old authority input|New Agent Session/)
      assert.match(view.container.textContent ?? '', /Loading Agent workspace/)
    })
  } finally { await view.unmount() }
})
