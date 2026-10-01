import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom } from '@/lib/testing/dom-install'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import { parseAuthoritySnapshot } from '@/lib/auth/authority'
import { setupApi, type SettingsState } from '@/lib/api/setup-client'
installTestDom()
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
const originals = [setupApi.settingsSchema, setupApi.settingsState, setupApi.settingsEnvUpdate] as const
const originalFetch = globalThis.fetch
test.afterEach(() => {
  [setupApi.settingsSchema, setupApi.settingsState, setupApi.settingsEnvUpdate] = originals
  globalThis.fetch = originalFetch
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})
for (const delayed of [false, true]) test(`saving provider invalidates ${delayed ? 'pending' : 'completed'} probe`, async () => {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, csrfToken: 'csrf', expiresAt: Date.now() + 100000, authority: parseAuthoritySnapshot({ owner: { kind: 'personal', id: 'owner' }, organization_id: 'org', teams: [], projects: [], capabilities: ['platform.manage'], authority_generation: 1 }) })
  const state: SettingsState = { schema_version: 1, config_path: '/fixture/config.toml', env_path: '/fixture/.env', section: 'agents', values: { LABBY_PHOENIX_OPENAI_BASE_URL: 'https://a.example/v1' }, sources: {} }
  setupApi.settingsState = async () => state
  setupApi.settingsSchema = async () => ({ schema_version: 1, sections: [], fields: [{ key: 'LABBY_PHOENIX_OPENAI_BASE_URL', label: 'Provider URL', description: 'API called by Labby.', section: 'agents', backend: 'env', control: 'text', risk: 'low', write_policy: 'editable', apply_mode: 'immediate', secret: false, required: false, env_override: null, min: null, max: null, options: [], example: null }] })
  setupApi.settingsEnvUpdate = async () => ({ ...state, values: { LABBY_PHOENIX_OPENAI_BASE_URL: 'https://b.example/v1' } })
  let finish!: () => void
  let signal: AbortSignal | null | undefined
  globalThis.fetch = async (_url, init) => {
    signal = init?.signal
    if (delayed) await new Promise<void>(resolve => { finish = resolve })
    return Response.json({ models: ['model-a'] })
  }
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { default: Page } = await import('./page')
  const view = await renderClient(<Page />)
  const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 20)) })
  const button = (label: string) => [...view.container.querySelectorAll('button')].find(item => item.textContent?.includes(label))!
  try {
    await settle()
    await act(async () => button('Check models').click())
    if (!delayed) assert.match(view.container.textContent ?? '', /listed 1 available model/)
    const input = view.container.querySelector('input')!
    await act(async () => {
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, 'https://b.example/v1')
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.dispatchEvent(new Event('change', { bubbles: true }))
    })
    await act(async () => (view.container.querySelector('[role=checkbox]') as HTMLElement).click())
    await act(async () => button('Save changes').click())
    assert.equal(signal?.aborted, true)
    if (delayed) await act(async () => { finish(); await new Promise(resolve => setTimeout(resolve, 10)) })
    assert.doesNotMatch(view.container.textContent ?? '', /listed 1 available model/)
    assert.equal(button('Check models').disabled, false)
  } finally { await view.unmount() }
})
