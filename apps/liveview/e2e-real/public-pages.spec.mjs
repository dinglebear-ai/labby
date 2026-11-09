import {test, expect} from '@playwright/test'

test('public readiness leads to a themed responsive sign-in without granting access', async ({page}, testInfo) => {
  await page.goto('/')
  await expect(page.locator('#gateway-status')).toHaveText('Gateway ready', {timeout: 10000})
  await page.getByRole('link', {name: 'Sign in', exact: true}).click()
  await expect(page).toHaveURL(/\/sign-in$/)
  await expect(page.locator('.console-auth .console-panel')).toBeVisible()
  await expect(page.locator('[data-labby-mark="fold"]')).toBeVisible()
  await expect(page.getByRole('heading', {name: 'Sign in to Labby', exact: true})).toBeVisible()

  const stylesheet = page.locator('link[rel="stylesheet"][href*="/assets/css/auth"]')
  await expect(stylesheet).toHaveCount(1)
  const stylesheetURL = await stylesheet.getAttribute('href')
  const response = await page.request.get(stylesheetURL, {timeout: 10000})
  expect(response.status()).toBe(200)
  expect(response.headers()['content-type']).toContain('text/css')

  const credential = page.getByLabel('Gateway credential', {exact: true})
  const submit = page.getByRole('button', {name: 'Sign in', exact: true})
  const controls = [credential, ...await page.getByRole('button').all()]
  for (const width of [1440, 740, 320]) {
    await page.setViewportSize({width, height: 800})
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
    for (const control of controls) {
      const bounds = await control.boundingBox()
      expect(bounds.height).toBeGreaterThanOrEqual(44)
      expect(bounds.width).toBeGreaterThanOrEqual(44)
      expect(bounds.x).toBeGreaterThanOrEqual(0)
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width)
    }
    const path = testInfo.outputPath(`sign-in-${width}.png`)
    await page.screenshot({path})
    await testInfo.attach(`sign-in-${width}`, {path, contentType: 'image/png'})
  }

  // A shorter mobile viewport exercises keyboard focus while content can scroll.
  await page.setViewportSize({width: 320, height: 480})
  await credential.focus()
  await page.keyboard.press('Tab')
  await expect(submit).toBeFocused()
  await page.keyboard.press('Shift+Tab')
  await expect(credential).toBeFocused()
  await credential.scrollIntoViewIfNeeded()
  const bounds = await credential.boundingBox()
  expect(bounds.y).toBeGreaterThanOrEqual(0)
  expect(bounds.y + bounds.height).toBeLessThanOrEqual(480)
  expect(await credential.evaluate(input => parseFloat(getComputedStyle(input).outlineWidth))).toBeGreaterThanOrEqual(2)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})
