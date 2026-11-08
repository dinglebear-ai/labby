import {test, expect} from '@playwright/test'

// Real Labby only. The reviewed mock is a design reference, not a network dependency
// or a source of fabricated runtime values. Screenshots are review artifacts.
test('implemented screens retain the mock shell and readable desktop/mobile layouts', async ({page}, testInfo) => {
  test.setTimeout(120000)
  await page.goto('/sign-in')
  await page.getByLabel('Gateway credential').fill(process.env.LABBY_E2E_CREDENTIAL)
  await page.getByRole('button', {name:'Sign in', exact:true}).click()
  await expect(page).toHaveURL(/\/gateway$/)
  const routes = [
    ['/gateway', '#server-list'],
    ['/gateway/servers/real-e2e-disabled', '#server-details-content'],
    ['/gateway/servers/real-e2e-disabled/catalog', '#catalog-results'],
    ['/gateway/upstreams/real-e2e-disabled/runtime', '#server-runtime-content'],
    ['/runtime', '#runtime-metrics'],
    ['/clients', '#clients-list, #clients-empty'],
    ['/diagnostics', '#diagnostics-findings'],
    ['/usage', '#usage-summary'],
    ['/activity', '#usage-calls-empty, .console-call-table'],
    ['/logs', '#server-log-lines'],
  ]
  for (const [width,height] of [[1280,900],[320,740]]) {
    await page.setViewportSize({width,height})
    for (const [route,ready] of routes) {
      await page.goto(route)
      await expect(page.locator(ready).first()).toBeVisible({timeout:15000})
      await expect(page.locator('h1')).toBeVisible()
      await expect(page.locator('nav[aria-label="Primary navigation"] a[aria-current="page"]')).toHaveCount(1)
      const dimensions=await page.evaluate(() => ({viewport:innerWidth, document:document.documentElement.scrollWidth}))
      expect(dimensions.document, `${route} horizontal page overflow at ${width}px`).toBeLessThanOrEqual(dimensions.viewport)
      if (width===1280 && route==='/gateway') {
        const icon=await page.locator('#servers-refresh svg').boundingBox()
        expect.soft(icon.width, 'refresh icon remains readable').toBeGreaterThanOrEqual(16)
        const connected=await page.locator('#server-list [data-state="connected"]').evaluate(el=>getComputedStyle(el).color)
        const disabled=await page.locator('#server-list [data-state="disabled"]').evaluate(el=>getComputedStyle(el).color)
        expect.soft(connected, 'connected and disabled states use distinct semantic colors').not.toBe(disabled)
      }
      if (width===1280) {
        const main=await page.locator('#console-integration').boundingBox()
        const panel=await page.locator('.console-panel').first().boundingBox()
        expect(Math.abs(panel.x-(main.x+24))).toBeLessThanOrEqual(1)
        expect(Math.abs(panel.width-(main.width-48))).toBeLessThanOrEqual(1)
      }
      const screenshot=testInfo.outputPath(`${width}-${route.replaceAll('/','-')}.png`)
      await page.screenshot({path:screenshot,fullPage:false})
      await testInfo.attach(`${width}-${route.replaceAll('/','-')}`, {path:screenshot,contentType:'image/png'})
    }
    await page.goto('/gateway')
    await page.getByRole('searchbox', {name:'Search servers'}).fill('no-such-server-visual-audit')
    await expect(page.locator('#servers-no-matches')).toBeVisible()
    await expect(page.locator('#server-list li')).toHaveCount(0)
    await page.getByRole('searchbox', {name:'Search servers'}).fill('real-e2e-disabled')
    await expect(page.getByRole('link', {name:'Real E2E Disabled',exact:true})).toBeVisible()
  }
})
