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
  await page.getByRole('button', { name: 'Tools', exact: true }).click()
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
  await page.getByRole('button', { name: 'Tools', exact: true }).click()
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
  await page.getByRole('button', { name: 'Tools', exact: true }).click()
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
  await assert.doesNotReject(() => page.getByRole('tab', { name: /Overview/ }).waitFor())
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
  await assert.doesNotReject(() => page.getByRole('tab', { name: /Overview/ }).waitFor())

  const detailOverflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth)
  assert.equal(detailOverflow, false)
})

test('overview, gateways, detail, and usage stay overflow-free on phone and tablet', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => { await browser.close() })

  for (const viewport of [
    { width: 390, height: 844, label: 'phone' },
    { width: 768, height: 1024, label: 'tablet' },
  ]) {
    const page = await browser.newPage({ viewport: { width: viewport.width, height: viewport.height } })
    for (const route of [
      '/',
      '/gateways/',
      '/gateway/?id=gw-2',
      '/usage/?focus=latency&percentile=p95&outcome=failed',
    ]) {
      await page.goto(`${baseUrl}${route}`, { waitUntil: 'networkidle' })
      const overflow = await page.evaluate(() => ({
        document: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        body: document.body.scrollWidth - document.body.clientWidth,
      }))
      assert.ok(overflow.document <= 1 && overflow.body <= 1, `${viewport.label} ${route} overflowed horizontally: ${JSON.stringify(overflow)}`)
    }
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

  await page.getByRole('tab', { name: /Routes/ }).click()
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
    page.waitForURL((url) => url.pathname.replace(/\/$/, '') === '/logs', {
      waitUntil: 'domcontentloaded',
    }),
    page.getByRole('link', { name: /Logs/ }).click(),
  ])

  const staleDocumentSurvived = await page.evaluate(
    () => '__labbySkewMarker' in window,
  )
  assert.equal(staleDocumentSurvived, false, 'build skew must replace the stale document')
})
