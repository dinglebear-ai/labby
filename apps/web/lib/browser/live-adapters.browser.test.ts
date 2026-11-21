import assert from 'node:assert/strict'
import { appendFileSync } from 'node:fs'
import { access, writeFile } from 'node:fs/promises'
import { setTimeout } from 'node:timers/promises'
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

async function registerThroughUi(
  page: Page,
  kind: AdapterKind,
  id: string,
  displayName: string,
  tool: string,
  readHarnessCsrf: () => string,
) {
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
  // A plausible but nonexistent host file passes local form validation.
  // The real server must refuse it without closing the dialog or probing.
  const sourceField = dialog.getByLabel(kind === 'cli'
    ? 'Manifest path on the gateway host'
    : 'Specification path on the gateway host', { exact: true })
  const source = kind === 'cli' ? manifest! : spec!
  const missingSource = source.replace(/(\.[^.]+)$/, '-missing$1')
  let prematureTests = 0
  const observeProbe = (request: import('playwright').Request) => {
    if (request.method() !== 'POST' || new URL(request.url()).pathname !== '/v1/gateway') return
    const body = request.postDataJSON()
    if (body?.action === 'gateway.test' && body.params?.name === id) prematureTests++
  }
  page.on('request', observeProbe)
  try {
    await sourceField.fill(missingSource)
    const [denied] = await Promise.all([waitAction('gateway.add'), add.click()])
    assert.equal(denied.status(), 422, 'the server must reject the missing local source')
    const rejection = await denied.json()
    assert.equal(rejection.kind, 'invalid_param')
    await dialog.getByRole('alert').waitFor({ state: 'visible', timeout: 10_000 })
    assert.equal(await sourceField.inputValue(), missingSource)
    assert.equal(prematureTests, 0)
  } finally {
    page.off('request', observeProbe)
  }

  await sourceField.fill(source)
  // Capture followups now, but report a rejected add immediately instead of
  // masking the first failure behind a discovery timeout.
  const followups = Promise.all([waitAction('gateway.test'), waitAction('gateway.discovered_tools')])
    .then(([tested, discovered]) => ({ tested, discovered }), error => ({ error }))
  const [created] = await Promise.all([waitAction('gateway.add'), add.click()])
  assert.ok(await created.request().headerValue('x-csrf-token'), 'the real Add Server UI must attach its session CSRF header')
  if (created.status() !== 200) {
    const body = await created.json().catch(() => ({}))
    assert.fail('guided ' + kind + ' Add Server rejected valid input (HTTP ' + created.status()
      + '): ' + (body.kind ?? 'unknown') + ' ' + (body.message ?? ''))
  }
  const followup = await followups
  if ('error' in followup) throw followup.error
  const { tested, discovered } = followup
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
  assert.equal(runtime.connected, true, kind + ' MCP connection failed: '
    + (typeof runtime.last_error === 'string' ? runtime.last_error : 'no runtime diagnostic'))
  assert.equal(runtime.server_name, kind === 'cli' ? 'labby-cli' : 'labby-openapi-mcp')
  assert.equal(runtime.tool_count, 1)
  assert.equal(discovered.status(), 200)
  const tools = await discovered.json() as Array<{ name: string } | string>
  assert.deepEqual(tools.map(entry => typeof entry === 'string' ? entry : entry.name), [tool])
  await dialog.waitFor({ state: 'hidden', timeout: 10_000 })
  await page.getByText('Server added and connection verified', { exact: true }).last().waitFor({ state: 'visible', timeout: 10_000 })
  await serverIsVisible(page, displayName)

  // These explicit harness calls do not use the UI action client; pass the
  // current fixture CSRF token here only. Real UI requests stay untouched.
  const scopeHeaders = Object.fromEntries(Object.entries(created.request().headers()).filter(
    ([name]) => name === 'x-labby-project-id' || name === 'x-labby-team-id',
  ))
  return async (service: 'gateway' | 'snippets', action: string, params: object) =>
    page.evaluate(async ({ service, action, params, scopeHeaders, csrf }) => {
      const response = await fetch('/v1/' + service, {
        method: 'POST',
        credentials: 'include',
        headers: { 'content-type': 'application/json', ...scopeHeaders, 'x-csrf-token': csrf },
        body: JSON.stringify({ action, params }),
      })
      return { status: response.status, body: await response.json() }
    }, { service, action, params, scopeHeaders, csrf: readHarnessCsrf() })
}

type BrowserAction = Awaited<ReturnType<typeof registerThroughUi>>

async function assertPersistedSchema(
  action: BrowserAction,
  kind: AdapterKind,
  id: string,
  tool: string,
) {
  const persisted = await action('gateway', 'gateway.get', { name: id })
  assert.equal(persisted.status, 200)
  assert.equal(persisted.body.config.name, id)
  assert.equal(persisted.body.config.url ?? null, null)
  if (kind === 'cli') {
    assert.equal(persisted.body.config.command, 'labby::guided-cli')
  } else {
    assert.ok(persisted.body.config.command?.endsWith('/labby-openapi-mcp'))
  }
  const catalog = await action('gateway', 'gateway.schema', { name: id })
  assert.equal(catalog.status, 200)
  assert.equal(catalog.body.name, id)
  assert.deepEqual(catalog.body.tools.map(({ name }: { name: string }) => name), [tool])
  const input = catalog.body.tools[0].input_schema
  const parameter = kind === 'cli' ? 'choice' : 'itemId'
  assert.equal(input.type, 'object')
  assert.deepEqual(input.required, [parameter])
  assert.equal(input.properties[parameter].type, 'string')
  if (kind === 'cli') assert.deepEqual(input.properties.choice.enum, ['world'])
}

async function exerciseCodeMode(
  action: BrowserAction,
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
    '  const search = await codemode.search({ query: ' + JSON.stringify(tool) + ', kinds: ["tool"] });',
    '  const hit = search.results.find(hit => hit.id === ' + JSON.stringify(qualifiedTool) + ');',
    '  if (!hit) throw new Error("new upstream missing from Code Mode search");',
    '  try { return { found: hit.id, output: await callTool(hit.id, ' + JSON.stringify(args) + ') }; }',
    '  catch (error) { return { found: hit.id, error: String(error.message || error) }; }',
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
    assert.equal(executed.body.result.found, qualifiedTool)
    assert.match(callSucceeded ? JSON.stringify(executed.body.result.output) : executed.body.result.error, expected)
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
      try {
        const registered: Array<{ canary: (typeof canaries)[number]; action: BrowserAction }> = []
        const canaries = [
          {
            kind: 'cli', id: 'guided-cli-browser', label: 'Browser CLI fixture', tool: 'fixture_greeting',
            args: { choice: 'world' }, expected: /Labby CLI browser canary/,
          },
          {
            // The string is valid by MCP schema but invalid as a path segment.
            // This proves the real adapter was invoked while deterministically
            // stopping before DNS or network. Dispatch round trip is exercised
            // by labby-openapi's happy_path_calls_allowed_operation wiremock test.
            kind: 'openapi', id: 'guided-openapi-browser', label: 'Browser OpenAPI fixture', tool: 'getFixtureItem',
            args: { itemId: '' },
            expected: /operation .getFixtureItem. path parameter .itemId. is missing or invalid/,
          },
        ] as const
        for (const canary of canaries) {
          await t.test(canary.kind + ' saves, discovers, and invokes its real MCP adapter', async () => {
            await page.goto('/gateways/', { waitUntil: 'domcontentloaded', timeout: 15_000 })
            const action = await registerThroughUi(page, canary.kind, canary.id, canary.label, canary.tool, () => csrf)
            await assertPersistedSchema(action, canary.kind, canary.id, canary.tool)
            await exerciseCodeMode(action, canary.id, canary.tool, canary.args, canary.expected, canary.kind === 'cli')
            registered.push({ canary, action })
          })
        }
        assert.equal(registered.length, canaries.length, 'both registrations must finish before restart')
        // The Rust supervisor owns the disposable process and exact restart.
        await writeFile(descriptor.restart_request_path, 'restart\n', { mode: 0o600, flag: 'wx' })
        const deadline = Date.now() + 30_000
        while (!await access(descriptor.restart_complete_path).then(() => true, () => false)) {
          assert.ok(Date.now() < deadline, 'disposable gateway did not complete its restart')
          await setTimeout(100, undefined, { signal })
        }
        await page.reload({ waitUntil: 'domcontentloaded', timeout: 15_000 })
        for (const { canary, action } of registered) {
          await serverIsVisible(page, canary.label)
          await assertPersistedSchema(action, canary.kind, canary.id, canary.tool)
          await exerciseCodeMode(action, canary.id, canary.tool, canary.args, canary.expected, canary.kind === 'cli')
          appendFileSync(progress, 'guided-' + canary.kind + '-completed\n', { mode: 0o600 })
        }
        assertCanaryFree(await page.locator('body').innerText(), await readScanSecrets(descriptor, signal), 'adapter canary DOM')
      } finally {
        await context.close()
      }
    },
  ), 'guided Add Server canaries')
})
