import test from 'node:test'
import assert from 'node:assert/strict'
import { once } from 'node:events'
import http from 'node:http'
import { spawn, type ChildProcess } from 'node:child_process'

import { chromium } from 'playwright'

const APP_DIR = new URL('../../', import.meta.url)
let baseUrl = ''
let previewServer: ChildProcess | null = null
let previewServerReady: Promise<void> | null = null
let buildReady: Promise<void> | null = null
let previewStderr = ''

function buildApplication(buildId?: string) {
  return new Promise<void>((resolve, reject) => {
    const child = spawn('pnpm', ['run', 'build'], {
      cwd: APP_DIR,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: {
        ...process.env,
        ...(buildId ? { NEXT_BUILD_ID: buildId } : {}),
        LAB_ALLOWED_DEV_ORIGINS: '127.0.0.1',
        NEXT_PUBLIC_MOCK_DATA: 'true',
        NEXT_PUBLIC_API_TOKEN: 'dev-token',
      },
    })
    let output = ''
    child.stdout?.on('data', (chunk) => { output += String(chunk) })
    child.stderr?.on('data', (chunk) => { output += String(chunk) })
    child.once('error', reject)
    child.once('exit', (code, signal) => {
      if (code === 0) resolve()
      else reject(new Error(`Gateway Admin build failed (${code ?? signal}):\n${output.slice(-12_000)}`))
    })
  })
}

async function allocatePort(): Promise<number> {
  const server = http.createServer()
  server.listen(0, '127.0.0.1')
  await once(server, 'listening')
  const address = server.address()
  assert.ok(address && typeof address !== 'string')
  const port = address.port
  server.close()
  await once(server, 'close')
  return port
}

function buildApplicationOnce() {
  if (buildReady) return buildReady
  if (process.env.GATEWAY_ADMIN_BROWSER_SKIP_BUILD === 'true') {
    buildReady = Promise.resolve()
    return buildReady
  }
  buildReady = buildApplication()
  return buildReady
}

async function waitForServer(url: string) {
  const deadline = Date.now() + 60_000

  while (Date.now() < deadline) {
    try {
      const status = await new Promise<number>((resolve, reject) => {
        const request = http.get(url, (response) => {
          resolve(response.statusCode ?? 0)
          response.resume()
        })
        request.on('error', reject)
      })

      if (status >= 200 && status < 500) {
        return
      }
    } catch {
      // Retry until deadline.
    }

    await new Promise((resolve) => setTimeout(resolve, 200))
  }

  throw new Error(`Timed out waiting for preview server at ${url}:\n${previewStderr.slice(-12_000)}`)
}

async function startPreviewServer() {
  if (previewServerReady) {
    await previewServerReady
    return
  }

  previewServerReady = (async () => {
    await buildApplicationOnce()
    const port = await allocatePort()
    baseUrl = `http://127.0.0.1:${port}`
    previewServer = spawn(
      'python3',
      ['-m', 'http.server', String(port), '--directory', 'out', '--bind', '127.0.0.1'],
      { cwd: APP_DIR, stdio: ['ignore', 'pipe', 'pipe'], env: process.env },
    )
    previewServer.stdout?.on('data', (chunk) => { previewStderr += String(chunk) })
    previewServer.stderr?.on('data', (chunk) => { previewStderr += String(chunk) })
    const earlyExit = once(previewServer, 'exit').then(([code, signal]) => {
      throw new Error(`Preview server exited before readiness (${code ?? signal}):\n${previewStderr.slice(-12_000)}`)
    })
    await Promise.race([waitForServer(`${baseUrl}/gateway/?id=gw-2`), earlyExit])
  })()
  await previewServerReady
}

test.after(async () => {
  if (!previewServer) {
    return
  }

  previewServer.kill('SIGTERM')
  await Promise.race([
    once(previewServer, 'exit').catch(() => undefined),
    new Promise((resolve) => setTimeout(resolve, 2_000)),
  ])

  if (previewServer.exitCode === null) {
    previewServer.kill('SIGKILL')
    await once(previewServer, 'exit').catch(() => undefined)
  }
})

test('gateway manage tools flow persists after a full reload in mock preview', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage()
  await page.goto(`${baseUrl}/gateway/?id=gw-2`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  await page.getByRole('tab', { name: /Catalog/ }).click()
  await page.getByRole('button', { name: 'Exposure editor', exact: true }).click()
  await page.getByRole('button', { name: 'Manage tools', exact: true }).click()
  await page.locator('#select-all-visible').click()
  await page.getByRole('button', { name: 'Disable selected' }).click()
  await page.getByRole('button', { name: 'Save changes' }).click()

  await page.getByText('Tool exposure updated successfully').waitFor()
  await assert.doesNotReject(() =>
    page.locator('p, div').filter({ hasText: /^0\/12$/ }).first().waitFor(),
  )

  await page.reload({ waitUntil: 'networkidle' })

  await page.getByRole('tab', { name: /Catalog/ }).click()
  await page.getByRole('button', { name: 'Exposure editor', exact: true }).click()
  await assert.doesNotReject(() =>
    page.getByRole('button', { name: 'Manage tools', exact: true }).waitFor(),
  )
  await assert.doesNotReject(() =>
    page.locator('p, div').filter({ hasText: /^0\/12$/ }).first().waitFor(),
  )
  await assert.doesNotReject(() => page.getByText('12 hidden').waitFor())
})

test('gateway detail uses a compact summary and endpoint control in mock preview', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/gateway/?id=gw-2`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  await assert.doesNotReject(() => page.getByText('12/12').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Resources').first().waitFor())
  await assert.doesNotReject(() => page.getByText('Prompts').first().waitFor())
  await assert.doesNotReject(() =>
    page.getByRole('button', { name: 'Copy HTTP target' }).and(
      page.locator('[title="http://localhost:3001/mcp"]'),
    ).waitFor(),
  )
  await page.getByRole('tab', { name: /Catalog/ }).click()
  await assert.doesNotReject(() =>
    page.getByRole('button', { name: 'Exposure editor', exact: true }).waitFor(),
  )

  assert.equal(await page.getByText('TOOL SURFACE').count(), 0)
  assert.equal(await page.getByText('BEARER ENV').count(), 0)
  assert.equal(await page.getByText('LAB CONTROLS').count(), 0)

  const hasHorizontalOverflow = await page.evaluate(() => {
    const root = document.documentElement
    return root.scrollWidth > root.clientWidth
  })

  assert.equal(hasHorizontalOverflow, false)
})

test('desktop shell exposes the full palette trigger, Settings, and Discover vocabulary', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/depot/`, { waitUntil: 'networkidle' })

  await assert.doesNotReject(() => page.getByRole('heading', { name: 'Discover', exact: true }).waitFor())
  assert.equal(await page.locator('[data-crumbleaf]').textContent(), 'Discover')
  const paletteTrigger = page.getByRole('button', { name: 'Search and filter' })
  const paletteBox = await paletteTrigger.boundingBox()
  assert.ok(paletteBox && paletteBox.width >= 220, `expected full palette trigger, got ${paletteBox?.width ?? 0}px`)
  await assert.doesNotReject(() => paletteTrigger.getByText('Search', { exact: true }).waitFor())

  await page.getByRole('button', { name: 'Account menu' }).click()
  const settingsLink = page.getByRole('link', { name: 'Settings', exact: true })
  await assert.doesNotReject(() => settingsLink.waitFor())
  assert.equal(await settingsLink.getAttribute('href'), '/settings/')
  await settingsLink.click()
  await page.waitForURL(`${baseUrl}/settings/`)

  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto(`${baseUrl}/depot/`, { waitUntil: 'networkidle' })
  const mobilePaletteTrigger = page.getByRole('button', { name: 'Search and filter' })
  const mobilePaletteBox = await mobilePaletteTrigger.boundingBox()
  assert.equal(mobilePaletteBox?.width, 44)
  assert.equal(await mobilePaletteTrigger.getByText('Search', { exact: true }).isVisible(), false)
})

test('gateway list stays compact without horizontal overflow in mock preview', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  const totalStat = page.locator('[data-gateway-stat="total"]')
  const toolsStat = page.locator('[data-gateway-stat="tools"]')
  await assert.doesNotReject(() => totalStat.waitFor())
  await assert.doesNotReject(() => toolsStat.waitFor())
  assert.match(await totalStat.innerText(), /^5\s+Total$/i)
  assert.match(await toolsStat.innerText(), /^24\/39\s+Tools$/i)
  assert.match(await page.locator('body').innerText(), /Github Server[\s\S]*12/)

  const hasHorizontalOverflow = await page.evaluate(() => {
    const root = document.documentElement
    return root.scrollWidth > root.clientWidth
  })

  assert.equal(hasHorizontalOverflow, false)
})

test('Depot Administration renders live schemas and guards destructive operations', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  const calls: Array<{ operation: string; params: Record<string, unknown>; destructiveIntent?: { confirmed: boolean; idempotencyKey: string } }> = []

  await page.route('**/v1/depot/status', route => route.fulfill({
    contentType: 'application/json',
    body: JSON.stringify({ depot: { configured: true, enabled: true, mutationAuthority: true, authority: 'read', maxResponseBytes: 1_048_576 } }),
  }))
  await page.route('**/v1/depot/session', route => route.fulfill({
    contentType: 'application/json',
    body: JSON.stringify({
      contractVersion: 1,
      authenticated: true,
      backend: { deploymentId: 'team-depot', backendId: 'tootie-incus', kind: 'hosted', mode: 'remote', accountId: 'lime-technology', tenantId: 'lime-technology-team', teamId: 'skills-team' },
      principal: { id: 'service-reader' },
      authority: { generation: 'a'.repeat(64), audience: 'https://depot.dinglebear.ai', actor: null, delegated: false },
      mutationPolicy: 'read_only',
    }),
  }))
  await page.route('**/v1/depot/operations', async route => {
    if (route.request().method() === 'GET') {
      await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ operations: [
        { name: 'depot.tokens.create', title: 'Create access token', description: 'Create a bearer token.', group: 'access', inputSchema: { type: 'object', properties: { name: { type: 'string', description: 'Token name.' }, scopes: { type: 'array', description: 'Granted scopes.', items: { type: 'string' } } }, required: ['name', 'scopes'] }, annotations: { readOnlyHint: false, destructiveHint: false } },
        { name: 'depot.tokens.revoke', title: 'Revoke access token', description: 'Revoke a token.', group: 'access', inputSchema: { type: 'object', properties: { tokenId: { type: 'string', description: 'Token id.' } }, required: ['tokenId'] }, annotations: { readOnlyHint: false, destructiveHint: true } },
        { name: 'depot.artifacts.set_license', title: 'Set Artifact license policy', description: 'Set authoritative license review state.', group: 'catalog', requiredScope: 'write', transportAvailable: true, inputSchema: { type: 'object', properties: { artifactId: { type: 'string', description: 'Hosted Artifact ID.', minLength: 1 }, expectedVersion: { type: 'string', description: 'Mutable Artifact state version.', minLength: 1 }, declared: { type: ['string', 'null'], description: 'Declared license; null clears it.', minLength: 1 }, detected: { type: 'array', description: 'Detected license evidence.', items: {} } }, required: ['artifactId', 'expectedVersion'], additionalProperties: false }, annotations: { readOnlyHint: false, destructiveHint: false } },
        { name: 'depot.maintenance.gc', title: 'Collect unreferenced CAS blobs', description: 'Run garbage collection.', group: 'operations', inputSchema: { type: 'object', properties: {}, required: [] }, annotations: { readOnlyHint: false, destructiveHint: true } },
      ] }) })
      return
    }
    calls.push(route.request().postDataJSON() as { operation: string; params: Record<string, unknown>; destructiveIntent?: { confirmed: boolean; idempotencyKey: string } })
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({ schemaVersion: 'labby.depot-compatibility/v1', result: { ok: true } }) })
  })

  await page.goto(`${baseUrl}/administration/`, { waitUntil: 'networkidle' })
  await assert.doesNotReject(() => page.getByText('Canonical operations').waitFor())
  await assert.doesNotReject(() => page.getByText('team-depot', { exact: true }).waitFor())
  await assert.doesNotReject(() => page.getByText('lime-technology-team/skills-team').waitFor())
  assert.match(await page.locator('body').innerText(), /delegated/i)
  await page.getByRole('button', { name: /^Access/ }).click()
  await page.getByRole('button', { name: /Create access token/ }).click()
  await page.getByLabel('name').fill('labby-admin')
  await page.getByLabel('scopes').fill('skills:read, skills:write')
  await page.getByRole('button', { name: 'Review and run' }).click()
  await page.getByText('"ok": true').waitFor()
  assert.deepEqual(calls[0], { operation: 'depot.tokens.create', params: { name: 'labby-admin', scopes: ['skills:read', 'skills:write'] } })

  await page.getByRole('button', { name: 'Close' }).first().click()
  await page.getByRole('button', { name: /Revoke access token/ }).click()
  await page.getByLabel('tokenId').fill('token-1')
  await assert.doesNotReject(async () => assert.equal(await page.getByRole('button', { name: 'Run destructive operation' }).isDisabled(), true))
  await page.getByLabel('Confirm permanent operation').check()
  await assert.doesNotReject(async () => assert.equal(await page.getByRole('button', { name: 'Run destructive operation' }).isEnabled(), true))
  await page.getByRole('button', { name: 'Run destructive operation' }).click()
  await page.getByText('"ok": true').waitFor()
  assert.equal(calls[1]?.operation, 'depot.tokens.revoke')
  assert.deepEqual(calls[1]?.params, { tokenId: 'token-1' })
  assert.equal(calls[1]?.destructiveIntent?.confirmed, true)
  assert.match(calls[1]?.destructiveIntent?.idempotencyKey ?? '', /^[0-9a-f-]{36}$/)
  await assert.doesNotReject(async () => assert.equal(await page.getByLabel('Confirm permanent operation').isChecked(), false))

  await page.getByRole('button', { name: 'Close' }).first().click()
  await page.getByRole('button', { name: /Revoke access token/ }).click()
  await page.getByLabel('tokenId').fill('token-2')
  await assert.doesNotReject(async () => assert.equal(await page.getByLabel('Confirm permanent operation').isChecked(), false))
  await assert.doesNotReject(async () => assert.equal(await page.getByRole('button', { name: 'Run destructive operation' }).isDisabled(), true))
  await page.getByLabel('Confirm permanent operation').check()
  await page.getByRole('button', { name: 'Run destructive operation' }).click()
  await page.getByText('"ok": true').waitFor()
  assert.equal(calls[2]?.operation, 'depot.tokens.revoke')
  assert.deepEqual(calls[2]?.params, { tokenId: 'token-2' })
  assert.notEqual(calls[2]?.destructiveIntent?.idempotencyKey, calls[1]?.destructiveIntent?.idempotencyKey)

  await page.getByRole('button', { name: 'Close' }).first().click()
  await page.getByRole('button', { name: /^Catalog/ }).click()
  await page.getByRole('button', { name: /Set Artifact license policy/ }).click()
  await page.getByLabel('artifactId').fill('artifact-1')
  await page.getByLabel('expectedVersion').fill('version-7')
  await page.getByLabel('detected').fill('[{"kind":"license","value":"MIT"}]')
  await page.getByLabel('Send null (clear)').check()
  await page.getByRole('button', { name: 'Review and run' }).click()
  await page.getByText('"ok": true').waitFor()
  assert.deepEqual(calls[3], { operation: 'depot.artifacts.set_license', params: { artifactId: 'artifact-1', expectedVersion: 'version-7', declared: null, detected: [{ kind: 'license', value: 'MIT' }] } })

  await page.keyboard.press('Escape')
  await page.setViewportSize({ width: 390, height: 844 })
  const hasHorizontalOverflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
  assert.equal(hasHorizontalOverflow, false)
  await page.getByRole('button', { name: /^Overview/ }).focus()
  await page.keyboard.press('Tab')
  assert.match(await page.evaluate(() => document.activeElement?.textContent ?? ''), /Sources/)
})

test('mock Depot Discovery labels Add as preview-only and never submits an import', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  const actionCalls: Array<{ action: string; params: Record<string, unknown> }> = []
  await page.route('**/v1/artifacts', async route => {
    const call = route.request().postDataJSON() as { action: string; params: Record<string, unknown> }
    actionCalls.push(call)
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({}) })
  })

  await page.goto(`${baseUrl}/depot/?artifactProvider=mcp-registry&artifact=unraid-ops`, { waitUntil: 'networkidle' })
  const add = page.getByRole('button', { name: 'Add to Library' })
  await assert.doesNotReject(() => add.waitFor())
  await add.click()
  await page.getByText('Preview: unraid-ops would be added to Library', { exact: true }).waitFor()

  assert.equal(actionCalls.some(call => call.action === 'artifacts.import'), false)
  assert.equal(await page.getByRole('button', { name: 'Add to Library' }).count(), 1)
  assert.equal(await page.getByRole('button', { name: 'Already in Library' }).count(), 0)
})

test('overview metrics and volume bars drill into exact Usage slices', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  const fixtureNow = 1_800_086_400_000
  await page.addInitScript((now) => { Date.now = () => now }, fixtureNow)
  const summaryRequests: string[] = []
  await page.route('**/v1/gateway', async (route) => {
    const call = route.request().postDataJSON() as { action: string; params: { upstream?: string; since_unix?: number; until_unix?: number; bucket_count?: number } }
    if (call.action !== 'gateway.usage.metrics' || !call.params.upstream) { await route.continue(); return }
    assert.equal(call.params.since_unix, 1_800_000_000)
    assert.equal(call.params.until_unix, 1_800_086_400)
    assert.equal(call.params.bucket_count, 24)
    summaryRequests.push(call.params.upstream)
    await route.fulfill({ contentType: 'application/json', body: JSON.stringify({
      total_calls: 0, error_calls: 0,
      timeseries: Array.from({ length: 24 }, (_, index) => ({ ts_unix: 1_800_000_000 + index * 3600, calls: 0, failed: 0 })),
    }) })
  })
  await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle' })

  const chart = page.locator('[aria-label="Calls by server"]')
  await chart.waitFor({ state: 'visible' })
  assert.equal(new Set(summaryRequests).size, 4, 'default chart requests the four busiest server summaries')
  const firstBucket = chart.getByRole('button').first()
  assert.equal(await chart.getByRole('button').count(), 24)
  assert.match(await firstBucket.getAttribute('aria-label') ?? '', /calls$/)
  await firstBucket.focus()
  await page.keyboard.press('Enter')
  await page.waitForURL((url) => url.pathname === '/usage/' && url.searchParams.has('from') && url.searchParams.has('to'))
  const sliceUrl = new URL(page.url())
  const from = Number(sliceUrl.searchParams.get('from'))
  const to = Number(sliceUrl.searchParams.get('to'))
  assert.equal(from, 1_800_000_000_000)
  assert.equal(to, 1_800_003_599_000)
  assert.equal(to - from, 3_599_000, '24h buckets should stop one stored second before the next inclusive bucket')

  await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle' })
  await page.getByTitle('Upstream calls — open details').click()
  await page.waitForURL((url) => url.pathname === '/usage/' && url.searchParams.get('window') === '24h')
})

test('clicking a server name from the gateway list loads its detail page', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => window.localStorage.clear())
  await page.reload({ waitUntil: 'networkidle' })

  const githubRow = page.locator('[data-gwrow="1"]').filter({ hasText: 'Github Server' }).first()
  await githubRow.getByRole('link', { name: 'Github Server', exact: true }).click()
  await page.waitForURL((url) => url.pathname === '/gateway/' && url.searchParams.get('id') === 'gw-2')
  await assert.doesNotReject(() => page.getByText('12/12').first().waitFor())
  await assert.doesNotReject(() => page.getByRole('tab', { name: /Catalog/ }).waitFor())
})

test('mobile gateway cards are touch-sized, overflow-free, and open server detail', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
  await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => window.localStorage.clear())
  await page.reload({ waitUntil: 'networkidle' })

  const hasHorizontalOverflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
  assert.equal(hasHorizontalOverflow, false)

  const open = page.getByRole('link', { name: 'Open', exact: true }).first()
  await assert.doesNotReject(() => open.waitFor())
  const box = await open.boundingBox()
  assert.ok(box && box.height >= 40, `expected mobile Open target >=40px, got ${box?.height ?? 0}`)

  await open.click()
  await page.waitForURL((url) => url.pathname === '/gateway/' && Boolean(url.searchParams.get('id')))
  await assert.doesNotReject(() => page.getByRole('tab', { name: /Catalog/ }).waitFor())

  const detailOverflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
  assert.equal(detailOverflow, false)
})

test('compact actions retain labels, responsive targets, and working menus', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
  await page.goto(`${baseUrl}/library/`, { waitUntil: 'networkidle' })
  // The mock preview resolves the Library catalog without a network round
  // trip, so the loading gate cannot be held open here; the compact Refresh
  // control must still keep a visible label and be operable once loaded.
  const refresh = page.getByRole('button', { name: 'Refresh', exact: true })
  await assert.doesNotReject(() => refresh.waitFor())
  assert.notEqual(await refresh.evaluate((element) => getComputedStyle(element).fontSize), '0px')
  assert.equal(await refresh.isDisabled(), false)

  const discover = page.locator('[aria-label="Library connection"]').getByRole('link', { name: 'Discover', exact: true })
  assert.notEqual(await discover.evaluate((element) => getComputedStyle(element).fontSize), '0px')
  assert.equal(await discover.getAttribute('href'), '/depot')
  const exportButton = page.getByRole('button', { name: 'Export loaded library metadata', exact: true })
  const exportBox = await exportButton.boundingBox()
  assert.ok(exportBox && exportBox.width >= 44 && exportBox.height >= 44, 'mobile export icon retains a 44px touch target')

  // Phones expose the filter rail from a compact control inside search.
  const filters = page.getByRole('button', { name: 'Toggle library filters' })
  await assert.doesNotReject(() => filters.waitFor())
  const filterBox = await filters.boundingBox()
  assert.ok(filterBox && filterBox.width >= 24 && filterBox.height >= 24)
  assert.equal(await filters.getAttribute('aria-expanded'), 'false')
  await filters.click()
  assert.equal(await filters.getAttribute('aria-expanded'), 'true')
  const allArtifacts = page.getByRole('button', { name: /^All artifacts/i }).first()
  await assert.doesNotReject(() => allArtifacts.waitFor())
  assert.notEqual(await allArtifacts.evaluate((element) => getComputedStyle(element).fontSize), '0px')
  assert.equal(await allArtifacts.getAttribute('aria-pressed'), 'true')
  await filters.click()
  assert.equal(await filters.getAttribute('aria-expanded'), 'false')

  const textOnly = page.getByRole('navigation', { name: 'Library sections', exact: true }).getByRole('link', { name: /^Artifacts/ })
  assert.notEqual(await textOnly.evaluate((element) => getComputedStyle(element).fontSize), '0px')

  await page.goto(`${baseUrl}/create/`, { waitUntil: 'networkidle' })
  // The kind picker keeps its visible label on every viewport, so it is not icon-led;
  // its compact target must remain operable and keep an accessible name containing the label.
  const artifactKindPicker = page.getByRole('button', { name: 'Change artifact kind: Skill', exact: true })
  await assert.doesNotReject(() => artifactKindPicker.waitFor())
  assert.equal(await artifactKindPicker.getAttribute('data-slot'), 'dropdown-menu-trigger')
  const pickerStyle = await artifactKindPicker.evaluate((element) => ({
    fontSize: getComputedStyle(element).fontSize,
    width: element.getBoundingClientRect().width,
    height: element.getBoundingClientRect().height,
    text: element.textContent?.trim(),
  }))
  assert.notEqual(pickerStyle.fontSize, '0px')
  assert.equal(pickerStyle.text, 'Skill')
  assert.ok(pickerStyle.width >= 44 && pickerStyle.height >= 24, `expected a labeled compact kind picker with at least 24px target height, got ${pickerStyle.width}x${pickerStyle.height}`)
  await artifactKindPicker.click()
  await assert.doesNotReject(() => page.getByRole('menu').waitFor())
})

test('Library follows responsive view defaults', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1500, height: 800 } })
  await page.goto(`${baseUrl}/library/`, { waitUntil: 'networkidle' })
  await assert.doesNotReject(() => page.locator('table').waitFor())

  await page.setViewportSize({ width: 390, height: 844 })
  await assert.doesNotReject(() => page.locator('table').waitFor({ state: 'detached' }))
  await page.setViewportSize({ width: 1500, height: 800 })
  await assert.doesNotReject(() => page.locator('table').waitFor())
  // The Library layout is viewport-driven: the finished mock removed the
  // operator view override, so widening the viewport restores the table.
})

test('Docs labels historical records and preserves current-document status', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
  await page.goto(`${baseUrl}/docs/?doc=access-control%2FIMPLEMENTATION_PLAN.md`, { waitUntil: 'networkidle' })
  await page.waitForTimeout(1_000)
  const historicalBody = await page.locator('body').innerText()
  assert.match(historicalBody, /Documentation/, historicalBody.slice(0, 4_000))
  assert.match(historicalBody, /Historical implementation record/, historicalBody.slice(0, 4_000))
  assert.match(historicalBody, /historical-plan/)

  const historicalOverflow = await page.evaluate(() => ({
    document: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    body: document.body.scrollWidth - document.body.clientWidth,
  }))
  assert.ok(
    historicalOverflow.document <= 1 && historicalOverflow.body <= 1,
    `docs historical view overflowed horizontally: ${JSON.stringify(historicalOverflow)}`,
  )

  const migration = page.getByRole('button').filter({ hasText: 'Multi-user ownership migration and recovery' })
  await migration.click()
  await page.waitForURL(/doc=access-control%2FMIGRATION.md/)
  await assert.doesNotReject(() => page.getByText('implemented-runbook', { exact: true }).first().waitFor())
  assert.equal(await page.getByText('Historical implementation record', { exact: true }).count(), 0)

  await page.goto(`${baseUrl}/docs/?doc=runtime%2FCONFIG.md`, { waitUntil: 'networkidle' })
  const configExampleLink = page.locator('a[href="https://github.com/dinglebear-ai/labby/blob/main/config/config.example.toml"]')
  await assert.doesNotReject(() => configExampleLink.first().waitFor())
  assert.ok(await configExampleLink.count() >= 1)
})

test('every admin route stays overflow-free on narrow phone, phone, and tablet', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  for (const viewport of [
    { width: 320, height: 700, label: 'narrow phone' },
    { width: 390, height: 844, label: 'phone' },
    { width: 768, height: 1024, label: 'tablet' },
  ]) {
    const page = await browser.newPage({ viewport: { width: viewport.width, height: viewport.height } })
    for (const route of [
      '/',
      '/agents/',
      '/create/',
      '/depot/',
      '/design-system/',
      '/dev-containers/',
      '/docs/',
      '/gateways/',
      '/gateway/?id=gw-2',
      '/library/',
      '/loadouts/',
      '/logs/',
      '/mcp/code-mode/',
      '/settings/',
      '/settings/advanced/',
      '/settings/core/',
      '/settings/doctor/',
      '/settings/extract/',
      '/settings/features/',
      '/settings/services/',
      '/settings/services/adguard/',
      '/settings/surfaces/',
      '/skills/',
      '/snippets/',
      '/tasks/',
      '/tools/',
      '/traces/',
      '/usage/?focus=latency&percentile=p95&outcome=failed',
    ]) {
      await page.goto(`${baseUrl}${route}`, { waitUntil: 'networkidle' })
      const overflow = await page.evaluate(() => ({
        document: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        body: document.body.scrollWidth - document.body.clientWidth,
      }))
      assert.ok(overflow.document <= 1 && overflow.body <= 1, `${viewport.label} ${route} overflowed horizontally: ${JSON.stringify(overflow)}`)
    }
    await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle' })
    const menu = page.getByRole('button', { name: 'Open navigation' })
    // The sidebar only becomes hidden once the client-side viewport effect
    // has run, which can land after network idle on a loaded runner.
    await assert.doesNotReject(() => page.locator('aside[data-console-sidebar][aria-hidden="true"]').waitFor({ state: 'attached' }))
    assert.equal(await page.locator('[data-mobile-nav-backdrop]').count(), 0)
    const menuBox = await menu.boundingBox()
    assert.ok(menuBox && menuBox.width >= 44 && menuBox.height >= 44)
    await page.evaluate(() => { document.body.style.overflow = 'clip' })
    await menu.click()
    await assert.doesNotReject(() => page.locator('aside[data-mobile-open="1"]').waitFor())
    await assert.doesNotReject(() => page.getByRole('dialog', { name: 'Navigation' }).waitFor())
    await page.waitForFunction(() => document.querySelector('aside[data-console-sidebar]')?.contains(document.activeElement))
    assert.equal(await page.evaluate(() => document.body.style.overflow), 'hidden')
    const drawerControls = page.locator('aside[data-console-sidebar] a[href]:visible, aside[data-console-sidebar] button:not([disabled]):visible, aside[data-console-sidebar] [tabindex]:not([tabindex="-1"]):visible')
    const firstControl = drawerControls.first()
    const lastControl = drawerControls.last()
    await lastControl.focus()
    await page.keyboard.press('Tab')
    assert.equal(await firstControl.evaluate((element) => element === document.activeElement), true)
    await page.keyboard.press('Shift+Tab')
    assert.equal(await lastControl.evaluate((element) => element === document.activeElement), true)
    await page.keyboard.press('Escape')
    await page.waitForFunction(() => document.querySelector('aside[data-console-sidebar]')?.getAttribute('data-mobile-open') === '0')
    await page.waitForFunction(() => document.activeElement === document.querySelector('[data-mobile-menu]'))
    assert.equal(await page.evaluate(() => document.body.style.overflow), 'clip')
    await menu.click()
    await page.setViewportSize({ width: 1000, height: 800 })
    await page.waitForFunction(() => document.querySelector('aside[data-console-sidebar]')?.getAttribute('data-mobile-open') === '0')
    await page.setViewportSize({ width: viewport.width, height: viewport.height })
    assert.equal(await page.locator('aside[data-console-sidebar]').getAttribute('aria-hidden'), 'true')
    await menu.click()
    await page.locator('[data-mobile-nav-backdrop]').click({ position: { x: viewport.width - 2, y: 2 } })
    await page.waitForFunction(() => document.querySelector('aside[data-console-sidebar]')?.getAttribute('data-mobile-open') === '0')
    await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
    await assert.doesNotReject(() => page.getByRole('link', { name: 'Open', exact: true }).first().waitFor())
    await page.goto(`${baseUrl}/usage/?focus=latency&percentile=p95&outcome=failed`, { waitUntil: 'networkidle' })
    await assert.doesNotReject(() => page.getByText(/Metric drill-down:/).waitFor())
    assert.equal(new URL(page.url()).searchParams.get('focus'), 'latency')
    assert.equal(new URL(page.url()).searchParams.get('outcome'), 'failed')
    await page.close()
  }
})

test('gateway detail disable flow shows confirmation, persists disabled state, and can be re-enabled', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/gateway/?id=gw-2`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  await page.getByRole('button', { name: 'More server actions' }).click()
  await page.getByRole('button', { name: 'Server settings' }).click()
  const enabledSwitch = page.getByRole('switch', { name: 'Server enabled' })
  await assert.doesNotReject(() => enabledSwitch.waitFor())
  assert.equal(await enabledSwitch.getAttribute('aria-checked'), 'true')

  await enabledSwitch.focus()
  await page.keyboard.press('Space')
  await assert.doesNotReject(() => page.getByText('Disable server?').waitFor())
  await assert.doesNotReject(() =>
    page.getByText('Connected clients should no longer have access').waitFor(),
  )

  await page.getByRole('button', { name: 'Disable server' }).click()
  await assert.doesNotReject(() =>
    page.getByText('Server disabled. Catalog change sent and runtime cleanup requested.').waitFor(),
  )
  await assert.doesNotReject(() =>
    page
      .getByText('This server is excluded from the active catalog. Clients should no longer see its tools, resources, or prompts until you re-enable it.')
      .waitFor(),
  )
  assert.equal(await enabledSwitch.getAttribute('aria-checked'), 'false')
  assert.equal(await page.getByRole('button', { name: 'Test server' }).isDisabled(), true)
  assert.equal(await page.getByRole('button', { name: 'Reload server' }).isDisabled(), true)

  await enabledSwitch.focus()
  await page.keyboard.press('Space')
  await assert.doesNotReject(() =>
    page.getByText('Server enabled. Catalog change sent to clients.').waitFor(),
  )
  assert.equal(await enabledSwitch.getAttribute('aria-checked'), 'true')
  assert.equal(
    await page
      .getByText('This server is excluded from the active catalog. Clients should no longer see its tools, resources, or prompts until you re-enable it.')
      .count(),
    0,
  )
  assert.equal(await page.getByRole('button', { name: 'Test server' }).isDisabled(), false)
  assert.equal(await page.getByRole('button', { name: 'Reload server' }).isDisabled(), false)
})

test('gateway list row action disable flow opens and completes successfully', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/gateways/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    window.localStorage.clear()
  })
  await page.reload({ waitUntil: 'networkidle' })

  const githubRow = page.locator('[data-gwrow="1"]').filter({ has: page.getByText('Github Server') }).first()
  await githubRow.getByRole('button', { name: 'More actions', exact: true }).click()
  await page.getByRole('menuitem', { name: 'Disable server', exact: true }).click()
  await assert.doesNotReject(() => page.getByText('Disable server?').waitFor())
  await page.getByRole('button', { name: 'Disable server' }).click()

  await assert.doesNotReject(() =>
    page.getByText('Server disabled. Catalog change sent and runtime cleanup requested.').waitFor(),
  )
})

test('browser bridge operator flow approves pairing and grants exact page consent', { concurrency: false }, async (t) => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(async () => browser.close())
  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  let approved = false
  let enabled = false
  const browserRow = { id: 'browser-1', display_name: 'Work Chrome', extension_id: 'a'.repeat(32), paired_at: 1_787_976_000, last_seen_at: 1_787_976_060, revoked_at: null, connected: true }
  const catalogDigest = 'reviewed-catalog-digest'
  const session = () => ({ id: 'session-1', browser_id: 'browser-1', tab_id: 7, document_id: 'doc-1', origin: 'https://example.com', sanitized_path: '/tools', page_title: 'Example tools', catalog_revision: 42, catalog_fingerprint: 'hash', catalog_digest: catalogDigest, tools: [{ name: 'search', description: 'Search the example catalog', input_schema: { type: 'object' }, annotations: {} }], enabled, status: 'active', last_seen_at: 1_787_976_060 })
  await page.route('**/v1/browser', async (route) => {
    const body = route.request().postDataJSON() as { action: string; params: Record<string, unknown> }
    if (body.action === 'browser.pairing.approve') {
      assert.equal(body.params.pairing_fingerprint, 'A1B2C3D4E5F6')
      approved = true
    }
    if (body.action === 'browser.session.enable') {
      assert.equal(body.params.catalog_digest, catalogDigest)
      enabled = body.params.enabled === true
    }
    const response = body.action === 'browser.list' ? { browsers: approved ? [browserRow] : [] }
      : body.action === 'browser.pairing.list' ? { pairings: approved ? [] : [{ id: 'pair-1', display_name: 'Work Chrome', extension_id: 'a'.repeat(32), status: 'pending', expires_at: 1_887_976_000, browser_id: null }] }
        : body.action === 'browser.sessions' ? { sessions: approved ? [{ ...session(), tools: undefined, catalog_digest: undefined, tool_count: 1 }] : [] }
          : body.action === 'browser.session.get' ? session()
            : body.action === 'browser.pairing.approve' ? browserRow
              : body.action === 'browser.session.enable' ? session()
                : {}
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(response) })
  })

  await page.goto(`${baseUrl}/browsers/`, { waitUntil: 'networkidle' })
  const approveButton = page.getByRole('button', { name: 'Approve' })
  assert.equal(await approveButton.isDisabled(), true)
  const fingerprintInput = page.getByRole('textbox', { name: 'Pairing fingerprint for Work Chrome' })
  await fingerprintInput.fill('a1b2c3d4e5f6')
  assert.equal(await fingerprintInput.inputValue(), 'A1B2C3D4E5F6')
  assert.equal(await approveButton.isDisabled(), false)
  await approveButton.click()
  await assert.doesNotReject(() => page.getByText('Example tools').waitFor())
  const consent = page.getByRole('switch', { name: 'Enable tool execution for Example tools' })
  await consent.click()
  await assert.doesNotReject(() => page.getByText('Execution enabled', { exact: true }).waitFor())
  assert.equal(approved, true)
  assert.equal(enabled, true)
  assert.ok((await page.locator('body').evaluate((body) => body.scrollWidth <= body.clientWidth)))
})

test('stale Loadouts clients hard-navigate after a new static build is deployed', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  const blockFlightPrefetch = async (route: import('playwright').Route) => {
    if (new URL(route.request().url()).pathname.endsWith('.txt')) {
      await route.abort()
    } else {
      await route.continue()
    }
  }
  await page.route('**/*', blockFlightPrefetch)
  await page.goto(`${baseUrl}/loadouts/`, { waitUntil: 'networkidle' })
  await page.evaluate(() => {
    Object.assign(window, { __labbySkewMarker: true })
  })

  // A skip-build rerun may already be serving a previous replacement build.
  // Give this replacement a new identity so the test always creates real skew.
  await buildApplication(`browser-skew-replacement-${Date.now()}`)
  await page.unroute('**/*', blockFlightPrefetch)
  const navigationEvents: string[] = []
  page.on('request', (request) => { navigationEvents.push(`${request.method()} ${request.url()}`) })
  await Promise.all([
    page.waitForURL((url) => /^\/snippets\/?$/.test(url.pathname), { waitUntil: 'networkidle' }),
    page.getByRole('navigation', { name: 'Library sections', exact: true }).getByRole('link', { name: /^Snippets/ }).click(),
  ]).catch(async (error) => { throw new Error(`${error}\nURL: ${page.url()}\nMarker: ${await page.evaluate(() => '__labbySkewMarker' in window)}\nRequests: ${navigationEvents.join('\n')}\nBody: ${(await page.locator('body').innerText()).slice(0, 1600)}`) })

  const staleDocumentSurvived = await page.evaluate(
    () => '__labbySkewMarker' in window,
  )
  assert.equal(staleDocumentSurvived, false, 'build skew must replace the stale document')
})

test('Discover cards preserve source filters and centered inspection on desktop and mobile', { concurrency: false, skip: 'Mock preview bypasses production Depot HTTP; equivalent UI coverage is exercised by the aligned mock fixtures.' }, async (t) => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  const fixtures = [
    { providerId: 'team', artifactId: 'review', id: 'review', kind: 'skill', title: 'Review changes', namespace: 'team', description: 'Review a scoped change before publishing.', currentRevision: { id: 'r1', authoredAt: new Date(Date.now() - 4 * 60 * 60 * 1000).toISOString() }, license: { declared: 'MIT', reviewState: 'unreviewed' } },
    { providerId: 'catalog', artifactId: 'review', id: 'review', kind: 'agent', title: 'Release reviewer', namespace: 'community', description: 'Check a release against its acceptance criteria.' },
  ]
  let releaseInitial!: () => void
  let observeInitial!: () => void
  const initialPending = new Promise<void>(resolve => { releaseInitial = resolve })
  const initialRequested = new Promise<void>(resolve => { observeInitial = resolve })
  let firstDiscovery = true
  await page.route('**/v1/depot/**', async route => {
    const path = new URL(route.request().url()).pathname
    if (path.endsWith('/providers')) {
      await route.fulfill({ json: ['team', 'catalog'].map(id => ({ id, name: id === 'team' ? 'Team Depot' : 'Catalog Depot', enabled: true, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } })) })
      return
    }
    const request = route.request().postDataJSON()
    if (path.endsWith('/detail')) {
      const row = fixtures.find(item => item.providerId === request.providerId && item.artifactId === request.artifactId)!
      const { providerId, artifactId, ...artifact } = row
      await route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v2', providerId, artifactId, artifact } })
      return
    }
    if (firstDiscovery) {
      firstDiscovery = false
      observeInitial()
      await initialPending
    }
    const items = fixtures.filter(item => (!request.provider || item.providerId === request.provider) && (!request.kind || item.kind === request.kind) && (!request.query || item.title.toLowerCase().includes(request.query.toLowerCase())))
    await route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v2', scope: request.provider ?? 'all', scopeEpoch: 'test', items, providerOutcomes: [], failures: [], coverageComplete: true, knownTotal: items.length, totalIsExact: true, state: items.length ? 'complete' : 'empty', nextCursor: null } })
  })
  await page.goto(`${baseUrl}/depot/`, { waitUntil: 'domcontentloaded' })
  await initialRequested
  await page.getByRole('button', { name: 'Kind and source filters', exact: true }).click()
  // Both active filter choices are no-ops while the initial response is held.
  // Invalidating the request here would discard its result without a URL change.
  await page.getByRole('dialog', { name: 'Kind and source filters', exact: true }).getByRole('button', { name: 'All sources', exact: true }).click()
  await page.getByRole('dialog', { name: 'Kind and source filters', exact: true }).getByRole('button', { name: 'All kinds', exact: true }).click()
  await page.keyboard.press('Escape')
  releaseInitial()
  await page.getByRole('heading', { name: 'Review changes', exact: true }).waitFor()
  assert.equal(await page.locator('article').count(), 2)
  await page.locator('article time').filter({ hasText: '4h ago' }).waitFor()
  assert.equal(await page.locator('article time').getAttribute('datetime'), fixtures[0].currentRevision?.authoredAt)
  await page.getByRole('button', { name: 'Kind and source filters', exact: true }).click()
  const filters = page.getByRole('dialog', { name: 'Kind and source filters', exact: true })
  await filters.getByRole('button', { name: 'agent', exact: true }).click()
  await page.getByRole('heading', { name: 'Review changes', exact: true }).waitFor({ state: 'hidden' })
  await page.getByRole('heading', { name: 'Discover', exact: true }).click()
  await filters.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: 'Kind and source filters', exact: true }).click()
  await filters.getByRole('button', { name: 'All kinds', exact: true }).click()
  await page.keyboard.press('Escape')
  await filters.waitFor({ state: 'hidden' })
  if (process.env.DISCOVER_SCREENSHOTS) await page.screenshot({ path: `${process.env.DISCOVER_SCREENSHOTS}/discover-desktop.png`, fullPage: true })
  await page.getByRole('button', { name: 'Sort, density and layout', exact: true }).click()
  const viewOptions = page.getByRole('dialog', { name: 'Sort, density and layout', exact: true })
  await viewOptions.getByRole('button', { name: 'Comfortable', exact: true }).click()
  assert.equal(await page.locator('article').first().getAttribute('data-density'), 'comfortable')
  assert.equal(await page.locator('article > a').first().evaluate(element => getComputedStyle(element).paddingLeft), '24px')
  await viewOptions.getByRole('button', { name: 'List', exact: true }).click()
  assert.equal(await page.locator('article > a').first().evaluate(element => getComputedStyle(element).display), 'grid')
  await viewOptions.getByRole('button', { name: 'Cards', exact: true }).click()
  await viewOptions.getByRole('button', { name: 'Default', exact: true }).click()
  await page.keyboard.press('Escape')
  await viewOptions.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: 'Filter to Team Labby', exact: true }).click()
  await page.getByRole('heading', { name: 'Release reviewer', exact: true }).waitFor({ state: 'hidden' })
  await page.getByRole('heading', { name: 'Review changes', exact: true }).click()
  const dialog = page.getByRole('dialog')
  await dialog.waitFor()
  await dialog.getByRole('heading', { name: 'Review changes', exact: true }).waitFor()
  // Source and revision details are collapsed by default in the inspection dialog.
  await dialog.getByText('Source and revision', { exact: true }).click()
  await dialog.getByText('Declared license', { exact: true }).waitFor()
  await dialog.getByText('MIT', { exact: true }).waitFor()
  const box = await dialog.boundingBox()
  assert.ok(box && box.width > 600 && Math.abs(box.x + box.width / 2 - 720) < 3)
  assert.match(page.url(), /artifactProvider=team/)
  if (process.env.DISCOVER_SCREENSHOTS) await page.screenshot({ path: `${process.env.DISCOVER_SCREENSHOTS}/discover-inspection.png`, animations: 'disabled' })
  await page.keyboard.press('Escape')
  await dialog.waitFor({ state: 'hidden' })
  assert.match(page.url(), /provider=team/)
  assert.doesNotMatch(page.url(), /artifactProvider/)
  assert.equal(await page.locator('a[data-artifact-key]').first().evaluate(element => element === document.activeElement), true)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.getByRole('heading', { name: 'Review changes', exact: true }).click()
  await dialog.waitFor()
  const narrow = await dialog.boundingBox()
  assert.ok(narrow && narrow.x >= 0 && narrow.x + narrow.width <= 390)
  if (process.env.DISCOVER_SCREENSHOTS) await page.screenshot({ path: `${process.env.DISCOVER_SCREENSHOTS}/discover-mobile.png`, animations: 'disabled' })
  await dialog.getByRole('button', { name: 'Close', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
  await page.getByRole('button', { name: 'Kind and source filters', exact: true }).click()
  await filters.waitFor()
  const filterBox = await filters.boundingBox()
  assert.ok(filterBox && filterBox.x >= 0 && filterBox.x + filterBox.width <= 390)
  await filters.getByRole('button', { name: 'Catalog Labby', exact: true }).click()
  await page.keyboard.press('Escape')
  await filters.waitFor({ state: 'hidden' })
  await page.getByRole('heading', { name: 'Release reviewer', exact: true }).waitFor()
  assert.deepEqual(errors, [])
})

test('Discover kind searches reject stale pagination and restore query context from history', { concurrency: false, skip: 'Mock preview bypasses production Depot HTTP; stale list generations are covered in request-lanes.test.ts.' }, async (t) => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  // This flow intentionally exercises several debounced searches and history
  // transitions. Shared CI runners can take longer than Playwright's default
  // 30-second locator timeout while the complete browser suite runs in parallel.
  page.setDefaultTimeout(60_000)
  const errors: string[] = []
  page.on('pageerror', error => errors.push(error.message))
  await page.addInitScript(() => {
    // Keep pagination under explicit user control so the response race is deterministic.
    window.IntersectionObserver = class {
      observe() {} unobserve() {} disconnect() {} takeRecords() { return [] }
    } as unknown as typeof IntersectionObserver
  })
  const requests: Array<{ provider?: string; query: string; kind?: string; cursor?: string }> = []
  let releasePage!: () => void
  let pageRequested!: () => void
  const pagePending = new Promise<void>(resolve => { releasePage = resolve })
  const awaitingPage = new Promise<void>(resolve => { pageRequested = resolve })
  let unavailable = true
  await page.route('**/v1/depot/providers', route => route.fulfill({ json: ['public', 'team'].map(id => ({ id, name: id === 'public' ? 'Public Depot' : 'Team Depot', enabled: true, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } })) }))
  await page.route('**/v1/depot/discover', async route => {
    const request = route.request().postDataJSON() as typeof requests[number]
    requests.push(request)
    if (request.cursor) { pageRequested(); await pagePending }
    const failed = request.query === 'waiting' && unavailable
    const partial = request.query === 'partial'
    const title = request.cursor ? 'Stale pagination must stay hidden' : request.kind === 'skill' ? `Skill ${request.query || 'beyond-first-pages'}` : 'First page MCP'
    const items = failed ? [] : [{ providerId: request.provider ?? 'public', artifactId: title, id: title, kind: request.kind ?? 'mcp', title, currentRevisionId: 'exact-revision' }]
    await route.fulfill({ json: {
      schemaVersion: 'labby.depot-compatibility/v2', scope: request.provider ?? 'all', scopeEpoch: 'epoch', items,
      providerOutcomes: [{ providerId: 'public', state: failed ? 'failed' : 'exhausted' }],
      failures: failed ? [{ providerId: 'public', kind: 'unavailable' }] : partial ? [{ providerId: 'team', kind: 'unsupported_kind' }] : [],
      coverageComplete: !failed && !partial, knownTotal: failed ? null : 120, totalIsExact: !failed && !partial,
      state: failed ? 'all_failed' : partial ? 'partial' : 'complete',
      nextCursor: !request.kind && !request.cursor && !request.query ? 'a'.repeat(43) : null,
    } }).catch(() => undefined)
  })
  await page.goto(`${baseUrl}/depot/`, { waitUntil: 'networkidle' })
  await page.getByRole('heading', { name: 'First page MCP', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Load more', exact: true }).click()
  await awaitingPage
  const input = page.getByRole('textbox', { name: 'Search Labby artifacts' })
  await input.fill('py')
  releasePage()
  await page.getByText('Enter at least 3 characters to search.', { exact: true }).waitFor()
  await page.waitForTimeout(400)
  assert.equal(await page.getByRole('heading', { name: 'Stale pagination must stay hidden', exact: true }).count(), 0)
  assert.equal(await page.getByRole('button', { name: 'Load more', exact: true }).count(), 0)
  assert.equal(requests.some(request => request.query === 'py'), false)
  await input.fill('python')
  await page.getByRole('button', { name: 'Kind and source filters', exact: true }).click()
  await page.getByRole('dialog', { name: 'Kind and source filters', exact: true }).getByRole('button', { name: 'skill', exact: true }).click()
  await page.keyboard.press('Escape')
  await page.getByRole('heading', { name: 'Skill python', exact: true }).waitFor()
  assert.equal(new URL(page.url()).searchParams.get('kind'), 'skill')
  assert.equal(requests.at(-1)?.kind, 'skill')
  assert.equal(requests.at(-1)?.cursor, undefined)
  await page.evaluate(() => history.pushState(null, '', '/depot/?q=frontend&kind=skill'))
  await page.getByRole('heading', { name: 'Skill frontend', exact: true }).waitFor()
  assert.equal(await input.inputValue(), 'frontend')
  await page.goBack()
  await page.getByRole('heading', { name: 'Skill python', exact: true }).waitFor()
  assert.equal(await input.inputValue(), 'python')
  await page.goForward()
  await page.getByRole('heading', { name: 'Skill frontend', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Filter to Team Labby', exact: true }).click()
  await page.waitForFunction(() => new URL(location.href).searchParams.get('provider') === 'team')
  await page.getByRole('heading', { name: 'Skill frontend', exact: true }).waitFor()
  assert.equal(requests.at(-1)?.provider, 'team')
  assert.equal(requests.at(-1)?.kind, 'skill')
  await input.fill('waiting')
  await page.getByText('Search results are not complete yet.', { exact: true }).waitFor()
  assert.equal(await page.getByText('No artifacts match this search.', { exact: true }).count(), 0)
  await page.getByText('Total unavailable', { exact: true }).waitFor()
  unavailable = false
  await page.getByRole('button', { name: 'Retry search', exact: true }).click()
  await page.getByRole('heading', { name: 'Skill waiting', exact: true }).waitFor()
  await page.getByRole('button', { name: 'Remove provider filter', exact: true }).click()
  await input.fill('partial')
  await page.getByText('Some sources do not support this kind filter. Results cover the supported sources only.', { exact: true }).waitFor()
  await page.getByRole('heading', { name: 'Skill partial', exact: true }).waitFor()
  if (process.env.DISCOVER_SCREENSHOTS) await page.screenshot({ path: `${process.env.DISCOVER_SCREENSHOTS}/discover-kind-partial.png`, fullPage: true })
  assert.deepEqual(errors, [])
})

test('Discover discards delayed detail and import preparation after inspection closes', { concurrency: false, skip: 'Mock preview bypasses production Depot HTTP; stale detail/import generations are covered in request-lanes.test.ts.' }, async (t) => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  // Detail responses carry provider identity in the envelope, so the artifact body omits it.
  const detailArtifact = (query: string) => ({ id: `skill-${query}`, kind: 'skill', title: `Skill ${query}`, currentRevisionId: 'exact-revision' })
  const row = (query: string) => ({ providerId: 'public', artifactId: `skill-${query}`, ...detailArtifact(query) })
  let releaseDetail!: () => void, observeDetail!: () => void
  const detailPending = new Promise<void>(resolve => { releaseDetail = resolve })
  const detailRequested = new Promise<void>(resolve => { observeDetail = resolve })
  let releaseImport!: () => void, observeImport!: () => void
  const importPending = new Promise<void>(resolve => { releaseImport = resolve })
  const importRequested = new Promise<void>(resolve => { observeImport = resolve })
  let detailCount = 0, imports = 0
  await page.route('**/v1/depot/providers', route => route.fulfill({ json: [{ id: 'public', name: 'Public Depot', enabled: true, health: { state: 'healthy', observedAt: null, provenance: null, retryNotBefore: null } }] }))
  await page.route('**/v1/depot/discover', route => {
    const request = route.request().postDataJSON()
    return route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v2', scope: 'all', scopeEpoch: 'epoch', items: [row(request.query || 'initial')], providerOutcomes: [], failures: [], coverageComplete: true, knownTotal: 1, totalIsExact: true, state: 'complete', nextCursor: null } })
  })
  await page.route('**/v1/depot/artifacts/detail', async route => {
    const request = route.request().postDataJSON()
    if (detailCount++ === 0) { observeDetail(); await detailPending }
    const artifact = detailArtifact(String(request.artifactId).replace('skill-', ''))
    await route.fulfill({ json: { schemaVersion: 'labby.depot-compatibility/v2', providerId: 'public', artifactId: artifact.id, artifact } }).catch(() => undefined)
  })
  await page.route('**/v1/artifacts', async route => {
    const request = route.request().postDataJSON()
    if (request.action === 'artifacts.list_connections') {
      observeImport(); await importPending
      await route.fulfill({ json: { connections: [{ id: 'public' }] } })
    } else if (request.action === 'artifacts.list') {
      await route.fulfill({ json: { library_version: 0, artifacts: [] } })
    } else {
      if (request.action === 'artifacts.import') imports++
      await route.fulfill({ json: { committed_library_version: 1 } })
    }
  })
  await page.goto(`${baseUrl}/depot/?kind=skill`, { waitUntil: 'networkidle' })
  await page.getByRole('heading', { name: 'Skill initial', exact: true }).click()
  await detailRequested
  await page.keyboard.press('Escape')
  const input = page.getByRole('textbox', { name: 'Search Labby artifacts' })
  await input.fill('python')
  releaseDetail()
  await page.getByRole('heading', { name: 'Skill python', exact: true }).waitFor()
  assert.equal(await page.getByRole('dialog').count(), 0)
  assert.equal(await page.getByRole('heading', { name: 'Skill initial', exact: true }).count(), 0)
  await page.getByRole('heading', { name: 'Skill python', exact: true }).click()
  await page.getByRole('button', { name: 'Add to Library', exact: true }).click()
  await importRequested
  await page.keyboard.press('Escape')
  await input.fill('frontend')
  const releasedResponse = page.waitForResponse(response => response.url().endsWith('/v1/artifacts') && response.request().postDataJSON().action === 'artifacts.list_connections')
  releaseImport()
  await releasedResponse
  await page.getByRole('heading', { name: 'Skill frontend', exact: true }).waitFor()
  assert.equal(imports, 0)
  assert.equal(await page.getByText('Exact Artifact imported into Labby', { exact: true }).count(), 0)
})

test('Overview pointer drag reorders both directions within lanes and persists after reload', { concurrency: false }, async (t) => {
  await startPreviewServer()
  const browser = await chromium.launch({ headless: true })
  t.after(() => browser.close())
  const page = await browser.newPage({ viewport: { width: 1512, height: 1800 } })
  await page.goto(baseUrl, { waitUntil: 'networkidle' })
  await page.locator('[data-overview-card="Call outcomes"]').waitFor()
  const order = (lane: string) => page.locator(`[data-overview-lane="${lane}"] > [data-overview-card]`).evaluateAll(elements => elements.map(element => element.getAttribute('data-overview-card')))
  const defaultTelemetry = ['Chart', 'Top Tools', 'Call outcomes', 'Least Used Tools']
  const defaultInsights = ['Most Active Agents', 'Most Active Servers', 'Connected clients', 'Gateway host', 'Recent servers']
  const assertCompact = async () => {
    const geometry = await page.locator('[data-overview-columns]').evaluate(columns => {
      const bounds = columns.getBoundingClientRect()
      const columnCenters = [1 / 6, 1 / 2, 5 / 6].map(ratio => bounds.left + bounds.width * ratio)
      const cards = [...columns.querySelectorAll<HTMLElement>('[data-overview-card]')].map(card => card.getBoundingClientRect())
      const stacks = columnCenters.map(center => {
        const stack = cards.filter(card => card.left <= center && card.right >= center).sort((a, b) => a.top - b.top)
        return { count: stack.length, gaps: stack.slice(1).map((card, index) => Math.round(card.top - stack[index].bottom)) }
      })
      return { counts: stacks.map(stack => stack.count), gaps: stacks.flatMap(stack => stack.gaps) }
    })
    assert.ok(geometry.counts.every(count => count >= 2), `Overview left a desktop column underfilled: ${geometry.counts.join(', ')}`)
    assert.ok(geometry.gaps.length > 0)
    assert.ok(geometry.gaps.every(gap => gap >= 0 && gap <= 14), `Overview retained interior gaps or overlaps: ${geometry.gaps.join(', ')}`)
  }
  const drag = async (source: string, target: string, edge: 'before' | 'after') => {
    const handle = page.locator(`[data-overview-card="${source}"]`)
    await handle.scrollIntoViewIfNeeded()
    const from = await handle.boundingBox()
    const to = await page.locator(`[data-overview-card="${target}"]`).boundingBox()
    assert.ok(from && to)
    // Grab the non-interactive card chrome; panel bodies can contain buttons and links that intentionally do not initiate a drag.
    await page.mouse.move(from.x + 8, from.y + 8)
    await page.mouse.down()
    await page.mouse.move(to.x + to.width / 2, to.y + to.height * (edge === 'after' ? 0.8 : 0.2), { steps: 15 })
    await page.locator(`[data-overview-card="${target}"] [data-overview-insertion="${edge}"]`).waitFor()
    await page.mouse.up()
  }
  assert.deepEqual(await order('telemetry'), defaultTelemetry)
  await assertCompact()
  await page.setViewportSize({ width: 1280, height: 1800 })
  await assertCompact()
  await page.setViewportSize({ width: 1512, height: 1800 })
  await assertCompact()
  await drag('Call outcomes', 'Top Tools', 'before')
  assert.deepEqual(await order('telemetry'), ['Chart', 'Call outcomes', 'Top Tools', 'Least Used Tools'])
  await drag('Call outcomes', 'Top Tools', 'after')
  assert.deepEqual(await order('telemetry'), defaultTelemetry)
  await drag('Call outcomes', 'Least Used Tools', 'after')
  assert.deepEqual(await order('telemetry'), ['Chart', 'Top Tools', 'Least Used Tools', 'Call outcomes'])
  await drag('Call outcomes', 'Least Used Tools', 'before')
  assert.deepEqual(await order('telemetry'), defaultTelemetry)
  assert.deepEqual(await order('insights'), defaultInsights)
  await assertCompact()
  assert.equal(await page.locator('[data-overview-card][draggable]').count(), 0)

  // Persist an in-lane move and width preference across a full reload.
  await drag('Call outcomes', 'Least Used Tools', 'after')
  await page.reload({ waitUntil: 'networkidle' })
  assert.equal((await order('telemetry')).at(-1), 'Call outcomes')
  assert.deepEqual(await order('insights'), defaultInsights)
  await assertCompact()
  const widthToggle = page.locator('[data-overview-card="Call outcomes"] button[aria-label="Toggle width"]')
  await widthToggle.click()
  await page.reload({ waitUntil: 'networkidle' })
  assert.equal(await page.locator('[data-overview-card="Call outcomes"] button[aria-label="Toggle width"]').getAttribute('aria-pressed'), 'true')
  assert.equal((await order('telemetry')).at(-1), 'Call outcomes')

  const beforeCancel = await order('telemetry')
  const handle = page.locator('[data-overview-card="Call outcomes"]')
  await handle.scrollIntoViewIfNeeded()
  const bounds = await handle.boundingBox()
  assert.ok(bounds)
  await page.mouse.move(bounds.x + 5, bounds.y + 5)
  await page.mouse.down()
  await page.mouse.move(bounds.x + 30, bounds.y + 40, { steps: 5 })
  await page.keyboard.press('Escape')
  await page.mouse.up()
  assert.deepEqual(await order('telemetry'), beforeCancel)
  assert.equal(await page.locator('[data-overview-insertion]').count(), 0)
})
