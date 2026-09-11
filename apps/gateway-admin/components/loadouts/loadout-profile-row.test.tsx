import test from 'node:test'
import assert from 'node:assert/strict'
import React, { act } from 'react'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import { LoadoutProfileLayout } from './loadout-profile-row'
import type { GatewayLoadout, ProtectedMcpRoute } from '@/lib/types/gateway'
import { renderToStaticMarkup } from 'react-dom/server'

const loadout: GatewayLoadout = { name: 'fixture-profile', upstreams: ['example'], services: [], expose_code_mode: false, expose_tools: true, expose_resources: false, expose_prompts: false, expose_skills: false }

test('profile table expands existing controls without inventing endpoint health or calls', async () => {
  installTestDom()
  const view = await renderClient(<LoadoutProfileLayout view="table" loadouts={[loadout]} routes={[]} routeStateUnavailable={false}><button>Existing edit control</button></LoadoutProfileLayout>)
  try {
    // No column promises call metrics that this tree does not collect.
    assert.doesNotMatch(view.container.textContent ?? '', /Calls 24h/)
    assert.match(view.container.textContent ?? '', /Not hosted/)
    assert.doesNotMatch(view.container.textContent ?? '', /Endpoint live|Existing edit control/)
    const toggle = view.container.querySelector('button')!
    assert.equal(toggle.style.backgroundColor, '', 'inline background must not suppress hover styles')
    assert.match(toggle.className, /py-2\.5/)
    assert.match(toggle.className, /hover:bg-/)
    await act(async () => toggle.click())
    assert.equal(toggle.getAttribute('aria-expanded'), 'true')
    assert.match(view.container.textContent ?? '', /Existing edit control/)
    assert.match(toggle.className, /aurora-accent-primary\)_6%/)
    await act(async () => toggle.click())
    assert.doesNotMatch(view.container.textContent ?? '', /Existing edit control/)
  } finally { await view.unmount() }
})

test('protected auth badge requires known matching route data', () => {
  const route: ProtectedMcpRoute = { name: 'fixture-route', enabled: true, public_host: 'fixture.example.invalid', public_path: '/mcp/demo', scopes: [], target: { kind: 'gateway_subset', loadout: loadout.name } }
  const render = (unavailable: boolean, routes: ProtectedMcpRoute[]) => renderToStaticMarkup(<LoadoutProfileLayout view="table" loadouts={[loadout]} routes={routes} routeStateUnavailable={unavailable}><span>Details</span></LoadoutProfileLayout>)
  const known = render(false, [route])
  assert.match(known, /Protected/)
  assert.match(known, /h-\[19px\]/)
  assert.match(known, /rounded-full/)
  assert.match(known, /fixture.example.invalid\/mcp\/demo/)
  const unavailable = render(true, [route])
  assert.match(unavailable, /Unavailable/)
  assert.doesNotMatch(unavailable, /Protected|fixture.example.invalid/)
  const unrelated = render(false, [{ ...route, target: { kind: 'gateway_subset', loadout: 'another-profile' } }])
  assert.match(unrelated, /Not hosted/)
  assert.doesNotMatch(unrelated, /Protected/)
})
