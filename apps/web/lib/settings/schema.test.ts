import test from 'node:test'
import assert from 'node:assert/strict'

import {
  buildDirtyEntries,
  buildDirtyEntriesByBackend,
  collectFieldInputErrors,
  isInvalidFieldInput,
  parseFieldInput,
  valueAsInputString,
} from './schema'
import type { SettingsFieldSpec } from '@/lib/api/setup-client'

const numberField: SettingsFieldSpec = {
  key: 'mcp.port',
  label: 'Port',
  description: '',
  section: 'surfaces',
  backend: 'config_toml',
  control: 'number',
  risk: 'restart',
  write_policy: 'editable',
  apply_mode: 'restart',
  secret: false,
  required: false,
  env_override: 'LABBY_MCP_HTTP_PORT',
  min: 1,
  max: 65535,
  options: [],
  example: '8765',
}

test('settings schema helpers parse scalar controls', () => {
  assert.equal(parseFieldInput(numberField, '8765'), 8765)
  assert.equal(parseFieldInput(numberField, ''), null)
  assert.equal(isInvalidFieldInput(parseFieldInput(numberField, '1.5')), true)
  assert.equal(isInvalidFieldInput(parseFieldInput(numberField, '70000')), true)
  assert.equal(parseFieldInput({ ...numberField, control: 'bool' }, true), true)
  assert.deepEqual(parseFieldInput({ ...numberField, control: 'string_list' }, 'a,b\nc'), ['a', 'b', 'c'])
})

test('settings inputs reject values outside the server schema', () => {
  const envPort = { ...numberField, backend: 'env' as const }
  assert.equal(isInvalidFieldInput(parseFieldInput(envPort, '')), true)
  const format = { ...numberField, control: 'enum' as const, options: [{ value: 'human', label: 'Readable text' }, { value: 'json', label: 'JSON' }] }
  assert.equal(parseFieldInput(format, 'json'), 'json')
  assert.equal(isInvalidFieldInput(parseFieldInput(format, 'yaml')), true)
  const publicUrl = { ...numberField, control: 'url' as const }
  assert.equal(parseFieldInput(publicUrl, 'https://example.com/labby'), 'https://example.com/labby')
  for (const invalid of ['https://', 'https://user:pass@example.com', 'https://example.com/?token=1', 'javascript:alert(1)']) {
    assert.equal(isInvalidFieldInput(parseFieldInput(publicUrl, invalid)), true, invalid)
  }
})

test('settings inputs validate host and browser origin lists', () => {
  const hosts = { ...numberField, key: 'mcp.allowed_hosts', control: 'string_list' as const }
  assert.deepEqual(parseFieldInput(hosts, 'example.com\nexample.com:8443\n::1'), ['example.com', 'example.com:8443', '::1'])
  for (const invalid of ['*', 'https://example.com', 'example.com/path', 'user@example.com']) {
    assert.equal(isInvalidFieldInput(parseFieldInput(hosts, invalid)), true, invalid)
  }
  const origins = { ...numberField, key: 'api.cors_origins', control: 'string_list' as const }
  assert.deepEqual(parseFieldInput(origins, 'https://example.com\nhttp://localhost:3000'), ['https://example.com', 'http://localhost:3000'])
  for (const invalid of ['*', 'example.com', 'https://example.com/path', 'https://user@example.com']) {
    assert.equal(isInvalidFieldInput(parseFieldInput(origins, invalid)), true, invalid)
  }
})

test('bind host settings accept addresses but reject ports and URLs', () => {
  const host = { ...numberField, key: 'LABBY_MCP_HTTP_HOST', control: 'text' as const }
  for (const valid of ['127.0.0.1', '::1', 'labby.local']) {
    assert.equal(parseFieldInput(host, valid), valid)
  }
  for (const invalid of ['', 'localhost:8765', 'localhost:80', '[::1]:80', 'https://labby.local', 'host/path']) {
    assert.equal(isInvalidFieldInput(parseFieldInput(host, invalid)), true, invalid)
  }
})

test('settings schema helpers surface invalid numeric errors without losing raw input', () => {
  const invalid = parseFieldInput(numberField, '70000')
  assert.equal(isInvalidFieldInput(invalid), true)
  assert.equal(valueAsInputString(invalid), '70000')
  assert.deepEqual(
    collectFieldInputErrors([numberField], new Set(['mcp.port']), { 'mcp.port': invalid }),
    { 'mcp.port': 'Must be at most 65535.' },
  )
})

test('settings schema helpers build dirty entries only for changed keys', () => {
  assert.deepEqual(buildDirtyEntries([numberField], new Set(['mcp.port']), { 'mcp.port': 8766 }, { 'mcp.port': 8765 }), [
    { key: 'mcp.port', value: 8766, previous: 8765 },
  ])
})

test('settings schema helpers emit unset for blank optional config fields', () => {
  const pathField: SettingsFieldSpec = {
    ...numberField,
    key: 'workspace.root',
    control: 'text',
    env_override: null,
    min: null,
    max: null,
  }
  assert.deepEqual(buildDirtyEntries([pathField], new Set(['workspace.root']), { 'workspace.root': '' }, { 'workspace.root': '/srv/lab' }), [
    { key: 'workspace.root', value: null, previous: '/srv/lab', unset: true },
  ])
})

test('settings schema helpers partition dirty entries by backend', () => {
  const envField: SettingsFieldSpec = { ...numberField, key: 'LABBY_MCP_HTTP_PORT', backend: 'env' }
  const partitioned = buildDirtyEntriesByBackend(
    [numberField, envField],
    new Set(['mcp.port', 'LABBY_MCP_HTTP_PORT']),
    { 'mcp.port': 8766, LABBY_MCP_HTTP_PORT: 8767 },
    { 'mcp.port': 8765, LABBY_MCP_HTTP_PORT: 8765 },
  )
  assert.deepEqual(partitioned.configEntries, [{ key: 'mcp.port', value: 8766, previous: 8765 }])
  assert.deepEqual(partitioned.envEntries, [{ key: 'LABBY_MCP_HTTP_PORT', value: 8767, previous: 8765 }])
})

test('settings schema helpers exclude config fields shadowed by env overrides', () => {
  const partitioned = buildDirtyEntriesByBackend(
    [numberField],
    new Set(['mcp.port']),
    { 'mcp.port': 8766 },
    { 'mcp.port': 8765 },
    { 'mcp.port': { source: 'env', overridden_by_env: 'LABBY_MCP_HTTP_PORT' } },
  )
  assert.deepEqual(partitioned.configEntries, [])
  assert.deepEqual(partitioned.envEntries, [])
})

test('settings schema helpers exclude env fields the process environment overrides', () => {
  const envField: SettingsFieldSpec = {
    ...numberField,
    key: 'LABBY_MCP_HTTP_PORT',
    backend: 'env',
    env_override: null,
  }
  const partitioned = buildDirtyEntriesByBackend(
    [envField],
    new Set(['LABBY_MCP_HTTP_PORT']),
    { LABBY_MCP_HTTP_PORT: 8766 },
    { LABBY_MCP_HTTP_PORT: 8765 },
    { LABBY_MCP_HTTP_PORT: { source: 'env', overridden_by_env: 'LABBY_MCP_HTTP_PORT' } },
  )
  assert.deepEqual(partitioned.envEntries, [])
  assert.deepEqual(partitioned.configEntries, [])
})

test('settings schema helpers do not stringify arrays as objects', () => {
  assert.equal(valueAsInputString(['a', 'b']), 'a\nb')
})
