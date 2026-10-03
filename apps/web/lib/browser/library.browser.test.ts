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

test('production Library loads, inspects, navigates, copies and recovers on desktop/mobile', { timeout: 240000 }, async t => {
  // Mock builds bypass the real Depot client. Use the production adapter with
  // intercepted authorized contract fixtures to verify the live rendering path.
  const built = process.env.LIBRARY_BROWSER_SKIP_BUILD === 'true' ? { stdout: '', stderr: '' } : await promisify(execFile)('pnpm', ['run', 'build'], {
    cwd: APP, env: { ...process.env, NEXT_PUBLIC_MOCK_DATA: 'false', NEXT_PUBLIC_API_TOKEN: '' },
    timeout: 180000, maxBuffer: 4 * 1024 * 1024,
  }).catch(error => {
    console.error(error.stdout ?? '', error.stderr ?? '')
    throw error
  })
  if (process.env.LIBRARY_SCREENSHOTS) {
    await mkdir(process.env.LIBRARY_SCREENSHOTS, { recursive: true })
    await writeFile(path.join(process.env.LIBRARY_SCREENSHOTS, 'production-build.log'), built.stdout + built.stderr)
  }
  const root = await mkdtemp(path.join(os.tmpdir(), 'labby-library-contract-'))
  await publishPreviewExport(new URL('out/', APP).pathname, exportPath(root))
  t.after(() => rm(root, { recursive: true, force: true }))
  const server = http.createServer(async (request, response) => {
    const pathname = new URL(request.url!, 'http://localhost').pathname
    try {
      const bytes = await readFile(path.join(exportPath(root), pathname, path.extname(pathname) ? '' : 'index.html'))
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
  const records = [
    { id: 'alpha', providerId: 'catalog-a', kind: 'skill', name: "sample; printf injected ' quote", title: 'Alpha Skill', namespace: 'community', description: 'Actual alpha description', currentRevisionId: 'rev-alpha',
      currentRevision: { id: 'rev-alpha', fileCount: 1, components: [{ path: 'README.md', kind: 'document', size: 32 }] },
      readme: { state: 'available', kind: 'readme', path: 'README.md', revisionId: 'rev-alpha', content: '# Actual Library README\n\nVERIFIED_DOCUMENT_MARKER' },
      lineage: { upstreamArtifactId: 'parent', following: true }, publication: { visibility: 'private' } },
    { id: 'beta', kind: 'plugin', name: 'beta', title: 'Beta Plugin', namespace: 'community', currentRevision: { id: 'rev-beta', components: [] } },
    { id: 'all', kind: 'skill', name: 'all', title: 'All Search Entry', namespace: 'community', currentRevision: { id: 'rev-all', components: [] } },
  ]
  for (const width of [1440, 390, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    page.setDefaultTimeout(10000)
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    let failed = true
    const calls: Array<{ operation: string; params: Record<string, unknown> }> = []
    await page.route('**/auth/session', route => route.fulfill({ json: {
      authenticated: true, user: { sub: 'fixture-reader' }, expires_at: Date.now() + 3600000,
      csrf_token: 'fixture-csrf', authority_state: 'ready', principal_id: 'fixture-reader',
      organization_id: 'fixture-installation', authority_generation: 1, active_owner: { kind: 'personal', id: 'fixture-reader' },
      teams: [], projects: [], capabilities: ['scope.read', 'scope.manage', 'platform.manage'],
    } }))
    await page.route('**/v1/**', async route => {
      const pathname = new URL(route.request().url()).pathname
      if (pathname === '/v1/depot/operations') {
        if (route.request().method() === 'GET') return route.fulfill({ json: { operations: ['depot.artifacts.list', 'depot.artifacts.get'].map(name => ({ name, title: name, description: name, inputSchema: { type: 'object', properties: {}, additionalProperties: false }, annotations: { readOnlyHint: true, destructiveHint: false } })) } })
        const request = route.request().postDataJSON()
        calls.push(request)
        if (failed && request.operation === 'depot.artifacts.list') return route.fulfill({ status: 503, json: { kind: 'source_unavailable', message: 'Fixture catalog offline' } })
        const result = request.operation === 'depot.artifacts.get' ? { artifact: records.find(record => record.id === request.params.artifactId) } : { artifacts: records, total: records.length }
        return route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v1', result } })
      }
      if (pathname === '/v1/depot/providers') return route.fulfill({ json: [{ id: 'catalog-a', name: 'Catalog A', enabled: true, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } }] })
      if (pathname === '/v1/depot/discover') return route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'fixture', items: [], providerOutcomes: [], failures: [], coverageComplete: true, knownTotal: 0, totalIsExact: true, state: 'empty', nextCursor: null } })
      if (pathname === '/v1/depot/artifacts/detail') return route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v2', providerId: 'catalog-a', artifactId: 'parent', artifact: { id: 'parent', name: 'parent', title: 'Exact upstream parent', kind: 'skill', currentRevision: { id: 'rev-parent' } } } })
      return route.fulfill({ status: 503, json: { kind: 'fixture_unavailable', message: 'Unavailable in isolated fixture' } })
    })
    const origin = `http://127.0.0.1:${address.port}`
    await page.goto(`${origin}/library/`, { waitUntil: 'domcontentloaded' })
    await page.getByText('Library unavailable', { exact: true }).first().waitFor()
    assert.equal(await page.getByText('No artifacts in your library yet.', { exact: false }).count(), 0)
    failed = false
    await page.getByRole('button', { name: 'Retry loading', exact: true }).click()
    await page.locator('[data-library-collection]').getByText('Alpha Skill', { exact: true }).waitFor()
    const betaRequest = page.waitForRequest(request => request.method() === 'POST' && new URL(request.url()).pathname === '/v1/depot/operations' && request.postDataJSON().params?.query === 'Beta')
    await page.evaluate(() => history.pushState(null, '', '/library/?q=Beta&kind=plugin'))
    await page.waitForFunction(() => (document.querySelector('input[aria-label="Search library"]') as HTMLInputElement)?.value === 'Beta')
    await page.getByText('Beta Plugin', { exact: true }).waitFor()
    await betaRequest
    assert.equal(await page.getByText('Alpha Skill', { exact: true }).count(), 0)
    await page.goBack()
    await page.waitForFunction(() => (document.querySelector('input[aria-label="Search library"]') as HTMLInputElement)?.value === '')
    await page.goForward()
    await page.waitForFunction(() => (document.querySelector('input[aria-label="Search library"]') as HTMLInputElement)?.value === 'Beta')
    await page.evaluate(() => history.replaceState(null, '', '/library/?q=all&kind=skill'))
    await page.getByText('All Search Entry', { exact: true }).waitFor()
    await page.waitForTimeout(350)
    assert.equal(new URL(page.url()).searchParams.get('q'), 'all')
    await page.evaluate(() => history.replaceState(null, '', '/library/?artifact=alpha'))
    const dialog = page.getByRole('dialog')
    await dialog.getByText('VERIFIED_DOCUMENT_MARKER', { exact: true }).waitFor()
    await dialog.locator('button[aria-expanded]').first().click()
    assert.equal(await dialog.getByText('reference.md', { exact: true }).count(), 0)
    assert.equal(await dialog.getByText('README.md', { exact: true }).count(), 2)
    assert.equal(await dialog.getByRole('button', { name: 'Add to Library', exact: true }).count(), 0)
    const expectedCommand = "depot add 'community/sample; printf injected '\\'' quote'"
    assert.equal(await dialog.locator('[aria-label="Install command"] code').textContent(), expectedCommand)
    await page.evaluate(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async () => { throw new Error('denied') } } }))
    await dialog.getByRole('button', { name: 'Copy Library link', exact: true }).click()
    await page.getByText('Could not copy share link. Allow clipboard access and try again.', { exact: true }).waitFor()
    await page.evaluate(() => Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: async (text: string) => { (window as unknown as { copied: string }).copied = text } } }))
    await dialog.getByRole('button', { name: 'Copy install command', exact: true }).click()
    assert.equal(await page.evaluate(() => (window as unknown as { copied: string }).copied), expectedCommand)
    await dialog.getByRole('button', { name: 'Copy Library link', exact: true }).click()
    assert.equal(await page.evaluate(() => (window as unknown as { copied: string }).copied), page.url())
    const downloadPromise = page.waitForEvent('download')
    await dialog.getByRole('button', { name: 'Export artifact', exact: true }).click()
    await page.getByRole('button', { name: 'JSON', exact: true }).click()
    const download = await downloadPromise
    assert.match(download.suggestedFilename(), /\.json$/)
    const exported = JSON.parse(await readFile((await download.path())!, 'utf8'))
    assert.equal(exported.id, 'alpha')
    assert.match(exported.readme.content, /VERIFIED_DOCUMENT_MARKER/)
    assert.deepEqual(exported.currentRevision.components.map((component: { path: string }) => component.path), ['README.md'])
    await page.reload({ waitUntil: 'domcontentloaded' })
    await page.getByRole('dialog').getByText('VERIFIED_DOCUMENT_MARKER', { exact: true }).waitFor()
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false, `Library overflow at ${width}px`)
    if (process.env.LIBRARY_SCREENSHOTS) await page.screenshot({ path: path.join(process.env.LIBRARY_SCREENSHOTS, `library-${width}.png`), animations: 'disabled' })
    await page.getByRole('dialog').getByRole('button', { name: 'Open upstream and fork options', exact: true }).click()
    await page.waitForURL(url => url.pathname === '/depot/' || url.pathname === '/depot')
    assert.equal(new URL(page.url()).searchParams.get('artifactProvider'), 'catalog-a')
    assert.equal(new URL(page.url()).searchParams.get('artifact'), 'parent')
    await page.getByRole('dialog').getByText('Exact upstream parent', { exact: true }).waitFor().catch(async error => {
      console.error('Upstream state', page.url(), (await page.locator('body').innerText()).slice(-5000))
      throw error
    })
    assert.equal(calls.some(call => call.operation === 'depot.artifacts.list' && call.params.query === 'Beta'), true)
    assert.deepEqual(errors, [])
    await page.close()
  }
})
