import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { act } from 'react'

import { installTestDom } from '@/lib/testing/dom-install'
import { setupApi } from '@/lib/api/setup-client'

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
