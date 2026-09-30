import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import { installTestDom } from '@/lib/testing/dom-install'
import { setupApi, type SettingsFieldSpec } from '@/lib/api/setup-client'

test('notification feed failure does not hide delivery settings and refresh clears its error', async () => {
  installTestDom()
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { default: NotificationsSettingsPage } = await import('./page')
  const originalSchema = setupApi.settingsSchema
  const originalState = setupApi.settingsState
  const originalFetch = globalThis.fetch
  let feedAvailable = false

  setupApi.settingsSchema = async () => ({ schema_version: 1, sections: [], fields: [] })
  setupApi.settingsState = async () => ({
    schema_version: 1,
    config_path: '/tmp/config.toml',
    env_path: '/tmp/.env',
    section: 'notifications',
    values: {},
    sources: {},
  })
  globalThis.fetch = async () => new Response(
    feedAvailable ? JSON.stringify({ notifications: [] }) : JSON.stringify({ message: 'feed unavailable' }),
    { status: feedAvailable ? 200 : 503 },
  )

  try {
    const view = await renderClient(<NotificationsSettingsPage />)
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    assert.match(view.container.textContent ?? '', /Delivery/)
    assert.match(view.container.textContent ?? '', /feed unavailable/)

    feedAvailable = true
    const refresh = [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Refresh'))
    assert.ok(refresh)
    await act(async () => {
      refresh.dispatchEvent(new MouseEvent('click', { bubbles: true }))
      await new Promise((resolve) => setTimeout(resolve, 0))
    })
    assert.doesNotMatch(view.container.textContent ?? '', /feed unavailable/)
    await view.unmount()
  } finally {
    setupApi.settingsSchema = originalSchema
    setupApi.settingsState = originalState
    globalThis.fetch = originalFetch
  }
})

for (const staleStatus of [200, 503]) {
  for (const latestStatus of [200, 503]) {
    test(`latest notification response (${latestStatus}) wins over stale response (${staleStatus})`, async () => {
      installTestDom()
      const { renderClient } = await import('@/lib/testing/dom-test-utils')
      const { default: NotificationsSettingsPage } = await import('./page')
      const originalSchema = setupApi.settingsSchema
      const originalState = setupApi.settingsState
      const originalFetch = globalThis.fetch
      const pending: { signal: AbortSignal | null | undefined, resolve: (response: Response) => void }[] = []
      setupApi.settingsSchema = async () => ({ schema_version: 1, sections: [], fields: [] })
      setupApi.settingsState = async () => ({
        schema_version: 1, config_path: '/tmp/config.toml', env_path: '/tmp/.env',
        section: 'notifications', values: {}, sources: {},
      })
      // Ignore cancellation to also exercise responses already received before abort.
      globalThis.fetch = async (_url, init) => new Promise<Response>((resolve) => {
        pending.push({ signal: init?.signal, resolve })
      })
      function response(title: string, status: number): Response {
        return new Response(JSON.stringify(status === 200 ? { notifications: [{
          id: title, createdAtUnixMs: 1, level: 'error', title,
          body: '', source: 'test', dedupeKey: title,
        }] } : { message: title }), { status })
      }

      let view: Awaited<ReturnType<typeof renderClient>> | undefined
      try {
        view = await renderClient(<NotificationsSettingsPage />)
        assert.equal(pending.length, 1)
        const refresh = [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Refresh'))
        assert.ok(refresh)
        await act(async () => {
          refresh.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        })
        assert.equal(pending.length, 2)
        await act(async () => { pending[1].resolve(response('latest response', latestStatus)) })
        assert.match(view.container.textContent ?? '', /latest response/)
        await act(async () => { pending[0].resolve(response('stale response', staleStatus)) })
        assert.match(view.container.textContent ?? '', /latest response/)
        assert.doesNotMatch(view.container.textContent ?? '', /stale response/)
        assert.equal(pending[0].signal?.aborted, true)

        await act(async () => {
          refresh.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        })
        assert.equal(pending.length, 3)
        await view.unmount()
        view = undefined
        assert.equal(pending[2].signal?.aborted, true)
        await act(async () => { pending[2].resolve(response('unmounted response', 200)) })
      } finally {
        await view?.unmount()
        setupApi.settingsSchema = originalSchema
        setupApi.settingsState = originalState
        globalThis.fetch = originalFetch
      }
    })
  }
}

test('refreshing the notification feed preserves unsaved delivery settings', async () => {
  installTestDom()
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { default: NotificationsSettingsPage } = await import('./page')
  const originalSchema = setupApi.settingsSchema
  const originalState = setupApi.settingsState
  const originalFetch = globalThis.fetch
  const field: SettingsFieldSpec = {
    key: 'notifications.enabled', label: 'Enabled', description: '',
    section: 'notifications', backend: 'config_toml', control: 'bool',
    risk: 'restart', write_policy: 'editable', apply_mode: 'restart',
    secret: false, required: false, env_override: null, min: null, max: null,
    options: [], example: null,
  }
  setupApi.settingsSchema = async () => ({ schema_version: 1, sections: [], fields: [field] })
  setupApi.settingsState = async () => ({
    schema_version: 1, config_path: '/tmp/config.toml', env_path: '/tmp/.env',
    section: 'notifications', values: { 'notifications.enabled': false }, sources: {},
  })
  let feedAvailable = true
  globalThis.fetch = async () => new Response(
    JSON.stringify(feedAvailable ? { notifications: [] } : { message: 'feed unavailable' }),
    { status: feedAvailable ? 200 : 503 },
  )

  let view: Awaited<ReturnType<typeof renderClient>> | undefined
  try {
    view = await renderClient(<NotificationsSettingsPage />)
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    const toggle = view.container.querySelector('[role="switch"]')
    assert.ok(toggle)
    await act(async () => {
      toggle.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    })
    assert.equal(toggle.getAttribute('aria-checked'), 'true')
    assert.match(view.container.textContent ?? '', /Confirm settings write/)

    const refresh = [...view.container.querySelectorAll('button')].find((button) => button.textContent?.includes('Refresh'))
    assert.ok(refresh)
    for (const available of [false, true]) {
      feedAvailable = available
      await act(async () => {
        refresh.dispatchEvent(new MouseEvent('click', { bubbles: true }))
        await new Promise((resolve) => setTimeout(resolve, 0))
      })
      assert.equal(toggle.getAttribute('aria-checked'), 'true')
      assert.match(view.container.textContent ?? '', /Confirm settings write/)
      if (available) assert.doesNotMatch(view.container.textContent ?? '', /feed unavailable/)
      else assert.match(view.container.textContent ?? '', /feed unavailable/)
    }
  } finally {
    await view?.unmount()
    setupApi.settingsSchema = originalSchema
    setupApi.settingsState = originalState
    globalThis.fetch = originalFetch
  }
})
