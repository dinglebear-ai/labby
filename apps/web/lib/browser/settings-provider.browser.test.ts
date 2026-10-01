import test from 'node:test'
import assert from 'node:assert/strict'
import http from 'node:http'
import { once } from 'node:events'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { readFile } from 'node:fs/promises'
import { chromium } from 'playwright'

test('built provider settings validate edits and fit desktop/mobile with mocked APIs', { timeout: 120000 }, async t => {
  // Other browser fixtures build with static mock data. This suite exercises
  // real API adapters against intercepted responses and needs its own export.
  await promisify(execFile)('pnpm', ['run', 'build'], {
    cwd: new URL('../../', import.meta.url),
    env: { ...process.env, NEXT_PUBLIC_MOCK_DATA: 'false', NEXT_PUBLIC_API_TOKEN: '' },
    timeout: 90_000,
    maxBuffer: 4 * 1024 * 1024,
  })
  const root = new URL('../../out/', import.meta.url)
  const server = http.createServer(async (request, response) => {
    const path = new URL(request.url!, 'http://localhost').pathname
    try { const bytes = await readFile(new URL('.' + path + (path.endsWith('/') ? 'index.html' : ''), root)); response.setHeader('Content-Type', path.endsWith('.js') ? 'application/javascript' : path.endsWith('.css') ? 'text/css' : 'text/html'); response.end(bytes) } catch { response.writeHead(404).end() }
  })
  server.listen(0, '127.0.0.1'); await once(server, 'listening'); t.after(() => server.close())
  const address = server.address(); assert.ok(address && typeof address !== 'string')
  const browser = await chromium.launch({ headless: true }); t.after(() => browser.close())
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 900 } })
    page.setDefaultTimeout(10000)
    let writes = 0
    const values: Record<string, unknown> = { LABBY_AGENT_PROVIDER_PROTOCOL: null, LABBY_PHOENIX_OPENAI_BASE_URL: null }
    const state = () => ({ schema_version: 1, config_path: '/fixture/config.toml', env_path: '/fixture/.env', section: 'agents', values, sources: Object.fromEntries(Object.keys(values).map(key => [key, { source: 'default', overridden_by_env: null }])) })
    const field = (key: string, label: string, control: string, description: string, options: unknown[] = []) => ({ key, label, control, description, options, section: 'agents', backend: 'env', risk: 'security_sensitive', write_policy: 'editable', apply_mode: 'partial', secret: false, required: false, env_override: null, min: null, max: null, example: null })
    await page.route('**/auth/session', route => route.fulfill({ json: { authenticated: true, user: { sub: 'fixture-owner' }, expires_at: Date.now() + 3600000, csrf_token: 'fixture-csrf', authority_state: 'ready', principal_id: 'fixture-owner', organization_id: 'fixture-installation', authority_generation: 1, active_owner: { kind: 'personal', id: 'fixture-owner' }, teams: [], projects: [], capabilities: ['platform.manage'], is_configured_admin: true } }))
    await page.route('**/v1/setup**', route => {
      const input = route.request().postDataJSON()
      if (input.action === 'settings.schema') return route.fulfill({ json: { schema_version: 1, sections: [{ id: 'agents', label: 'Agent provider', description: 'Connect your provider', advanced: false }], fields: [field('LABBY_AGENT_PROVIDER_PROTOCOL', 'Agent provider protocol', 'enum', 'Choose OpenAI for standard APIs; Phoenix requires session extensions.', [{ value: 'openai', label: 'OpenAI-compatible API' }, { value: 'phoenix', label: 'Phoenix session extension' }]), field('LABBY_PHOENIX_OPENAI_BASE_URL', 'Agent provider URL', 'url', 'Address reached from the Labby server, not this browser.')] } })
      if (input.action === 'settings.env.update') { writes++; for (const entry of input.params.entries) values[entry.key] = entry.value }
      return route.fulfill({ json: state() })
    })
    await page.route('**/v1/agents/**', route => route.fulfill({ json: { models: ['fixture-model'] } }))
    await page.goto(`http://127.0.0.1:${address.port}/settings/agents/`, { waitUntil: 'networkidle' })
    const url = page.getByRole('textbox', { name: 'Agent provider URL', exact: true }); await url.waitFor()
    assert.ok(await page.getByText('Address reached from the Labby server, not this browser.').isVisible())
    await page.getByRole('combobox', { name: 'Agent provider protocol', exact: true }).click(); assert.equal(await page.getByRole('option').count(), 2)
    await page.getByRole('option', { name: 'OpenAI-compatible API', exact: true }).click()
    await url.fill('file:///invalid'); await page.getByRole('checkbox', { name: 'Confirm settings write' }).check(); await page.getByRole('button', { name: 'Save changes', exact: true }).click()
    await page.getByText('Enter an HTTP or HTTPS URL without credentials, query, or fragment.').waitFor(); assert.equal(writes, 0); assert.equal(await url.inputValue(), 'file:///invalid')
    await url.fill('https://provider.example/v1'); assert.equal(writes, 0)
    await page.getByRole('checkbox', { name: 'Confirm settings write' }).check()
    const saved = page.waitForResponse(response => response.url().includes('/v1/setup') && response.request().postData()?.includes('settings.env.update') === true)
    await page.getByRole('button', { name: 'Save changes', exact: true }).click(); await saved
    assert.equal(writes, 1)
    await page.getByRole('button', { name: 'Check models', exact: true }).click(); await page.getByText('listed 1 available model', { exact: false }).waitFor()
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), `settings overflow at ${width}px`)
    await page.close()
  }
})
