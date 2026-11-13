import {test, expect} from '@playwright/test'
import {readFileSync} from 'node:fs'
import {resolve} from 'node:path'

const api = 'http://127.0.0.1:4299'
const protectedPages = ['/gateway', '/clients', '/runtime', '/usage', '/activity', '/logs', '/diagnostics']

// These tests exercise the production HTTP auth middleware and LiveView session
// vault. No page.route interception, identity injection, or fixture endpoints.
test('anonymous callers cannot open protected pages or dispatch gateway reads', async ({page, request}) => {
  for (const path of protectedPages) {
    await page.goto(path)
    await expect(page).toHaveURL(/\/sign-in$/)
    await expect(page.locator('#identity, #server-list, #usage-summary')).toHaveCount(0)
  }
  const denied = await request.post(`${api}/v1/gateway`, {
    data: {action: 'gateway.list', params: {}}, timeout: 10000,
  })
  expect(denied.status()).toBe(401)
})

test('an invalid credential never establishes a console session', async ({page}) => {
  await page.goto('/sign-in')
  const credential = page.getByLabel('Gateway credential', {exact: true})
  await credential.focus()
  await credential.press('Enter')
  expect(await credential.evaluate(input => input.validity.valueMissing)).toBe(true)
  await expect(page.getByRole('alert')).toHaveCount(0)
  await expect(page).toHaveURL(/\/sign-in$/)

  const invalid = 'invalid-isolated-e2e-credential'
  await credential.fill(invalid)
  const failed = page.waitForResponse(response => response.url().endsWith('/sign-in') && response.request().method() === 'POST', {timeout: 10000})
  await credential.press('Enter')
  expect((await failed).status()).toBe(401)
  await expect(page.getByRole('alert')).toContainText('Sign-in failed.')
  await expect(page.getByRole('alert')).toHaveAttribute('id', 'sign-in-error')
  await expect(credential).toHaveAttribute('aria-describedby', 'credential-help sign-in-error')
  await expect(credential).toHaveValue('')
  expect(await credential.getAttribute('value')).toBeNull()
  expect((await page.content()).includes(invalid)).toBe(false)
  expect(page.url().includes(invalid)).toBe(false)
  expect(await page.evaluate(value => [...Object.values(localStorage), ...Object.values(sessionStorage)].some(stored => stored.includes(value)), invalid)).toBe(false)
  await page.goto('/gateway')
  await expect(page).toHaveURL(/\/sign-in$/)
})

test('owner sign-out revokes the console session even when its old cookie is replayed', async ({page, context, browser}) => {
  await page.goto('/sign-in')
  await page.getByLabel('Gateway credential').fill(process.env.LABBY_E2E_CREDENTIAL)
  await page.getByRole('button', {name: 'Sign in', exact: true}).click()
  await expect(page.locator('#identity')).not.toBeEmpty()
  const oldCookies = await context.cookies()
  expect(oldCookies.length).toBeGreaterThan(0)
  await page.locator('#labby-workspace').getByLabel('Session menu', {exact: true}).click()
  await expect(page.getByRole('button', {name: 'Sign out', exact: true})).toBeVisible()
  await page.getByRole('button', {name: 'Sign out', exact: true}).click()
  await expect(page).toHaveURL(/\/sign-in$/)
  const replay = await browser.newContext()
  try {
    await replay.addCookies(oldCookies)
    const stale = await replay.newPage()
    await stale.goto('http://127.0.0.1:4298/gateway')
    await expect(stale).toHaveURL(/\/sign-in$/)
    await expect(stale.locator('#identity, #server-list')).toHaveCount(0)
  } finally { await replay.close() }
})

test('real gateway cookie requires CSRF and logout makes a captured cookie unusable', async ({playwright}) => {
  const owner = await playwright.request.newContext({baseURL: api})
  const replay = await playwright.request.newContext({baseURL: api})
  try {
    const exchange = await owner.post('/auth/bearer-session', {
      headers: {authorization: `Bearer ${process.env.LABBY_E2E_CREDENTIAL}`}, timeout: 10000,
    })
    expect(exchange.status()).toBe(200)
    const state = await owner.storageState()
    const session = await owner.get('/auth/session', {timeout: 10000})
    const identity = await session.json()
    expect(identity.authenticated).toBe(true)
    expect(identity.is_admin).toBe(true)
    expect(typeof identity.csrf_token).toBe('string')
    const noCsrf = await owner.post('/auth/logout', {timeout: 10000})
    expect(noCsrf.status()).toBe(422)
    expect((await (await owner.get('/auth/session')).json()).authenticated).toBe(true)
    const revoked = await owner.post('/auth/logout', {
      headers: {'x-csrf-token': identity.csrf_token}, timeout: 10000,
    })
    expect(revoked.status()).toBe(204)
    // Replay through a fresh HTTP client, not the cookie-cleared owner client.
    const cookie = state.cookies.map(row => `${row.name}=${row.value}`).join('; ')
    const denied = await replay.post('/v1/gateway', {
      headers: {cookie, 'x-csrf-token': identity.csrf_token},
      data: {action: 'gateway.list', params: {}}, timeout: 10000,
    })
    expect(denied.status()).toBe(401)
  } finally { await owner.dispose(); await replay.dispose() }
})


test('a real project credential has a narrower ceiling, a bound session and durable revocation', async ({playwright, page}) => {
  const root = process.env.LABBY_E2E_RESTRICTED_ROOT
  if (!root) throw new Error('Restricted real daemon root is not configured')
  const credential = readFileSync(resolve(root, 'credential.txt'), 'utf8').trim()
  const restricted = await playwright.request.newContext({baseURL: 'http://127.0.0.1:4301'})
  try {
    const authorization = `Bearer ${credential}`
    const introspection = await restricted.get('/v1/access/credentials/self', {headers: {authorization}, timeout: 10000})
    expect(introspection.status()).toBe(200)
    const identity = await introspection.json()
    expect(identity.scopes).toEqual(['lab:read'])
    expect(identity.project_id).toBe('bootstrap-default')
    expect(identity.route_id).toBe('operator')
    const catalog = await restricted.get('/v1/catalog', {headers: {authorization}, timeout: 10000})
    expect(catalog.status()).toBe(200)
    const services = (await catalog.json()).services
    expect(services.map(service => service.name)).toEqual(['gateway'])
    expect(services.every(service => service.actions.every(action => action.requires_admin !== true))).toBe(true)
    const denied = await restricted.post('/v1/server_logs', {
      headers: {authorization}, data: {action: 'server_logs.query', params: {limit: 1}}, timeout: 10000,
    })
    expect(denied.status()).toBe(403)
    // The console currently accepts static setup credentials, not project
    // credentials. Assert that this unsupported path fails closed.
    await page.goto('/sign-in')
    await page.getByLabel('Gateway credential').fill(credential)
    await page.getByRole('button', {name: 'Sign in', exact: true}).click()
    await expect(page.getByText('Sign-in failed.', {exact: false})).toBeVisible()
    const projectSession = await restricted.post('/auth/local-session', {
      headers: {authorization, host: 'lab.example.test', origin: 'https://lab.example.test'}, timeout: 10000,
    })
    expect(projectSession.status()).toBe(201)
    const cookie = projectSession.headers()['set-cookie'].split(';')[0]
    const csrf = (await projectSession.json()).csrf_token
    const sessionCatalog = await restricted.get('/v1/catalog', {headers: {cookie}, timeout: 10000})
    expect(sessionCatalog.status()).toBe(200)
    const revoked = await restricted.delete(`/v1/access/credentials/${encodeURIComponent(identity.credential_id)}`, {
      headers: {authorization}, timeout: 10000,
    })
    expect(revoked.status()).toBe(200)
    const replay = await restricted.get('/v1/access/credentials/self', {headers: {authorization}, timeout: 10000})
    expect(replay.status()).toBe(401)
    const staleSession = await restricted.get('/v1/catalog', {headers: {cookie, 'x-csrf-token': csrf}, timeout: 10000})
    expect(staleSession.status()).toBe(401)
  } finally { await restricted.dispose() }
})
