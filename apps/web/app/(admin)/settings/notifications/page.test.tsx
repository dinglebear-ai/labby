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
