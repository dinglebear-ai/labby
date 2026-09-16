import assert from 'node:assert/strict'
import test from 'node:test'

import { installTestDom } from '@/lib/testing/dom-install.ts'

installTestDom()

let ReactModule: typeof import('react')
let act: typeof import('react').act
let __setBrowserSessionStateForTests: typeof import('@/lib/auth/session-store.ts').__setBrowserSessionStateForTests
let teamInvitationHref: typeof import('@/lib/auth/team-invitation.ts').teamInvitationHref
let renderClient: typeof import('@/lib/testing/dom-test-utils.tsx').renderClient
let PeoplePage: typeof import('./people-page.tsx').PeoplePage

test.before(async () => {
  ReactModule = await import('react')
  act = ReactModule.act
  ;({ __setBrowserSessionStateForTests } = await import('@/lib/auth/session-store.ts'))
  ;({ teamInvitationHref } = await import('@/lib/auth/team-invitation.ts'))
  ;({ renderClient } = await import('@/lib/testing/dom-test-utils.tsx'))
  ;({ PeoplePage } = await import('./people-page.tsx'))
})

const originalFetch = globalThis.fetch

function authenticate() {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'team-admin', email: 'admin@example.com' },
    expiresAt: Date.now() + 60_000,
    csrfToken: 'csrf-team-admin',
    authorityState: 'ready',
  })
}

function team(teamId: string, name: string) {
  return {
    team_id: teamId,
    name,
    status: 'active',
    role: 'admin',
    policy_epoch: 1,
    membership_epoch: 1,
    global_revision: 1,
  }
}

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
    }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 5)) })
  }
  throw lastError
}

async function setInputValue(input: HTMLInputElement, value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set
    assert.ok(setter)
    setter.call(input, value)
    input.dispatchEvent(new window.InputEvent('input', { bubbles: true, data: value }) as unknown as Event)
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

function buttonByText(container: Element, label: string) {
  const button = [...container.querySelectorAll<HTMLButtonElement>('button')]
    .find((candidate) => candidate.textContent?.trim() === label)
  assert.ok(button, `missing button: ${label}`)
  return button
}

test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  window.sessionStorage.clear()
  document.body.replaceChildren()
})

test('one-team onboarding hides team selection and keeps role and expiry behind progressive disclosure', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({ teams: [team('team-one', 'Engineering')] })

  const view = await renderClient(ReactModule.createElement(PeoplePage))
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /Engineering/))
    assert.equal(view.container.querySelector('#invite-team'), null, 'one active team must not create a needless choice')
    assert.equal(view.container.querySelector('#invite-role'), null)
    assert.equal(view.container.querySelector('#invite-expiry'), null)
    assert.match(view.container.textContent ?? '', /Recommended defaults: Member · expires in 7 days./)

    await act(async () => { buttonByText(view.container, 'Role and expiry options').click() })
    assert.ok(view.container.querySelector('#invite-role'), 'advanced role control should be available on demand')
    assert.ok(view.container.querySelector('#invite-expiry'), 'advanced expiry control should be available on demand')
  } finally {
    await view.unmount()
  }
})

test('multiple active teams expose a team selector without changing recommended invitation defaults', async () => {
  authenticate()
  globalThis.fetch = async () => Response.json({
    teams: [team('team-one', 'Engineering'), team('team-two', 'Design')],
  })

  const view = await renderClient(ReactModule.createElement(PeoplePage))
  try {
    await waitFor(() => assert.ok(view.container.querySelector('#invite-team')))
    assert.match(view.container.textContent ?? '', /Recommended defaults: Member · expires in 7 days./)
    assert.equal(view.container.querySelector('#invite-role'), null)
    assert.equal(view.container.querySelector('#invite-expiry'), null)
  } finally {
    await view.unmount()
  }
})

test('creating an invitation uses member and seven-day defaults and keeps the secret in the URL fragment', async () => {
  authenticate()
  const token = 'cd'.repeat(32)
  const requests: Array<{ action: string; params: Record<string, unknown> }> = []
  globalThis.fetch = async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string; params: Record<string, unknown> }
    requests.push(body)
    if (body.action === 'access.team.list') {
      return Response.json({ teams: [team('team-one', 'Engineering')] })
    }
    if (body.action === 'access.team_invitation.create') {
      return Response.json({
        team_id: 'team-one',
        role: 'member',
        status: 'pending',
        team_membership_epoch: 1,
        expires_at: 999,
        token,
      })
    }
    throw new Error(`unexpected action ${body.action}`)
  }

  let copied = ''
  Object.defineProperty(window.navigator, 'clipboard', {
    configurable: true,
    value: { writeText: async (value: string) => { copied = value } },
  })

  const view = await renderClient(ReactModule.createElement(PeoplePage))
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /Engineering/))
    const email = view.container.querySelector<HTMLInputElement>('#invite-email')
    assert.ok(email)
    await setInputValue(email, 'new.user@example.com')

    const createButton = buttonByText(view.container, 'Create invitation')
    assert.equal(createButton.disabled, false, 'entering an email should enable invitation creation')
    await act(async () => {
      createButton.click()
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    await waitFor(() => assert.match(view.container.textContent ?? '', /Invitation ready/))

    const create = requests.find((request) => request.action === 'access.team_invitation.create')
    assert.deepEqual(create?.params, {
      team_id: 'team-one',
      email: 'new.user@example.com',
      role: 'member',
      ttl_seconds: 7 * 24 * 60 * 60,
    })

    const linkInput = [...view.container.querySelectorAll<HTMLInputElement>('input')]
      .find((candidate) => candidate.readOnly)
    assert.ok(linkInput)
    const expectedLink = teamInvitationHref(window.location.origin, token)
    assert.equal(linkInput.value, expectedLink)
    const [requestUrl, fragment] = linkInput.value.split('#', 2)
    assert.equal(requestUrl.includes(token), false)
    assert.equal(fragment.includes(token), true)

    await act(async () => { buttonByText(view.container, 'Copy invitation link').click() })
    assert.equal(copied, expectedLink)
  } finally {
    await view.unmount()
  }
})

test('unmount aborts the in-flight team load instead of allowing a late state update', async () => {
  authenticate()
  let signal: AbortSignal | undefined
  globalThis.fetch = async (_input, init) => {
    signal = init?.signal as AbortSignal | undefined
    return await new Promise<Response>((_resolve, reject) => {
      signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')), { once: true })
    })
  }

  const view = await renderClient(ReactModule.createElement(PeoplePage))
  await waitFor(() => assert.ok(signal))
  await view.unmount()
  assert.equal(signal?.aborted, true)
})
