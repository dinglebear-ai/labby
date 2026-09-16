// Tests for the schema-backed settings client contract.

import test from 'node:test'
import assert from 'node:assert/strict'

import {
  MOCK_ENV_SCHEMA,
  MOCK_SETTINGS_SCHEMA,
  setupApi,
  type SettingsState,
  type SettingsUpdateEntry,
} from './setup-client'

function isSettingsState(v: unknown): v is SettingsState {
  if (typeof v !== 'object' || v === null) return false
  const obj = v as Record<string, unknown>
  return (
    typeof obj.schema_version === 'number' &&
    typeof obj.config_path === 'string' &&
    typeof obj.env_path === 'string' &&
    typeof obj.section === 'string' &&
    typeof obj.values === 'object' &&
    obj.values !== null &&
    typeof obj.sources === 'object' &&
    obj.sources !== null
  )
}

test('settings schema carries risk source and write policy metadata', () => {
  const field = MOCK_SETTINGS_SCHEMA.fields.find((item) => item.key === 'services.built_in_upstream_apis_enabled')
  assert.equal(field?.write_policy, 'editable')
  assert.equal(field?.apply_mode, 'immediate')
  assert.equal(field?.backend, 'config_toml')
})

test('env schema marks token secret and not editable', () => {
  const token = MOCK_ENV_SCHEMA.find((item) => item.key === 'LABBY_MCP_HTTP_TOKEN')
  assert.equal(token?.secret, true)
  assert.equal(token?.editable, false)
})

test('settings state contract is section scoped', () => {
  const state: SettingsState = {
    schema_version: 1,
    config_path: '~/.config/labby/config.toml',
    env_path: '~/.labby/.env',
    section: 'core',
    values: { LABBY_LOG: 'labby=info' },
    sources: { LABBY_LOG: { source: 'env', overridden_by_env: null } },
  }
  assert.equal(isSettingsState(state), true)
  assert.equal(state.section, 'core')
})

test('settings update entries support explicit unset', () => {
  const entry: SettingsUpdateEntry = { key: 'mcp.port', value: null, previous: 8765, unset: true }
  assert.equal(entry.unset, true)
})

test('settingsConfigUpdate sends confirm and entries through setup action transport', async () => {
  const originalFetch = globalThis.fetch
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      state: {
        schema_version: 1,
        config_path: '/tmp/config.toml',
        env_path: '/tmp/.env',
        section: 'surfaces',
        values: {},
        sources: {},
      },
      backup_path: null,
    }), { status: 200 })
  }) as typeof fetch

  try {
    const entries: SettingsUpdateEntry[] = [{ key: 'mcp.port', value: 8766, previous: 8765 }]
    await setupApi.settingsConfigUpdate('surfaces', entries, true)
    assert.deepEqual(requestBody, {
      action: 'settings.config.update',
      params: { section: 'surfaces', entries, confirm: true },
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('publicProxyRender uses the shared setup action and keeps advanced format selection explicit', async () => {
  const originalFetch = globalThis.fetch
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      public_origin: 'https://labby.example.com',
      backend_origin: 'http://127.0.0.1:8765',
      oauth_callback_url: 'https://labby.example.com/auth/google/callback',
      mcp_url: 'https://labby.example.com/mcp',
      recommended: 'caddy',
      configs: { nginx: 'server {}' },
      verification: ['curl --fail-with-body https://labby.example.com/health'],
    }), { status: 200 })
  }) as typeof fetch

  try {
    const result = await setupApi.publicProxyRender('https://labby.example.com', { format: 'nginx' })
    assert.equal(result.recommended, 'caddy')
    assert.equal(result.oauth_callback_url, 'https://labby.example.com/auth/google/callback')
    assert.equal(result.mcp_url, 'https://labby.example.com/mcp')
    assert.deepEqual(requestBody, {
      action: 'public_proxy.render',
      params: { public_url: 'https://labby.example.com', format: 'nginx' },
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('Tailscale Funnel client keeps inspection read-only and mutations explicit', async () => {
  const originalFetch = globalThis.fetch
  const requestBodies: unknown[] = []
  globalThis.fetch = (async (_input, init) => {
    const body = JSON.parse(String(init?.body)) as { action: string }
    requestBodies.push(body)
    if (body.action === 'tailscale_funnel.inspect') {
      return new Response(JSON.stringify({
        cli_available: true,
        version: '1.90.0',
        backend_running: true,
        online: true,
        dns_name: 'labby.tailnet.ts.net',
        public_origin: 'https://labby.tailnet.ts.net',
        https_port: 443,
        funnel_status_readable: true,
        configured_backend: null,
        https_enabled: true,
        funnel_enabled: true,
        activation_required: false,
        ready_to_configure: true,
        blockers: [],
        verification: [],
      }), { status: 200 })
    }
    return new Response(JSON.stringify({
      changed: true,
      configured: body.action === 'tailscale_funnel.configure',
      public_origin: 'https://labby.tailnet.ts.net:8443',
      backend_origin: 'http://127.0.0.1:8765',
      https_port: 8443,
      oauth_callback_url: 'https://labby.tailnet.ts.net:8443/auth/google/callback',
      mcp_url: 'https://labby.tailnet.ts.net:8443/mcp',
      activation_required: false,
      activation_url: null,
      activation_message: null,
      verification: [],
    }), { status: 200 })
  }) as typeof fetch

  try {
    await setupApi.tailscaleFunnelInspect()
    await setupApi.tailscaleFunnelConfigure({ httpsPort: 8443 })
    await setupApi.tailscaleFunnelDisable({ backendUrl: 'http://127.0.0.1:8765', httpsPort: 8443 })
    assert.deepEqual(requestBodies, [
      { action: 'tailscale_funnel.inspect', params: { https_port: 443 } },
      { action: 'tailscale_funnel.configure', params: { https_port: 8443 } },
      {
        action: 'tailscale_funnel.disable',
        params: { backend_url: 'http://127.0.0.1:8765', https_port: 8443 },
      },
    ])
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('settingsEnvUpdate sends confirm and entries through setup action transport', async () => {
  const originalFetch = globalThis.fetch
  let requestBody: unknown
  globalThis.fetch = (async (_input, init) => {
    requestBody = JSON.parse(String(init?.body))
    return new Response(JSON.stringify({
      schema_version: 1,
      config_path: '/tmp/config.toml',
      env_path: '/tmp/.env',
      section: 'core',
      values: {},
      sources: {},
    }), { status: 200 })
  }) as typeof fetch

  try {
    const entries: SettingsUpdateEntry[] = [{ key: 'LABBY_LOG', value: 'labby=debug', previous: 'labby=info' }]
    await setupApi.settingsEnvUpdate('core', entries, true)
    assert.deepEqual(requestBody, {
      action: 'settings.env.update',
      params: { section: 'core', entries, confirm: true },
    })
  } finally {
    globalThis.fetch = originalFetch
  }
})
