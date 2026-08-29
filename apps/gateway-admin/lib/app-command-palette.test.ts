import assert from 'node:assert/strict'
import test from 'node:test'

import {
  appCommandItems,
  buildAppCommandState,
  buildAddServerInput,
  buildGatewayAlerts,
  buildPaletteCounts,
  countPaletteFilterMatches,
  detectPaletteAddTransport,
  gatewayMatchesPaletteFilters,
  paletteFiltersActive,
  parsePaletteEnvPairs,
  togglePaletteFilter,
  buildPaletteFooterLabel,
  describeGatewayConnection,
  findAppCommandItemById,
  paletteScopeShows,
  parsePaletteScope,
  type PaletteServerFilters,
} from './app-command-palette'

test('app command palette ranks server searches first', () => {
  const state = buildAppCommandState('server')

  assert.equal(state.activeItemId, 'destination-gateways')
  assert.equal(state.groups[0]?.key, 'best-match')
  assert.equal(state.groups[0]?.items[0]?.href, '/gateways')
  assert.equal(state.groups[0]?.items[0]?.title, 'Gateway')
})

test('app command palette includes core admin destinations', () => {
  const hrefs = new Set(appCommandItems.map((item) => item.href))

  for (const href of [
    '/',
    '/gateways',
    '/snippets',
    '/sessions',
    '/tasks',
    '/logs',
    '/discovery',
    '/create',
    '/library',
    '/agents',
    '/stash',
    '/containers',
    '/instance',
    '/team',
    '/team/library',
    '/team/projects',
    '/team/activity',
    '/team/stash',
    '/usage',
    '/settings',
    '/docs',
  ]) {
    assert.equal(hrefs.has(href), true, `${href} should be searchable`)
  }

  // Removed surfaces (no backing service): must not be advertised as destinations.
  for (const href of ['/marketplace', '/chat', '/setup', '/activity', '/registry']) {
    assert.equal(hrefs.has(href), false, `${href} should not be searchable — surface was removed`)
  }
})

test('app command palette reports empty state for unmatched queries', () => {
  const state = buildAppCommandState('zzzz-no-match')

  assert.equal(state.activeItemId, null)
  assert.deepEqual(state.items, [])
  assert.deepEqual(state.groups, [])
})

test('findAppCommandItemById returns matching command item', () => {
  const item = findAppCommandItemById('destination-usage', appCommandItems)

  assert.equal(item?.title, 'Usage')
  assert.equal(item?.href, '/usage')
})

test('mock-only destinations identify themselves in palette copy', () => {
  for (const id of [
    'destination-sessions', 'destination-tasks', 'destination-logs',
    'destination-discovery', 'destination-create', 'destination-library',
    'destination-agents', 'destination-stash', 'destination-containers', 'destination-instance',
    'destination-team-overview', 'destination-team-library', 'destination-team-projects',
    'destination-team-activity', 'destination-team-stash',
  ]) {
    const item = findAppCommandItemById(id, appCommandItems)
    assert.ok(item)
    assert.match(item.title, /Mock/)
    assert.match(item.description, /mock data/i)
    assert.equal(item.actionHint, 'Open mock')
  }
})

test('parsePaletteScope strips recognised prefixes', () => {
  assert.deepEqual(parsePaletteScope('>reload'), { scope: 'actions', query: 'reload' })
  assert.deepEqual(parsePaletteScope('# mcp'), { scope: 'servers', query: 'mcp' })
  assert.deepEqual(parsePaletteScope('/usage'), { scope: 'pages', query: 'usage' })
  assert.deepEqual(parsePaletteScope('  gateway '), { scope: null, query: 'gateway' })
  // `@` scopes agent sessions in the mock; this console has no such surface.
  assert.deepEqual(parsePaletteScope('@codex'), { scope: null, query: '@codex' })
})

test('paletteScopeShows gates sections by active scope', () => {
  assert.equal(paletteScopeShows(null, 'servers'), true)
  assert.equal(paletteScopeShows('servers', 'servers'), true)
  assert.equal(paletteScopeShows('servers', 'actions'), false)
})

test('buildPaletteCounts drops empty buckets and footer summarises matches', () => {
  assert.deepEqual(buildPaletteCounts({ servers: 5, actions: 0, pages: 2, alerts: 1 }), [
    { key: 'Servers', value: 5 },
    { key: 'Pages', value: 2 },
    { key: 'Alerts', value: 1 },
  ])

  assert.equal(
    buildPaletteFooterLabel({ servers: 5, actions: 1, pages: 0, alerts: 0 }),
    '5 servers · 1 action match',
  )
  assert.equal(
    buildPaletteFooterLabel({ servers: 0, actions: 0, pages: 0, alerts: 0 }),
    'No matches',
  )
})

test('describeGatewayConnection maps live status to the mock vocabulary', () => {
  assert.deepEqual(
    describeGatewayConnection({ status: { healthy: true, connected: true } }),
    { label: 'healthy', tone: 'success' },
  )
  assert.deepEqual(
    describeGatewayConnection({ enabled: false, status: { healthy: true, connected: true } }),
    { label: 'disabled', tone: 'muted' },
  )
  assert.deepEqual(
    describeGatewayConnection({
      status: { healthy: false, connected: false, last_error: 'Connection refused' },
    }),
    { label: 'disconnected', tone: 'error' },
  )
  assert.deepEqual(
    describeGatewayConnection({
      status: { healthy: false, connected: false, last_error: 'HTTP 401 Unauthorized' },
    }),
    { label: 'needs auth', tone: 'warn' },
  )
  assert.deepEqual(
    describeGatewayConnection({
      status: { healthy: false, connected: true },
      warnings: [{ code: 'DISCOVERY_FAILED', message: 'tools/list timed out' }],
    }),
    { label: 'degraded', tone: 'warn' },
  )
})

test('buildGatewayAlerts surfaces only unhealthy enabled gateways, capped', () => {
  const alerts = buildGatewayAlerts([
    { id: 'a', name: 'Aurora', status: { healthy: true, connected: true } },
    { id: 'b', name: 'mcp.sh', status: { healthy: false, connected: false } },
    { id: 'c', name: 'Unifi', enabled: false, status: { healthy: false, connected: false } },
    {
      id: 'd',
      name: 'unRAID',
      status: { healthy: false, connected: false, last_error: '401 unauthorized' },
    },
  ])

  assert.deepEqual(alerts, [
    { id: 'alert-b', gatewayId: 'b', label: 'mcp.sh disconnected', tone: 'error' },
    { id: 'alert-d', gatewayId: 'd', label: 'unRAID needs auth', tone: 'warn' },
  ])

  const capped = buildGatewayAlerts(
    ['a', 'b', 'c', 'd'].map((id) => ({
      id,
      name: id,
      status: { healthy: false, connected: false },
    })),
  )
  assert.equal(capped.length, 3)
})

test('detectPaletteAddTransport splits URLs from stdio commands', () => {
  assert.equal(detectPaletteAddTransport('https://mcp.example/mcp'), 'http')
  assert.equal(detectPaletteAddTransport('HTTP://mcp.example/mcp'), 'http')
  assert.equal(detectPaletteAddTransport('uvx my-mcp-server --flag'), 'stdio')
  assert.equal(detectPaletteAddTransport('   '), null)
})

test('parsePaletteEnvPairs drops malformed entries', () => {
  assert.deepEqual(parsePaletteEnvPairs('A=1, B = two ,broken, =nokey'), { A: '1', B: 'two' })
  assert.deepEqual(parsePaletteEnvPairs(''), {})
})

test('buildAddServerInput produces a real CreateGatewayInput', () => {
  const base = {
    name: '',
    auth: 'none' as const,
    tokenEnv: '',
    env: '',
    proxyResources: true,
    proxyPrompts: false,
  }

  assert.deepEqual(
    buildAddServerInput({ ...base, target: 'https://mcp.example/mcp', name: 'example' }),
    {
      name: 'example',
      transport: 'http',
      config: { url: 'https://mcp.example/mcp', proxy_resources: true, proxy_prompts: false },
    },
  )

  assert.deepEqual(
    buildAddServerInput({
      ...base,
      target: 'https://mcp.example/mcp',
      auth: 'bearer',
      tokenEnv: 'EXAMPLE_TOKEN',
    }),
    {
      // Name falls back to the endpoint hostname when left blank.
      name: 'mcp.example',
      transport: 'http',
      config: {
        url: 'https://mcp.example/mcp',
        bearer_token_env: 'EXAMPLE_TOKEN',
        proxy_resources: true,
        proxy_prompts: false,
      },
    },
  )

  assert.deepEqual(
    buildAddServerInput({ ...base, target: 'https://mcp.example/mcp', auth: 'oauth', name: 'x' }),
    {
      name: 'x',
      transport: 'http',
      config: {
        url: 'https://mcp.example/mcp',
        oauth_enabled: true,
        proxy_resources: true,
        proxy_prompts: false,
      },
    },
  )

  assert.deepEqual(
    buildAddServerInput({
      ...base,
      target: 'uvx my-mcp-server --flag',
      env: 'TOKEN=abc',
      proxyPrompts: true,
    }),
    {
      name: 'uvx',
      transport: 'stdio',
      config: {
        command: 'uvx',
        args: ['my-mcp-server', '--flag'],
        env: { TOKEN: 'abc' },
        proxy_resources: true,
        proxy_prompts: true,
      },
    },
  )

  // Quoted args survive because the shared stdio tokenizer does the splitting.
  assert.deepEqual(
    buildAddServerInput({ ...base, target: 'npx -y "my server" --flag', name: 'quoted' })?.config,
    {
      command: 'npx',
      args: ['-y', 'my server', '--flag'],
      proxy_resources: true,
      proxy_prompts: false,
    },
  )

  assert.equal(buildAddServerInput({ ...base, target: '  ' }), null)
  // Unterminated quote is unusable, not silently mangled.
  assert.equal(buildAddServerInput({ ...base, target: 'npx "broken' }), null)
})

test('palette server filters combine OR within a group and AND across groups', () => {
  const gateways = [
    { transport: 'http', status: { healthy: true, connected: true } },
    { transport: 'stdio', status: { healthy: false, connected: false } },
    { transport: 'http', enabled: false, status: { healthy: false, connected: false } },
  ]

  assert.equal(
    gateways.filter((g) => gatewayMatchesPaletteFilters(g, { status: ['healthy'], transport: [] }))
      .length,
    1,
  )
  assert.equal(
    gateways.filter((g) =>
      gatewayMatchesPaletteFilters(g, { status: ['disconnected'], transport: ['stdio'] }),
    ).length,
    1,
  )
  assert.equal(
    gateways.filter((g) =>
      gatewayMatchesPaletteFilters(g, { status: ['healthy', 'disabled'], transport: [] }),
    ).length,
    2,
  )

  assert.equal(countPaletteFilterMatches(gateways, 'status', 'enabled'), 2)
  assert.equal(countPaletteFilterMatches(gateways, 'transport', 'http'), 2)
})

test('togglePaletteFilter flips membership without mutating the input', () => {
  const base: PaletteServerFilters = { status: [], transport: [] }
  assert.equal(paletteFiltersActive(base), false)

  const withHealthy = togglePaletteFilter(base, 'status', 'healthy')
  assert.deepEqual(withHealthy, { status: ['healthy'], transport: [] })
  assert.deepEqual(base.status, [])
  assert.equal(paletteFiltersActive(withHealthy), true)

  assert.deepEqual(togglePaletteFilter(withHealthy, 'status', 'healthy'), {
    status: [],
    transport: [],
  })
})
