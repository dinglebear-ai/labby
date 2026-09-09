#!/usr/bin/env node
// Local browser evidence only. Never proxies requests or enables production auth.
import { createServer } from 'node:http'
import { readFile, realpath, stat } from 'node:fs/promises'
import { dirname, extname, join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createOverviewFixtureMetrics } from './overview-fixture-data.mjs'
import { createUsageFixtureCalls, createUsageFixtureMetrics } from './usage-fixture-data.mjs'
import { fixtureTools, describeFixtureTool } from './tools-fixture-data.mjs'
import { createBrowserFixture } from './browser-fixture-data.mjs'
import { createLogsFixture } from './logs-fixture-data.mjs'
import { administrationFixtureOperations } from './administration-fixture-data.mjs'
import { snippetFixtures } from './snippets-fixture-data.mjs'
import { loadoutFixtures, loadoutRouteFixtures } from './loadouts-fixture-data.mjs'
import { createGatewayFixtureRows, createGatewayFixtureRuntime } from './gateway-fixture-data.mjs'

const schemaVersion = 'labby.depot-compatibility/v2'
const asOf = '2026-09-08T12:00:00Z'
const warning = 'Fixture data — not production'
const banner = `<style data-discover-fixture>html::after{content:"${warning}";position:fixed;bottom:0;left:0;right:0;z-index:2147483647;background:#ffcf55;color:#211600;text-align:center;font:700 14px/30px system-ui;pointer-events:none}body{padding-bottom:30px!important}</style>`
const consoleStatusFixture = `<script>window.__LABBY_CONSOLE_STATUS_FIXTURE__={connected:14,total:15,sessions:2,tools:32}</script>`
const providers = ['catalog', 'team'].map((id) => ({ id, name: id === 'catalog' ? 'Fixture Catalog' : 'Fixture Team', enabled: true, sourceOrigins: ['mcp-registry', 'acp-registry', 'ard'], health: { state: 'healthy', observedAt: 1788868800, provenance: 'fixture', retryNotBefore: null } }))
const revision = (index) => `sha256:${String(index + 1).padStart(64, '0')}`
const rows = [
  ['skill', 'Document Review', 'Review technical documents with reproducible checks.', ['documentation', 'review'], 'claude-skill'],
  ['mcp', 'Repository Explorer', 'Read-only repository tools and source navigation.', ['development', 'repository'], 'mcp'],
  ['agent', 'Release Assistant', 'A careful release preparation assistant.', ['release', 'automation'], 'claude-agent'],
  ['skill', 'Accessibility Audit', 'Evaluate semantic markup and keyboard interactions.', ['accessibility', 'frontend'], 'claude-skill'],
  ['loadout', 'Research Workspace', 'A curated research workspace configuration.', ['research'], null],
  ['skill', 'Archive Maintenance', 'An older catalog fixture excluded from the New feed.', ['maintenance'], 'claude-skill'],
].map(([kind, title, description, tags, originalFormat], index) => ({
  providerId: index % 2 ? 'team' : 'catalog', artifactId: `fixture-${index + 1}`, id: `fixture-${index + 1}`,
  sourceOrigin: index === 1 ? 'mcp-registry' : index === 2 ? 'ard' : null,
  kind, namespace: index % 2 ? 'example-team' : 'example-publisher', name: title.toLowerCase().replaceAll(' ', '-'), title, description,
  currentRevisionId: revision(index), firstSeenAt: index === 5 ? '2026-08-01T12:00:00Z' : `2026-09-0${8 - index}T10:00:00Z`,
  license: { declared: 'MIT', redistribution: 'allowed', reviewState: 'approved', takedownState: 'clear' },
  publication: { state: 'published', visibility: 'public', distribution: 'allowed' },
  revisionCount: index + 1, descriptor: { tags }, provenance: { originalFormat, originalVersion: originalFormat ? '1' : null },
  currentRevision: { id: revision(index), contentDigest: revision(index), authoredAt: '2026-09-01T00:00:00Z', fileCount: 2 },
}))

// Explicit fixture rankings, never computed or installed in a production gateway.
const highlights = {
  popular: { state: 'ready', items: [1, 0, 2, 4, 3, 5].map((index, rank) => ({ artifact: rows[index], installs: [58000, 42000, 27000, 19000, 18000, 15000][rank] })) },
  team: { state: 'ready', items: [3, 1, 5].map(index => ({ artifact: rows[index] })) },
  loadouts: { state: 'ready', items: [4, 2, 0, 1].map(index => ({ artifact: rows[index] })) },
}

function detail(row) {
  const { providerId, artifactId, ...artifact } = row
  return { schemaVersion, providerId, artifactId, artifact: { ...artifact, readme: { state: 'available', kind: 'readme', path: 'README.md', revisionId: row.currentRevisionId, content: `# ${row.title}\n\n${row.description}\n\n## Fixture example\n\nThis is deterministic local fixture content, not production metadata.\n\n- Exact revision identity\n- Source-provided tags and format\n\n\`\`\`text\nRead-only browser preview\n\`\`\`\n` } } }
}

const defaultFixture = { rows, highlights }
export async function startDiscoverFixture({ port = 43129, root = resolve(dirname(fileURLToPath(import.meta.url)), '../out'), onRequest, fixture = defaultFixture } = {}) {
  const { rows, highlights } = fixture
  const memberIds = new Set(fixture.memberArtifactIds ?? [rows[0].artifactId])
  if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Invalid fixture port')
  const staticRoot = await realpath(root)
  let requestCount = 0
  const server = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    res.setHeader('cache-control', 'no-store')
    res.setHeader('x-discover-fixture', 'not-production')
    res.setHeader('x-content-type-options', 'nosniff')
    res.setHeader('content-security-policy', "default-src 'self'; connect-src 'self'; img-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; object-src 'none'; base-uri 'none'; frame-src 'none'; form-action 'none'")
    const json = (code, value) => { res.writeHead(code, { 'content-type': 'application/json' }); res.end(JSON.stringify(value)) }
    const deny = () => json(403, { kind: 'fixture_read_only', message: 'Fixture mutations are disabled.' })
    if (req.headers.host !== new URL(origin).host || (req.headers.origin && req.headers.origin !== origin)) return json(403, { kind: 'fixture_origin_rejected' })
    try {
      const path = new URL(req.url, origin).pathname
      // Bounded fixture-only diagnostics: no query values, request bodies,
      // identity headers, cookies, tokens, or arbitrary header strings.
      if (onRequest && requestCount < 500) {
        const requestId = ++requestCount
        const requestUrl = new URL(req.url, origin)
        const safePath = path.replace(/[^a-zA-Z0-9/_.!%-]/g, '_').slice(0, 180)
        const queryKeys = [...new Set(requestUrl.searchParams.keys())].filter((key) => ['artifact', 'artifactProvider', 'provider', 'feed', 'q', '_rsc'].includes(key))
        res.once('finish', () => onRequest({ requestId, method: ['GET', 'HEAD', 'POST', 'DELETE', 'PUT', 'PATCH'].includes(req.method) ? req.method : 'OTHER', path: safePath, queryKeys, rsc: req.headers.rsc === '1', status: res.statusCode }))
      }
      let body
      if (req.method === 'POST') {
        let raw = ''
        for await (const chunk of req) {
          raw += chunk
          if (Buffer.byteLength(raw) > 65536) return json(413, { kind: 'fixture_body_too_large' })
        }
        body = JSON.parse(raw)
      }
      if (req.method === 'GET' && path === '/auth/session') return json(200, { authenticated: true, user: { sub: 'fixture-user', email: 'fixture@example.invalid' }, expires_at: 4102444800, csrf_token: 'fixture-only-not-a-secret', is_admin: false, project_id: 'fixture-project' })
      if (req.method === 'POST' && path === '/v1/snippets') {
        if (!['snippets.list', 'snippets.get'].includes(body?.action)) return deny()
        const params = body.params
        if (!params || typeof params !== 'object' || Array.isArray(params)) return json(400, { kind: 'invalid_fixture_query' })
        if (body.action === 'snippets.list') {
          if (Object.keys(params).length) return json(400, { kind: 'invalid_fixture_query' })
          return json(200, { snippets: snippetFixtures.map(({ body: source, ...snippet }) => snippet) })
        }
        if (Object.keys(params).some(key => key !== 'name') || typeof params.name !== 'string' || params.name.length > 256) return json(400, { kind: 'invalid_fixture_query' })
        const snippet = snippetFixtures.find(item => item.name === params.name)
        return snippet ? json(200, snippet) : json(404, { kind: 'fixture_not_found' })
      }
      if (req.method === 'POST' && path === '/v1/server_logs') {
        if (body?.action !== 'server_logs.query') return deny()
        const params = body.params
        const strings = ['level', 'service', 'action', 'kind', 'query']
        const allowed = [...strings, 'limit', 'max_scan_bytes', 'stop_after_limit', 'correlated_only']
        if (!params || typeof params !== 'object' || Array.isArray(params) || Object.keys(params).some(key => !allowed.includes(key)) || strings.some(key => params[key] !== undefined && (typeof params[key] !== 'string' || params[key].length > 4096)) || (params.limit !== undefined && (!Number.isInteger(params.limit) || params.limit < 1 || params.limit > 1000)) || (params.max_scan_bytes !== undefined && (!Number.isInteger(params.max_scan_bytes) || params.max_scan_bytes < 1 || params.max_scan_bytes > 16777216)) || ['stop_after_limit', 'correlated_only'].some(key => params[key] !== undefined && typeof params[key] !== 'boolean')) return json(400, { kind: 'invalid_fixture_query' })
        return json(200, createLogsFixture(params))
      }
      if (req.method === 'POST' && path === '/v1/browser') {
        const fields = { 'browser.list': 'browsers', 'browser.pairing.list': 'pairings', 'browser.sessions': 'sessions' }
        if (!Object.hasOwn(fields, body?.action)) return deny()
        if (!body.params || typeof body.params !== 'object' || Array.isArray(body.params) || Object.keys(body.params).length) return json(400, { kind: 'invalid_fixture_query' })
        const field = fields[body.action]
        return json(200, { [field]: createBrowserFixture()[field] })
      }
      if (req.method === 'POST' && path === '/v1/setup') {
        const params = body?.params
        if (!params || typeof params !== 'object' || Array.isArray(params)) return json(400, { kind: 'invalid_fixture_query' })
        if (body.action === 'state') {
          if (Object.keys(params).length) return json(400, { kind: 'invalid_fixture_query' })
          return json(200, {
            first_run: false,
            env_path: '~/.labby/.env',
            draft_path: '~/.labby/.env.draft',
            last_completed_step: 4,
            draft_stale: false,
            has_draft: false,
            draft_entry_count: 0,
            env_mtime_unix_seconds: null,
            draft_mtime_unix_seconds: null,
            state: { kind: 'ready', services: [] },
          })
        }
        if (body.action === 'settings.state') {
          if (Object.keys(params).some(key => key !== 'section') || typeof params.section !== 'string') return json(400, { kind: 'invalid_fixture_query' })
          const values = params.section === 'features'
            ? { 'code_mode.enabled': true }
            : params.section === 'surfaces'
              ? { LABBY_MCP_GATEWAY_URL: 'https://labby.tootie.tv' }
              : {}
          return json(200, {
            schema_version: 1,
            config_path: '~/.config/labby/gateway.toml',
            env_path: '~/.labby/.env',
            section: params.section,
            values,
            sources: {},
          })
        }
        return deny()
      }
      if (req.method === 'GET' && path === '/v1/depot/status') return json(200, { depot: { configured: true, enabled: true, mutationAuthority: false, authority: 'read', maxResponseBytes: 1048576 } })
      if (req.method === 'GET' && path === '/v1/depot/operations') return json(200, { operations: administrationFixtureOperations })
      if (req.method === 'GET' && path === '/v1/depot/publish') return json(200, { available: false, reason: 'fixture_read_only' })
      if (req.method === 'POST' && path === '/v1/gateway') {
        if (['gateway.loadout.list_state', 'gateway.protected_route.list_state'].includes(body?.action)) {
          if (!body.params || typeof body.params !== 'object' || Array.isArray(body.params) || Object.keys(body.params).length) return json(400, { kind: 'invalid_fixture_query' })
          return json(200, body.action === 'gateway.loadout.list_state' ? loadoutFixtures : loadoutRouteFixtures)
        }
        if (body?.action === 'gateway.code_mode.get') {
          if (!body.params || typeof body.params !== 'object' || Array.isArray(body.params) || Object.keys(body.params).length) return json(400, { kind: 'invalid_fixture_query' })
          return json(200, { enabled: true, timeout_ms: 5000, max_tool_calls: 8, max_response_bytes: 24576, max_response_tokens: 6000 })
        }
        if (body?.action === 'gateway.list' || body?.action === 'gateway.mcp.list') {
          if (!body.params || typeof body.params !== 'object' || Array.isArray(body.params) || Object.keys(body.params).length) return json(400, { kind: 'invalid_fixture_gateway_query' })
          return json(200, body.action === 'gateway.list' ? createGatewayFixtureRows() : createGatewayFixtureRuntime())
        }
        if (body?.action === 'gateway.usage.calls' || (body?.action === 'gateway.usage.metrics' && body.params?.bucket_count === 0)) {
          const params = body.params
          const strings = ['upstream', 'tool', 'capability', 'operation', 'actor', 'outcome', 'search']
          const calls = body.action === 'gateway.usage.calls'
          const allowed = ['since_unix', 'until_unix', 'subject_scoped', ...strings, ...(calls ? ['limit', 'cursor', 'include_total'] : ['bucket_count', 'timezone', 'timezone_offset_minutes', 'include_facets'])]
          if (!params || typeof params !== 'object' || Array.isArray(params) || Object.keys(params).some(key => !allowed.includes(key)) ||
              !Number.isSafeInteger(params.since_unix) || !Number.isSafeInteger(params.until_unix) || params.since_unix < 0 || params.until_unix > 4102444800 || params.until_unix <= params.since_unix || params.until_unix - params.since_unix > 604800 ||
              strings.some(key => params[key] !== undefined && (typeof params[key] !== 'string' || params[key].length > 1024)) ||
              ['subject_scoped', 'include_total', 'include_facets'].some(key => params[key] !== undefined && typeof params[key] !== 'boolean') ||
              (calls && (!Number.isInteger(params.limit) || params.limit < 1 || params.limit > 100 || (params.cursor !== undefined && (typeof params.cursor !== 'string' || !/^fixture:\d{1,4}$/.test(params.cursor))))) ||
              (params.timezone !== undefined && (typeof params.timezone !== 'string' || params.timezone.length > 128)) ||
              (params.timezone_offset_minutes !== undefined && (!Number.isInteger(params.timezone_offset_minutes) || Math.abs(params.timezone_offset_minutes) > 840))) return json(400, { kind: 'invalid_fixture_usage_query' })
          try {
            return json(200, calls ? createUsageFixtureCalls(params) : createUsageFixtureMetrics(params))
          } catch (error) {
            if (error instanceof RangeError || error instanceof TypeError) return json(400, { kind: 'invalid_fixture_usage_query' })
            throw error
          }
        }
        if (body?.action !== 'gateway.usage.metrics') return deny()
        const params = body.params
        const allowed = ['since_unix', 'until_unix', 'bucket_count', 'timezone', 'timezone_offset_minutes', 'include_facets']
        if (!params || typeof params !== 'object' || Array.isArray(params) || Object.keys(params).some(key => !allowed.includes(key)) || !Number.isSafeInteger(params.since_unix) || !Number.isSafeInteger(params.until_unix) || params.since_unix < 0 || params.until_unix > 4102444800 || params.until_unix <= params.since_unix || params.until_unix - params.since_unix > 604800 || !Number.isInteger(params.bucket_count) || params.bucket_count < 1 || params.bucket_count > 24 || (params.timezone !== undefined && (typeof params.timezone !== 'string' || params.timezone.length > 128)) || (params.timezone_offset_minutes !== undefined && (!Number.isInteger(params.timezone_offset_minutes) || Math.abs(params.timezone_offset_minutes) > 840)) || (params.include_facets !== undefined && typeof params.include_facets !== 'boolean')) return json(400, { kind: 'invalid_fixture_metrics_query' })
        return json(200, createOverviewFixtureMetrics(params))
      }
      if (req.method === 'POST' && path === '/v1/gateway/codemode/tools/search') {
        if (!body || Object.keys(body).some(key => !['query', 'limit'].includes(key)) || typeof body.query !== 'string' || body.query.length > 1024 || !Number.isInteger(body.limit) || body.limit < 1 || body.limit > 50) return json(400, { kind: 'invalid_fixture_query' })
        const hits = fixtureTools.filter(tool => `${tool.path} ${tool.description} ${tool.tags.join(' ')}`.toLowerCase().includes(body.query.toLowerCase()))
        return json(200, { results: hits.slice(0, body.limit), total: hits.length, truncated: hits.length > body.limit })
      }
      if (req.method === 'POST' && path === '/v1/gateway/codemode/tools/describe') {
        if (!body || Object.keys(body).some(key => key !== 'target') || typeof body.target !== 'string' || body.target.length > 1024) return json(400, { kind: 'invalid_fixture_target' })
        const description = describeFixtureTool(body.target)
        return description ? json(200, description) : json(404, { kind: 'not_found' })
      }
      if (req.method === 'POST' && path === '/v1/depot/operations') {
        // Legacy catalog reads used by Library. Keep their v1 envelope distinct
        // from federated Discover and reject every unlisted operation.
        const params = body?.params
        if (body?.operation === 'depot.artifacts.list') {
          if (!params || !Number.isInteger(params.limit) || params.limit < 1 || params.limit > 200 || (params.query !== undefined && (typeof params.query !== 'string' || params.query.length > 4096)) || (params.cursor !== undefined && (typeof params.cursor !== 'string' || !/^fixture-offset:\d{1,6}$/.test(params.cursor)))) return json(400, { kind: 'invalid_fixture_query' })
          const query = (params.query ?? '').toLowerCase()
          const matching = rows.filter(row => `${row.title} ${row.namespace} ${row.description} ${row.descriptor.tags.join(' ')}`.toLowerCase().includes(query))
          const offset = params.cursor ? Number(params.cursor.split(':')[1]) : 0
          if (offset > matching.length) return json(400, { kind: 'invalid_fixture_cursor' })
          const end = offset + params.limit
          return json(200, { schemaVersion: 'labby.depot-compatibility/v1', result: { artifacts: matching.slice(offset, end), total: matching.length, ...(end < matching.length ? { nextCursor: `fixture-offset:${end}` } : {}) } })
        }
        if (body?.operation === 'depot.artifacts.get') {
          if (!params || typeof params.artifactId !== 'string' || !params.artifactId || params.artifactId.length > 2048) return json(400, { kind: 'invalid_fixture_identity' })
          const row = rows.find(row => row.id === params.artifactId)
          return row ? json(200, { schemaVersion: 'labby.depot-compatibility/v1', result: { artifact: detail(row).artifact } }) : json(404, { kind: 'not_found' })
        }
        return deny()
      }
      if (req.method === 'GET' && path === '/v1/depot/providers') return json(200, providers)
      if (req.method === 'POST' && path === '/v1/depot/discover') {
        if (!body || ![null, undefined, 'catalog', 'team'].includes(body.provider) || ![undefined, 'mcp-registry', 'acp-registry', 'ard'].includes(body.sourceOrigin) || typeof body.query !== 'string' || !Number.isInteger(body.limit) || body.limit < 1 || body.limit > 200 || body.cursor || (body.feed !== undefined && body.feed !== 'new')) return json(400, { kind: 'invalid_fixture_query' })
        const items = rows.filter((row) => (!body.provider || row.providerId === body.provider) && (!body.sourceOrigin || row.sourceOrigin === body.sourceOrigin) && `${row.title} ${row.description} ${row.descriptor.tags.join(' ')}`.toLowerCase().includes(body.query.toLowerCase()) && (body.feed !== 'new' || row.firstSeenAt >= '2026-09-01T12:00:00Z'))
        const selected = providers.filter((p) => !body.provider || p.id === body.provider)
        return json(200, { schemaVersion, scope: body.provider ?? 'all', ...(body.sourceOrigin ? { sourceOrigin: body.sourceOrigin } : {}), scopeEpoch: 'fixture-v1', items: items.slice(0, body.limit), providerOutcomes: selected.map((p) => ({ providerId: p.id, state: 'exhausted' })), failures: [], coverageComplete: true, knownTotal: items.length, totalIsExact: true, state: items.length ? 'complete' : 'empty', nextCursor: null, ...(!body.provider && !body.sourceOrigin && !body.feed && !body.query ? { highlights } : {}), ...(body.feed === 'new' ? { feed: 'new', asOf, rankingVersion: 'new/v1', feedCoverage: selected.map((p) => ({ providerId: p.id, coverage: { population: 'hosted', complete: true, unknownFirstSeen: 0 } })) } : {}) })
      }
      if (req.method === 'POST' && path === '/v1/depot/artifacts/detail') {
        const row = rows.find((r) => r.providerId === body?.providerId && r.artifactId === body?.artifactId)
        return row ? json(200, detail(row)) : json(404, { kind: 'not_found' })
      }
      if (req.method === 'POST' && path === '/v1/artifacts' && body?.action === 'artifacts.depot_membership') {
        const items = body.params?.items
        if (!Array.isArray(items) || !items.length || items.length > 100 || items.some((i) => !i || ['connection_id', 'artifact_id', 'revision_id'].some((key) => typeof i[key] !== 'string'))) return json(400, { kind: 'invalid_fixture_membership' })
        return json(200, { library_version: 3, items: items.map((item) => {
          const row = rows.find(row => row.providerId === item.connection_id && row.artifactId === item.artifact_id && memberIds.has(row.artifactId))
          return { ...item, status: row ? (item.revision_id === row.currentRevisionId ? 'exact_revision_present' : 'different_revision_present') : 'absent' }
        }) })
      }
      if (req.method === 'POST' && path === '/v1/artifacts' && body?.action === 'artifacts.list_connections') {
        return json(200, { connections: providers.map(({ id, name }) => ({ id, name })) })
      }
      if (req.method === 'POST' && path === '/v1/artifacts' && body?.action === 'artifacts.list') {
        return json(200, { library_version: 3, items: [] })
      }
      if (req.method !== 'GET' && req.method !== 'HEAD') return deny()
      if (path.startsWith('/v1/') || path.startsWith('/auth/')) return json(404, { kind: 'fixture_route_not_implemented' })
      if (path === '/_fixture/tasks-compact') {
        res.setHeader('content-security-policy', "default-src 'none'; style-src 'unsafe-inline'; frame-src 'self'; base-uri 'none'; form-action 'none'")
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
        return res.end(req.method === 'HEAD' ? undefined : '<!doctype html><html><head><title>Tasks — 1120px fixture viewport</title><style>body{margin:0;background:#07131c;color:#e6f4fb;font:14px system-ui}p{margin:12px}iframe{display:block;width:1120px;height:844px;border:0;margin:0 12px}</style></head><body><p>1120 × 844 CSS pixels — fixture data, not device emulation</p><iframe title="Tasks at compact desktop width" src="/tasks/"></iframe></body></html>')
      }
      if (path === '/_fixture/mobile' || path === '/_fixture/mobile/library' || path === '/_fixture/mobile/gateways' || path === '/_fixture/mobile/create' || path === '/_fixture/mobile/dev-containers') {
        // A real nested viewport exercises the exported page's media queries.
        // No arbitrary target URL, production connection, or device emulation.
        res.setHeader('content-security-policy', "default-src 'none'; style-src 'unsafe-inline'; frame-src 'self'; base-uri 'none'; form-action 'none'")
        const library = path === '/_fixture/mobile/library'
        const gateways = path === '/_fixture/mobile/gateways'
        const create = path === '/_fixture/mobile/create'
        const containers = path === '/_fixture/mobile/dev-containers'
        const label = containers ? 'Dev Containers' : create ? 'Create' : gateways ? 'Gateway' : library ? 'Library' : 'Discover'
        const target = containers ? '/dev-containers/' : create ? '/create/' : gateways ? '/gateways/' : library ? '/library/' : '/depot/'
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
        return res.end(req.method === 'HEAD' ? undefined : `<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><title>${label} — 390px fixture viewport</title><style>body{margin:0;background:#07131c;color:#e6f4fb;font:14px system-ui}h1{font-size:16px;margin:12px}iframe{display:block;width:390px;height:844px;border:0;margin:0 12px}</style></head><body><h1>390 × 844 CSS pixels — fixture data, not device emulation</h1><iframe title="${label} at phone width" src="${target}"></iframe></body></html>`)
      }
      const decoded = decodeURIComponent(req.url.split('?')[0])
      if (decoded.includes('\\') || decoded.includes('\0') || decoded.split('/').includes('..')) return json(400, { kind: 'invalid_path' })
      let candidate = resolve(staticRoot, `.${decodeURIComponent(path)}`)
      // Match Labby's path-only static handler. Next output:export encodes
      // Flight/segment requests in filenames, not the RSC header.
      if ((await stat(candidate)).isDirectory()) candidate = join(candidate, 'index.html')
      candidate = await realpath(candidate)
      if (!candidate.startsWith(`${staticRoot}${sep}`)) return json(403, { kind: 'invalid_path' })
      const extension = extname(candidate)
      const types = { '.html': 'text/html', '.txt': 'text/plain; charset=utf-8', '.css': 'text/css', '.js': 'text/javascript', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon', '.woff2': 'font/woff2' }
      let content = await readFile(candidate)
      if (extension === '.html') content = Buffer.from(content.toString().replace('</head>', `${consoleStatusFixture}${banner}</head>`))
      res.writeHead(200, { 'content-type': types[extension] ?? 'application/octet-stream' })
      res.end(req.method === 'HEAD' ? undefined : content)
    } catch (error) {
      json(error instanceof SyntaxError ? 400 : 404, { kind: 'fixture_request_failed' })
    }
  })
  await new Promise((resolveListen, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolveListen) })
  return { server, url: `http://127.0.0.1:${server.address().port}` }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2)
  const options = {}
  for (let index = 0; index < args.length; index += 2) {
    if (!['--port', '--reference'].includes(args[index]) || !args[index + 1] || options[args[index]]) throw new Error('Usage: node scripts/serve-discover-fixture.mjs [--port 43129] [--reference /absolute/reference.html]')
    options[args[index]] = args[index + 1]
  }
  const fixture = options['--reference'] ? (await import('./discover-reference-data.mjs')).referenceFixture(await readFile(options['--reference'], 'utf8')) : defaultFixture
  const { server, url } = await startDiscoverFixture({ port: options['--port'] ? Number(options['--port']) : 43129, fixture, onRequest: (entry) => console.log('[fixture-request]', JSON.stringify(entry)) })
  console.log(`${warning}: ${url}/depot/ (loopback only; all mutations rejected)`)
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => server.close(() => process.exit(0)))
}
