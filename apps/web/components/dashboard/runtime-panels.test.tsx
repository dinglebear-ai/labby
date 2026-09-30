import test from 'node:test'
import assert from 'node:assert/strict'
import { renderToStaticMarkup } from 'react-dom/server'
import { ConnectedClientsPanel, GatewayHostPanel, connectionAge, overviewHealthUrl, overviewRuntimeKey } from './runtime-panels'

test('connection ages are bounded and render elapsed units', () => {
  assert.equal(connectionAge(-60), '0m')
  assert.equal(connectionAge(3660), '1h 1m')
  assert.equal(connectionAge(180000), '2d 2h')
})

test('host panel keeps metrics unavailable while showing supplied health and inbound count', () => {
  const html = renderToStaticMarkup(<GatewayHostPanel health={{ status: 'ok', pid: 4321, uptime_s: 3660 }} clients={[]} />)
  assert.ok(html.includes('up 1h 1m'))
  assert.ok(html.includes('0 observed MCP sessions'))
  assert.equal((html.match(/Unavailable/g) ?? []).length, 5)
  assert.doesNotMatch(html, /tootie|linux\/amd64|14d 6h/)
})

test('host panel reports initial sampling without claiming measurements are unavailable', () => {
  const html = renderToStaticMarkup(<GatewayHostPanel loading />)
  assert.equal((html.match(/Sampling…/g) ?? []).length, 5)
  assert.doesNotMatch(html, /Unavailable/)
})

test('client rows preserve observed transport and version without displaying subject', () => {
  const html = renderToStaticMarkup(<ConnectedClientsPanel clients={[{ subject: 'private-subject', authorized_client_id: null, client_name: 'Example client', client_version: '1.2', transport: 'http', connected_at: 'invalid' }]} />)
  assert.ok(html.includes('Example client'))
  assert.ok(html.includes('v1.2 · http'))
  assert.doesNotMatch(html, /private-subject/)
})

test('unavailable clients never render as an empty live population', () => {
  const html = renderToStaticMarkup(<ConnectedClientsPanel unavailable />)
  assert.ok(html.includes('Connected clients are unavailable.'))
  assert.doesNotMatch(html, /No connected clients observed/)
})


test('health stays on the configured API host and reverse proxy prefix', () => {
  assert.equal(overviewHealthUrl('/v1', 'https://local.example/overview/'), 'https://local.example/health')
  assert.equal(overviewHealthUrl('https://remote.example/labby/v1', 'https://local.example/'), 'https://remote.example/labby/health')
  assert.equal(overviewHealthUrl('https://remote.example/labby/v1/gateway', 'https://local.example/'), 'https://remote.example/labby/health')
})

test('runtime cache identity changes for authority, epoch and API target', () => {
  const key = overviewRuntimeKey('clients', '/v1', 'team-a', 2)
  assert.deepEqual(key, ['clients', '/v1', 'team-a', 2])
  assert.notDeepEqual(key, overviewRuntimeKey('clients', '/v1', 'team-b', 2))
  assert.notDeepEqual(key, overviewRuntimeKey('clients', '/v1', 'team-a', 3))
  assert.notDeepEqual(key, overviewRuntimeKey('clients', 'https://remote.example/v1', 'team-a', 2))
})


test('host panel renders measured resources and network units', () => {
  const html = renderToStaticMarkup(<GatewayHostPanel metrics={{ available: true, scope: 'daemon-visible host', cpu_percent: 25, memory_used_bytes: 1073741824, memory_total_bytes: 2147483648, disk_used_bytes: 2147483648, disk_total_bytes: 4294967296, network_rx_bytes_per_second: 1024, network_tx_bytes_per_second: 2048, sample_ms: 5000, hostname: 'gateway-host', platform: 'linux/x86_64', cpu_cores: 8, memory_limit_is_cgroup: true, child_process_count: 5, child_rss_bytes: 536870912 }} />)
  assert.ok(html.includes('25.0% · 8 cores'))
  assert.ok(html.includes('1.0 GiB / 2.0 GiB'))
  assert.ok(html.includes('2.0 GiB / 4.0 GiB'))
  assert.ok(html.includes('5 child processes'))
  assert.ok(html.includes('512.0 MiB child RSS'))
  assert.ok(html.includes('5.0s average'))
  assert.ok(html.includes('gateway-host · linux/x86_64'))
  assert.ok(html.includes('1.0 GiB'))
  assert.ok(html.includes('1.0 KiB/s'))
  assert.ok(html.includes('2.0 KiB/s'))
})
