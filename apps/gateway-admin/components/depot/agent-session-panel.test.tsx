import assert from 'node:assert/strict'
import test from 'node:test'
import { installTestDom } from '@/lib/testing/dom-install'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import type { AgentHarnessView, AgentRunResult, AgentView } from '@/lib/agent-tasks/client'

const browser = installTestDom()
Object.defineProperty(globalThis, 'DocumentFragment', { value: browser.DocumentFragment, configurable: true })
Object.defineProperty(globalThis, 'NodeFilter', { value: browser.NodeFilter, configurable: true })
Object.defineProperty(globalThis, 'HTMLInputElement', { value: browser.HTMLInputElement, configurable: true })
Object.defineProperty(globalThis, 'HTMLTextAreaElement', { value: browser.HTMLTextAreaElement, configurable: true })

const agent: AgentView = {
  agent_id: 'review-agent',
  owner_kind: 'team',
  owner_id: 'team-1',
  version: 4,
  state: 'active',
  content_digest: 'sha256:content',
  repository_digest: 'sha256:repository',
  image_digest: 'sha256:image',
  harness_digest: 'sha256:harness',
  harness_id: 'codex-readonly',
  loadout_digest: 'sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945',
  catalog_generation: 'empty-catalog-v1',
}

const harness: AgentHarnessView = {
  id: 'codex-readonly',
  digest: 'sha256:harness',
  available: true,
  content_digest: agent.content_digest,
  repository_digest: agent.repository_digest,
  image_digest: agent.image_digest,
  loadout_digest: agent.loadout_digest,
  catalog_generation: agent.catalog_generation,
}

async function waitFor(assertion: () => void) {
  const { act } = await import('react')
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
  }
  throw lastError
}

test('session ledger renders durable status, agent revision and resume linkage', async () => {
  const [{ createElement }, { renderToStaticMarkup }, { AgentSessionLedger }] = await Promise.all([
    import('react'),
    import('react-dom/server'),
    import('./agent-session-panel'),
  ])
  const html = renderToStaticMarkup(createElement(AgentSessionLedger, { sessions: [{
    agent_id: agent.agent_id,
    agent_version: agent.version,
    session_id: 'session-current-1234567890',
    status: 'completed',
    input_digest: 'sha256:input',
    output_digest: 'sha256:output',
    resumed_from_session_id: 'session-prior-1234567890',
    authority_expires_at: 100,
    created_at: 10,
    updated_at: 20,
    completed_at: 20,
  }], loading: false, onRefresh: () => {}, onOpen: () => {} }))
  assert.match(html, /Retained sessions/)
  assert.match(html, /completed/)
  assert.match(html, /review-agent · v4/)
  assert.match(html, /Resumed from/)
})

test('session viewer stops a live run and follows the new ID returned by resume', async () => {
  document.body.replaceChildren()
  const [{ act, createElement }, { renderClient }, { AgentSessionViewer }] = await Promise.all([
    import('react'),
    import('@/lib/testing/dom-test-utils'),
    import('./agent-session-panel'),
  ])
  const originalFetch = globalThis.fetch
  const actions: string[] = []
  let status = 'running'
  let resumed: AgentRunResult | undefined
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    const body = JSON.parse(await request.text()) as { action: string }
    actions.push(body.action)
    if (body.action === 'agents.session.transcript') return Response.json({ agent_id: agent.agent_id, session_id: 'session-live', status, input: 'Inspect access', transcript: 'Checking policy', truncated: false })
    if (body.action === 'agents.session.stop') {
      status = 'cancelled'
      return Response.json({ agent_id: agent.agent_id, session_id: 'session-live', status: 'cancelling' })
    }
    if (body.action === 'agents.session.resume') return Response.json({ agent_id: agent.agent_id, agent_version: agent.version, session_id: 'session-resumed', status: 'admitted', input_digest: 'sha256:input', authority_expires_at: 2_000, resumed_from_session_id: 'session-live' })
    return Response.json({ agent_id: agent.agent_id, agent_version: agent.version, session_id: 'session-live', status, input_digest: 'sha256:input', output_digest: null, error_code: status === 'cancelled' ? 'cancelled' : null, resumed_from_session_id: null, authority_expires_at: 1_000, created_at: 10, updated_at: 20, completed_at: status === 'cancelled' ? 20 : null })
  }
  const view = await renderClient(createElement(AgentSessionViewer, {
    selected: { agent_id: agent.agent_id, session_id: 'session-live' },
    onClose: () => {},
    onChanged: () => {},
    onResumed: session => { resumed = session },
  }))
  try {
    await waitFor(() => assert.ok([...document.querySelectorAll('button')].some(button => button.textContent?.includes('Stop'))))
    const stop = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('Stop'))!
    await act(async () => { stop.click(); await Promise.resolve() })
    await waitFor(() => assert.ok([...document.querySelectorAll('button')].some(button => button.textContent?.includes('Resume'))))
    const resume = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('Resume'))!
    await act(async () => { resume.click(); await Promise.resolve() })
    await waitFor(() => assert.equal(resumed?.session_id, 'session-resumed'))
    assert.ok(actions.includes('agents.session.stop'))
    assert.ok(actions.includes('agents.session.resume'))
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
  }
})

test('new session wizard reuses its logical request key after an uncertain response', async () => {
  document.body.replaceChildren()
  const [{ act, createElement }, { renderClient }, { NewAgentSessionWizard }] = await Promise.all([
    import('react'),
    import('@/lib/testing/dom-test-utils'),
    import('./new-agent-session-wizard'),
  ])
  const originalFetch = globalThis.fetch
  const requests: Request[] = []
  let started: AgentRunResult | undefined
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    if (requests.length === 1) throw new TypeError('network response lost')
    return Response.json({
      agent_id: agent.agent_id,
      agent_version: agent.version,
      session_id: 'session-new',
      status: 'admitted',
      input_digest: 'sha256:input',
      authority_expires_at: 1_000,
      resumed_from_session_id: null,
    })
  }
  const view = await renderClient(createElement(NewAgentSessionWizard, { open: true, onOpenChange: () => {}, agents: [agent], harnesses: [harness], onStarted: session => { started = session } }))
  try {
    const deadline = Date.now() + 2_000
    while (!document.body.querySelector('[role="dialog"]') && Date.now() < deadline) {
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.match(document.body.textContent ?? '', /Operator-provisioned host runtime/)
    assert.match(document.body.textContent ?? '', /Empty loadout · \[\] pin/)
    assert.match(document.body.textContent ?? '', /does not select or provision a separate container/)
    const textarea = document.querySelector<HTMLTextAreaElement>('#agent-session-input')
    assert.ok(textarea)
    const setter = Object.getOwnPropertyDescriptor(browser.HTMLTextAreaElement.prototype, 'value')?.set
    assert.ok(setter)
    await act(async () => {
      setter.call(textarea, 'Review the authorization boundary')
      textarea.dispatchEvent(new browser.InputEvent('input', { bubbles: true, data: 'Review the authorization boundary' }) as unknown as Event)
      await Promise.resolve()
    })
    const start = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('Start Session'))
    assert.ok(start)
    assert.equal(start.disabled, false)
    await act(async () => { start.click(); await Promise.resolve() })
    await waitFor(() => assert.match(document.body.textContent ?? '', /network response lost/))
    await act(async () => { start.click(); await Promise.resolve() })
    await waitFor(() => assert.equal(started?.session_id, 'session-new'))
    assert.equal(started?.session_id, 'session-new')
    const bodies = await Promise.all(requests.map(async request => JSON.parse(await request.text())))
    assert.equal(bodies.length, 2)
    assert.deepEqual({ ...bodies[0], params: { ...bodies[0].params, idempotency_key: '<key>' } }, {
      action: 'agents.run',
      params: { agent_id: 'review-agent', input: 'Review the authorization boundary', idempotency_key: '<key>' },
    })
    assert.match(bodies[0].params.idempotency_key, /^[0-9a-f-]{36}$/)
    assert.equal(bodies[1].params.idempotency_key, bodies[0].params.idempotency_key)
  } finally {
    globalThis.fetch = originalFetch
    await view.unmount()
  }
})

test('first session creates one definition from exact approved harness pins before running it', async () => {
  document.body.replaceChildren()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority: { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'team', id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read', 'scope.create', 'scope.operate'], generation: 1 } })
  const [{ act, createElement }, { renderClient }, { NewAgentSessionWizard }] = await Promise.all([
    import('react'),
    import('@/lib/testing/dom-test-utils'),
    import('./new-agent-session-wizard'),
  ])
  const originalFetch = globalThis.fetch
  const requests: Request[] = []
  let started: AgentRunResult | undefined
  globalThis.fetch = async (input, init) => {
    const request = new Request(new URL(String(input), 'http://labby.test'), init)
    requests.push(request)
    const body = JSON.parse(await request.clone().text()) as { action: string }
    if (body.action === 'agents.create') return Response.json({ ...agent, agent_id: 'first-agent', version: 1 })
    return Response.json({ agent_id: 'first-agent', agent_version: 1, session_id: 'session-first', status: 'admitted', input_digest: 'sha256:input', authority_expires_at: 1_000, resumed_from_session_id: null })
  }
  const view = await renderClient(createElement(NewAgentSessionWizard, { open: true, onOpenChange: () => {}, agents: [], harnesses: [harness], onStarted: session => { started = session } }))
  try {
    await waitFor(() => assert.ok(document.body.querySelector('[role="dialog"]')))
    assert.match(document.body.textContent ?? '', /Start your first Agent session/)
    assert.match(document.body.textContent ?? '', /copy these exact server-approved references/)

    const input = document.querySelector<HTMLInputElement>('#agent-definition-id')!
    const textarea = document.querySelector<HTMLTextAreaElement>('#agent-session-input')!
    const inputSetter = Object.getOwnPropertyDescriptor(browser.HTMLInputElement.prototype, 'value')?.set
    const textareaSetter = Object.getOwnPropertyDescriptor(browser.HTMLTextAreaElement.prototype, 'value')?.set
    assert.ok(inputSetter)
    assert.ok(textareaSetter)
    await act(async () => {
      inputSetter.call(input, 'first-agent')
      input.dispatchEvent(new browser.InputEvent('input', { bubbles: true, data: 'first-agent' }) as unknown as Event)
      textareaSetter.call(textarea, 'Inspect the current workspace')
      textarea.dispatchEvent(new browser.InputEvent('input', { bubbles: true, data: 'Inspect the current workspace' }) as unknown as Event)
      await Promise.resolve()
    })
    const start = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('Start Session'))!
    assert.equal(start.disabled, false)
    await act(async () => { start.click(); await Promise.resolve() })
    await waitFor(() => assert.equal(started?.session_id, 'session-first'))

    const bodies = await Promise.all(requests.map(async request => JSON.parse(await request.text())))
    const runKey = bodies[1].params.idempotency_key
    assert.match(runKey, /^[0-9a-f-]{36}$/)
    bodies[1].params.idempotency_key = '<key>'
    assert.deepEqual(bodies, [
      {
        action: 'agents.create',
        params: {
          agent_id: 'first-agent',
          owner_kind: 'team',
          owner_id: 'team-1',
          content_digest: harness.content_digest,
          repository_digest: harness.repository_digest,
          image_digest: harness.image_digest,
          harness_digest: harness.digest,
          loadout_digest: harness.loadout_digest,
          catalog_generation: harness.catalog_generation,
        },
      },
      { action: 'agents.run', params: { agent_id: 'first-agent', input: 'Inspect the current workspace', idempotency_key: '<key>' } },
    ])
  } finally {
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await view.unmount()
  }
})

test('an unmatched existing definition can be replaced by an exact approved runnable definition', async () => {
  document.body.replaceChildren()
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'principal-1' }, expiresAt: Date.now() + 10_000, csrfToken: 'csrf', authority: { schemaVersion: 1, compatibilityGeneration: 1, principalId: 'principal-1', organizationId: 'org-1', activeOwner: { kind: 'team', id: 'team-1' }, activeTeamId: 'team-1', teams: [{ id: 'team-1', role: 'member', membershipEpoch: 1, policyEpoch: 1 }], projects: [], capabilities: ['scope.read', 'scope.create', 'scope.operate'], generation: 1 } })
  const [{ act, createElement }, { renderClient }, { NewAgentSessionWizard }] = await Promise.all([
    import('react'),
    import('@/lib/testing/dom-test-utils'),
    import('./new-agent-session-wizard'),
  ])
  const unmatched = { ...agent, agent_id: 'stale-agent', harness_digest: 'sha256:retired' }
  const originalFetch = globalThis.fetch
  const actions: string[] = []
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    actions.push(body.action)
    if (body.action === 'agents.create') return Response.json({ ...agent, agent_id: body.params.agent_id, version: 1 })
    return Response.json({ agent_id: body.params.agent_id, agent_version: 1, session_id: 'session-replacement', status: 'admitted', input_digest: 'sha256:input', authority_expires_at: 1_000 })
  }
  const view = await renderClient(createElement(NewAgentSessionWizard, { open: true, onOpenChange: () => {}, agents: [unmatched], harnesses: [harness], canCreate: true, onStarted: () => {} }))
  try {
    await waitFor(() => assert.match(document.body.textContent ?? '', /Create an Agent and start a session/))
    assert.doesNotMatch(document.body.textContent ?? '', /stale-agent · revision/)
    const input = document.querySelector<HTMLInputElement>('#agent-definition-id')!
    const textarea = document.querySelector<HTMLTextAreaElement>('#agent-session-input')!
    const inputSetter = Object.getOwnPropertyDescriptor(browser.HTMLInputElement.prototype, 'value')?.set
    const textareaSetter = Object.getOwnPropertyDescriptor(browser.HTMLTextAreaElement.prototype, 'value')?.set
    assert.ok(inputSetter)
    assert(textareaSetter)
    await act(async () => {
      inputSetter.call(input, 'replacement-agent')
      input.dispatchEvent(new browser.InputEvent('input', { bubbles: true, data: 'replacement-agent' }) as unknown as Event)
      textareaSetter.call(textarea, 'Inspect this workspace')
      textarea.dispatchEvent(new browser.InputEvent('input', { bubbles: true, data: 'Inspect this workspace' }) as unknown as Event)
      await Promise.resolve()
    })
    const start = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent?.includes('Start Session'))!
    assert.equal(start.disabled, false)
    await act(async () => { start.click(); await Promise.resolve() })
    await waitFor(() => assert.deepEqual(actions, ['agents.create', 'agents.run']))
  } finally {
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
    await view.unmount()
  }
})
