import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import { READINESS_CHECKS } from '@/lib/settings/readiness'

installTestDom()
Object.defineProperty(globalThis, "self", { configurable: true, value: window })
const originalFetch = globalThis.fetch
test.afterEach(() => {
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})

async function waitFor(check: () => void) {
  let failure: unknown
  for (let attempt = 0; attempt < 100; attempt++) {
    try { check(); return } catch (error) { failure = error }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
  }
  throw failure
}

test('saved progress resumes and application deferral remains visibly unverified', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, csrfToken: 'csrf', expiresAt: Date.now() + 100000 })
  const checks = READINESS_CHECKS.map((check) => ({ check, status: check === 'gateway_authenticated' ? 'verified' : 'pending', verified_at: check === 'gateway_authenticated' ? 100 : null, resource_id: check === 'gateway_authenticated' ? 'authenticated-request' : null }))
  let deferrals = 0
  globalThis.fetch = async (_url, options) => {
    const request = JSON.parse(String(options?.body)) as { action: string; params: Record<string, unknown> }
    assert.deepEqual(request.params, {})
    if (request.action === 'readiness.clients.defer') {
      assert.equal(new Headers(options?.headers).get('x-csrf-token'), 'csrf')
      deferrals++
      checks[3] = { ...checks[3], status: 'deferred', verified_at: 101, resource_id: 'no-external-clients-selected' }
    } else assert.equal(request.action, 'readiness.state')
    return Response.json({ ready: false, checks, evidence_max_age_seconds: 86400 })
  }
  const { FirstUseReadiness } = await import('./FirstUseReadiness')
  const view = await renderClient(<FirstUseReadiness />).catch((error) => { throw error instanceof AggregateError ? error.errors[0] : error })
  try {
    await waitFor(() => assert.match(view.container.textContent ?? '', /1 check verified/))
    assert.doesNotMatch(view.container.textContent ?? '', /Your first-use checks passed/)
    await act(async () => [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Use Labby only'))!.click())
    await waitFor(() => assert.match(view.container.textContent ?? '', /external applications deferred/))
    assert.equal(deferrals, 1)
    assert.equal(view.container.querySelector('[role="progressbar"]')?.getAttribute('aria-valuetext'), '1 of 6 checks verified')
    assert.match(view.container.textContent ?? '', /Next step/)
    assert.match(view.container.textContent ?? '', /Deferred/)
    assert.doesNotMatch(view.container.textContent ?? '', /Your first-use checks passed/)
  } finally { await view.unmount() }
})

test('readiness errors remain visible and do not become a successful check', async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, csrfToken: 'csrf', expiresAt: Date.now() + 100000 })
  globalThis.fetch = async () => Response.json({ message: 'Access setup is required' }, { status: 503 })
  const { FirstUseReadiness } = await import('./FirstUseReadiness')
  const view = await renderClient(<FirstUseReadiness />).catch((error) => { throw error instanceof AggregateError ? error.errors[0] : error })
  try {
    await waitFor(() => assert.match(view.container.querySelector('[role="alert"]')?.textContent ?? '', /Access setup is required/))
    assert.doesNotMatch(view.container.textContent ?? '', /Your first-use checks passed/)
    assert.match(view.container.textContent ?? '', /Unverified/)
  } finally { await view.unmount() }
})
