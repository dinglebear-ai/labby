import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import {
  resolveGatewayEndpoint,
  SettingsOverviewCards,
  type SettingsOverviewSnapshot,
} from './SettingsOverview'

test('settings overview only reports an explicitly configured MCP endpoint', () => {
  assert.equal(
    resolveGatewayEndpoint({ LABBY_MCP_GATEWAY_URL: 'https://mcp.example.test/' }),
    'https://mcp.example.test/mcp',
  )
  assert.equal(
    resolveGatewayEndpoint({ 'public_urls.mcp_gateway': 'https://mcp.example.test/mcp/?x=1#y' }),
    'https://mcp.example.test/mcp',
  )
  // The app origin is a separate entrypoint and must never be guessed as the MCP endpoint.
  assert.equal(resolveGatewayEndpoint({ 'public_urls.app': 'https://lab.example.test' }), undefined)
  assert.equal(resolveGatewayEndpoint({ LABBY_PUBLIC_URL: 'https://lab.example.test' }), undefined)
  assert.equal(resolveGatewayEndpoint({}), undefined)
})

test('settings overview follows the Gateway, Console, Diagnostics order', async () => {
  installTestDom()
  const snapshot: SettingsOverviewSnapshot = {
    codeModeEnabled: true,
    gatewayEndpoint: 'https://labby.example.test/mcp',
    configPath: '~/.config/labby/gateway.toml',
    errors: [],
  }
  const view = await renderClient(
    <SettingsOverviewCards snapshot={snapshot} console={<section data-test-console>Console</section>} />,
  )
  try {
    const text = view.container.textContent ?? ''
    assert.ok(text.indexOf('Gateway') < text.indexOf('Console'))
    assert.ok(text.indexOf('Console') < text.indexOf('Diagnostics'))
    assert.match(text, /Gateway Endpoint/)
    assert.match(text, /https:\/\/labby\.example\.test\/mcp/)
    assert.match(text, /Config Path/)
    const switches = Array.from(view.container.querySelectorAll('[role="switch"]'))
    assert.deepEqual(switches.map((element) => element.getAttribute('aria-label')), ['Code Mode'])
    assert.ok(switches.every((element) => element.getAttribute('aria-readonly') === 'true'))
    assert.equal(view.container.querySelector('[role="alert"]'), null)
    const values = Array.from(view.container.querySelectorAll('code'))
    assert.equal(values.length, 2)
    assert.ok(values.every((element) => element.style.lineHeight === 'normal'))
  } finally {
    await view.unmount()
  }
})

test('settings overview reports unread sections instead of asserting defaults', async () => {
  installTestDom()
  const view = await renderClient(
    <SettingsOverviewCards snapshot={{ errors: ['Features: HTTP 401'] }} console={null} />,
  )
  try {
    const text = view.container.textContent ?? ''
    assert.equal(view.container.querySelector('[role="switch"]'), null)
    assert.match(text, /Not reported/)
    assert.match(text, /Not configured/)
    assert.match(view.container.querySelector('[role="alert"]')?.textContent ?? '', /Features: HTTP 401/)
  } finally {
    await view.unmount()
  }
})
