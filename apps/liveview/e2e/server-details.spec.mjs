import {test, expect} from '@playwright/test'

const fixture = 'http://127.0.0.1:4199'
const mode = (page, value) => page.request.post(`${fixture}/fixture/server_details/${value}`)

async function signIn(page) {
  await mode(page, 'normal')
  await page.request.post('http://127.0.0.1:4199/fixture/inventory/normal')
  await page.goto('/sign-in')
  await page.getByLabel('Gateway credential').fill('fixture-only')
  await page.getByRole('button', {name: 'Sign in', exact: true}).click()
  await expect(page).toHaveURL(/\/gateway$/)
  await page.goto('/gateway/servers/fixture-server')
  await expect(page.getByRole('heading', {name: 'Fixture Fetch'})).toBeVisible()
}

test('server detail snapshot, refresh recovery, isolation and expired access', async ({page, browser}) => {
  await signIn(page)
  await expect(page.locator('#server-capabilities')).toContainText('3 / 4')
  await expect(page.locator('#server-details')).not.toContainText('do-not-display')
  await expect(page.locator('#server-details')).not.toContainText('private-warning')
  const isolated = await browser.newContext()
  const stranger = await isolated.newPage()
  await stranger.goto('http://127.0.0.1:4198/gateway/servers/fixture-server')
  await expect(stranger).toHaveURL(/\/sign-in$/)
  await isolated.close()
  await page.setViewportSize({width: 320, height: 800})
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  for (const failure of ['malformed', 'unavailable', 'wrong_id']) {
    await mode(page, failure)
    await page.getByRole('button', {name: 'Refresh details', exact: true}).click()
    await expect(page.locator('#server-details-error')).toBeVisible()
    await expect(page.locator('#server-details-content')).toHaveCount(0)
    await mode(page, 'normal')
    await page.getByRole('button', {name: 'Refresh details', exact: true}).click()
    await expect(page.getByRole('heading', {name: 'Fixture Fetch'})).toBeVisible()
  }
  await mode(page, 'unauthorized')
  await page.getByRole('button', {name: 'Refresh details', exact: true}).click()
  await expect(page).toHaveURL(/\/sign-in$/)
  await page.goto('/gateway/servers/fixture-server')
  await expect(page).toHaveURL(/\/sign-in$/)
  await mode(page, 'normal')
})
