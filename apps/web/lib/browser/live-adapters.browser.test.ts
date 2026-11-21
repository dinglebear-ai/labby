import assert from 'node:assert/strict'
import { appendFileSync } from 'node:fs'
import test from 'node:test'

import { chromium, type Page } from 'playwright'

import {
  assertCanaryFree,
  ownedBrowserLaunchOptions,
  readLiveDescriptor,
  readPrivateCsrf,
  readScanSecrets,
  useBrowserWithAbort,
  withAbsoluteDeadline,
} from './live-backend-harness.ts'

type AdapterKind = 'cli' | 'openapi'
const manifest = process.env.LABBY_LIVE_BROWSER_ADAPTER_MANIFEST
const spec = process.env.LABBY_LIVE_BROWSER_ADAPTER_SPEC
const liveEnabled = Boolean(process.env.LABBY_LIVE_BROWSER_DESCRIPTOR)
const apiBase = 'https://adapter-canary.invalid/catalog'

async function serverIsVisible(page: Page, displayName: string) {
  await page.waitForFunction(
    name => document.body.innerText.split('\n').some(line => line.trim() === name),
    displayName,
    { timeout: 10_000 },
  )
}

async function registerThroughUi(page: Page, kind: AdapterKind, id: string, displayName: string, tool: string) {
  await page.getByRole('button', { name: 'Gateway actions, search and filters', exact: true }).click({ timeout: 10_000 })
  await page.getByRole('menuitem', { name: 'Add server', exact: true }).click({ timeout: 10_000 })
  const dialog = page.getByRole('dialog', { name: 'Add server' })
  await dialog.locator('label[for="transport-' + kind + '"]').click({ timeout: 10_000 })
  const add = dialog.getByRole('button', { name: 'Add server', exact: true })
  assert.equal(await add.isEnabled(), false)
  await dialog.getByLabel('Display name', { exact: true }).fill(displayName)
  await dialog.getByLabel('ID', { exact: true }).fill(id)
  if (kind === 'cli') {
    const manifestField = dialog.getByLabel('Manifest path on the gateway host', { exact: true })
    await manifestField.fill('relative.toml')
    assert.equal(await add.isEnabled(), false)
    await manifestField.fill(manifest!)
  } else {
    await dialog.getByLabel('Specification path on the gateway host', { exact: true }).fill(spec!)
    await dialog.getByLabel('API base URL', { exact: true }).fill(apiBase)
    const operations = dialog.getByLabel('Allowed operation IDs', { exact: true })
    await operations.fill(tool + ',' + tool)
    assert.equal(await add.isEnabled(), false)
    await operations.fill(tool)
    assert.equal(await dialog.getByRole('switch', { name: 'Allow write operations' }).getAttribute('aria-checked'), 'false')
  }
  assert.equal(await add.isEnabled(), true)

  const waitAction = (name: string) => page.waitForResponse(response => {
    if (response.request().method() !== 'POST' || new URL(response.url()).pathname !== '/v1/gateway') return false
    const body = response.request().postDataJSON()
    return body?.action === name && (body.params?.name === id || body.params?.spec?.name === id)
  }, { timeout: 15_000 })
  const [created, tested, discovered] = await Promise.all([
    waitAction('gateway.add'), waitAction('gateway.test'), waitAction('gateway.discovered_tools'), add.click(),
  ])
  assert.equal(created.status(), 200, 'guided Add Server must commit a normal upstream')
  assert.deepEqual(created.request().postDataJSON().params, {
    spec: { name: id, display_name: displayName },
    confirm: true,
    adapter: kind === 'cli'
      ? { kind, manifest_path: manifest }
      : { kind, spec_path: spec, base_url: apiBase, allowed_operations: [tool] },
  })
  assert.equal(tested.status(), 200)
  const runtime = await tested.json()
  assert.equal(runtime.name, id)
  assert.equal(runtime.connected, true)
  assert.equal(runtime.tool_count, 1)
  assert.equal(discovered.status(), 200)
  const tools = await discovered.json() as Array<{ name: string } | string>
  assert.deepEqual(tools.map(entry => typeof entry === 'string' ? entry : entry.name), [tool])
  await dialog.waitFor({ state: 'hidden', timeout: 10_000 })
  await page.getByText('Server added and connection verified', { exact: true }).last().waitFor({ state: 'visible', timeout: 10_000 })
  await serverIsVisible(page, displayName)

  // Carry the normal UI request's scope into Code Mode; never copy credential
  // or auth headers. The browser context still supplies its own session cookie.
  const scopeHeaders = Object.fromEntries(Object.entries(created.request().headers()).filter(
    ([name]) => name === 'x-labby-project-id' || name === 'x-labby-team-id',
  ))
  return async (service: 'gateway' | 'snippets', action: string, params: object) =>
    page.evaluate(async ({ service, action, params, scopeHeaders }) => {
      const response = await fetch('/v1/' + service, {
        method: 'POST',
        credentials: 'include',
        headers: { 'content-type': 'application/json', ...scopeHeaders },
        body: JSON.stringify({ action, params }),
      })
      return { status: response.status, body: await response.json() }
    }, { service, action, params, scopeHeaders })
}

async function exerciseCodeMode(
  action: Awaited<ReturnType<typeof registerThroughUi>>,
  id: string,
  tool: string,
  args: object,
  expected: RegExp,
  callSucceeded: boolean,
) {
  const qualifiedTool = id + '::' + tool
  const name = id + '-code-mode'
  const fence = String.fromCharCode(96).repeat(3)
  const body = [
    '---', 'name: ' + name, 'description: Disposable guided adapter Code Mode canary',
    'tools:', '  - ' + JSON.stringify(qualifiedTool), '---', fence + 'js',
    'async () => {',
    '  try { return await callTool(' + JSON.stringify(qualifiedTool) + ', ' + JSON.stringify(args) + '); }',
    '  catch (error) { return { error: String(error.message || error) }; }',
    '}', fence, '',
  ].join('\n')
  assert.equal((await action('snippets', 'snippets.create', { name, body })).status, 200)
  try {
    const executed = await action('snippets', 'snippets.exec', { name, params: {} })
    assert.equal(executed.status, 200, 'must reach the authenticated native Code Mode runtime')
    assert.equal(executed.body.receipt_status, 'persisted')
    assert.equal(typeof executed.body.execution_id, 'string')
    assert.deepEqual(executed.body.calls.map(({ id, ok }: { id: string; ok: boolean }) => ({ id, ok })), [
      { id: qualifiedTool, ok: callSucceeded },
    ])
    assert.match(JSON.stringify(executed.body.result), expected)
  } finally {
    assert.equal((await action('snippets', 'snippets.remove', { name })).status, 200)
  }
}

test('guided Add Server adapters in the real Gateway Admin', {
  skip: liveEnabled ? false : 'requires the Linux live browser supervisor and its two fixture files',
}, async t => {
  const descriptor = await readLiveDescriptor()
  assert.ok(descriptor)
  assert.ok(manifest && spec, 'the supervisor must provide both adapter fixtures')
  const progress = process.env.LABBY_LIVE_BROWSER_PROGRESS
  assert.ok(progress, 'the supervisor must provide the completion marker path')
  let csrf = await readPrivateCsrf(descriptor)
  await withAbsoluteDeadline(signal => useBrowserWithAbort(
    signal,
    async () => chromium.launch(await ownedBrowserLaunchOptions(chromium.executablePath())),
    async browser => {
      const context = await browser.newContext({
        baseURL: descriptor.base_url,
        storageState: descriptor.storage_state_path,
        viewport: { width: 1360, height: 900 },
        reducedMotion: 'reduce',
      })
      const page = await context.newPage()
      // Reuse the real UI's session refresh, without probing /auth/session
      // from a test request which would rotate its cookie and CSRF token.
      page.on('response', async response => {
        if (new URL(response.url()).pathname !== '/auth/session' || !response.ok()) return
        const session = await response.json().catch(() => null)
        if (typeof session?.csrf_token === 'string') csrf = session.csrf_token
      })
      await page.route('**/v1/**', async route => {
        const headers = route.request().headers()
        if (route.request().method() !== 'POST' || headers['x-csrf-token']) return route.continue()
        return route.continue({ headers: { ...headers, 'x-csrf-token': csrf } })
      })
      try {
        for (const canary of [
          {
            kind: 'cli', id: 'guided-cli-browser', label: 'Browser CLI fixture', tool: 'fixture_greeting',
            command: '/labby', args: { choice: 'world' }, expected: /Labby CLI browser canary/,
          },
          {
            // The string is valid by MCP schema but invalid as a path segment.
            // This proves the real adapter was invoked while deterministically
            // stopping before DNS or network. Dispatch round trip is exercised
            // by labby-openapi's happy_path_calls_allowed_operation wiremock test.
            kind: 'openapi', id: 'guided-openapi-browser', label: 'Browser OpenAPI fixture', tool: 'getFixtureItem',
            command: '/labby-openapi-mcp', args: { itemId: '' },
            expected: /operation .getFixtureItem. path parameter .itemId. is missing or invalid/,
          },
        ] as const) {
          await t.test(canary.kind + ' saves, discovers, and invokes its real MCP adapter', async () => {
            await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
            const action = await registerThroughUi(page, canary.kind, canary.id, canary.label, canary.tool)
            const persisted = await action('gateway', 'gateway.get', { name: canary.id })
            assert.equal(persisted.status, 200)
            assert.equal(persisted.body.config.name, canary.id)
            assert.equal(persisted.body.config.url ?? null, null)
            assert.ok(persisted.body.config.command?.endsWith(canary.command))
            await exerciseCodeMode(action, canary.id, canary.tool, canary.args, canary.expected, canary.kind === 'cli')
            await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
            await serverIsVisible(page, canary.label)
            appendFileSync(progress, 'guided-' + canary.kind + '-completed\n', { mode: 0o600 })
          })
        }
        assertCanaryFree(await page.locator('body').innerText(), await readScanSecrets(descriptor, signal), 'adapter canary DOM')
      } finally {
        await context.close()
      }
    },
  ), 'guided Add Server canaries')
})
