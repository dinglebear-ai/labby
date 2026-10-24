import test from 'node:test'
import assert from 'node:assert/strict'
import { installTestDom } from '../../lib/testing/dom-install.ts'

const browserWindow = installTestDom()
let act: typeof import('react')['act']
let createElement: typeof import('react')['createElement']
let SWRConfig: typeof import('swr')['SWRConfig']
let renderClient: typeof import('../../lib/testing/dom-test-utils.tsx')['renderClient']
let UpstreamOauthCard: typeof import('./upstream-oauth-card.tsx')['UpstreamOauthCard']
let upstreamOauthApi: typeof import('../../lib/api/upstream-oauth-client.ts')['upstreamOauthApi']
let originalStart: typeof upstreamOauthApi.start
let originalStatus: typeof upstreamOauthApi.status
const originalOpen = browserWindow.open

test.before(async () => {
  ;({ act, createElement } = await import('react'))
  ;({ SWRConfig } = await import('swr'))
  ;({ renderClient } = await import('../../lib/testing/dom-test-utils.tsx'))
  ;({ UpstreamOauthCard } = await import('./upstream-oauth-card.tsx'))
  ;({ upstreamOauthApi } = await import('../../lib/api/upstream-oauth-client.ts'))
  originalStart = upstreamOauthApi.start
  originalStatus = upstreamOauthApi.status
})

test.afterEach(() => {
  upstreamOauthApi.start = originalStart
  upstreamOauthApi.status = originalStatus
  browserWindow.open = originalOpen
})

function fakePopup() {
  return { closed: false, opener: {}, location: { href: 'about:blank' }, close() { this.closed = true } }
}

async function renderCard(status: Partial<Awaited<ReturnType<typeof upstreamOauthApi.status>>> = {}) {
  upstreamOauthApi.status = async () => ({ authenticated: false, state: 'disconnected', ...status }) as Awaited<ReturnType<typeof upstreamOauthApi.status>>
  return renderClient(createElement(SWRConfig, {
    value: { provider: () => new Map(), dedupingInterval: 0, revalidateOnFocus: false },
  }, createElement(UpstreamOauthCard, { name: 'test-upstream' })))
}

function clickConnect(container: HTMLElement) {
  const button = [...container.querySelectorAll('button')].find(node => node.textContent === 'Connect')
  assert.ok(button)
  button.click()
}

test('opens the isolated tab before a delayed OAuth start and navigates after settlement', async () => {
  const calls: string[] = []
  const popup = fakePopup()
  browserWindow.open = (() => { calls.push('open'); return popup }) as typeof browserWindow.open
  let complete!: (value: Awaited<ReturnType<typeof upstreamOauthApi.start>>) => void
  upstreamOauthApi.start = () => { calls.push('start'); return new Promise(resolve => { complete = resolve }) }
  const view = await renderCard()
  try {
    await act(async () => clickConnect(view.container))
    assert.deepEqual(calls, ['open', 'start'])
    assert.equal(popup.opener, null)
    assert.equal(popup.location.href, 'about:blank')
    await act(async () => complete({ authorization_url: 'https://provider.example/authorize' } as Awaited<ReturnType<typeof upstreamOauthApi.start>>))
    assert.equal(popup.location.href, 'https://provider.example/authorize')
  } finally { await view.unmount() }
  assert.equal(popup.closed, true)
})

test('a blocked popup does not begin an OAuth request', async () => {
  let starts = 0
  browserWindow.open = (() => null) as typeof browserWindow.open
  upstreamOauthApi.start = async () => { starts += 1; throw new Error('must not start') }
  const view = await renderCard()
  try {
    await act(async () => clickConnect(view.container))
    assert.equal(starts, 0)
    assert.match(view.container.textContent ?? '', /Popup blocked/)
  } finally { await view.unmount() }
})

test('start failure closes the owned tab and exposes recovery', async () => {
  const popup = fakePopup()
  browserWindow.open = (() => popup) as typeof browserWindow.open
  upstreamOauthApi.start = async () => { throw new Error('Provider unavailable') }
  const view = await renderCard()
  try {
    await act(async () => clickConnect(view.container))
    assert.equal(popup.closed, true)
    assert.match(view.container.textContent ?? '', /Provider unavailable/)
    assert.ok([...view.container.querySelectorAll('button')].some(node => node.textContent === 'Connect'))
  } finally { await view.unmount() }
})

test('unmount aborts pending start and a late response cannot navigate the tab', async () => {
  const popup = fakePopup()
  browserWindow.open = (() => popup) as typeof browserWindow.open
  let complete!: (value: Awaited<ReturnType<typeof upstreamOauthApi.start>>) => void
  let signal: AbortSignal | undefined
  upstreamOauthApi.start = (_name, nextSignal) => { signal = nextSignal; return new Promise(resolve => { complete = resolve }) }
  const view = await renderCard()
  await act(async () => clickConnect(view.container))
  await view.unmount()
  assert.equal(signal?.aborted, true)
  assert.equal(popup.closed, true)
  await act(async () => complete({ authorization_url: 'https://provider.example/late' } as Awaited<ReturnType<typeof upstreamOauthApi.start>>))
  assert.equal(popup.location.href, 'about:blank')
})


test('scope upgrades for a dedicated upstream use provider-neutral consent text', async () => {
  const view = await renderCard({
    authenticated: false,
    state: 'scope_upgrade_required',
    credential_source: 'dedicated',
  })
  try {
    assert.match(view.container.textContent ?? '', /Grant the permissions required by this server/)
    assert.doesNotMatch(view.container.textContent ?? '', /Google/)
    assert.ok([...view.container.querySelectorAll('button')].some(node => node.textContent === 'Grant scopes'))
  } finally { await view.unmount() }
})


test('unattended OAuth does not offer browser sign-in when its grant fails', async () => {
  const view = await renderCard({
    authenticated: false,
    credential_source: 'dedicated',
    state: 'refresh_failed',
    refresh_error: 'Service credential is unavailable',
    ...{ authorization_mode: 'client_credentials' },
  })
  try {
    assert.match(view.container.textContent ?? '', /Service credential is unavailable/)
    assert.ok(![...view.container.querySelectorAll('button')].some(node => node.textContent === 'Connect'))
  } finally { await view.unmount() }
})

test('upstream scope challenges remain visible when the Google grant has no missing scopes', async () => {
  const view = await renderCard({
    authenticated: false,
    state: 'scope_upgrade_required',
    credential_source: 'google_provider',
    required_scopes: ['files:write'],
    google_credential_broker: {
      account_selector_configured: true,
      client_bound: true,
      required_scopes: [],
      granted_scopes: [],
      missing_scopes: [],
    },
  })
  try {
    assert.match(view.container.textContent ?? '', /files:write/)
  } finally { await view.unmount() }
})


test('clearing an unattended grant does not claim to disconnect its service identity', async () => {
  const view = await renderCard({
    authenticated: true,
    credential_source: 'dedicated',
    state: 'connected',
    authorization_mode: 'client_credentials',
  })
  try {
    assert.ok([...view.container.querySelectorAll('button')].some(node => node.textContent === 'Clear cached token'))
    assert.ok(![...view.container.querySelectorAll('button')].some(node => node.textContent === 'Disconnect'))
  } finally { await view.unmount() }
})
