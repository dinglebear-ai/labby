import test from 'node:test'
import assert from 'node:assert/strict'
import http from 'node:http'
import { once } from 'node:events'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import os from 'node:os'
import { chromium } from 'playwright'
import { publishPreviewExport, exportPath } from './preview-export.ts'

const APP = new URL('../../', import.meta.url)

test('production Discover collections use retrieved catalog evidence and recover on desktop/mobile', { timeout: 240000 }, async t => {
  // Mock builds bypass the real Depot client. Use the production adapter with
  // intercepted authorized contract fixtures to verify the live rendering path.
  const built = await promisify(execFile)('pnpm', ['run', 'build'], {
    cwd: APP, env: { ...process.env, NEXT_PUBLIC_MOCK_DATA: 'false', NEXT_PUBLIC_API_TOKEN: '' },
    timeout: 180000, maxBuffer: 4 * 1024 * 1024,
  }).catch(error => {
    console.error(error.stdout ?? '', error.stderr ?? '')
    throw error
  })
  if (process.env.DISCOVER_SCREENSHOTS) {
    await mkdir(process.env.DISCOVER_SCREENSHOTS, { recursive: true })
    await writeFile(path.join(process.env.DISCOVER_SCREENSHOTS, 'production-build.log'), built.stdout + built.stderr)
  }
  const root = await mkdtemp(path.join(os.tmpdir(), 'labby-discover-contract-'))
  await publishPreviewExport(new URL('out/', APP).pathname, exportPath(root))
  t.after(() => rm(root, { recursive: true, force: true }))
  const server = http.createServer(async (request, response) => {
    const pathname = new URL(request.url!, 'http://localhost').pathname
    try {
      const bytes = await readFile(path.join(exportPath(root), pathname, pathname.endsWith('/') ? 'index.html' : ''))
      response.setHeader('Content-Type', pathname.endsWith('.js') ? 'application/javascript' : pathname.endsWith('.css') ? 'text/css' : pathname.endsWith('.json') ? 'application/json' : 'text/html')
      response.end(bytes)
    } catch { response.writeHead(404).end() }
  })
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  t.after(() => server.close())
  const address = server.address()
  assert.ok(address && typeof address !== 'string')
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const artifacts = [
    { providerId: 'catalog-a', artifactId: 'skill', kind: 'skill', title: 'Review changes', description: 'Review a scoped change before publishing.', namespace: 'community', sourceOrigin: 'github', currentRevision: { id: 'revision-skill', authoredAt: '2026-10-01T12:00:00Z' } },
    { providerId: 'catalog-b', artifactId: 'loadout', kind: 'loadout', title: 'Operations loadout', description: 'A reported loadout for incident response.', namespace: 'operations', sourceOrigin: 'ard', currentRevision: { id: 'revision-loadout', authoredAt: '2026-10-02T12:00:00Z' } },
    { providerId: 'catalog-a', artifactId: 'agent', kind: 'agent', title: 'Release reviewer', description: 'Check a release against its acceptance criteria.', namespace: 'releases', sourceOrigin: 'mcp-registry', currentRevision: { id: 'revision-agent', authoredAt: '2026-10-03T10:00:00Z' } },
  ]
  // More than a rail's eight-card limit, classified sources, and long titles reproduce
  // the real mock catalog's intrinsic/absolute-descendant layout pressure.
  artifacts.push(...Array.from({ length: 9 }, (_, index) => ({
    providerId: index % 2 ? 'catalog-b' : 'catalog-a', artifactId: `extra-${index}`,
    kind: 'skill', title: `Long-reported-artifact-title-${'catalog-evidence-'.repeat(5)}${index}`,
    description: 'Reported artifact description '.repeat(8), namespace: 'community', sourceOrigin: 'github',
    currentRevision: { id: `revision-extra-${index}`, authoredAt: '2026-09-30T12:00:00Z' },
  })))
  for (const width of [1440, 320, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    page.setDefaultTimeout(10000)
    await page.addInitScript(() => localStorage.setItem('theme', 'dark'))
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    let mode: 'failed' | 'complete' | 'partial' | 'empty' = 'failed'
    let first = true, reads = 0
    let release!: () => void
    const initial = new Promise<void>(resolve => { release = resolve })
    await page.route('**/auth/session', route => route.fulfill({ json: {
      authenticated: true, user: { sub: 'fixture-reader' }, expires_at: Date.now() + 3600000,
      csrf_token: 'fixture-csrf', authority_state: 'ready', principal_id: 'fixture-reader',
      organization_id: 'fixture-installation', authority_generation: 1, active_owner: { kind: 'personal', id: 'fixture-reader' },
      teams: [], projects: [], capabilities: ['scope.read', 'scope.manage', 'platform.manage'],
    } }))
    await page.route('**/v1/**', async route => {
      const pathname = new URL(route.request().url()).pathname
      if (pathname === '/v1/depot/providers') return route.fulfill({ json: ['catalog-a', 'catalog-b'].map(id => ({ id, name: id, enabled: true, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } })) })
      if (pathname !== '/v1/depot/discover') return route.fulfill({ status: 503, json: { kind: 'fixture_unavailable', message: 'Runtime data unavailable in the isolated fixture.' } })
      reads++
      const request = route.request().postDataJSON()
      assert.equal(request.provider, null)
      if (first) { first = false; await initial }
      const failed = mode === 'failed'
      const rows = failed || mode === 'empty' ? [] : artifacts
      return route.fulfill({ json: {
        schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'fixture', items: rows,
        providerOutcomes: [], failures: failed ? [{ providerId: 'catalog-a', kind: 'index_not_ready' }] : mode === 'partial' ? [{ providerId: 'catalog-c', kind: 'unavailable' }] : [],
        coverageComplete: mode === 'complete' || mode === 'empty', knownTotal: failed ? null : rows.length,
        totalIsExact: mode === 'complete' || mode === 'empty', state: failed ? 'all_failed' : mode, nextCursor: null,
      } })
    })
    const collections = page.locator('[data-discover-rails="1"]')
    const snapshot = async (state: string) => {
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false, `${state} overflow at ${width}px`)
      if (process.env.DISCOVER_SCREENSHOTS) {
        await collections.screenshot({ path: path.join(process.env.DISCOVER_SCREENSHOTS, `discover-${width}-${state}-collections.png`), animations: 'disabled' })
        await page.screenshot({ path: path.join(process.env.DISCOVER_SCREENSHOTS, `discover-${width}-${state}-page.png`), animations: 'disabled' })
      }
    }
    await page.goto(`http://127.0.0.1:${address.port}/depot/`, { waitUntil: 'domcontentloaded' })
    await collections.getByText('Checking connected sources…', { exact: true }).waitFor()
    await snapshot('loading-dark')
    release()
    await collections.getByText('The catalog index is still preparing.', { exact: false }).waitFor()
    if (width < 640) {
      const message = await collections.getByRole('status').boundingBox()
      const retry = await collections.getByRole('button', { name: 'Retry search', exact: true }).boundingBox()
      assert.ok(message && retry && message.width >= width - 90 && retry.y >= message.y + message.height, 'mobile failure explanation must remain readable above actions')
    }
    await snapshot('failed-dark')
    assert.equal(await collections.getByText('No artifacts returned', { exact: true }).count(), 0)
    mode = 'complete'
    await collections.getByRole('button', { name: 'Retry search', exact: true }).click()
    await collections.getByRole('heading', { name: 'Recently updated', exact: true }).waitFor()
    assert.equal(reads, 2, 'real production discovery adapter must retry')
    assert.equal(await collections.locator('section').count(), 3)
    const recentCards = collections.locator('section[aria-label="Recently updated"] [data-discover-collection-artifact]')
    assert.equal(await recentCards.count(), 8)
    assert.match(await recentCards.first().textContent() ?? '', /via catalog-a/, 'reported source provenance remains available to assistive technology')
    if (width < 640) {
      const rail = recentCards.first().locator('..')
      assert.equal(await rail.evaluate(element => element.scrollWidth > element.clientWidth), true, 'collection should retain a local horizontal scroller')
      await recentCards.last().focus()
      assert.equal(await rail.evaluate(element => element.scrollLeft > 0), true, 'last collection artifact remains keyboard reachable through local scrolling')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), width)
      await rail.evaluate(element => { element.scrollLeft = 0 })
      const layout = page.getByRole('group', { name: 'Discovery layout' })
      await layout.getByRole('button', { name: 'Table view', exact: true }).click()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), width, 'phone table mode must keep collection provenance locally contained')
      await layout.getByRole('button', { name: 'Card view', exact: true }).click()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), width, 'phone card mode must keep collection provenance locally contained')
    }
    assert.equal(await collections.locator('section[aria-label="Recently updated"] [data-discover-collection-artifact]').first().getAttribute('data-discover-collection-artifact'), 'catalog-a:agent')
    assert.equal(await collections.locator('section[aria-label="From connected sources"] [data-discover-collection-artifact]').count(), 2)
    const loadout = collections.locator('section[aria-label="Loadouts to explore"] [data-discover-collection-artifact]')
    assert.equal(await loadout.count(), 1)
    assert.match(await loadout.getAttribute('href') ?? '', /artifactProvider=catalog-b.*artifact=loadout/)
    assert.doesNotMatch(await collections.innerText(), /Popular This Week|New From Your Team|Pairs With Your Loadouts|Recommendation evidence/)
    await loadout.focus()
    assert.equal(await loadout.evaluate(element => document.activeElement === element), true)
    await snapshot('success-dark')
    await page.evaluate(() => { document.documentElement.classList.remove('dark'); document.documentElement.classList.add('light') })
    await snapshot('success-light')
    mode = 'partial'
    await page.reload({ waitUntil: 'networkidle' })
    await collections.getByRole('heading', { name: 'Loadouts to explore', exact: true }).waitFor()
    assert.equal(await collections.getByRole('button', { name: 'Retry search', exact: true }).count(), 1)
    assert.match(await collections.innerText(), /Operations loadout/)
    await snapshot('partial-dark')
    mode = 'empty'
    await page.reload({ waitUntil: 'networkidle' })
    await collections.getByText('No artifacts returned', { exact: true }).waitFor()
    assert.match(await collections.getByRole('link', { name: 'Publish artifact', exact: true }).getAttribute('href') ?? '', /^\/create\/?$/)
    assert.match(await collections.getByRole('link', { name: 'Review sources', exact: true }).getAttribute('href') ?? '', /^\/settings\/depot\/?$/)
    await snapshot('empty-dark')
    assert.deepEqual(errors, [])
    await page.close()
  }
})
