import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'

import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils'
import {
  resolveGatewayEndpoint,
  SettingsOverviewCards,
  type SettingsOverviewSnapshot,
} from './SettingsOverview'

test('settings overview resolves the effective public MCP endpoint', () => {
  assert.equal(
    resolveGatewayEndpoint({ LABBY_MCP_GATEWAY_URL: 'https://mcp.example.test/' }),
    'https://mcp.example.test/mcp',
  )
  assert.equal(
    resolveGatewayEndpoint({ 'public_urls.app': 'https://lab.example.test' }),
    'https://lab.example.test/mcp',
  )
  assert.equal(resolveGatewayEndpoint({}), 'Unavailable')
})

test('settings overview follows the mock Gateway, Console, Diagnostics order', async () => {
  installTestDom()
  const snapshot: SettingsOverviewSnapshot = {
    codeModeEnabled: true,
    gatewayEndpoint: 'https://labby.example.test/mcp',
    configPath: '~/.config/labby/gateway.toml',
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
    assert.deepEqual(switches.map((element) => element.getAttribute('aria-label')), [
      'Code Mode',
      'Auto-Reconnect',
      'Usage Telemetry',
    ])
    assert.ok(switches.every((element) => element.getAttribute('aria-readonly') === 'true'))
    const values = Array.from(view.container.querySelectorAll('code'))
    assert.equal(values.length, 2)
    assert.ok(values.every((element) => element.style.lineHeight === 'normal'))
  } finally {
    await view.unmount()
  }
})
