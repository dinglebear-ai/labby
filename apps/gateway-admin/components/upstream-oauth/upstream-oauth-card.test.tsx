import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'

import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { UpstreamOauthStatus } from '@/lib/types/upstream-oauth'
import { UpstreamOauthCard } from './upstream-oauth-card'

const disconnected: UpstreamOauthStatus = {
  authenticated: false,
  upstream: 'test',
  credential_source: 'dedicated',
  expires_within_5m: false,
  state: 'disconnected',
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail })
  return { promise, resolve, reject }
}

async function waitFor(assertion: () => void) {
  const deadline = Date.now() + 2_000
  let lastError: unknown
  while (Date.now() < deadline) {
    try { assertion(); return } catch (error) { lastError = error }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)) })
  }
  throw lastError
}

function popup(closed = false) {
  return {
    closed,
    closeCalls: 0,
    location: { href: 'about:blank' },
    opener: null,
    close() { this.closeCalls += 1; this.closed = true },
  }
}

async function renderConnect(name: string) {
  const view = await renderClient(<UpstreamOauthCard name={name} />)
  await waitFor(() => assert.match(view.container.textContent ?? '', /Connect/))
  const button = [...view.container.querySelectorAll('button')]
    .find((candidate) => candidate.textContent?.trim() === 'Connect')
  assert.ok(button)
  return { view, button }
}

test('UpstreamOauthCard opens the popup synchronously before start resolves', async () => {
  const window = installTestDom()
  upstreamOauthApi.status = async () => disconnected
  const start = deferred<{ authorization_url: string }>()
  upstreamOauthApi.start = () => start.promise
  const opened = popup()
  let popupOpened = false
  window.open = (() => { popupOpened = true; return opened }) as unknown as typeof window.open
  const { view, button } = await renderConnect('sync-popup')

  await act(async () => { button.dispatchEvent(new MouseEvent('click', { bubbles: true })); await Promise.resolve() })
  assert.equal(popupOpened, true)
  assert.equal(opened.location.href, 'about:blank')
  start.resolve({ authorization_url: 'https://issuer.example/authorize' })
  await waitFor(() => assert.equal(opened.location.href, 'https://issuer.example/authorize'))
  await view.unmount()
})

test('UpstreamOauthCard reports a blocked popup without starting OAuth', async () => {
  const window = installTestDom()
  upstreamOauthApi.status = async () => disconnected
  let starts = 0
  upstreamOauthApi.start = async () => { starts += 1; return { authorization_url: 'https://unused' } }
  window.open = (() => null) as typeof window.open
  const { view, button } = await renderConnect('blocked-popup')
  await act(async () => { button.dispatchEvent(new MouseEvent('click', { bubbles: true })) })
  assert.equal(starts, 0)
  assert.match(view.container.textContent ?? '', /Popup blocked/)
  await view.unmount()
})

test('UpstreamOauthCard closes the blank popup when start fails', async () => {
  const window = installTestDom()
  upstreamOauthApi.status = async () => disconnected
  upstreamOauthApi.start = async () => { throw new Error('start failed') }
  const opened = popup()
  window.open = (() => opened) as unknown as typeof window.open
  const { view, button } = await renderConnect('start-failure')
  await act(async () => { button.dispatchEvent(new MouseEvent('click', { bubbles: true })); await Promise.resolve() })
  await waitFor(() => assert.equal(opened.closeCalls, 1))
  assert.match(view.container.textContent ?? '', /start failed/)
  await view.unmount()
})

test('UpstreamOauthCard detects a user-closed blank popup', async () => {
  const window = installTestDom()
  upstreamOauthApi.status = async () => disconnected
  const start = deferred<{ authorization_url: string }>()
  upstreamOauthApi.start = () => start.promise
  const opened = popup()
  window.open = (() => opened) as unknown as typeof window.open
  const { view, button } = await renderConnect('closed-popup')
  await act(async () => { button.dispatchEvent(new MouseEvent('click', { bubbles: true })); await Promise.resolve() })
  opened.closed = true
  start.resolve({ authorization_url: 'https://issuer.example/authorize' })
  await waitFor(() => assert.match(view.container.textContent ?? '', /Authorization tab was closed/))
  assert.equal(opened.location.href, 'about:blank')
  await view.unmount()
})

test('UpstreamOauthCard navigates the synchronously opened popup after start succeeds', async () => {
  const window = installTestDom()
  upstreamOauthApi.status = async () => disconnected
  upstreamOauthApi.start = async () => ({ authorization_url: 'https://issuer.example/authorize' })
  const opened = popup()
  window.open = (() => opened) as unknown as typeof window.open
  const { view, button } = await renderConnect('successful-popup')
  await act(async () => { button.dispatchEvent(new MouseEvent('click', { bubbles: true })); await Promise.resolve() })
  await waitFor(() => assert.equal(opened.location.href, 'https://issuer.example/authorize'))
  assert.equal(opened.closeCalls, 0)
  await view.unmount()
})
