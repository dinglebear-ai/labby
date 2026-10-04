import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import type { Gateway, GatewayConfig } from '@/lib/types/gateway'
import { GatewayCompactCatalog } from './gateway-compact-catalog'
installTestDom()
const gateway = { id: 'cortex', name: 'cortex', config: { proxy_resources: false }, status: {}, discovery: { tools: [{ name: 'search', description: 'Search memory', exposed: true, matched_by: '*' }, { name: 'save', description: 'Save memory', exposed: false, matched_by: null }], prompts: [], resources: [{ name: 'memory', exposed: false }] } } as unknown as Gateway

test('compact catalog filters exposure and saves a precise per-tool allowlist', async () => {
  const writes: Partial<GatewayConfig>[] = []
  const view = await renderClient(<GatewayCompactCatalog gateway={gateway} onSave={async (config) => { writes.push(config) }} onAdvanced={() => {}} />)
  const hidden = [...view.container.querySelectorAll('button')].find((button) => button.textContent === 'hidden')!
  await act(async () => hidden.click())
  assert.equal(view.container.querySelector('[aria-label="Hide search"]'), null)
  const expose = view.container.querySelector<HTMLButtonElement>('[aria-label="Expose save"]')!
  assert.ok(expose)
  await act(async () => expose.click())
  assert.deepEqual(writes, [{ expose_tools: ['search', 'save'] }])
  const run = view.container.querySelector<HTMLButtonElement>('[aria-label="Run save: browser execution unavailable"]')!
  assert.equal(run.disabled, true)
  await view.unmount()
})

test('compact tabs show observation availability rather than empty inventory lengths', async () => {
  const unknown = { state: 'unknown' as const, discovered: null, exposed: null }
  const scoped = { ...gateway, discovery: { tools: [], resources: [], prompts: [] }, status: { ...gateway.status,
    discovered_skill_count: 999, capability_observation: { scope: 'credential' as const, tools: unknown,
      resources: { ...unknown, state: 'failed' as const }, prompts: { state: 'stale' as const, discovered: 2, exposed: 1 },
      skills: { state: 'known' as const, discovered: 0, exposed: 0 } } } }
  const view = await renderClient(<GatewayCompactCatalog gateway={scoped} onSave={async () => {}} onAdvanced={() => {}} />)
  try {
    const tab = (name: string) => [...view.container.querySelectorAll('button')].find(button => button.textContent?.startsWith(name))!
    assert.match(tab('tools').textContent ?? '', /Not discovered/)
    assert.match(tab('resources').textContent ?? '', /Discovery failed/)
    assert.match(tab('prompts').textContent ?? '', /1\/2 · stale/)
    assert.match(tab('skills').textContent ?? '', /0\/0/)
    await act(async () => tab('skills').click())
    assert.match(view.container.textContent ?? '', /Nothing of this type discovered/)
    assert.doesNotMatch(view.container.textContent ?? '', /reports a skill count/)
  } finally { await view.unmount() }
})

test('catalog cannot imply a disabled resource proxy exposes a resource', async () => {
  const view = await renderClient(<GatewayCompactCatalog gateway={gateway} onSave={async () => assert.fail('must not save')} onAdvanced={() => {}} />)
  const resources = [...view.container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('resources'))!
  await act(async () => resources.click())
  assert.equal(view.container.querySelector<HTMLButtonElement>('[aria-label="Expose memory"]')?.disabled, true)
  await view.unmount()
})
