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
    page.getByRole('button', { name: 'Copy command' }).and(
      page.locator('[title="http://localhost:3001/mcp"]'),
    ).waitFor(),
  )
  await page.getByRole('tab', { name: /Catalog/ }).click()
  await assert.doesNotReject(() =>
    page.getByRole('button', { name: 'Manage tools', exact: true }).waitFor(),
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
  const paletteTrigger = page.getByRole('button', { name: 'Search and filter' })
  const paletteBox = await paletteTrigger.boundingBox()
  assert.ok(paletteBox && paletteBox.width >= 220, `expected full palette trigger, got ${paletteBox?.width ?? 0}px`)
  await assert.doesNotReject(() => paletteTrigger.getByText(/Search —/).waitFor())

  await page.getByRole('button', { name: 'Account menu' }).click()
  await assert.doesNotReject(() => page.getByRole('link', { name: 'Settings', exact: true }).waitFor())
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
  assert.match(await page.locator('body').innerText(), /github-server[\s\S]*12/)

  const hasHorizontalOverflow = await page.evaluate(() => {
    const root = document.documentElement
    return root.scrollWidth > root.clientWidth
  })

  assert.equal(hasHorizontalOverflow, false)
})

test('overview metrics and volume bars drill into exact Usage slices', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1360, height: 960 } })
  await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle' })

  const bars = page.locator('.recharts-bar-rectangle .recharts-rectangle')
  await page.waitForFunction(() =>
    Array.from(document.querySelectorAll('.recharts-bar-rectangle .recharts-rectangle')).some((node) => {
      const box = node.getBoundingClientRect()
      return box.width > 1 && box.height > 1
    }),
  )
  let clicked = false
  for (let index = 0; index < await bars.count(); index += 1) {
    const bar = bars.nth(index)
    const box = await bar.boundingBox()
    if (box && box.width > 1 && box.height > 1) {
      await bar.click()
      clicked = true
      break
    }
  }
  assert.equal(clicked, true, 'expected at least one clickable volume bar')
  await page.waitForURL((url) => url.pathname === '/usage/' && url.searchParams.has('from') && url.searchParams.has('to'))
  const sliceUrl = new URL(page.url())
  const from = Number(sliceUrl.searchParams.get('from'))
  const to = Number(sliceUrl.searchParams.get('to'))
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

  const githubRow = page.locator('[data-gwrow="1"]').filter({ hasText: 'github-server' }).first()
  await githubRow.getByRole('link', { name: 'github-server', exact: true }).click()
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

test('icon-led actions are square, touch-sized, and retain accessible labels', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
  await page.route('**/v1/depot/**', async () => new Promise(() => undefined))
  await page.goto(`${baseUrl}/library/`, { waitUntil: 'domcontentloaded' })
  const loadingRefresh = page.getByRole('button', { name: 'Refresh', exact: true })
  await assert.doesNotReject(() => loadingRefresh.waitFor())
  assert.equal(await loadingRefresh.evaluate((element) => getComputedStyle(element).fontSize), '0px')
  await page.unroute('**/v1/depot/**')
  await page.goto(`${baseUrl}/library/`, { waitUntil: 'networkidle' })

  const discover = page.getByRole('link', { name: 'Discover', exact: true })
  const discoverStyle = await discover.evaluate((element) => ({
    fontSize: getComputedStyle(element).fontSize,
    title: element.getAttribute('title'),
    width: element.getBoundingClientRect().width,
    height: element.getBoundingClientRect().height,
  }))
  assert.equal(discoverStyle.fontSize, '0px')
  assert.equal(discoverStyle.title, 'Discover')
  assert.ok(discoverStyle.width >= 44 && discoverStyle.height >= 44)

  const filter = page.getByRole('button', { name: 'Filter library by artifact type' })
  assert.equal(await filter.getAttribute('title'), 'Filters')
  assert.equal(await filter.evaluate((element) => getComputedStyle(element).fontSize), '0px')

  const textOnly = page.getByRole('link', { name: 'Artifacts', exact: true })
  assert.notEqual(await textOnly.evaluate((element) => getComputedStyle(element).fontSize), '0px')

  await page.goto(`${baseUrl}/create/`, { waitUntil: 'networkidle' })
  const artifactTypeMenu = page.getByRole('button', { name: 'Skill', exact: true })
  await assert.doesNotReject(() => artifactTypeMenu.waitFor())
  assert.equal(await artifactTypeMenu.getAttribute('data-slot'), 'dropdown-menu-trigger')
  const exportStyle = await artifactTypeMenu.evaluate((element) => ({
    fontSize: getComputedStyle(element).fontSize,
    width: element.getBoundingClientRect().width,
    height: element.getBoundingClientRect().height,
    label: element.getAttribute('aria-label'),
  }))
  assert.equal(exportStyle.fontSize, '0px')
  assert.ok(exportStyle.width >= 44 && exportStyle.height >= 44)
  assert.equal(exportStyle.label, 'Skill')
  await artifactTypeMenu.click()
  await assert.doesNotReject(() => page.getByRole('menu').waitFor())
})

test('Library follows responsive view defaults until the operator chooses a view', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  const page = await browser.newPage({ viewport: { width: 1000, height: 800 } })
  await page.goto(`${baseUrl}/library/`, { waitUntil: 'networkidle' })
  await assert.doesNotReject(() => page.locator('table').waitFor())

  await page.setViewportSize({ width: 390, height: 844 })
  await assert.doesNotReject(() => page.locator('table').waitFor({ state: 'detached' }))
  await page.setViewportSize({ width: 1000, height: 800 })
  await assert.doesNotReject(() => page.locator('table').waitFor())

  await page.getByRole('button', { name: 'Cards view' }).click()
  await assert.doesNotReject(() => page.locator('table').waitFor({ state: 'detached' }))
  await page.setViewportSize({ width: 390, height: 844 })
  await page.setViewportSize({ width: 1000, height: 800 })
  assert.equal(await page.locator('table').count(), 0)
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
      '/stash/',
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
    assert.equal(await page.locator('aside[data-console-sidebar]').getAttribute('aria-hidden'), 'true')
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

  const githubRow = page.locator('[data-gwrow="1"]').filter({ has: page.getByText('github-server') }).first()
  const disableButton = githubRow.getByRole('button', { name: 'Disable server' })
  await assert.doesNotReject(() => disableButton.waitFor())

  await disableButton.click()
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
  await page.route('**/v1/browser', async (route) => {
    const body = route.request().postDataJSON() as { action: string; params: Record<string, unknown> }
    if (body.action === 'browser.pairing.approve') approved = true
    if (body.action === 'browser.session.enable') enabled = body.params.enabled === true
    const response = body.action === 'browser.list' ? { browsers: approved ? [browserRow] : [] }
      : body.action === 'browser.pairing.list' ? { pairings: approved ? [] : [{ id: 'pair-1', display_name: 'Work Chrome', extension_id: 'a'.repeat(32), status: 'pending', expires_at: 1_887_976_000, browser_id: null }] }
        : body.action === 'browser.sessions' ? { sessions: approved ? [{ id: 'session-1', browser_id: 'browser-1', tab_id: 7, document_id: 'doc-1', origin: 'https://example.com', sanitized_path: '/tools', page_title: 'Example tools', catalog_revision: 42, catalog_fingerprint: 'hash', tools: [{ name: 'search', description: 'Search the example catalog', input_schema: { type: 'object' }, annotations: {} }], enabled, status: 'active', last_seen_at: 1_787_976_060 }] : [] }
          : body.action === 'browser.pairing.approve' ? browserRow
            : body.action === 'browser.session.enable' ? { enabled }
              : {}
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(response) })
  })

  await page.goto(`${baseUrl}/browsers/`, { waitUntil: 'networkidle' })
  await page.getByRole('button', { name: 'Approve' }).click()
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

  await buildApplication('browser-skew-replacement')
  await page.unroute('**/*', blockFlightPrefetch)
  await Promise.all([
    page.waitForURL('**/snippets/', { waitUntil: 'networkidle' }),
    page.getByRole('link', { name: 'Snippets' }).click(),
  ])

  const staleDocumentSurvived = await page.evaluate(
    () => '__labbySkewMarker' in window,
  )
  assert.equal(staleDocumentSurvived, false, 'build skew must replace the stale document')
})
