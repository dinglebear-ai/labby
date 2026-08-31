import assert from 'node:assert/strict'
import { appendFileSync } from 'node:fs'
import test from 'node:test'

import { chromium, request as playwrightRequest, type Page } from 'playwright'

import {
  assertCanaryFree,
  captureFailureEvidence,
  observeLivePage,
  readPrivateCsrf,
  readLiveDescriptor,
  withAbsoluteDeadline,
} from './live-backend-harness.ts'

const liveEnabled = Boolean(process.env.LABBY_LIVE_BROWSER_DESCRIPTOR)
const nightlyEnabled = process.env.LABBY_LIVE_BROWSER_NIGHTLY === 'true'
const progressPath = process.env.LABBY_LIVE_BROWSER_PROGRESS
let progressBytes = 0
function progress(message: string) {
  const rendered = `${message}\n`
  progressBytes += Buffer.byteLength(rendered)
  if (progressPath && progressBytes <= 16 * 1024) appendFileSync(progressPath, rendered, { mode: 0o600 })
}

async function action(page: Page, csrfToken: string, service: string, name: string, params: object) {
  return page.evaluate(async ({ csrfToken, service, name, params }) => {
    const session = await fetch('/auth/session', { credentials: 'include', cache: 'no-store' })
    const sessionBody = await session.json()
    if (!session.ok || !sessionBody.authenticated) return { status: session.status, body: sessionBody }
    const response = await fetch(`/v1/${service}`, {
      method: 'POST', credentials: 'include', cache: 'no-store',
      headers: { 'content-type': 'application/json', 'x-csrf-token': csrfToken },
      body: JSON.stringify({ action: name, params }),
    })
    return {
      status: response.status,
      body: await response.json(),
      sessionAuthenticated: sessionBody.authenticated === true,
      csrfLength: typeof sessionBody.csrf_token === 'string' ? sessionBody.csrf_token.length : 0,
    }
  }, { csrfToken, service, name, params })
}

test('embedded Gateway Admin completes a real backend journey', {
  concurrency: false,
  skip: liveEnabled ? false : 'outer supervisor did not supply LABBY_LIVE_BROWSER_DESCRIPTOR',
}, async () => {
  progress('test-start')
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  const csrfToken = await readPrivateCsrf(descriptor)
  progress('descriptor-read')
  await withAbsoluteDeadline((async () => {
    progress('chromium-launch-start')
    const browser = await chromium.launch({ headless: true })
    progress('chromium-launched')
    let context: import('playwright').BrowserContext | undefined
    try {
      context = await browser.newContext({
        baseURL: descriptor.base_url,
        storageState: descriptor.storage_state_path,
        viewport: { width: 1360, height: 900 },
      })
    } catch (error) {
      await browser.close()
      throw error
    }
    await context.tracing.start({ screenshots: false, snapshots: false, sources: false })
    const page = await context.newPage()
    const evidence = observeLivePage(page, descriptor.base_url)
    let failure: unknown
    const ownedName = `browser-${descriptor.run_id.toLowerCase()}`
    const ownedRoute = `${ownedName}-route`
    const cleanupFailures: string[] = []
    try {
      progress('health-session-catalog')
      for (const route of ['/health', '/auth/session', '/v1/catalog']) {
        const response = await page.request.get(route)
        progress(`${route}:${response.status()}`)
        assert.ok(response.ok(), `${route} returned ${response.status()}`)
      }
      const anonymous = await playwrightRequest.newContext({ baseURL: descriptor.base_url })
      const denied = await anonymous.post('/v1/gateway', {
        data: { action: 'gateway.remove', params: { name: 'browser-denied' } },
      })
      await anonymous.dispose()
      assert.ok([401, 403].includes(denied.status()), `unauthorized mutation returned ${denied.status()}`)
      progress(`anonymous-denial:${denied.status()}`)

      await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
      progress('embedded-ui-loaded')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
      progress(`page-errors:${evidence.pageErrors.length}:csp:${evidence.cspViolations.length}`)
      assert.equal(evidence.pageErrors.length, 0)
      assert.equal(evidence.cspViolations.length, 0)

      const added = await action(page, csrfToken, 'gateway', 'gateway.add', {
        spec: { name: ownedName, url: 'http://127.0.0.1:9/mcp' },
      })
      progress(`gateway-add:${added.status}:auth=${added.sessionAuthenticated}:csrf=${added.csrfLength}`)
      assert.equal(added.status, 200, JSON.stringify(added.body))
      progress('gateway-added')
      await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
      progress('page-reloaded')
      const persisted = await action(page, csrfToken, 'gateway', 'gateway.get', { name: ownedName })
      progress(`gateway-get:${persisted.status}`)
      assert.equal(persisted.status, 200)

      const staged = await action(page, csrfToken, 'gateway', 'gateway.protected_route.stage_add', {
        route: {
          name: ownedRoute, enabled: true, public_host: 'browser.invalid',
          public_path: '/mcp', upstream: null, backend_url: '', scopes: [],
          target: { kind: 'gateway_subset', loadout: 'production' },
        },
      })
      progress(`protected-route-stage:${staged.status}:${JSON.stringify(staged.body)}`)
      assert.equal(staged.status, 200, JSON.stringify(staged.body))
      assert.equal(staged.body.restart_required, true)
      progress('protected-route-staged')
      assert.match(JSON.stringify(staged.body), /restart/i)

      // A real rapid duplicate reaches backend serialization; the UI must not
      // turn it into two successful state transitions.
      const duplicate = await Promise.all([
        action(page, csrfToken, 'gateway', 'gateway.add', { spec: { name: `${ownedName}-duplicate`, url: 'http://127.0.0.1:9/mcp' } }),
        action(page, csrfToken, 'gateway', 'gateway.add', { spec: { name: `${ownedName}-duplicate`, url: 'http://127.0.0.1:9/mcp' } }),
      ])
      assert.equal(duplicate.filter((result) => result.status === 200).length, 1)
      assert.ok(duplicate.some((result) => result.status === 409 || result.status >= 400))
      progress('duplicate-serialized')
      progress(`requests:count=${evidence.requests.length}:failures=${evidence.requests.filter((request) => (request.status ?? 0) >= 400).length}`)

      assert.ok(evidence.requests.some((request) => request.path === '/auth/session'))
      assert.ok(evidence.requests.some((request) => request.path === '/v1/catalog'))
      assert.ok(evidence.requests.some((request) => request.path === '/v1/gateway' && request.method === 'POST'))
      const scanSecrets = (await import('node:fs/promises')).readFile(descriptor.scan_secrets_path, 'utf8')
        .then((value) => value.split('\n').filter(Boolean))
      assertCanaryFree(await page.locator('body').innerText(), await scanSecrets, 'DOM')
      assertCanaryFree(evidence, await scanSecrets, 'browser evidence')
      progress('evidence-asserted')
      await context.tracing.stop()
      progress('trace-stopped')
    } catch (error) {
      failure = error
      await captureFailureEvidence({ browser, context, page, descriptor, evidence, error })
      throw error
    } finally {
      for (const [name, operation] of [
        [ownedRoute, 'gateway.protected_route.stage_remove'],
        [`${ownedName}-duplicate`, 'gateway.remove'],
        [ownedName, 'gateway.remove'],
      ] as const) {
        const result = await action(page, csrfToken, 'gateway', operation, { name }).catch((error) => ({ status: 0, body: String(error) }))
        progress(`cleanup:${operation}:${result.status}`)
        if (![200, 404].includes(result.status) && operation === 'gateway.remove') {
          const absent = await action(page, csrfToken, 'gateway', 'gateway.get', { name })
          progress(`cleanup-observe:${name}:${absent.status}`)
          if (absent.status === 404) continue
          if (result.status >= 500 && absent.status >= 500) {
            // The outer Rust supervisor owns and verifies deletion of the
            // complete disposable installation after this real 5xx path.
            progress(`cleanup-deferred-to-owned-root:${name}`)
            continue
          }
        }
        if (![200, 404].includes(result.status)) cleanupFailures.push(`${operation}(${name})=${result.status}`)
      }
      if (!failure) await context.close()
      else await context.close().catch(() => undefined)
      await browser.close()
      if (!failure) assert.deepEqual(cleanupFailures, [], `live browser cleanup failed: ${cleanupFailures.join(', ')}`)
    }
  })(), 'live Gateway Admin journey')
})

test('nightly mobile viewport has no overflow and essential landmarks', {
  concurrency: false,
  skip: liveEnabled && nightlyEnabled ? false : 'nightly live browser coverage is disabled',
}, async () => {
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  const browser = await chromium.launch({ headless: true })
  try {
    const context = await browser.newContext({
      baseURL: descriptor.base_url, storageState: descriptor.storage_state_path,
      viewport: { width: 390, height: 844 },
    })
    const page = await context.newPage()
    await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth), false)
    assert.ok(await page.getByRole('main').count())
    assert.ok(await page.getByRole('navigation').count())
    await context.close()
  } finally {
    await browser.close()
  }
})
