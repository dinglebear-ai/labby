import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { installTestDom } from '@/lib/testing/dom-install'
import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store'
import { tailcatSetupApi, SetupApiError, type TailcatConfiguration } from '@/lib/api/setup-client'

installTestDom()
Object.defineProperty(globalThis, 'HTMLInputElement', { configurable: true, value: window.HTMLInputElement })
Object.defineProperty(globalThis, 'self', { configurable: true, value: window })
async function renderSetup() {
  const { renderClient } = await import('@/lib/testing/dom-test-utils')
  const { TailcatSetup } = await import('./TailcatSetup')
  const view = await renderClient(<TailcatSetup />)
  return { ...view, refresh: () => view.rerender(<TailcatSetup />) }
}
const originals = [tailcatSetupApi.tailcatConfigure, tailcatSetupApi.tailcatEnroll, tailcatSetupApi.tailcatEnable] as const
test.afterEach(() => {
  [tailcatSetupApi.tailcatConfigure, tailcatSetupApi.tailcatEnroll, tailcatSetupApi.tailcatEnable] = originals
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
})
function session(projectId = 'project-a', configured = true) {
  __setBrowserSessionStateForTests({ status: 'authenticated', user: { sub: 'owner' }, expiresAt: Date.now() + 60000,
    csrfToken: 'csrf', projectId, isAdmin: true, isConfiguredAdmin: configured })
}
const configured: TailcatConfiguration = { configured: true, changed: true, dry_run: false, project_id: 'project-a',
  upstream: 'tailcat-a', loadout: 'tailcat-a', route: 'tailcat-a', controller_enabled: false,
  restart_required: true, credential_enrollment_required: true, service_installed: false }
function button(container: HTMLElement, label: string) {
  return [...container.querySelectorAll('button')].find(item => item.textContent === label)!
}
async function fill(container: HTMLElement) {
  await act(async () => {
    for (const [id, value] of [['tailcat-resource', 'https://labby.example/sandbox'], ['tailcat-relay', 'https://tailcat.example/map'], ['tailcat-node', '/fixture/node']]) {
      const input = container.querySelector(`#${id}`) as HTMLInputElement
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, value)
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.dispatchEvent(new Event('change', { bubbles: true }))
    }
  })
}

test('a platform administrator without configured operator identity receives no enabled setup controls', async () => {
  session('project-a', false)
  const view = await renderSetup()
  try {
    assert.match(view.container.textContent ?? '', /Sign in as the configured operator/)
    assert.equal([...view.container.querySelectorAll('button')].every(item => item.disabled), true)
  } finally { await view.unmount() }
})

test('enrollment retry retains the operation key and surfaces the backend denial', async () => {
  session()
  tailcatSetupApi.tailcatConfigure = async () => configured
  const keys: string[] = []
  tailcatSetupApi.tailcatEnroll = async (_project, key) => {
    keys.push(key)
    if (keys.length === 1) throw new SetupApiError('Assign the exact Tailcat project loadout first.', 403, 'tailcat_assignment_required')
    return { enrolled: true, credential_id: 'credential-a', credential_file: '/private/credential-a', expires_at: 2000000000,
      controller_enabled: false, restart_required: true }
  }
  const view = await renderSetup()
  try {
    await fill(view.container)
    await act(async () => button(view.container, 'Save configuration').click())
    await act(async () => button(view.container, 'Enroll credential').click())
    assert.match(view.container.querySelector('[role=alert]')?.textContent ?? '', /Assign the exact/)
    await act(async () => button(view.container, 'Enroll credential').click())
    assert.equal(keys.length, 2)
    assert.equal(keys[0], keys[1])
    assert.match(view.container.textContent ?? '', /\/private\/credential-a/)
    assert.equal(button(view.container, 'Enable Tailcat').disabled, false)
  } finally { await view.unmount() }
})

test('a dry-run preview never starts enrollment or enables the controller', async () => {
  session()
  let preview: boolean | undefined
  let enrolled = false
  tailcatSetupApi.tailcatEnroll = async () => { enrolled = true; throw Error('unexpected enrollment') }
  tailcatSetupApi.tailcatConfigure = async request => { preview = request.dry_run; return { ...configured, dry_run: true } }
  const view = await renderSetup()
  try {
    await fill(view.container)
    await act(async () => button(view.container, 'Preview configuration').click())
    assert.equal(preview, true)
    assert.match(view.container.textContent ?? '', /No configuration was written/)
    assert.equal(enrolled, false)
    assert.equal(button(view.container, 'Enable Tailcat').disabled, true)
  } finally { await view.unmount() }
})

test('a project switch aborts enrollment and discards a late response', async () => {
  session()
  tailcatSetupApi.tailcatConfigure = async () => configured
  let finish!: () => void
  let signal: AbortSignal | undefined
  tailcatSetupApi.tailcatEnroll = async (_project, _key, requestedSignal) => {
    signal = requestedSignal
    await new Promise<void>(resolve => { finish = resolve })
    return { enrolled: true, credential_id: 'old', credential_file: '/private/old-project', expires_at: 2000000000,
      controller_enabled: false, restart_required: true }
  }
  const view = await renderSetup()
  try {
    await fill(view.container)
    await act(async () => button(view.container, 'Save configuration').click())
    await act(async () => button(view.container, 'Enroll credential').click())
    await act(async () => session('project-b'))
    await view.refresh()
    assert.equal(signal?.aborted, true)
    await act(async () => { finish(); await new Promise(resolve => setTimeout(resolve, 0)) })
    assert.doesNotMatch(view.container.textContent ?? '', /old-project/)
    assert.equal(button(view.container, 'Enable Tailcat').disabled, true)
    assert.equal(button(view.container, 'Enroll credential').disabled, false)
  } finally { await view.unmount() }
})

test('a refreshed page can enroll against live policy and enable a retained credential ID', async () => {
  session()
  let configurations = 0
  tailcatSetupApi.tailcatConfigure = async () => { configurations++; return configured }
  tailcatSetupApi.tailcatEnroll = async () => ({ enrolled: true, credential_id: 'retained-id', credential_file: '/private/retained',
    expires_at: 2000000000, controller_enabled: false, restart_required: true })
  const enabled: Array<[string, string]> = []
  tailcatSetupApi.tailcatEnable = async (project, credential) => {
    enabled.push([project, credential])
    return { enabled: true, changed: true, restart_required: true }
  }
  const first = await renderSetup()
  await act(async () => button(first.container, 'Enroll credential').click())
  assert.equal((first.container.querySelector('#tailcat-credential-id') as HTMLInputElement).value, 'retained-id')
  await first.unmount()
  const resumed = await renderSetup()
  try {
    await act(async () => {
      const input = resumed.container.querySelector('#tailcat-credential-id') as HTMLInputElement
      Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, 'retained-id')
      input.dispatchEvent(new Event('input', { bubbles: true }))
      input.dispatchEvent(new Event('change', { bubbles: true }))
    })
    await act(async () => button(resumed.container, 'Enable Tailcat').click())
    assert.deepEqual(enabled, [['project-a', 'retained-id']])
    assert.equal(configurations, 0)
    assert.match(resumed.container.textContent ?? '', /Controller enabled in configuration/)
    assert.doesNotMatch(resumed.container.textContent ?? '', /\/private\/retained/)
  } finally { await resumed.unmount() }
})
