import test from 'node:test'
import assert from 'node:assert/strict'
import http from 'node:http'
import { once } from 'node:events'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { readFile, mkdtemp, rm, stat } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { chromium } from 'playwright'
import { publishPreviewExport, exportPath } from './preview-export.ts'

let baseUrl = ''
let server: http.Server | undefined
let root = ''
let ready: Promise<void> | undefined
function startPreviewServer() {
  return ready ??= (async () => {
    await promisify(execFile)('pnpm', ['run', 'build'], {
      cwd: new URL('../../', import.meta.url), timeout: 90_000, maxBuffer: 4 * 1024 * 1024,
      env: { ...process.env, NEXT_PUBLIC_MOCK_DATA: 'false', NEXT_PUBLIC_API_TOKEN: '' },
    })
    root = await mkdtemp(path.join(os.tmpdir(), 'labby-pr888-browser-'))
    await publishPreviewExport(new URL('../../out/', import.meta.url).pathname, exportPath(root))
    server = http.createServer(async (request, response) => {
      const pathname = new URL(request.url!, 'http://localhost').pathname
      try {
        if (!pathname.endsWith('/') && (await stat(path.join(exportPath(root), pathname))).isDirectory()) { response.writeHead(301, { Location: pathname + '/' }).end(); return }
        const bytes = await readFile(path.join(exportPath(root), pathname, pathname.endsWith('/') ? 'index.html' : ''))
        response.setHeader('Content-Type', pathname.endsWith('.js') ? 'application/javascript' : pathname.endsWith('.css') ? 'text/css' : pathname.endsWith('.txt') ? 'text/plain' : 'text/html')
        response.end(bytes)
      } catch { response.writeHead(404).end() }
    })
    server.listen(0, '127.0.0.1'); await once(server, 'listening')
    const address = server.address(); assert.ok(address && typeof address !== 'string')
    baseUrl = `http://127.0.0.1:${address.port}`
  })()
}
test.after(async () => {
  if (server) await new Promise<void>((resolve, reject) => server!.close(error => error ? reject(error) : resolve()))
  if (root) await rm(root, { recursive: true, force: true })
})

const pr888OperatorSession = { authenticated: true, user: { sub: 'fixture-owner' }, expires_at: 4102444800000, csrf_token: 'fixture-csrf', authority_state: 'ready', principal_id: 'fixture-owner', organization_id: 'fixture-installation', authority_generation: 1, active_owner: { kind: 'personal', id: 'fixture-owner' }, teams: [], projects: [], capabilities: ['platform.manage', 'scope.read', 'scope.manage', 'scope.operate'], is_configured_admin: true }

// The real static app and fetch/schema/form execute in Chromium. HTTP responses
// are fixtures; these are browser integration tests, not live-daemon OAuth E2E.
for (const width of [1360, 390]) {
  test(`lazy command catalog dispatches validated typed actions at ${width}px`, { concurrency: false, timeout: 180_000 }, async t => {
    await startPreviewServer()
    const browser = await chromium.launch({ headless: true })
    t.after(() => browser.close())
    const page = await browser.newPage({ viewport: { width, height: 900 } })
    page.setDefaultTimeout(10_000)
    await page.route('**/auth/session', route => route.fulfill({ json: pr888OperatorSession }))
    const writes: unknown[] = []
    await page.route('**/v1/catalog', route => route.fulfill({ json: { services: [{
      name: 'fixture', description: 'Hermetic service', actions: [{ name: 'fixture.inspect', description: 'Inspect fixture', destructive: false,
        params: [{ name: 'limit', ty: 'integer', required: true, description: 'Maximum rows' }], returns: 'object' }],
    }] } }))
    await page.route('**/v1/fixture', async route => { writes.push(route.request().postDataJSON()); await route.fulfill({ json: { rows: [] } }) })
    await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
    await page.getByRole('heading', { name: 'Servers', exact: true }).waitFor()
    await page.getByRole('button', { name: 'Search and filter', exact: true }).click()
    const palette = page.getByRole('dialog')
    await palette.getByRole('textbox', { name: 'Search command palette' }).fill('> fixture')
    await palette.getByRole('option').filter({ has: page.getByText('fixture', { exact: true }) }).click()
    await palette.getByRole('option').filter({ hasText: 'fixture.inspect' }).click()
    const limit = palette.locator('#param-limit')
    await limit.waitFor()
    await palette.getByRole('button', { name: 'Run', exact: true }).click()
    assert.deepEqual(writes, [], 'required input must prevent dispatch')
    await limit.fill('7')
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false)
    if (process.env.LABBY_UI_SCREENSHOT_DIR) {
      await page.locator('[data-sonner-toast]').waitFor({ state: 'hidden' })
      await page.screenshot({ path: path.join(process.env.LABBY_UI_SCREENSHOT_DIR, `pr888-catalog-${width}.png`) })
    }
    const sent = page.waitForResponse(response => response.url().endsWith('/v1/fixture'))
    await palette.getByRole('button', { name: 'Run', exact: true }).click()
    await sent
    await palette.waitFor({ state: 'hidden' })
    assert.deepEqual(writes, [{ action: 'fixture.inspect', params: { limit: 7 } }])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth), false)
  })
}

test('malformed lazy catalog fails closed and reload accepts repaired data', { concurrency: false, timeout: 180_000 }, async t => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const page = await browser.newPage()
  page.setDefaultTimeout(10_000)
  await page.route('**/auth/session', route => route.fulfill({ json: pr888OperatorSession }))
  let repaired = false
  let executions = 0
  await page.route('**/v1/catalog', route => route.fulfill({ json: { services: [{ name: 'fixture', description: 'Hermetic service', actions: [{
    name: 'fixture.inspect', description: 'Inspect fixture', destructive: repaired ? false : 'false', params: [], returns: 'object',
  }] }] } }))
  await page.route('**/v1/fixture', async route => { executions++; await route.fulfill({ json: {} }) })
  await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
  await page.getByRole('heading', { name: 'Servers', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Search and filter', exact: true }).click()
  const palette = page.getByRole('dialog')
  await palette.getByRole('textbox', { name: 'Search command palette' }).fill('> fixture')
  await palette.getByText('Failed to load actions — check server connection', { exact: true }).waitFor()
  assert.equal(await palette.getByRole('option').filter({ hasText: 'fixture' }).count(), 0)
  assert.equal(executions, 0)
  repaired = true
  await page.reload({ waitUntil: 'networkidle' })
  await page.getByRole('button', { name: 'Search and filter', exact: true }).click()
  await palette.getByRole('textbox', { name: 'Search command palette' }).fill('> fixture')
  await palette.getByRole('option').filter({ has: page.getByText('fixture', { exact: true }) }).click()
  const sent = page.waitForResponse(response => response.url().endsWith('/v1/fixture'))
  await palette.getByRole('option').filter({ hasText: 'fixture.inspect' }).click()
  await sent
  await palette.waitFor({ state: 'hidden' })
  assert.equal(executions, 1)
})

test('matching static navigation build retains document and survives reload', { concurrency: false, timeout: 180_000 }, async t => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const page = await browser.newPage()
  page.setDefaultTimeout(10_000)
  await page.route('**/auth/session', route => route.fulfill({ json: pr888OperatorSession }))
  await page.goto(`${baseUrl}/settings/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => { Object.assign(window, { __pr888NavigationMarker: true }) })
  await page.getByRole('button', { name: 'Search and filter', exact: true }).click()
  const palette = page.getByRole('dialog')
  await palette.getByRole('textbox', { name: 'Search command palette' }).fill('/ snippets')
  await palette.getByRole('option').filter({ hasText: 'Snippets' }).first().click()
  await page.waitForURL(url => /^\/snippets\/?$/.test(url.pathname), { waitUntil: 'networkidle' })
  assert.equal(await page.evaluate(() => '__pr888NavigationMarker' in window), true, 'matching build must support client navigation')
  await page.reload({ waitUntil: 'networkidle' })
  await page.getByRole('heading', { name: 'Snippets', exact: true }).waitFor()
  assert.equal(await page.evaluate(() => '__pr888NavigationMarker' in window), false)
})
