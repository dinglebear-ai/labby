import {test, expect} from '@playwright/test'

const fixture = 'http://127.0.0.1:4199'
const mode = (page, value) => page.request.post(`${fixture}/fixture/server_runtime/${value}`)

async function signIn(page) {
  await mode(page, 'normal')
  await page.request.post('http://127.0.0.1:4199/fixture/inventory/normal')
  await page.goto('/sign-in')
  await page.getByLabel('Gateway credential').fill('fixture-only')
  await page.getByRole('button', {name: 'Sign in', exact: true}).click()
  await expect(page).toHaveURL(/\/gateway$/)
  await page.goto('/gateway/upstreams/fixture-server/runtime')
  await expect(page.locator('h1').filter({hasText: 'Runtime snapshot'})).toBeVisible()
}

test('server runtime snapshot, refresh recovery, isolation and expired access', async ({page, browser}) => {
  await signIn(page)
  await expect(page.locator('#server-runtime-capabilities')).toContainText('3 / 4')
  await expect(page.locator('#server-runtime')).not.toContainText('do-not-display')
  await expect(page.locator('#server-runtime')).not.toContainText('private-owner')
  const isolated = await browser.newContext()
  const stranger = await isolated.newPage()
  await stranger.goto('http://127.0.0.1:4198/gateway/upstreams/fixture-server/runtime')
  await expect(stranger).toHaveURL(/\/sign-in$/)
  await isolated.close()
  await page.setViewportSize({width: 320, height: 800})
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  for (const failure of ['malformed', 'unavailable', 'wrong_id']) {
    await mode(page, failure)
    await page.getByRole('button', {name: 'Refresh runtime', exact: true}).click()
    await expect(page.locator('#server-runtime-error')).toBeVisible()
    await expect(page.locator('#server-runtime-content')).toHaveCount(0)
    await mode(page, 'normal')
    await page.getByRole('button', {name: 'Refresh runtime', exact: true}).click()
    await expect(page.locator('#server-runtime-capabilities')).toContainText('3 / 4')
  }
  await mode(page, 'empty')
  await page.getByRole('button', {name: 'Refresh runtime', exact: true}).click()
  await expect(page.locator('#server-runtime-empty')).toBeVisible()
  await expect(page.locator('#server-runtime-content')).toHaveCount(0)
  await mode(page, 'disconnected')
  await page.getByRole('button', {name: 'Refresh runtime', exact: true}).click()
  await expect(page.locator('#server-runtime-content')).toContainText('Not reported')
  await expect(page.locator('#server-runtime-content [data-state="disconnected"]')).toBeVisible()
  await mode(page, 'unauthorized')
  await page.getByRole('button', {name: 'Refresh runtime', exact: true}).click()
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.goto('/gateway/upstreams/fixture-server/runtime')
  await expect(page).toHaveURL(/\/sign-in$/)
  await mode(page, 'normal')
})
